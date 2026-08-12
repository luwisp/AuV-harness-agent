pub mod context;
pub mod parser;

use chrono::Utc;
use std::path::PathBuf;

use crate::config::HarnessConfig;
use crate::error::{HarnessError, Result};
use crate::feedback::{FeedbackContext, FeedbackRunner, format_feedback_for_llm};
use crate::guardrails::{GuardContext, GuardrailPipeline};
use crate::llm::LlmProvider;
use crate::memory::MemoryStore;
use crate::tools::context::ToolContext;
use crate::tools::ToolRegistry;
use crate::types::{Action, GuardResult, Message, Role};

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
}

impl AgentLoop {
    /// Create a new agent loop with all required components.
    ///
    /// A random session ID is generated for audit logging and guardrail
    /// context.
    pub fn new(
        llm: Box<dyn LlmProvider>,
        guardrails: GuardrailPipeline,
        tools: ToolRegistry,
        feedback: FeedbackRunner,
        memory: MemoryStore,
        config: HarnessConfig,
        context_builder: ContextBuilder,
        workspace_root: PathBuf,
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
        }
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

            // b. Parse response into action
            let action = ActionParser::parse(&response)?;

            // b2. Add assistant message to conversation history.
            // Preserve native tool_calls so APIs that require
            // proper tool_call_id tracking (DeepSeek, etc.) work.
            messages.push(Message {
                role: Role::Assistant,
                content: response.content.clone(),
                tool_calls: response.tool_calls.clone(),
                tool_call_id: None,
            });

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

            let guard_result = self.guardrails.check(&action, &guard_ctx).await;

            match guard_result {
                GuardResult::Denied { reason, .. } => {
                    return Err(HarnessError::GuardrailBlocked(reason));
                }
                GuardResult::NeedsApproval { risk_level, reasons } => {
                    return Err(HarnessError::GuardrailNeedsApproval(format!(
                        "Risk level: {}. Reasons: {}",
                        risk_level,
                        reasons.join("; ")
                    )));
                }
                GuardResult::Allowed => {
                    // Proceed
                }
            }

            // d. Check for final answer
            if let Action::FinalAnswer { ref summary } = action {
                return Ok((summary.clone(), messages));
            }

            // e. Execute tool call
            if let Action::ToolCall { ref id, ref name, ref params } = action {
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
                        // Inject the error as a tool result so the LLM can
                        // see it and potentially correct itself.
                        messages.push(Message {
                            role: Role::Tool,
                            content: format!("Error: {}", e),
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

                messages.push(Message {
                    role: Role::Tool,
                    content: tool_content,
                    tool_calls: None,
                    tool_call_id: Some(id.clone()),
                });
            }

            // h. Stop judgment (after the turn is processed)
            if self.stop_judgment(&action, turn, tokens_used) {
                if let Action::FinalAnswer { summary } = action {
                    return Ok((summary, messages));
                }
                // If we hit max_turns or token budget without a final answer,
                // return the last content as the answer
                if turn + 1 >= max_turns {
                    return Err(HarnessError::MaxTurnsReached);
                }
                if let Some(budget) = token_budget {
                    if tokens_used >= budget {
                        return Err(HarnessError::TokenBudgetExhausted);
                    }
                }
            }
        }

        Err(HarnessError::MaxTurnsReached)
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
    use crate::guardrails::GuardrailPipeline;
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

        let mock = MockLlmProvider::new(vec![tool_call_response(
            "call-1",
            "bash",
            r#"{"command": "dangerous operation"}"#,
        )]);

        let mut agent = build_test_loop(mock, pipeline, echo_registry(), empty_feedback());

        let result = agent.run("Do something dangerous").await;
        assert!(result.is_err(), "expected guardrail block, got {:?}", result);
        match result.unwrap_err() {
            HarnessError::GuardrailBlocked(reason) => {
                assert!(reason.contains("Dangerous tool blocked"));
            }
            other => panic!("expected GuardrailBlocked, got {:?}", other),
        }

        // Should have 1 trace entry (the attempt was recorded before guardrail check)
        assert_eq!(agent.turn_count(), 1);
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
        );

        let mock = MockLlmProvider::new(vec![tool_call_response(
            "call-1",
            "bash",
            r#"{"command": "sensitive operation"}"#,
        )]);

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
        );

        let result = agent.run("Do something sensitive").await;
        assert!(
            result.is_err(),
            "expected guardrail block for escalated action, got {:?}",
            result
        );
        match result.unwrap_err() {
            HarnessError::GuardrailBlocked(_) => {}
            other => panic!("expected GuardrailBlocked, got {:?}", other),
        }
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
}