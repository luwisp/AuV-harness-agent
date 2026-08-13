pub mod context;
pub mod parser;

use chrono::Utc;
use std::path::PathBuf;
use tokio::sync::mpsc;

use crate::config::HarnessConfig;
use crate::error::{HarnessError, Result};
use crate::events::AgentEvent;
use crate::feedback::{FeedbackContext, FeedbackRunner, format_feedback_for_llm};
use crate::guardrails::{GuardContext, GuardrailPipeline};
use crate::llm::LlmProvider;
use crate::memory::MemoryStore;
use crate::tools::context::ToolContext;
use crate::tools::ToolRegistry;
use crate::types::{Action, GuardResult, Message, Role, ToolCall};

use context::ContextBuilder;
use parser::ActionParser;

// ============================================================================
// TraceEntry
// ============================================================================

/// A single entry in the agent's execution trace log.
///
/// In a future phase this will be replaced by the full observability module
/// (`src/observability/`), but for now a simple `Vec<TraceEntry>` is enough
/// to capture what happened on each turn.
#[derive(Debug, Clone)]
pub struct TraceEntry {
    /// Which turn of the loop this entry corresponds to (0-indexed).
    pub turn: usize,
    /// The action the agent decided to take on this turn.
    pub action: Action,
    /// Cumulative tokens consumed up to and including this turn.
    pub tokens_used: u32,
    /// When this turn was recorded.
    pub timestamp: chrono::DateTime<Utc>,
}

// ============================================================================
// AgentLoop
// ============================================================================

/// The central agent loop that ties together the LLM provider, guardrails,
/// tool registry, feedback runner, and memory store.
///
/// # Lifecycle
///
/// 1. Load memory and build the initial context.
/// 2. Loop:
///    - Call the LLM with the current message history.
///    - Parse the response into a concrete [`Action`].
///    - Run the action through the guardrail pipeline.
///    - If the action is denied, return an error.
///    - If the action is a final answer, return the summary.
///    - If the action is a tool call, execute it and run feedback.
///    - Inject the tool result and feedback into the message history.
///    - Check stop conditions (max turns, token budget).
/// 3. Return the final answer.
pub struct AgentLoop {
    llm: Box<dyn LlmProvider>,
    guardrails: GuardrailPipeline,
    tools: ToolRegistry,
    feedback: FeedbackRunner,
    memory: MemoryStore,
    config: HarnessConfig,
    trace: Vec<TraceEntry>,
    context_builder: ContextBuilder,
    session_id: String,
    workspace_root: PathBuf,
    /// Optional event sender for streaming progress to the UI (TUI, REPL).
    event_tx: Option<mpsc::Sender<AgentEvent>>,
}

impl AgentLoop {
    /// Create a new agent loop with all required components.
    ///
    /// A random session ID is generated for audit logging and guardrail
    /// context.
    ///
    /// `event_tx` is an optional channel sender: when provided, the loop
    /// emits [`AgentEvent`] messages so the UI (TUI or REPL) can show
    /// live progress. Pass `None` for headless / test usage.
    pub fn new(
        llm: Box<dyn LlmProvider>,
        guardrails: GuardrailPipeline,
        tools: ToolRegistry,
        feedback: FeedbackRunner,
        memory: MemoryStore,
        config: HarnessConfig,
        context_builder: ContextBuilder,
        workspace_root: PathBuf,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        Self {
            llm,
            guardrails,
            tools,
            feedback,
            memory,
            config,
            trace: Vec::new(),
            context_builder,
            session_id,
            workspace_root,
            event_tx,
        }
    }

    /// Send an event to the UI if a sender is configured.
    ///
    /// Non-blocking: if the channel is full the event is dropped.
    /// The UI is best-effort — never block the agent on a slow consumer.
    fn emit(&self, event: AgentEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.try_send(event);
        }
    }

    /// 运行时调整审批力度（REPL `/approval` 指令），无需重建 agent。
    pub fn set_approval_level(&mut self, level: crate::guardrails::ApprovalLevel) {
        self.guardrails.set_approval_level(level);
    }

    /// 已注册的工具注册表（测试/诊断用）。
    pub fn tools(&self) -> &crate::tools::ToolRegistry {
        &self.tools
    }

    /// Run the agent loop to completion for the given task.
    ///
    /// Returns the final answer string on success, or an error if the loop
    /// is interrupted by guardrails, max turns, or token budget.
    pub async fn run(&mut self, task: &str) -> Result<String> {
        let (result, _messages) = self.run_with_history(task, &[]).await?;
        Ok(result)
    }

    /// Run the agent loop with existing conversation history.
    ///
    /// Unlike [`run`], this method prepends the given message history so the
    /// LLM sees all previous turns. This is used by the interactive REPL to
    /// maintain continuity across multiple user inputs.
    ///
    /// Returns a tuple of the final answer string and all messages generated
    /// during this run (including the initial context and any tool
    /// interactions). The returned messages can be passed as `history` to a
    /// subsequent call to continue the conversation.
    pub async fn run_with_history(
        &mut self,
        task: &str,
        history: &[Message],
    ) -> Result<(String, Vec<Message>)> {
        // 1. Load memory (only on first run — cheap no-op for subsequent calls)
        self.memory.load_all()?;

        // 2. Build initial context with existing conversation history
        let mut messages = self.context_builder.build(history, task);

        let mut tokens_used: u32 = 0;
        let max_turns = self.config.agent.max_turns;
        let token_budget = self.config.agent.token_budget;

        // 3. Main loop
        for turn in 0..max_turns {
            // a. Call LLM with tools
            let tool_list = self.tools.list_tools();
            let response = self.llm.complete(&messages, &tool_list).await?;
            tokens_used += response.usage.total_tokens;

            // Emit progress update for the UI
            self.emit(AgentEvent::ProgressUpdate {
                turn,
                tokens_used,
                risk_level: "Low".to_string(), // TODO: derive from guardrails
            });

            // b. Parse response into action
            let action = ActionParser::parse(&response)?;

            // b2. Add assistant message to conversation history.
            // CRITICAL: only store the tool call that actually executes
            // below. APIs like DeepSeek strictly require every
            // tool_call_id in an assistant message to be answered by a
            // following tool message — storing tool calls that never get
            // executed (e.g. extra calls in a multi-call response) breaks
            // that pairing and makes the next LLM request fail with
            // HTTP 400 "insufficient tool messages following tool_calls".
            let assistant_tool_calls = response.tool_calls.as_ref().and_then(|tcs| {
                let matching: Vec<ToolCall> = tcs
                    .iter()
                    .filter(|tc| matches!(&action, Action::ToolCall { id, .. } if id == &tc.id))
                    .cloned()
                    .collect();
                if matching.is_empty() {
                    None
                } else {
                    Some(matching)
                }
            });
            let assistant_msg = Message {
                role: Role::Assistant,
                content: response.content.clone(),
                // DeepSeek 思考模式：必须把本轮的 reasoning_content 原样
                // 保存在 assistant 消息里，供下一轮请求回传。
                reasoning_content: response.reasoning_content.clone(),
                tool_calls: assistant_tool_calls,
                tool_call_id: None,
            };
            self.emit(AgentEvent::MessageAdded {
                message: assistant_msg.clone(),
            });
            messages.push(assistant_msg);

            // Record trace
            self.trace.push(TraceEntry {
                turn,
                action: action.clone(),
                tokens_used,
                timestamp: Utc::now(),
            });

            // c. Guardrail check
            let guard_ctx = GuardContext {
                session_id: self.session_id.clone(),
                workspace_root: self.workspace_root.clone(),
                user_id: None,
            };

            // 审批上下文预览：随审批块展示最近对话（子 agent 审批时
            // 用户借此查看子对话上下文）
            self.guardrails
                .set_approval_preview(approval_preview_text(&messages));

            let guard_result = self.guardrails.check(&action, &guard_ctx).await;

            match guard_result {
                GuardResult::Denied { reason, .. } => {
                    // 护栏拒绝（静态规则拦截 / 沙箱硬校验 / 审批拒绝）
                    // 不作为致命错误终止整个循环：与工具执行失败一致，
                    // 把拒绝原因作为 Tool 结果注入，让 LLM 看到后调整
                    // 操作重试（例如把超时降到上限内）。若 LLM 反复尝试
                    // 被拦操作，由 max_turns 兜底。非工具动作（如
                    // final_answer 被静态规则拒绝）仍直接终止。
                    match &action {
                        Action::ToolCall { id, name, params } => {
                            let err_msg = format!("被护栏拦截：{}", reason);
                            self.emit(AgentEvent::ToolCallCompleted {
                                name: name.clone(),
                                detail: tool_call_detail(name, params),
                                result_content: err_msg.clone(),
                                success: false,
                            });
                            messages.push(Message {
                                role: Role::Tool,
                                content: err_msg,
                                reasoning_content: None,
                                tool_calls: None,
                                tool_call_id: Some(id.clone()),
                            });
                            continue;
                        }
                        _ => {
                            let err = HarnessError::GuardrailBlocked(reason);
                            self.emit(AgentEvent::Finished {
                                result: Err(err.to_string()),
                            });
                            return Err(err);
                        }
                    }
                }
                GuardResult::NeedsApproval { risk_level, reasons } => {
                    let err = HarnessError::GuardrailNeedsApproval(format!(
                        "Risk level: {}. Reasons: {}",
                        risk_level,
                        reasons.join("; ")
                    ));
                    self.emit(AgentEvent::Finished {
                        result: Err(err.to_string()),
                    });
                    return Err(err);
                }
                GuardResult::Allowed => {
                    // Proceed
                }
            }

            // d. Check for final answer
            if let Action::FinalAnswer { ref summary } = action {
                let summary_str = summary.clone();
                self.emit(AgentEvent::Finished {
                    result: Ok(summary_str.clone()),
                });
                return Ok((summary_str, messages));
            }

            // e. Execute tool call
            if let Action::ToolCall { ref id, ref name, ref params } = action {
                // 具体命令/参数摘要，开始与完成事件共用
                let detail = tool_call_detail(name, params);
                // Emit: tool call started（带具体命令/参数摘要）
                self.emit(AgentEvent::ToolCallStarted {
                    name: name.clone(),
                    detail: detail.clone(),
                });

                let tool_ctx = ToolContext {
                    workspace_root: self.workspace_root.clone(),
                    command_timeout: std::time::Duration::from_secs(
                        self.config.sandbox.max_timeout_secs,
                    ),
                    network_allowed: self.config.sandbox.network_allowed,
                };

                let tool_result = match self.tools.execute(name, params, &tool_ctx) {
                    Ok(result) => result,
                    Err(e) => {
                        // Emit: tool call failed
                        let err_msg = format!("Error: {}", e);
                        self.emit(AgentEvent::ToolCallCompleted {
                            name: name.clone(),
                            detail: detail.clone(),
                            result_content: err_msg.clone(),
                            success: false,
                        });
                        // Inject the error as a tool result so the LLM can
                        // see it and potentially correct itself.
                        messages.push(Message {
                            role: Role::Tool,
                            content: err_msg,
                            reasoning_content: None,
                            tool_calls: None,
                            tool_call_id: Some(id.clone()),
                        });
                        continue;
                    }
                };

                // f. Run feedback
                let feedback_ctx = FeedbackContext {
                    workspace_root: self.workspace_root.clone(),
                    changed_files: tool_result
                        .artifacts
                        .iter()
                        .map(|a| a.path.clone())
                        .collect(),
                };

                let feedback_results = self.feedback.run_all(&action, &feedback_ctx).await;

                // g. Inject tool result (with proper tool_call_id)
                let mut tool_content = format!(
                    "Success: {}\nResult: {}",
                    tool_result.success, tool_result.content
                );

                if !feedback_results.is_empty() {
                    tool_content.push('\n');
                    tool_content.push_str(&format_feedback_for_llm(&feedback_results));
                }

                self.emit(AgentEvent::ToolCallCompleted {
                    name: name.clone(),
                    detail,
                    result_content: tool_content.clone(),
                    success: tool_result.success,
                });

                messages.push(Message {
                    role: Role::Tool,
                    content: tool_content,
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: Some(id.clone()),
                });
            }

            // h. Stop judgment (after the turn is processed)
            if self.stop_judgment(&action, turn, tokens_used) {
                if let Action::FinalAnswer { summary } = action {
                    self.emit(AgentEvent::Finished {
                        result: Ok(summary.clone()),
                    });
                    return Ok((summary, messages));
                }
                // If we hit max_turns or token budget without a final answer,
                // return the last content as the answer
                if turn + 1 >= max_turns {
                    let err = HarnessError::MaxTurnsReached;
                    self.emit(AgentEvent::Finished {
                        result: Err(err.to_string()),
                    });
                    return Err(err);
                }
                if let Some(budget) = token_budget {
                    if tokens_used >= budget {
                        let err = HarnessError::TokenBudgetExhausted;
                        self.emit(AgentEvent::Finished {
                            result: Err(err.to_string()),
                        });
                        return Err(err);
                    }
                }
            }
        }

        let err = HarnessError::MaxTurnsReached;
        self.emit(AgentEvent::Finished {
            result: Err(err.to_string()),
        });
        Err(err)
    }

    /// Determine whether the loop should stop after this turn.
    ///
    /// Returns `true` when:
    /// - The action is a `FinalAnswer`
    /// - The turn count has reached `max_turns`
    /// - The token budget has been exhausted
    fn stop_judgment(&self, action: &Action, turn: usize, tokens_used: u32) -> bool {
        matches!(action, Action::FinalAnswer { .. })
            || turn + 1 >= self.config.agent.max_turns
            || self
                .config
                .agent
                .token_budget
                .map_or(false, |b| tokens_used >= b)
    }

    /// Return a reference to the execution trace collected so far.
    pub fn trace(&self) -> &[TraceEntry] {
        &self.trace
    }

    /// Return the number of turns executed so far.
    pub fn turn_count(&self) -> usize {
        self.trace.len()
    }
}

/// 构建审批上下文预览：最近 5 条消息，每条截断 60 字符（约 300 字符）。
///
/// 供审批块/审批事件的「上下文（最近消息）」段使用——子 agent 审批时
/// 用户借此查看子对话的最近内容。
pub fn approval_preview_text(messages: &[Message]) -> Option<String> {
    const MAX_LINES: usize = 5;
    const MAX_CHARS: usize = 60;
    if messages.is_empty() {
        return None;
    }
    let preview = messages
        .iter()
        .rev()
        .take(MAX_LINES)
        .rev()
        .map(|m| {
            let role = match m.role {
                Role::User => "用户",
                Role::Assistant => "助手",
                Role::Tool => "工具",
                Role::System => "系统",
            };
            let text: String = m.content.chars().take(MAX_CHARS).collect();
            let ellipsis = if m.content.chars().count() > MAX_CHARS { "…" } else { "" };
            format!("[{role}] {text}{ellipsis}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(preview)
}

/// 从工具调用参数提取可读的命令/参数摘要：
/// bash 显示命令行本身，其他工具显示紧凑 JSON；超长截断。
/// 供事件发射与 REPL 历史标注共用。
pub fn tool_call_detail(name: &str, params: &serde_json::Value) -> String {
    const MAX_CHARS: usize = 120;
    let raw = match (name, params.get("command")) {
        ("bash", Some(serde_json::Value::String(cmd))) => cmd.clone(),
        _ => serde_json::to_string(params).unwrap_or_default(),
    };
    if raw.chars().count() <= MAX_CHARS {
        raw
    } else {
        let head: String = raw.chars().take(MAX_CHARS).collect();
        format!("{}…", head)
    }
}

// ============================================================================
// Integration tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HarnessConfig;
    use crate::feedback::{FeedbackChannel, FeedbackContext};
    use crate::guardrails::approval::ApprovalGate;
    use crate::guardrails::audit::AuditLog;
    use crate::guardrails::rules::StaticRuleEngine;
    use crate::guardrails::sandbox::SandboxBoundary;
    use crate::guardrails::{ApprovalLevel, GuardrailPipeline};
    use crate::llm::mock::MockLlmProvider;
    use crate::memory::MemoryStore;
    use crate::tools::{Tool, ToolRegistry};
    use crate::tools::context::ToolContext;
    use crate::types::{
        Action, FinishReason, LlmResponse, TokenUsage, ToolCall,
        ToolResult, FeedbackResult,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Build a minimal tool registry with a single "echo" tool.
    fn echo_registry() -> ToolRegistry {
        struct EchoTool;
        impl Tool for EchoTool {
            fn name(&self) -> &str {
                "echo"
            }
            fn description(&self) -> &str {
                "Echoes a message"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type": "object", "properties": {"message": {"type": "string"}}})
            }
            fn execute(
                &self,
                params: &serde_json::Value,
                _ctx: &ToolContext,
            ) -> std::result::Result<ToolResult, HarnessError> {
                let msg = params["message"].as_str().unwrap_or("no message");
                Ok(ToolResult {
                    success: true,
                    content: format!("echo: {}", msg),
                    structured: None,
                    artifacts: vec![],
                })
            }
        }
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool)).unwrap();
        reg
    }

    /// Build a pipeline that allows everything.
    fn permissive_pipeline() -> GuardrailPipeline {
        let rules = StaticRuleEngine::new();
        let assessors: Vec<Box<dyn crate::guardrails::assessor::RiskAssessor>> = vec![];
        let sandbox = SandboxBoundary {
            workspace_root: PathBuf::from("/tmp"),
            allowed_commands: vec![],
            forbidden_commands: vec![],
            max_timeout: Duration::from_secs(300),
            network_allowed: true,
        };
        GuardrailPipeline::for_testing(rules, assessors, sandbox)
    }

    /// Build a feedback runner with no channels.
    fn empty_feedback() -> FeedbackRunner {
        FeedbackRunner::new(vec![], 0)
    }

    /// Build a context builder with a minimal system prompt and no tool menu.
    fn minimal_context_builder() -> ContextBuilder {
        ContextBuilder::new(
            "You are a test agent. When done, start with FINAL ANSWER:".to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        )
    }

    /// A helper to create a simple LlmResponse with just content.
    fn content_response(content: &str) -> LlmResponse {
        LlmResponse {
            content: content.to_string(),
            reasoning_content: None,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            tool_calls: None,
        }
    }

    /// A helper to create a LlmResponse with a tool call.
    fn tool_call_response(id: &str, name: &str, args: &str) -> LlmResponse {
        LlmResponse {
            content: String::new(),
            reasoning_content: None,
            finish_reason: FinishReason::ToolCalls,
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            tool_calls: Some(vec![ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: args.to_string(),
            }]),
        }
    }

    /// Build a full AgentLoop for testing.
    fn build_test_loop(
        mock: MockLlmProvider,
        guardrails: GuardrailPipeline,
        tools: ToolRegistry,
        feedback: FeedbackRunner,
    ) -> AgentLoop {
        let tempdir = TempDir::new().expect("tempdir");
        let memory = MemoryStore::new(tempdir.path().to_path_buf()).expect("memory store");
        let mut config = HarnessConfig::default();
        config.agent.max_turns = 10; // reasonable default for tests
        config.agent.token_budget = None;

        AgentLoop::new(
            Box::new(mock),
            guardrails,
            tools,
            feedback,
            memory,
            config,
            minimal_context_builder(),
            PathBuf::from("/tmp/test"),
            None, // no events in tests
        )
    }

    // -----------------------------------------------------------------------
    // Test: Simple task — one turn, FinalAnswer
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_agent_loop_simple_task() {
        let mock = MockLlmProvider::new(vec![content_response(
            "FINAL ANSWER: The task is complete. Here is the result.",
        )]);
        let mut agent = build_test_loop(mock, permissive_pipeline(), echo_registry(), empty_feedback());

        let result = agent.run("Do something simple").await;
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), "The task is complete. Here is the result.");
        assert_eq!(agent.turn_count(), 1, "should complete in 1 turn");
    }

    // -----------------------------------------------------------------------
    // Test: Tool call then FinalAnswer
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_agent_loop_tool_call() {
        let mock = MockLlmProvider::new(vec![
            tool_call_response("call-1", "echo", r#"{"message": "hello"}"#),
            content_response("FINAL ANSWER: Echo completed successfully."),
        ]);
        let mut agent = build_test_loop(mock, permissive_pipeline(), echo_registry(), empty_feedback());

        let result = agent.run("Echo hello").await;
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), "Echo completed successfully.");
        assert_eq!(agent.turn_count(), 2, "should complete in 2 turns (tool call + final answer)");

        // Verify the tool call was recorded in the trace
        assert_eq!(agent.trace().len(), 2);
        assert!(
            matches!(agent.trace()[0].action, Action::ToolCall { ref name, .. } if name == "echo")
        );
        assert!(
            matches!(agent.trace()[1].action, Action::FinalAnswer { .. })
        );
    }

    // -----------------------------------------------------------------------
    // Test: Multiple tool calls in one response keep tool_call_id pairing
    // -----------------------------------------------------------------------

    /// DeepSeek and other strict APIs reject requests where an assistant
    /// message carries tool_calls that are not each followed by a tool
    /// message with the matching tool_call_id.  When the LLM returns
    /// several tool calls in one response, only the first is executed —
    /// the assistant message must therefore store only that one call, or
    /// the next LLM request fails with HTTP 400.
    #[tokio::test]
    async fn test_agent_loop_multiple_tool_calls_keep_pairing() {
        let mock = MockLlmProvider::new(vec![
            LlmResponse {
                content: String::new(),
                reasoning_content: None,
                finish_reason: FinishReason::ToolCalls,
                usage: TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 10,
                    total_tokens: 20,
                },
                tool_calls: Some(vec![
                    ToolCall {
                        id: "call-1".to_string(),
                        name: "echo".to_string(),
                        arguments: r#"{"message": "first"}"#.to_string(),
                    },
                    ToolCall {
                        id: "call-2".to_string(),
                        name: "echo".to_string(),
                        arguments: r#"{"message": "second"}"#.to_string(),
                    },
                ]),
            },
            content_response("FINAL ANSWER: Only the first tool call ran."),
        ]);
        let mut agent = build_test_loop(mock, permissive_pipeline(), echo_registry(), empty_feedback());

        let (result, messages) = agent.run_with_history("Two tool calls", &[]).await.unwrap();
        assert_eq!(result, "Only the first tool call ran.");

        // Every assistant message with tool_calls must be directly followed
        // by a tool message answering each stored tool_call_id, with no
        // intervening user/assistant message (API pairing invariant).
        for (i, msg) in messages.iter().enumerate() {
            if let Some(tcs) = &msg.tool_calls {
                for tc in tcs {
                    let answered = messages[i + 1..]
                        .iter()
                        .take_while(|n| {
                            !matches!(n.role, Role::User | Role::Assistant)
                        })
                        .any(|n| n.tool_call_id.as_deref() == Some(tc.id.as_str()));
                    assert!(
                        answered,
                        "tool_call_id {} is not answered by a following tool message",
                        tc.id
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test: Guardrail intercepts dangerous action
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_agent_loop_guardrail_intercept() {
        // Build a guardrail that blocks any tool called "dangerous_tool"
        let mut rules = StaticRuleEngine::new();
        rules.add_rule(crate::guardrails::rules::GuardRule {
            id: "block-dangerous".into(),
            name: "Block dangerous tool".into(),
            pattern: crate::guardrails::rules::RulePattern::CommandGlob {
                globs: vec!["*dangerous*".into()],
            },
            action: crate::guardrails::rules::RuleAction::Deny("Dangerous tool blocked".into()),
            priority: 100,
        });

        let assessors: Vec<Box<dyn crate::guardrails::assessor::RiskAssessor>> = vec![];
        let sandbox = SandboxBoundary {
            workspace_root: PathBuf::from("/tmp"),
            allowed_commands: vec![],
            forbidden_commands: vec![],
            max_timeout: Duration::from_secs(300),
            network_allowed: true,
        };
        let pipeline = GuardrailPipeline::for_testing(rules, assessors, sandbox);

        let mock = MockLlmProvider::new(vec![
            // Turn 1: Dangerous tool call — denied by L1 static rules.
            // The denial is injected as a tool result so the LLM can adjust.
            tool_call_response(
                "call-1",
                "bash",
                r#"{"command": "dangerous operation"}"#,
            ),
            // Turn 2: LLM gives up on the dangerous approach and answers.
            content_response("FINAL ANSWER: The dangerous operation was blocked, I will not retry."),
        ]);

        let mut agent = build_test_loop(mock, pipeline, echo_registry(), empty_feedback());

        let result = agent.run("Do something dangerous").await;
        assert!(result.is_ok(), "expected recovery via final answer, got {:?}", result);
        assert_eq!(
            result.unwrap(),
            "The dangerous operation was blocked, I will not retry."
        );

        // Both the attempt and the recovery turn are recorded
        assert_eq!(agent.turn_count(), 2);
    }

    // -----------------------------------------------------------------------
    // Test: Max turns reached
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_agent_loop_max_turns() {
        // Always return a tool call — the agent should stop at max_turns
        let mut responses = Vec::new();
        for i in 0..5 {
            responses.push(tool_call_response(
                &format!("call-{}", i),
                "echo",
                &format!(r#"{{"message": "turn-{}"}}"#, i),
            ));
        }

        let mock = MockLlmProvider::new(responses);

        let mut config = HarnessConfig::default();
        config.agent.max_turns = 3; // Only 3 turns allowed
        config.agent.token_budget = None;

        let tempdir = TempDir::new().expect("tempdir");
        let memory = MemoryStore::new(tempdir.path().to_path_buf()).expect("memory store");

        let mut agent = AgentLoop::new(
            Box::new(mock),
            permissive_pipeline(),
            echo_registry(),
            empty_feedback(),
            memory,
            config,
            minimal_context_builder(),
            PathBuf::from("/tmp/test"),
            None,
        );

        let result = agent.run("Keep going").await;
        assert!(result.is_err(), "expected MaxTurnsReached, got {:?}", result);
        match result.unwrap_err() {
            HarnessError::MaxTurnsReached => {}
            other => panic!("expected MaxTurnsReached, got {:?}", other),
        }

        assert_eq!(agent.turn_count(), 3, "should stop at exactly max_turns");
    }

    // -----------------------------------------------------------------------
    // Test: Feedback loop — tool call changes code, feedback runs
    // -----------------------------------------------------------------------

    /// A feedback channel that records whether it was invoked.
    struct SpyFeedbackChannel {
        invoked: Mutex<bool>,
    }

    impl SpyFeedbackChannel {
        #[allow(dead_code)]
        fn new() -> Self {
            Self {
                invoked: Mutex::new(false),
            }
        }
    }

    #[async_trait]
    impl FeedbackChannel for SpyFeedbackChannel {
        fn name(&self) -> &str {
            "spy"
        }

        fn should_run(&self, _action: &Action, _context: &FeedbackContext) -> bool {
            true
        }

        async fn run(
            &self,
            _context: &FeedbackContext,
        ) -> std::result::Result<FeedbackResult, HarnessError> {
            *self.invoked.lock().unwrap() = true;
            Ok(FeedbackResult {
                channel: "spy".to_string(),
                passed: true,
                errors: vec![],
                summary: "Spy channel ran".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn test_agent_loop_feedback_loop() {
        let invoked = std::sync::Arc::new(Mutex::new(false));

        struct SharedSpyChannel {
            invoked: std::sync::Arc<Mutex<bool>>,
        }

        #[async_trait]
        impl FeedbackChannel for SharedSpyChannel {
            fn name(&self) -> &str {
                "spy"
            }
            fn should_run(&self, _action: &Action, _context: &FeedbackContext) -> bool {
                true
            }
            async fn run(
                &self,
                _context: &FeedbackContext,
            ) -> std::result::Result<FeedbackResult, HarnessError> {
                *self.invoked.lock().unwrap() = true;
                Ok(FeedbackResult {
                    channel: "spy".to_string(),
                    passed: true,
                    errors: vec![],
                    summary: "Spy channel ran".to_string(),
                })
            }
        }

        let invoked_flag = invoked.clone();
        let feedback = FeedbackRunner::new(
            vec![Box::new(SharedSpyChannel {
                invoked: invoked_flag,
            })],
            3,
        );

        let mock = MockLlmProvider::new(vec![
            // Turn 1: Write code (tool call)
            tool_call_response("call-1", "echo", r#"{"message": "code changed"}"#),
            // Turn 2: Another tool call
            tool_call_response("call-2", "echo", r#"{"message": "verify"}"#),
            // Turn 3: Final answer
            content_response("FINAL ANSWER: All changes applied and verified."),
        ]);

        let mut agent = build_test_loop(mock, permissive_pipeline(), echo_registry(), feedback);

        let result = agent.run("Make some changes").await;
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), "All changes applied and verified.");

        // Verify feedback was invoked (at least once for each tool call)
        assert!(*invoked.lock().unwrap(), "feedback spy should have been invoked");
        assert_eq!(agent.turn_count(), 3, "should complete in 3 turns");
    }

    // -----------------------------------------------------------------------
    // Test: Tool execution error is handled gracefully
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_agent_loop_tool_error_recovery() {
        // Mock returns a tool call for a non-existent tool, then a final answer
        let mock = MockLlmProvider::new(vec![
            tool_call_response("call-1", "nonexistent_tool", r#"{}"#),
            content_response("FINAL ANSWER: I tried but the tool was not found."),
        ]);

        let mut agent = build_test_loop(mock, permissive_pipeline(), echo_registry(), empty_feedback());

        let result = agent.run("Use a missing tool").await;
        // The tool error should be injected into messages, and the LLM should
        // get a chance to recover on the next turn.
        assert!(result.is_ok(), "expected recovery via final answer, got {:?}", result);
        assert_eq!(result.unwrap(), "I tried but the tool was not found.");
        assert_eq!(agent.turn_count(), 2);
    }

    // -----------------------------------------------------------------------
    // Test: Guardrail NeedsApproval without approval gate
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_agent_loop_guardrail_needs_approval() {
        // Build a guardrail that escalates a specific tool. With a 1ms
        // approval timeout, the escalation will always time out and the
        // pipeline will return Denied, which the agent translates to
        // GuardrailBlocked.
        let mut rules = StaticRuleEngine::new();
        rules.add_rule(crate::guardrails::rules::GuardRule {
            id: "escalate-sensitive".into(),
            name: "Escalate sensitive op".into(),
            pattern: crate::guardrails::rules::RulePattern::CommandGlob {
                globs: vec!["*sensitive*".into()],
            },
            action: crate::guardrails::rules::RuleAction::Escalate,
            priority: 50,
        });

        let assessors: Vec<Box<dyn crate::guardrails::assessor::RiskAssessor>> = vec![];
        let sandbox = SandboxBoundary {
            workspace_root: PathBuf::from("/tmp"),
            allowed_commands: vec![],
            forbidden_commands: vec![],
            max_timeout: Duration::from_secs(300),
            network_allowed: true,
        };

        // Use a 1ms approval timeout so escalation always times out -> Denied
        let pipeline = GuardrailPipeline::new(
            rules,
            assessors,
            ApprovalGate::new(Duration::from_millis(1)),
            sandbox,
            AuditLog::new(PathBuf::from("/dev/null")),
            ApprovalLevel::default(),
        );

        let mock = MockLlmProvider::new(vec![
            // Turn 1: escalated command — approval times out (1ms gate),
            // denial injected as tool result, LLM adjusts.
            tool_call_response(
                "call-1",
                "bash",
                r#"{"command": "sensitive operation"}"#,
            ),
            // Turn 2: final answer after the denial
            content_response("FINAL ANSWER: Approval required, I will stop here."),
        ]);

        let tempdir = TempDir::new().expect("tempdir");
        let memory = MemoryStore::new(tempdir.path().to_path_buf()).expect("memory store");
        let mut config = HarnessConfig::default();
        config.agent.max_turns = 10;
        config.agent.token_budget = None;

        let mut agent = AgentLoop::new(
            Box::new(mock),
            pipeline,
            echo_registry(),
            empty_feedback(),
            memory,
            config,
            minimal_context_builder(),
            PathBuf::from("/tmp/test"),
            None,
        );

        let result = agent.run("Do something sensitive").await;
        assert!(result.is_ok(), "expected recovery via final answer, got {:?}", result);
        assert_eq!(
            result.unwrap(),
            "Approval required, I will stop here."
        );
        assert_eq!(agent.turn_count(), 2, "denial injected, LLM adjusted on turn 2");
    }

    // -----------------------------------------------------------------------
    // Test: Token budget exhaustion
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_agent_loop_token_budget_exhausted() {
        // Each response uses 15 tokens. Budget is 30, so 2 turns max.
        let mock = MockLlmProvider::new(vec![
            tool_call_response("call-1", "echo", r#"{"message": "a"}"#),
            tool_call_response("call-2", "echo", r#"{"message": "b"}"#),
            content_response("FINAL ANSWER: done"),
        ]);

        let tempdir = TempDir::new().expect("tempdir");
        let memory = MemoryStore::new(tempdir.path().to_path_buf()).expect("memory store");
        let mut config = HarnessConfig::default();
        config.agent.max_turns = 50; // high enough not to trigger
        config.agent.token_budget = Some(30); // 2 * 15 = 30, so 3rd turn would exceed

        let mut agent = AgentLoop::new(
            Box::new(mock),
            permissive_pipeline(),
            echo_registry(),
            empty_feedback(),
            memory,
            config,
            minimal_context_builder(),
            PathBuf::from("/tmp/test"),
            None,
        );

        let result = agent.run("Use tokens").await;
        assert!(
            result.is_err(),
            "expected TokenBudgetExhausted, got {:?}",
            result
        );
        match result.unwrap_err() {
            HarnessError::TokenBudgetExhausted => {}
            other => panic!("expected TokenBudgetExhausted, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // tool_call_detail tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_call_detail_bash_extracts_command() {
        let params = serde_json::json!({"command": "uname -a && ls"});
        assert_eq!(tool_call_detail("bash", &params), "uname -a && ls");
    }

    #[test]
    fn test_tool_call_detail_other_tool_shows_json() {
        let params = serde_json::json!({"path": "src/main.rs"});
        assert_eq!(tool_call_detail("read_file", &params), r#"{"path":"src/main.rs"}"#);
    }

    #[test]
    fn test_tool_call_detail_truncates_long_command() {
        let cmd = "x".repeat(300);
        let params = serde_json::json!({"command": cmd});
        let detail = tool_call_detail("bash", &params);
        assert_eq!(detail.chars().count(), 121);
        assert!(detail.ends_with('…'));
    }

    // -----------------------------------------------------------------------
    // approval_preview_text tests
    // -----------------------------------------------------------------------

    fn msg(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn test_approval_preview_text_empty_messages_returns_none() {
        assert!(approval_preview_text(&[]).is_none(), "空消息列表应返回 None");
    }

    #[test]
    fn test_approval_preview_text_formats_roles_in_order() {
        let messages = vec![
            msg(Role::User, "请计算 2+2"),
            msg(Role::Assistant, "我需要使用计算器"),
            msg(Role::Tool, "结果为 4"),
        ];
        let preview = approval_preview_text(&messages).expect("应有预览");
        let expected = "[用户] 请计算 2+2\n[助手] 我需要使用计算器\n[工具] 结果为 4";
        assert_eq!(preview, expected);
    }

    #[test]
    fn test_approval_preview_text_truncates_long_content() {
        let long = "长".repeat(100);
        let messages = vec![msg(Role::System, &long)];
        let preview = approval_preview_text(&messages).expect("应有预览");
        assert_eq!(
            preview,
            format!("[系统] {}{}", "长".repeat(60), "…"),
            "超长内容应截断到 60 字符并加省略号"
        );
    }

    #[test]
    fn test_approval_preview_text_limits_to_last_5() {
        let messages: Vec<Message> = (0..7)
            .map(|i| msg(Role::User, &format!("消息 {i}")))
            .collect();
        let preview = approval_preview_text(&messages).expect("应有预览");
        let lines: Vec<&str> = preview.lines().collect();
        assert_eq!(lines.len(), 5, "只保留最后 5 条");
        assert_eq!(lines[0], "[用户] 消息 2");
        assert_eq!(lines[4], "[用户] 消息 6");
    }
}