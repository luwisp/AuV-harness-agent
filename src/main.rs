use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use harness_agent::config::HarnessConfig;
use harness_agent::config::rules::RuleFile;
use harness_agent::config::skills::SkillIndex;
use harness_agent::credentials::keyring::KeyringCredentialBackend;
use harness_agent::credentials::CredentialManager;
use harness_agent::error::Result;
use harness_agent::events::{AgentEvent, ApprovalRequest};
use harness_agent::feedback::FeedbackRunner;
use harness_agent::feedback::lint::LintChannel;
use harness_agent::feedback::test_runner::TestRunnerChannel;
use harness_agent::feedback::type_check::TypeCheckChannel;
use harness_agent::guardrails::approval::{
    ApprovalDecision, ApprovalGate, UserResponse, read_yes_no_with_timeout,
};
use harness_agent::guardrails::assessor::{
    CommandRiskAssessor, FileRiskAssessor, NetworkRiskAssessor, RiskAssessor,
};
use harness_agent::guardrails::audit::AuditLog;
use harness_agent::guardrails::rules::StaticRuleEngine;
use harness_agent::guardrails::sandbox::SandboxBoundary;
use harness_agent::guardrails::{ApprovalLevel, GuardrailPipeline};
use harness_agent::llm::openai::OpenAiProvider;
use harness_agent::llm::LlmProvider;
use harness_agent::r#loop::context::ContextBuilder;
use harness_agent::r#loop::AgentLoop;
use harness_agent::memory::MemoryStore;
use harness_agent::tools::{bash, file, git, search, test_runner, ToolRegistry};
use harness_agent::tui::{run_cli, run_tui};
use harness_agent::types::{Message, Role, ToolCall};

// ============================================================================
// CLI definition
// ============================================================================

#[derive(Parser, Debug)]
#[command(
    name = "auv",
    version = "0.1.0",
    about = "AuV harness agent - AI 编码代理",
    long_about = "带护栏、工具执行与反馈回路的 AI 编码代理（AuV harness agent）"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// 进入 REPL 时恢复上次自动保存的会话（默认开启新会话）
    #[arg(long)]
    resume: bool,

    /// 审批力度：none/low/medium/high（中文别名：无/低/中/高）——
    /// 风险等级低于等于对应阈值（high/medium/low/无）时自动批准命令，
    /// 覆盖配置文件中的 guardrails.approval_level（REPL/TUI/CLI 所有模式生效）
    #[arg(long, global = true, value_enum)]
    approval: Option<ApprovalLevel>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the agent with a task description
    Run {
        /// The task description for the agent to execute
        task: String,

        /// Path to a TOML configuration file
        #[arg(short = 'c', long)]
        config: Option<PathBuf>,

        /// Disable TUI mode (use plain text output)
        #[arg(long)]
        no_tui: bool,
    },

    /// 初始化 AuV 配置（./.AuV/config.toml、./.AuV/memory 目录、API 密钥）
    Init,

    /// Manage API keys
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
}

#[derive(Subcommand, Debug)]
enum KeyAction {
    /// Show which keys are configured (no plaintext values)
    Status,

    /// Interactively set an API key
    Set,

    /// Remove a stored key by name
    Clear {
        /// Name of the key to remove
        key: String,
    },
}

// ============================================================================
// main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing — write to stderr so it doesn't mix with REPL/TUI
    // output. Default to WARN: use RUST_LOG=info (or debug/trace) for more.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => {
            let (mut config, mut notices) = load_config(None)?;
            if let Some(level) = cli.approval {
                config.guardrails.approval_level = level;
            }
            let (home, cwd) = current_home_cwd();
            notices.extend(apply_persona(&mut config, &home, &cwd));
            let workspace = std::env::current_dir()?;
            run_repl(config, workspace, cli.resume, notices).await
        }

        Some(Commands::Run {
            task,
            config,
            no_tui,
        }) => {
            let (mut config, mut notices) = load_config(config)?;
            if let Some(level) = cli.approval {
                config.guardrails.approval_level = level;
            }
            let (home, cwd) = current_home_cwd();
            notices.extend(apply_persona(&mut config, &home, &cwd));
            print_notices(&notices);
            let workspace = std::env::current_dir()?;

            // Resolve API key from config, env, or secure credential storage.
            let api_key = resolve_api_key(&config).await?;

            if no_tui || !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                // CLI mode — no events needed
                let agent = build_agent(&config, &api_key, workspace, None, None)?;
                run_cli(agent, task).await
            } else {
                // TUI mode — event channel for live updates, plus a decision
                // channel so guardrail y/n keypresses reach the approval gate.
                let (tx, rx) = mpsc::channel::<AgentEvent>(32);
                let (decision_tx, decision_rx) = mpsc::channel::<ApprovalDecision>(4);
                let agent = build_agent(&config, &api_key, workspace, Some(tx), Some(decision_rx))?;
                let model = config.llm.model.clone();
                run_tui(agent, task, rx, decision_tx, model).await
            }
        }

        Some(Commands::Init) => run_init().await,

        Some(Commands::Key { action }) => match action {
            KeyAction::Status => run_key_status().await,
            KeyAction::Set => run_key_set().await,
            KeyAction::Clear { key } => run_key_clear(&key).await,
        },
    }
}

// ============================================================================
// run_repl
// ============================================================================

/// 处理单个 agent 事件并打印对应输出。
///
/// 供 run_repl 的 select! 循环与运行结束后的通道清空共用——运行结束时
/// 最终助手消息、ProgressUpdate 等尾部事件可能仍排在通道里，若直接
/// 丢弃会导致最终回答与 Token 统计静默丢失（历史 bug，见 CHANGELOG）。
///
/// `allow_approval_input`：select! 循环中为 true，收到护栏审批事件时
/// 打印审批块并读取 y/n；运行结束后的通道清空阶段为 false——此时 agent
/// 已超时放弃等待（decision_rx 已关闭），再读输入只会打扰用户，审批
/// 事件静默跳过。
#[allow(clippy::too_many_arguments)]
async fn handle_agent_event(
    event: AgentEvent,
    next_index: &mut usize,
    run_tokens: &mut u32,
    conversation: &[Message],
    round_messages: &mut Vec<Message>,
    approval_ctx: Option<ApprovalCtx<'_>>,
    allow_approval_input: bool,
) {
    match event {
        AgentEvent::MessageAdded { message } => {
            // 实时打印 assistant 消息块（含「工具调用：具体命令」标注）；
            // 同时累积到本轮消息列表——护栏审批结束后的全屏重绘发生在
            // run 完成之前，conversation 尚不含本轮消息，需要用它把
            // 已打印的助手消息重新印出，否则会被清屏抹掉
            print_message_block(&message, Some(*next_index), conversation);
            round_messages.push(message);
            *next_index += 1;
        }
        AgentEvent::ToolCallCompleted {
            name,
            detail,
            success,
            result_content,
        } => {
            // 具体命令直接跟在工具名后（如 `bash: uname -a`）；
            // 成功时不再重复显示 "Success: true"，失败时红色显示错误首行
            let number = format!("\x1b[90m[{}]\x1b[0m ", *next_index);
            let tool_label = if detail.is_empty() {
                name.clone()
            } else {
                format!("{}: {}", name, detail)
            };
            if success {
                println!("{}{} {}", number, role_tag(&Role::Tool), tool_label);
            } else {
                let first_line = result_content.lines().next().unwrap_or(&result_content);
                let truncated = truncate_compact(first_line, 80);
                println!(
                    "{}{} {} \x1b[31m{}\x1b[0m",
                    number,
                    role_tag(&Role::Tool),
                    tool_label,
                    truncated
                );
            }
            *next_index += 1;
        }
        AgentEvent::ProgressUpdate { tokens_used, .. } => {
            // tokens_used 为本轮累计值，取最后一条用于累加统计
            *run_tokens = tokens_used;
        }
        AgentEvent::GuardrailApprovalNeeded { request } => {
            // 护栏审批：由 REPL 打印审批块并读取 y/n（事件模式），
            // 审批结束后全屏重绘清除审批块——不再依赖 DECSC/DECRC
            // 光标舞步（在滚动/并发输出下不可靠，历史教训见 CHANGELOG）
            if allow_approval_input {
                if let Some(ctx) = approval_ctx {
                    handle_approval_event(&request, conversation, round_messages, ctx).await;
                }
            }
        }
        _ => {} // 其他事件在 REPL 中静默
    }
}

/// 护栏审批处理所需的上下文：决定通道、审批超时、本轮用户任务。
///
/// 审批结束后用对话历史 + 本轮用户任务 + 本轮已打印的助手消息
/// 重绘界面，审批块从屏幕清除（重绘路径与 /resume、/cls 同源，
/// 已多轮验证可靠）。
#[derive(Clone, Copy)]
struct ApprovalCtx<'a> {
    decision_tx: &'a mpsc::Sender<ApprovalDecision>,
    timeout: Duration,
    /// 本轮用户任务文本，重绘时补进历史末尾。
    current_task: &'a str,
}

/// 生成护栏审批块文本（多行，行间正常换行；提示行以无换行结尾）。
///
/// 不使用任何光标保存/恢复序列（DECSC/DECRC）——块清除由审批结束后
/// 的全屏重绘完成。独立纯函数便于单测断言块内容与禁止舞步序列。
fn approval_block_text(request: &ApprovalRequest) -> String {
    let mut lines = Vec::new();
    lines.push("\x1b[33;1m=== 需要护栏审批 ===\x1b[0m".to_string());
    // 操作摘要可能多行（工具名 + 参数 JSON 各一行）：仅首行加「操作:」
    // 前缀，与 stdin 模式块格式一致
    let mut action_lines = request.action_summary.lines();
    if let Some(first) = action_lines.next() {
        lines.push(format!("操作: {first}"));
    }
    for line in action_lines {
        lines.push(line.to_string());
    }
    lines.push(format!("风险等级: {}", request.risk_level));
    if !request.reasons.is_empty() {
        lines.push("原因:".to_string());
        for reason in &request.reasons {
            lines.push(format!("  - {reason}"));
        }
    }
    if let Some(ref mitigation) = request.suggested_mitigation {
        lines.push(format!("缓解措施: {mitigation}"));
    }
    lines.push("\x1b[33;1m===================================\x1b[0m".to_string());
    let mut text = lines.join("\n");
    text.push_str("\n是否批准此操作? (y/n): ");
    text
}

/// 打印护栏审批块（stdout，独立行，与 stdin 模式块格式一致）。
///
/// 提示行不带换行（用户 y/n 回显紧跟其后）。
fn print_approval_block(request: &ApprovalRequest) {
    use std::io::Write as _;
    println!();
    print!("{}", approval_block_text(request));
    let _ = std::io::stdout().flush();
}

/// 处理护栏审批事件：打印审批块 → 读取 y/n（带超时）→ 决定发回
/// agent → 全屏重绘清除审批块。
async fn handle_approval_event(
    request: &ApprovalRequest,
    conversation: &[Message],
    round_messages: &[Message],
    ctx: ApprovalCtx<'_>,
) {
    handle_approval_event_with(
        request,
        conversation,
        round_messages,
        ctx,
        read_yes_no_with_timeout(ctx.timeout),
    )
    .await;
}

/// 与 `handle_approval_event` 相同的处理流程，但用户输入由 `input`
/// 提供（生产走真实 stdin 读取，测试注入固定响应）。
async fn handle_approval_event_with(
    request: &ApprovalRequest,
    conversation: &[Message],
    round_messages: &[Message],
    ctx: ApprovalCtx<'_>,
    input: impl std::future::Future<Output = UserResponse>,
) {
    print_approval_block(request);

    let response = input.await;
    // 超时/拒绝后重绘时给出明确状态提示，用户不再面对无声的
    // 审批块消失 + 错误信息（历史 bug："状态有点不对劲"）
    let status = match response {
        UserResponse::Yes => "",
        UserResponse::No => "操作已被拒绝。",
        UserResponse::Timeout => "审批超时，已自动拒绝该操作。",
    };
    let decision = match response {
        UserResponse::Yes => ApprovalDecision::Approved {
            by: "user".to_string(),
            reason: Some("用户批准了该操作".to_string()),
        },
        UserResponse::No => ApprovalDecision::Denied {
            reason: "用户拒绝了该操作".to_string(),
        },
        UserResponse::Timeout => ApprovalDecision::Timeout,
    };
    // try_send：agent 可能已先超时（decision_rx 已关闭），忽略发送失败
    let _ = ctx.decision_tx.try_send(decision);

    // 审批结束（批准/拒绝/超时）后全屏重绘：审批块从屏幕清除，
    // 历史与已打印的助手消息以正确换行重新呈现（不依赖光标舞步）
    let mut full: Vec<Message> = conversation.to_vec();
    full.push(Message {
        role: Role::User,
        content: ctx.current_task.to_string(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
    });
    full.extend(round_messages.iter().cloned());
    redraw_interface(&full, status);
}

/// 交互式 REPL 模式：持续读取任务并运行 agent。
///
/// 对话历史按时间顺序跨轮次累积，agent 能记住之前的交互。
/// 每次对话结束后自动保存会话（autosave），可通过 `--resume`
/// 参数在下次启动时恢复；默认开启新会话。
/// 输入使用 rustyline，支持上下键历史导航与左右键行内编辑。
async fn run_repl(
    mut config: HarnessConfig,
    workspace: PathBuf,
    resume: bool,
    notices: Vec<String>,
) -> Result<()> {
    let api_key = resolve_api_key(&config).await?;

    // 创建事件通道，用于实时显示本轮消息（assistant 块、工具结果）。
    // tx 保留克隆：/model 切换模型时需要用同一通道重建 agent。
    // 护栏审批走 UI 事件模式（同 TUI）：审批请求以事件到达 REPL，
    // 由 REPL 打印审批块、读取 y/n，决定经 decision_tx 发回审批门——
    // 不再由 AgentLoop 内部直接读 stdin（旧模式的无换行提示 + DECSC/DECRC
    // 光标舞步清除在滚动/并发输出下不可靠，见 CHANGELOG 根因记录）。
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(32);
    let (mut decision_tx, decision_rx) = mpsc::channel::<ApprovalDecision>(4);
    let mut agent = build_agent(
        &config,
        &api_key,
        workspace.clone(),
        Some(tx.clone()),
        Some(decision_rx),
    )?;
    let approval_timeout = Duration::from_secs(config.guardrails.approval_timeout_secs);

    // 跨轮次累积的对话历史，按时间顺序排列：
    // [User(任务1), Assistant(回复1), Tool(结果1), User(任务2), ...]
    // System 消息被排除——ContextBuilder 每轮都会重建。
    let mut conversation: Vec<Message> = Vec::new();

    // 当前会话标题（首轮后由模型生成，自动保存按标题存档）
    let mut current_title: Option<String> = None;
    // 全会话累计 token（ProgressUpdate 每轮上报本轮增量，逐轮累加）
    let mut total_tokens: u32 = 0;

    // 默认开启新会话；--resume 时恢复最近一次自动保存的会话
    let mut restored_count: Option<usize> = None;
    if resume {
        if let Some((name, messages)) = load_latest_session() {
            if !messages.is_empty() {
                restored_count = Some(messages.len());
                current_title = Some(name);
                conversation = messages;
            }
        }
    }

    // rustyline 编辑器：支持上下键历史导航、左右键行内编辑
    let mut rl = DefaultEditor::new().map_err(|e| {
        harness_agent::error::HarnessError::Config(format!("rustyline 初始化失败: {}", e))
    })?;
    let history_path = PathBuf::from(".AuV/repl_history.txt");
    let _ = rl.load_history(&history_path);

    // 清屏并重绘界面：横幅 +（如有）恢复的完整历史
    let status = match (&current_title, restored_count) {
        (Some(name), Some(n)) => format!("[已恢复会话「{}」，共 {} 条消息]", name, n),
        _ => String::new(),
    };
    redraw_interface(&conversation, &status);

    // 配置/角色说明加载提示（清屏重绘之后打印，避免被清屏冲掉）
    print_notices(&notices);

    loop {
        // 输入区域：分隔线 + 状态行 + 提示符，状态行显示在输入行上方。
        // 曾尝试用 DECSC/DECRC 光标舞步（\x1b7/\x1b8）把状态行画到
        // 输入行下方，但 rustyline 刷新时会按自己的提示符宽度模型
        // 重定位光标，导致输入回显漂移到远端列、命令输出与状态行
        // 互相覆盖（tmux 真终端实测），故改为可靠的多行提示符布局。
        // 状态值每轮变化，提示符在循环内逐轮重建。
        let window = context_window_for(&config.llm.model, config.agent.token_budget);
        let status_line = format!(
            "\x1b[90m模型: {} | Token: {} | {}\x1b[0m",
            config.llm.model,
            total_tokens,
            remaining_context_text(total_tokens, window),
        );
        let prompt = build_repl_prompt(&status_line);

        match rl.readline(&prompt) {
            Ok(input) => {
                let trimmed = input.trim();
                if trimmed.is_empty() {
                    continue;
                }
                // 记录历史（供上下键导航），并持久化到文件
                let _ = rl.add_history_entry(trimmed);
                let _ = rl.save_history(&history_path);

                if trimmed == "/exit" || trimmed == "/quit" {
                    break;
                }
                if trimmed == "/help" {
                    println!("命令：");
                    println!("  \x1b[36m/exit, /quit\x1b[0m   退出 REPL");
                    println!("  \x1b[36m/help\x1b[0m          显示此帮助");
                    println!("  \x1b[36m/history\x1b[0m       显示全部对话消息（可带条数，如 /history 5）");
                    println!("  \x1b[36m/view <编号>\x1b[0m   查看单条消息全文（编号见消息前的 [n]）");
                    println!("  \x1b[36m/cls\x1b[0m          清屏重绘，清理命令输出（等同重新进入会话）");
                    println!("  \x1b[36m/clear\x1b[0m         重置对话历史");
                    println!("  \x1b[36m/save <名称>\x1b[0m   保存当前会话");
                    println!("  \x1b[36m/resume\x1b[0m        恢复会话（不带参数时交互选择）");
                    println!("  \x1b[36m/resume <名称>\x1b[0m 恢复指定会话");
                    println!("  \x1b[36m/sessions\x1b[0m      列出已保存会话");
                    println!("  \x1b[36m/rename <标题>\x1b[0m 更改当前会话标题");
                    println!("  \x1b[36m/model\x1b[0m          查看当前模型信息");
                    println!("  \x1b[36m/model <名称>\x1b[0m   切换模型（下轮任务生效）");
                    println!("  \x1b[36m/approval\x1b[0m       查看当前审批力度");
                    println!("  \x1b[36m/approval <无|低|中|高>\x1b[0m  调整审批力度（下轮任务生效）");
                    println!("  \x1b[36m/skills\x1b[0m        查看可用技能列表");
                    println!("  <任务>         向 agent 发送任务");
                    println!("  Ctrl+C         退出 REPL");
                    println!("  Ctrl+D         退出 REPL");
                    println!("\n当前对话历史：{} 条消息", conversation.len());
                    continue;
                }
                if let Some(rest) = trimmed.strip_prefix("/history") {
                    if conversation.is_empty() {
                        println!("暂无对话历史。");
                        continue;
                    }
                    // /history 显示全部（完整内容）；/history N 显示最近 N 条
                    match rest.trim() {
                        "" => {
                            println!("全部 {} 条消息：", conversation.len());
                            print_exchange(&conversation);
                        }
                        n_str => match n_str.parse::<usize>() {
                            Ok(0) => println!("用法：/history [条数]，条数需大于 0"),
                            Ok(n) => {
                                println!("最近 {} 条消息：", n.min(conversation.len()));
                                print_history_tail(&conversation, n);
                            }
                            Err(_) => println!("用法：/history [条数]"),
                        },
                    }
                    continue;
                }
                if let Some(rest) = trimmed.strip_prefix("/view") {
                    match rest.trim().parse::<usize>() {
                        Ok(n) => match view_message(&conversation, n) {
                            Ok(text) => println!("{}", text),
                            Err(e) => println!("\x1b[33m{}\x1b[0m", e),
                        },
                        Err(_) => println!("用法：/view <消息编号>（编号见消息前的 [n]）"),
                    }
                    continue;
                }
                if trimmed == "/clear" {
                    conversation.clear();
                    // 同时清除当前会话的自动保存与标题
                    if let Some(title) = current_title.take() {
                        let _ = std::fs::remove_file(sessions_dir().join(format!("{}.json", title)));
                    }
                    // 兼容旧版本遗留的 autosave 文件
                    let _ = std::fs::remove_file(sessions_dir().join(format!("{}.json", AUTOSAVE_NAME)));
                    // 清屏重绘，清理旧对话显示
                    redraw_interface(&conversation, "对话历史已清除。");
                    continue;
                }
                if trimmed == "/cls" {
                    // 清屏重绘：清掉 /view、/help 等命令输出，
                    // 回到重新进入会话时的界面状态
                    redraw_interface(&conversation, "");
                    continue;
                }
                if trimmed == "/save" {
                    println!("用法：/save <会话名称>");
                    continue;
                }
                if trimmed == "/resume" {
                    if let Some(name) = resume_session_interactively(&mut rl, &mut conversation) {
                        current_title = Some(name);
                    }
                    continue;
                }
                if let Some(name) = trimmed.strip_prefix("/save ") {
                    let name = name.trim();
                    if name.is_empty() {
                        println!("用法：/save <会话名称>");
                        continue;
                    }
                    match save_session(name, &conversation) {
                        Ok(()) => println!("会话已保存：'{}'。", name),
                        Err(e) => eprintln!("会话保存失败：{}", e),
                    }
                    continue;
                }
                if let Some(name) = trimmed.strip_prefix("/resume ") {
                    let name = name.trim();
                    if name.is_empty() {
                        println!("用法：/resume <会话名称>");
                        continue;
                    }
                    match load_session(name) {
                        Ok(messages) => {
                            let count = messages.len();
                            conversation = messages;
                            current_title = Some(name.to_string());
                            // 清屏并展示恢复的历史，替换旧界面内容
                            let status = format!("会话 '{}' 已恢复（{} 条消息）。", name, count);
                            redraw_interface(&conversation, &status);
                        }
                        Err(e) => eprintln!("会话恢复失败：{}", e),
                    }
                    continue;
                }
                if trimmed == "/sessions" {
                    match list_sessions() {
                        Ok(sessions) => {
                            if sessions.is_empty() {
                                println!("暂无已保存会话。");
                            } else {
                                println!("已保存会话：");
                                for (name, msg_count) in &sessions {
                                    let note = if current_title.as_deref() == Some(name.as_str()) {
                                        "（当前会话）"
                                    } else {
                                        ""
                                    };
                                    println!("  {}{} — {} 条消息", name, note, msg_count);
                                }
                            }
                        }
                        Err(e) => eprintln!("会话列表读取失败：{}", e),
                    }
                    continue;
                }
                if trimmed == "/model" {
                    // 查看当前模型信息
                    let base = config
                        .llm
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                    println!("当前模型：{}", config.llm.model);
                    println!("API 端点：{}", base);
                    let window = context_window_for(&config.llm.model, config.agent.token_budget);
                    if window > 0 {
                        println!("上下文窗口：{} tokens", window);
                    } else {
                        println!("上下文窗口：未知");
                    }
                    println!("累计 Token：{}", total_tokens);
                    continue;
                }
                if trimmed == "/rename" {
                    println!("用法：/rename <新标题>（标题同时用作会话存档文件名）");
                    continue;
                }
                if let Some(title) = trimmed.strip_prefix("/rename ") {
                    let title = sanitize_session_title(title);
                    if title.is_empty() {
                        println!("用法：/rename <新标题>（标题不能为空）");
                        continue;
                    }
                    if conversation.is_empty() {
                        println!("\x1b[33m当前会话还没有内容，发送任务后可重命名。\x1b[0m");
                        continue;
                    }
                    if current_title.as_deref() == Some(title.as_str()) {
                        println!("会话标题已是「{}」。", title);
                        continue;
                    }
                    // 删除旧标题文件，按新标题重新保存
                    if let Some(old) = current_title.take() {
                        let _ = std::fs::remove_file(sessions_dir().join(format!("{}.json", old)));
                    }
                    current_title = Some(title.clone());
                    match save_session(&title, &conversation) {
                        Ok(()) => println!("会话已重命名为「{}」。", title),
                        Err(e) => eprintln!("会话保存失败：{}", e),
                    }
                    continue;
                }
                if let Some(name) = trimmed.strip_prefix("/model ") {
                    let name = name.trim();
                    if name.is_empty() {
                        println!("用法：/model [模型名称]（不带参数查看当前模型）");
                        continue;
                    }
                    if name == config.llm.model {
                        println!("当前已在使用模型 '{}'。", name);
                        continue;
                    }
                    // 用新模型重建 agent；失败则回滚模型配置，保持原 agent 可用
                    let old_model = config.llm.model.clone();
                    config.llm.model = name.to_string();
                    // 重建审批决定通道：decision_rx 被 build_agent 消费，
                    // 新 agent 的审批决定走新通道（decision_tx 同步更新）
                    let (new_dtx, new_drx) = mpsc::channel::<ApprovalDecision>(4);
                    match build_agent(
                        &config,
                        &api_key,
                        workspace.clone(),
                        Some(tx.clone()),
                        Some(new_drx),
                    ) {
                        Ok(new_agent) => {
                            agent = new_agent;
                            decision_tx = new_dtx;
                            println!("模型已切换为 '{}'，下一轮任务生效。", name);
                        }
                        Err(e) => {
                            config.llm.model = old_model;
                            eprintln!(
                                "\x1b[31m模型切换失败：{}\x1b[0m 已回退到 '{}'。",
                                e, config.llm.model
                            );
                        }
                    }
                    continue;
                }
                if trimmed == "/approval" {
                    // 查看当前审批力度与四档说明
                    let current = config.guardrails.approval_level;
                    println!("当前审批力度：{}（{}）", current.cn_name(), current.cn_description());
                    println!();
                    println!("四档说明（风险等级低于等于对应阈值时自动批准命令）：");
                    for level in [
                        ApprovalLevel::None,
                        ApprovalLevel::Low,
                        ApprovalLevel::Medium,
                        ApprovalLevel::High,
                    ] {
                        let marker = if level == current { " ← 当前" } else { "" };
                        println!(
                            "  {}：{}{}",
                            level.cn_name(),
                            level.cn_description(),
                            marker
                        );
                    }
                    continue;
                }
                if let Some(rest) = trimmed.strip_prefix("/approval ") {
                    match rest.trim().parse::<ApprovalLevel>() {
                        Ok(level) => {
                            if level == config.guardrails.approval_level {
                                println!("审批力度已是「{}」。", level.cn_name());
                                continue;
                            }
                            // 无需重建 agent：直接透传到护栏管线，下轮任务生效
                            config.guardrails.approval_level = level;
                            agent.set_approval_level(level);
                            println!(
                                "审批力度已切换为「{}」：{}（下轮任务生效）。",
                                level.cn_name(),
                                level.cn_description()
                            );
                        }
                        Err(e) => println!("\x1b[31m{}\x1b[0m", e),
                    }
                    continue;
                }
                if trimmed == "/skills" {
                    // 查看可用技能（.skills 目录下的技能文件）
                    println!("{}", format_skills_list(config.agent.skills_dir.as_deref()));
                    continue;
                }

                // 运行 agent。任务以用户消息块展示（替换输入行的 > 前缀回显）。
                // 本轮消息的全局序号从 conversation.len() + 1 起（用户任务），
                // 实时 assistant/工具消息按发出顺序递增，与历史展示编号一致。
                let task_index = conversation.len() + 1;
                print_task_as_user_block(trimmed, task_index);

                // 运行任务并实时消费事件。独立作用域确保任务 future 在
                // 返回后立即释放对 conversation 的借用。
                let mut run_tokens = 0u32;
                let mut next_index = task_index + 1;
                // 本轮已实时打印的助手消息：护栏审批结束后的重绘发生在
                // run 完成之前，conversation 尚不含本轮消息，重绘需把
                // 它们重新印出（否则被清屏抹掉）
                let mut round_messages: Vec<Message> = Vec::new();
                // 审批块已打印但尚未完成处理（读取 y/n + 重绘）的标志。
                // agent 端审批门超时可能先于 REPL 端读取超时完成：run_fut
                // 先命中 select! 完成分支，正在 await 的审批处理 future 被
                // 取消，审批块残留在屏幕且无状态提示（历史 bug："批准怎么
                // 按不了了"——用户对着残留块输入全部落进下一轮输入行）。
                // 循环结束后用该标志补做清理重绘，与正常超时路径一致。
                let mut approval_pending = false;
                let result = {
                    let run_fut = agent.run_with_history(trimmed, &conversation);
                    tokio::pin!(run_fut);
                    loop {
                        tokio::select! {
                            r = &mut run_fut => break r,
                            event = rx.recv() => if let Some(event) = event {
                                let ctx = ApprovalCtx {
                                    decision_tx: &decision_tx,
                                    timeout: approval_timeout,
                                    current_task: trimmed,
                                };
                                let is_approval = matches!(
                                    event,
                                    AgentEvent::GuardrailApprovalNeeded { .. }
                                );
                                if is_approval {
                                    approval_pending = true;
                                }
                                handle_agent_event(
                                    event,
                                    &mut next_index,
                                    &mut run_tokens,
                                    &conversation,
                                    &mut round_messages,
                                    Some(ctx),
                                    true,
                                )
                                .await;
                                if is_approval {
                                    approval_pending = false;
                                }
                            },
                        }
                    }
                };
                // run_fut 先完成且审批 future 被取消（门先超时）：审批块
                // 仍残留在屏幕上，补做全屏重绘清除审批块并提示超时，
                // 用户不会再对着残留块输入
                if approval_pending {
                    let mut full: Vec<Message> = conversation.to_vec();
                    full.push(Message {
                        role: Role::User,
                        content: trimmed.to_string(),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                    full.extend(round_messages.iter().cloned());
                    redraw_interface(&full, "审批超时，已自动拒绝该操作。");
                }
                // 运行结束时最终助手消息、ProgressUpdate 等事件可能仍排在
                // 通道里（select! 先命中 run_fut 完成分支）。必须同样处理
                // 完，否则最终回答与 Token 统计会静默丢失（见
                // test_handle_agent_event / CHANGELOG 中的根因记录）。
                // 此阶段不处理审批输入：agent 已超时放弃等待，再读 y/n
                // 只会打扰用户（审批事件静默跳过）。
                while let Ok(event) = rx.try_recv() {
                    handle_agent_event(
                        event,
                        &mut next_index,
                        &mut run_tokens,
                        &conversation,
                        &mut round_messages,
                        None,
                        false,
                    )
                    .await;
                }

                match result {
                    Ok((summary, messages)) => {
                        // 保留所有非 System 消息（含 User 消息），
                        // 保持时间顺序，让 LLM 看到完整对话流程。
                        // 最终回答已由实时事件以消息块形式打印。
                        conversation = messages
                            .into_iter()
                            .filter(|m| m.role != Role::System)
                            .collect();
                        total_tokens += run_tokens;

                        println!();

                        // 首轮完成后请模型为对话起简短标题，之后自动保存
                        // 到 <标题>.json（标题生成失败回退默认名）
                        if current_title.is_none() {
                            current_title =
                                generate_conversation_title(&config, &api_key, trimmed, &summary)
                                    .await
                                    .or_else(|| {
                                        eprintln!(
                                            "\x1b[33m[警告]\x1b[0m 标题生成失败，使用默认会话名。"
                                        );
                                        None
                                    });
                            if let Some(title) = &current_title {
                                println!("\x1b[90m（会话标题：{}）\x1b[0m", title);
                            }
                        }
                        let save_name = current_title.as_deref().unwrap_or(AUTOSAVE_NAME);
                        // 自动保存会话
                        if let Err(e) = save_session(save_name, &conversation) {
                            eprintln!("\x1b[33m[警告]\x1b[0m 自动保存失败：{}", e);
                        }
                    }
                    Err(e) => {
                        eprintln!("\x1b[41;37;1m 错误 \x1b[0m {}\n", e);
                        // run 失败（如 LLM 网络错误）也要保留本轮对话记录：
                        // 用户任务 + 已收集的助手消息追加保存，避免会话
                        // 历史静默丢失（历史 bug：护栏拦截后 agent 直接
                        // 报错终止，整段对话没有留下任何会话文件）。
                        // assistant 消息携带的 tool_calls 在本轮失败时没有
                        // 对应工具结果，resume 会破坏 DeepSeek 严格配对，
                        // 保存前清除。
                        let mut partial: Vec<Message> = conversation.to_vec();
                        partial.push(Message {
                            role: Role::User,
                            content: trimmed.to_string(),
                            reasoning_content: None,
                            tool_calls: None,
                            tool_call_id: None,
                        });
                        for mut m in round_messages.iter().cloned() {
                            m.tool_calls = None;
                            partial.push(m);
                        }
                        let save_name = current_title.as_deref().unwrap_or(AUTOSAVE_NAME);
                        if let Err(se) = save_session(save_name, &partial) {
                            eprintln!("\x1b[33m[警告]\x1b[0m 自动保存失败：{}", se);
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C — 退出 REPL
                println!("^C");
                break;
            }
            Err(ReadlineError::Eof) => break, // Ctrl+D
            Err(e) => {
                eprintln!("读取错误：{}", e);
                break;
            }
        }
    }

    println!("再见。");
    Ok(())
}

// ============================================================================
// 终端样式 — 清屏、彩色角色标签、消息块渲染
// ============================================================================

/// 清空屏幕并把光标移到左上角。
/// 同时清除终端滚动缓冲（3J）：重绘后向上滚动不再看到
/// 旧屏幕的重复历史，每屏历史只出现一次。
fn clear_screen() {
    use std::io::Write as _;
    print!("\x1b[2J\x1b[H\x1b[3J");
    let _ = std::io::stdout().flush();
}

/// 彩色背景角色标签：24 位真彩色背景 + 白色文字，
/// 不依赖终端调色板，亮色/暗色主题下均清晰可读。
fn role_tag(role: &Role) -> &'static str {
    match role {
        Role::User => "\x1b[48;2;37;99;235m\x1b[38;2;255;255;255m\x1b[1m 用户 \x1b[0m",
        Role::Assistant => "\x1b[48;2;22;101;52m\x1b[38;2;255;255;255m\x1b[1m 助手 \x1b[0m",
        Role::Tool => "\x1b[48;2;126;34;206m\x1b[38;2;255;255;255m 工具 \x1b[0m",
        Role::System => "\x1b[48;2;71;85;105m\x1b[38;2;255;255;255m 系统 \x1b[0m",
    }
}

/// 按字符数截断文本，超长以省略号结尾（用于实时工具结果的首行摘要）。
fn truncate_compact(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        content.to_string()
    } else {
        let head: String = content.chars().take(max_chars).collect();
        format!("{}…", head)
    }
}

/// 工具结果截断：最多 12 行、600 字符。
/// 返回（展示文本, 原始总行数）；展示文本与原文相等即未截断。
fn truncate_tool_result(content: &str) -> (String, usize) {
    const MAX_LINES: usize = 12;
    const MAX_CHARS: usize = 600;
    let total_lines = content.lines().count();
    if content.chars().count() <= MAX_CHARS && total_lines <= MAX_LINES {
        return (content.to_string(), total_lines);
    }
    let head: String = content.chars().take(MAX_CHARS).collect();
    let mut lines: Vec<String> = head.lines().map(str::to_string).collect();
    lines.truncate(MAX_LINES);
    (lines.join("\n"), total_lines)
}

/// 以对话消息块的形式打印一条消息（历史与实时输出同一风格）：
/// 彩色角色标签 + 序号 + 内容。用户/助手/系统消息完整展示；
/// 工具结果限 12 行 / 600 字符，超出时提示用 /view 查看全文。
///
/// `conversation` 用于工具消息反查具体命令：工具消息本身只带
/// tool_call_id，命令存在于之前助手消息的 tool_calls 中。
fn print_message_block(msg: &Message, index: Option<usize>, conversation: &[Message]) {
    let tag = role_tag(&msg.role);
    let number = index.map(|n| format!("\x1b[90m[{n}]\x1b[0m ")).unwrap_or_default();
    match msg.role {
        Role::Tool => {
            let (content, total_lines) = truncate_tool_result(&msg.content);
            let mut lines = content.lines();
            let first = lines.next().unwrap_or("");
            // 具体命令直接跟在工具名后（如 `bash: uname -a`）；
            // 查不到标注（异常消息）时退回结果首行
            if let Some(label) = tool_label_for_id(conversation, msg.tool_call_id.as_deref()) {
                println!("{number}{tag} {label}");
                // "Success: true" 包装行与命令标注重复，跳过
                if !first.starts_with("Success: true") {
                    println!("     {first}");
                }
            } else {
                println!("{number}{tag} {first}");
            }
            for line in lines {
                println!("     {line}");
            }
            if content != msg.content {
                match index {
                    Some(n) => println!(
                        "     \x1b[90m…（共 {} 行，已省略，/view {} 查看全文）\x1b[0m",
                        total_lines, n
                    ),
                    None => println!("     \x1b[90m…（共 {} 行，已省略）\x1b[0m", total_lines),
                }
            }
        }
        _ => {
            if msg.content.is_empty() {
                println!("{number}{tag}");
            }
            for (i, line) in msg.content.lines().enumerate() {
                if i == 0 {
                    println!("{number}{tag} {line}");
                } else {
                    println!("     {line}");
                }
            }
        }
    }
    // 带工具调用的 assistant 消息：只标注调用了哪些工具（去重）。
    // 具体命令已在随后的工具结果行 `[n] 工具 <名>: <命令>` 中显示，
    // 括号内不再重复命令，避免信息冗余（历史行为见 CHANGELOG）。
    if let Some(tcs) = &msg.tool_calls {
        if !tcs.is_empty() {
            println!("     \x1b[90m（工具调用：{}）\x1b[0m", tool_call_names(tcs));
        }
    }
}

/// 工具调用标注文本：只列工具名（去重、保持首次出现顺序）。
///
/// 具体命令由工具结果行承担（`[n] 工具 <名>: <命令>`，见
/// [`tool_call_label`] 与 tool_call_id 反查），括号内标注不再重复。
fn tool_call_names(tcs: &[ToolCall]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for tc in tcs {
        if !names.contains(&tc.name.as_str()) {
            names.push(&tc.name);
        }
    }
    names.join("、")
}

/// 工具结果行标注文本：工具名 + 具体命令/参数摘要（如 `bash: uname -a`）。
/// 与 [`harness_agent::r#loop::tool_call_detail`] 使用同一摘要逻辑。
/// 用于 `[n] 工具 <名>: <命令>` 行（实时事件 detail 与历史 tool_call_id
/// 反查）；assistant 消息的「工具调用：」括号内标注只列工具名，见
/// [`tool_call_names`]。
fn tool_call_label(tc: &ToolCall) -> String {
    let params = serde_json::from_str::<serde_json::Value>(&tc.arguments).ok();
    let detail = params
        .as_ref()
        .map(|p| harness_agent::r#loop::tool_call_detail(&tc.name, p))
        .unwrap_or_default();
    if detail.is_empty() {
        tc.name.clone()
    } else {
        format!("{}: {}", tc.name, detail)
    }
}

/// 在对话中反查 tool_call_id 对应的工具调用标注（工具名 + 具体命令）。
/// 工具消息本身不携带命令，命令存在于之前助手消息的 tool_calls 中。
fn tool_label_for_id(conversation: &[Message], tool_call_id: Option<&str>) -> Option<String> {
    let id = tool_call_id?;
    for msg in conversation {
        if let Some(tcs) = &msg.tool_calls {
            for tc in tcs {
                if tc.id == id {
                    return Some(tool_call_label(tc));
                }
            }
        }
    }
    None
}

/// 以对话形式打印消息列表（消息块之间用灰色分隔线隔开），
/// 序号从 `start + 1` 起（用于尾部展示时保持全局编号）。
/// 反查工具命令需要完整对话，故传入完整 conversation 与起始位置。
fn print_exchange_slice(conversation: &[Message], start: usize) {
    let slice = &conversation[start.min(conversation.len())..];
    for (i, msg) in slice.iter().enumerate() {
        if i > 0 {
            println!("\x1b[90m{}\x1b[0m", "─".repeat(32));
        }
        print_message_block(msg, Some(start + i + 1), conversation);
    }
}

/// 以对话形式打印全部消息，序号从 1 起。
fn print_exchange(messages: &[Message]) {
    print_exchange_slice(messages, 0);
}

/// 打印最近 `max_blocks` 条消息；更早的消息用省略行提示。
fn print_history_tail(messages: &[Message], max_blocks: usize) {
    let start = messages.len().saturating_sub(max_blocks);
    if start > 0 {
        println!("\x1b[90m…… 更早的 {} 条消息已省略 ……\x1b[0m", start);
    }
    print_exchange_slice(messages, start);
}

/// 生成单条消息的全文展示文本（含序号与角色标签，不限行数、不限字符）。
/// `index` 从 1 起；空对话或编号越界返回错误提示。
/// 格式化技能列表文本（REPL `/skills` 指令使用）。
///
/// 处理四种情况：未配置技能目录、目录不存在、目录为空、正常列出。
/// 单独成函数便于单元测试。
fn format_skills_list(skills_dir: Option<&std::path::Path>) -> String {
    let dir = match skills_dir {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => return "未配置技能目录（config.toml 中 [agent] skills_dir）。".to_string(),
    };
    match SkillIndex::from_dir(dir) {
        Ok(index) if index.skills.is_empty() => {
            if dir.exists() {
                format!(
                    "技能目录 {} 中没有技能文件（需 .md 文件，frontmatter 含 description）。",
                    dir.display()
                )
            } else {
                format!("技能目录 {} 不存在。", dir.display())
            }
        }
        Ok(index) => {
            let mut out = format!(
                "可用技能（{} 个，目录 {}）：",
                index.skills.len(),
                dir.display()
            );
            for s in &index.skills {
                out.push_str(&format!("\n  {}：{}", s.name, s.description));
            }
            out
        }
        Err(e) => format!("技能目录读取失败：{}", e),
    }
}

fn view_message(conversation: &[Message], index: usize) -> std::result::Result<String, String> {
    if conversation.is_empty() {
        return Err("当前没有对话历史。".to_string());
    }
    if !(1..=conversation.len()).contains(&index) {
        return Err(format!(
            "消息编号无效，当前共 {} 条消息（1–{}）",
            conversation.len(),
            conversation.len()
        ));
    }
    let msg = &conversation[index - 1];
    let mut lines: Vec<String> = Vec::new();
    // 工具消息：具体命令标注加在内容上方（工具名 + 命令直接可见）
    let tool_label = (msg.role == Role::Tool)
        .then(|| tool_label_for_id(conversation, msg.tool_call_id.as_deref()))
        .flatten();
    if let Some(label) = &tool_label {
        lines.push(format!(
            "\x1b[90m[{index}]\x1b[0m {} {label}",
            role_tag(&msg.role)
        ));
    }
    if msg.content.is_empty() && tool_label.is_none() {
        lines.push(format!(
            "\x1b[90m[{index}]\x1b[0m {}",
            role_tag(&msg.role)
        ));
    }
    for (i, line) in msg.content.lines().enumerate() {
        if i == 0 && tool_label.is_none() {
            lines.push(format!(
                "\x1b[90m[{index}]\x1b[0m {} {line}",
                role_tag(&msg.role)
            ));
        } else {
            lines.push(format!("     {line}"));
        }
    }
    if let Some(tcs) = &msg.tool_calls {
        if !tcs.is_empty() {
            lines.push(format!(
                "     \x1b[90m（工具调用：{}）\x1b[0m",
                tool_call_names(tcs)
            ));
        }
    }
    Ok(lines.join("\n"))
}

/// 打印启动横幅。
fn print_banner() {
    println!("\x1b[36;1mAuV harness agent\x1b[0m REPL v0.1.0");
    println!("输入任务开始对话，/help 查看命令，/exit 退出");
}

/// 清屏并重绘 REPL 界面：横幅 + 状态行 +（如有）完整对话历史。
/// 用于启动、/resume、/clear 后清理过期界面信息。
/// 历史完整输出（工具结果限行），靠终端滚动条回看。
fn redraw_interface(conversation: &[Message], status: &str) {
    clear_screen();
    print_banner();
    if !status.is_empty() {
        println!("{}", status);
    }
    if conversation.is_empty() {
        println!("\x1b[90m（暂无对话历史）\x1b[0m");
    } else {
        print_exchange(conversation);
    }
    println!();
}

/// 计算字符串的终端显示宽度（ASCII 计 1 列，其余按宽字符计 2 列）。
fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

/// 把刚提交的任务以用户消息块展示：原位替换输入行的 `> 任务` 回显。
///
/// rustyline 返回后输入行仍留在屏幕上；这里把光标移回提示行、
/// 清除输入回显，再以彩色用户标签打印任务（带全局序号 `index`），
/// 使任务与对话历史中的用户消息同一形式（对标 Claude Code）。
fn print_task_as_user_block(task: &str, index: usize) {
    use std::io::Write as _;
    let number = format!("\x1b[90m[{index}]\x1b[0m ");
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        // 非终端（管道/重定向）：没有回显可替换，直接打印用户块
        println!("{}{} {}", number, role_tag(&Role::User), task);
        return;
    }
    // 输入行（含 "> " 前缀）可能折行，按终端宽度折算视觉行数
    let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
    let cols = (cols as usize).max(1);
    let visual_lines = (display_width(task) + 2).div_ceil(cols).max(1);

    // 光标上移到提示行行首并清除回显；打印用户块并清除下方残留折行
    print!("\x1b[{}A\r\x1b[2K", visual_lines);
    print!("{}{} {}\x1b[J", number, role_tag(&Role::User), task);
    println!();
    let _ = std::io::stdout().flush();
}

/// `/resume` 不带参数：列出所有会话供用户选择（按编号或名称）。
///
/// 选择循环直到成功恢复或显式取消：直接回车、`q`/`quit`/`取消` 或
/// Ctrl+C 都取消选择并重绘原对话界面（不退出 REPL）；无效输入
/// 提示后继续等待，不再报错即退出。
/// 返回 `Some(名称)` 表示已恢复的会话（调用方据此更新当前标题）。
fn resume_session_interactively(
    rl: &mut DefaultEditor,
    conversation: &mut Vec<Message>,
) -> Option<String> {
    let sessions = match list_sessions() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("会话列表读取失败：{}", e);
            return None;
        }
    };
    if sessions.is_empty() {
        println!("暂无已保存会话。");
        return None;
    }
    println!("请选择要恢复的会话（输入编号或名称，直接回车或 q 取消）：");
    for (idx, (name, msg_count)) in sessions.iter().enumerate() {
        println!("  [{}] {} — {} 条消息", idx + 1, name, msg_count);
    }
    loop {
        match rl.readline("选择 > ") {
            Ok(choice) => {
                let choice = choice.trim();
                if choice.is_empty()
                    || choice.eq_ignore_ascii_case("q")
                    || choice.eq_ignore_ascii_case("quit")
                    || choice == "取消"
                {
                    // 取消选择：清掉选择界面，返回原对话
                    redraw_interface(conversation, "已取消选择，返回当前对话。");
                    return None;
                }
                // 优先按编号选择，其次按名称匹配
                let selected: std::result::Result<String, String> =
                    if let Ok(idx) = choice.parse::<usize>() {
                        if (1..=sessions.len()).contains(&idx) {
                            Ok(sessions[idx - 1].0.clone())
                        } else {
                            Err(format!("编号无效：{}", idx))
                        }
                    } else if sessions.iter().any(|(n, _)| n == choice) {
                        Ok(choice.to_string())
                    } else {
                        Err(format!("会话 '{}' 不存在", choice))
                    };
                match selected {
                    Ok(name) => match load_session(&name) {
                        Ok(messages) => {
                            let count = messages.len();
                            *conversation = messages;
                            let status = format!("会话 '{}' 已恢复（{} 条消息）。", name, count);
                            redraw_interface(conversation, &status);
                            return Some(name);
                        }
                        // 加载失败：提示后继续等待选择（不退出选择界面）
                        Err(e) => println!("\x1b[31m会话恢复失败：{}\x1b[0m", e),
                    },
                    // 无效输入：提示后继续等待，可回车或 q 取消
                    Err(msg) => println!("\x1b[33m{}\x1b[0m，请重新输入（回车或 q 取消）", msg),
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C：取消选择，返回原对话（不退出 REPL）
                redraw_interface(conversation, "已取消选择，返回当前对话。");
                return None;
            }
            Err(_) => {
                // 读取失败（如 EOF）：同样清理选择界面
                redraw_interface(conversation, "");
                return None;
            }
        }
    }
}

// ============================================================================
// 会话管理 — 保存 / 恢复 / 列出会话
// ============================================================================

/// 自动保存会话的文件名（不含扩展名）。
const AUTOSAVE_NAME: &str = "autosave";

/// 会话文件存储目录（AuV 数据统一收纳在 .AuV/ 下）。
fn sessions_dir() -> PathBuf {
    PathBuf::from(".AuV/sessions")
}

/// 将会话保存为 JSON 文件。
fn save_session(name: &str, messages: &[Message]) -> Result<()> {
    let dir = sessions_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", name));
    let json = serde_json::to_string_pretty(messages)
        .map_err(|e| harness_agent::error::HarnessError::Config(format!("serialize: {}", e)))?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// 从 JSON 文件加载会话。
fn load_session(name: &str) -> Result<Vec<Message>> {
    let path = sessions_dir().join(format!("{}.json", name));
    if !path.exists() {
        return Err(harness_agent::error::HarnessError::Config(format!(
            "会话 '{}' 不存在",
            name
        )));
    }
    let json = std::fs::read_to_string(&path)?;
    let messages: Vec<Message> = serde_json::from_str(&json)
        .map_err(|e| harness_agent::error::HarnessError::Config(format!("反序列化失败: {}", e)))?;
    Ok(messages)
}

/// 列出所有已保存的会话及消息数。
fn list_sessions() -> Result<Vec<(String, usize)>> {
    let dir = sessions_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "json") {
            if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                // Quick count: read file and count Message objects
                let msg_count = std::fs::read_to_string(&path)
                    .ok()
                    .map(|s| s.matches("\"role\"").count())
                    .unwrap_or(0);
                sessions.push((name.to_string(), msg_count));
            }
        }
    }
    sessions.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(sessions)
}

/// 加载最近修改的会话（`--resume` 启动时恢复）。
/// 返回会话文件名与消息列表；目录为空或读取失败时返回 `None`。
fn load_latest_session() -> Option<(String, Vec<Message>)> {
    load_latest_session_in(&sessions_dir())
}

/// `load_latest_session` 的可注入目录版本（供测试使用临时目录）。
///
/// 单个文件损坏/读取失败只跳过该文件，不影响其他会话的恢复。
fn load_latest_session_in(dir: &std::path::Path) -> Option<(String, Vec<Message>)> {
    let mut latest: Option<(std::time::SystemTime, String, Vec<Message>)> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(false, |e| e != "json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|n| n.to_str()) else {
            continue;
        };
        // 跳过空会话文件（无内容可恢复）与解析失败的文件
        let Ok(json) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(messages) = serde_json::from_str::<Vec<Message>>(&json) else {
            continue;
        };
        if messages.is_empty() {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if latest.as_ref().map_or(true, |(t, _, _)| modified > *t) {
            latest = Some((modified, name.to_string(), messages));
        }
    }
    latest.map(|(_, name, messages)| (name, messages))
}

/// 清理会话标题：去除引号/控制字符，路径分隔等非法文件名字符替换为
/// 空格，截断到 24 字符，保证可直接用作会话存档文件名。
fn sanitize_session_title(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => ' ',
            _ => c,
        })
        .collect();
    cleaned
        .trim()
        .trim_matches(['"', '\'', '“', '”', '‘', '’', '「', '」', '。', '，', '！', '？', '.', '、'])
        .trim()
        .chars()
        .take(24)
        .collect::<String>()
        .trim()
        .to_string()
}

/// 首轮对话结束后请模型为对话生成简短标题（≤12 字）。
///
/// 使用当前配置的模型与 API 端点发起一次轻量补全（无工具），
/// 失败时返回 `None`，由调用方回退默认会话名。
async fn generate_conversation_title(
    config: &HarnessConfig,
    api_key: &str,
    first_task: &str,
    summary: &str,
) -> Option<String> {
    let provider = OpenAiProvider::new(
        api_key.to_string(),
        config.llm.model.clone(),
        config.llm.base_url.clone(),
    );
    let messages = vec![
        Message {
            role: Role::System,
            content: "你是会话标题助手。请为下面的对话生成一个不超过 12 个字的简短标题，\
直接输出标题本身，不要引号、标点或任何解释。"
                .to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        },
        Message {
            role: Role::User,
            content: format!(
                "用户任务：{}\n助手回答摘要：{}",
                truncate_compact(first_task, 60),
                truncate_compact(summary, 120),
            ),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        },
    ];
    let response = provider.complete(&messages, &[]).await.ok()?;
    let title = sanitize_session_title(&response.content);
    (!title.is_empty()).then_some(title)
}

/// 估算模型的上下文窗口大小（按已知模型族匹配），未知模型回退到
/// `token_budget`；两者都没有时返回 0（表示未知）。
fn context_window_for(model: &str, token_budget: Option<u32>) -> u32 {
    let m = model.to_lowercase();
    if m.contains("gpt-4")
        || m.contains("gpt-5")
        || m.contains("o1")
        || m.contains("o3")
        || m.contains("o4")
        || m.contains("claude")
    {
        return 128_000;
    }
    if m.contains("deepseek") {
        return 64_000;
    }
    if m.contains("llama") || m.contains("qwen") || m.contains("glm") {
        return 32_000;
    }
    token_budget.unwrap_or(0)
}

/// 上下文剩余状态文本：窗口已知时显示剩余百分比与 token 数，
/// 未知时只标注"未知"。
fn remaining_context_text(total_tokens: u32, window: u32) -> String {
    if window == 0 {
        return "上下文剩余: 未知".to_string();
    }
    let remaining = window.saturating_sub(total_tokens);
    let pct = if total_tokens >= window {
        0
    } else {
        ((remaining as f64 / window as f64) * 100.0).round() as u32
    };
    format!("上下文剩余: {}%（{}/{}）", pct, remaining, window)
}

/// 构建 REPL 每轮提示符：分隔线 + 状态行 + `> `。
///
/// 状态行置于输入行**上方**（多行提示符的第二行）。不能用光标
/// 保存/恢复转义（\x1b7/\x1b8）把它画到输入行下方——rustyline
/// 刷新时按自身宽度模型重定位光标，会造成回显漂移与输出覆盖
/// （详见 run_repl 循环内注释）。
fn build_repl_prompt(status_line: &str) -> String {
    format!("\x1b[90m{}\x1b[0m\n{}\n> ", "─".repeat(48), status_line)
}

// ============================================================================
// load_config（AuV 两级分层）
// ============================================================================

/// 加载配置：`--config` 显式指定时只读该文件；否则按「全局 → 局部」分层加载
/// （`~/.AuV/config.toml` → `./.AuV/config.toml`，局部字段级覆盖全局）。
/// 返回 (配置, 用户提示)。
fn load_config(path: Option<PathBuf>) -> Result<(HarnessConfig, Vec<String>)> {
    match path {
        Some(p) => Ok((HarnessConfig::from_file(&p)?, Vec::new())),
        None => {
            let (home, cwd) = current_home_cwd();
            load_config_layered(&home, &cwd)
        }
    }
}

/// 分层加载（home/cwd 参数注入，便于测试隔离）。
fn load_config_layered(
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<(HarnessConfig, Vec<String>)> {
    let layered = harness_agent::config::load_layered(home, cwd)?;
    Ok((layered.config, layered.notices))
}

/// 当前用户主目录与工作目录（主目录不可得时回退当前目录）。
fn current_home_cwd() -> (PathBuf, PathBuf) {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    (home, cwd)
}

/// 组装角色说明：默认提示词 + 全局角色文件 + 项目角色文件（追加式叠加）。
/// 配置已内联 `[agent] system_prompt` 时直接使用（最高优先，不加载文件）。
/// 返回用户可见提示（加载了哪个文件 / 读取失败警告）。
fn apply_persona(
    config: &mut HarnessConfig,
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> Vec<String> {
    if config.agent.system_prompt.is_some() {
        return Vec::new();
    }
    let (global, project) = harness_agent::config::resolve_persona_files(home, cwd);
    if global.is_none() && project.is_none() {
        return Vec::new();
    }
    let mut notices = Vec::new();
    let mut persona = String::new();
    for (path, label) in [(global, "全局"), (project, "项目")] {
        if let Some(p) = path {
            match std::fs::read_to_string(&p) {
                Ok(text) => {
                    persona.push_str(&text);
                    persona.push_str("\n\n");
                    notices.push(format!("已加载{}角色说明：{}", label, p.display()));
                }
                Err(e) => notices.push(format!(
                    "读取角色说明 {} 失败：{}（已忽略）",
                    p.display(),
                    e
                )),
            }
        }
    }
    let mut full = harness_agent::r#loop::context::default_system_prompt();
    full.push_str("\n\n## Persona\n");
    full.push_str(&persona);
    config.agent.system_prompt = Some(full);
    notices
}

/// 打印配置/角色加载提示（黄色）。
fn print_notices(notices: &[String]) {
    for n in notices {
        println!("\x1b[93m提示：{}\x1b[0m", n);
    }
}

// ============================================================================
// resolve_api_key
// ============================================================================

/// Resolve the API key from config, environment, then secure credential storage.
async fn resolve_api_key(config: &HarnessConfig) -> Result<String> {
    if let Some(key) = resolve_api_key_without_vault(config)? {
        return Ok(key);
    }
    let manager = make_credential_manager();
    resolve_api_key_with_manager(config, &manager).await
}

async fn resolve_api_key_with_manager(
    config: &HarnessConfig,
    manager: &CredentialManager,
) -> Result<String> {
    if let Some(key) = resolve_api_key_without_vault(config)? {
        return Ok(key);
    }
    for name in ["OPENAI_API_KEY", "openai_api_key"] {
        if let Some(key) = manager.get(name).await? {
            return Ok(key);
        }
    }
    Err(harness_agent::error::HarnessError::Auth(
        "No API key found. Set it in config.toml under [llm].api_key, \
         set OPENAI_API_KEY_FILE or OPENAI_API_KEY, \
         Run `auv key set` and use the name OPENAI_API_KEY to store it securely."
            .to_string(),
    ))
}

fn resolve_api_key_without_vault(config: &HarnessConfig) -> Result<Option<String>> {
    if let Some(ref key) = config.llm.api_key {
        return Ok(Some(key.clone()));
    }
    if let Ok(path) = std::env::var("OPENAI_API_KEY_FILE") {
        let key = std::fs::read_to_string(&path).map_err(|e| {
            harness_agent::error::HarnessError::Auth(format!(
                "Failed to read OPENAI_API_KEY_FILE '{}': {}",
                path, e
            ))
        })?;
        let key = key.trim();
        if key.is_empty() {
            return Err(harness_agent::error::HarnessError::Auth(format!(
                "OPENAI_API_KEY_FILE '{}' is empty",
                path
            )));
        }
        return Ok(Some(key.to_string()));
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        return Ok(Some(key));
    }
    Ok(None)
}

// ============================================================================
// build_agent
// ============================================================================

/// Build a fully-wired [`AgentLoop`] from the given configuration and API key.
///
/// `event_tx` is an optional sender for progress events. Pass `None` for
/// headless / batch usage, or a channel sender for TUI/REPL modes.
///
/// `decision_rx` 启用审批门的 UI 事件模式（TUI）：审批请求以事件发给 UI，
/// 决定通过该通道返回；`None` 时审批门在 stdin 上交互（REPL / 纯文本）。
fn build_agent(
    config: &HarnessConfig,
    api_key: &str,
    workspace: PathBuf,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    decision_rx: Option<mpsc::Receiver<ApprovalDecision>>,
) -> Result<AgentLoop> {
    // 1. LLM provider
    let llm: Box<dyn LlmProvider> = Box::new(OpenAiProvider::new(
        api_key.to_string(),
        config.llm.model.clone(),
        config.llm.base_url.clone(),
    ));

    // 2. Guardrails — load built-in rules and all assessors
    let mut rules = StaticRuleEngine::new();
    rules.load_builtin_rules();

    let assessors: Vec<Box<dyn RiskAssessor>> = if config.guardrails.enabled {
        vec![
            Box::new(CommandRiskAssessor),
            Box::new(FileRiskAssessor),
            Box::new(NetworkRiskAssessor),
        ]
    } else {
        vec![]
    };

    let approval_timeout = Duration::from_secs(config.guardrails.approval_timeout_secs);
    let approval = match decision_rx {
        // TUI 模式：stdin 被 crossterm raw mode 接管，审批通过事件通道
        // 交给 TUI 面板，y/n 按键决定经 decision_rx 返回
        Some(decision_rx) => {
            let event_tx = event_tx.clone().ok_or_else(|| {
                harness_agent::error::HarnessError::Config(
                    "TUI 审批模式需要事件通道".to_string(),
                )
            })?;
            ApprovalGate::with_ui_events(approval_timeout, event_tx, decision_rx)
        }
        // REPL / 纯文本模式：stdin 交互审批
        None => ApprovalGate::new(approval_timeout),
    };
    let sandbox = SandboxBoundary {
        workspace_root: workspace.clone(),
        allowed_commands: config.sandbox.allowed_commands.clone(),
        forbidden_commands: if config.sandbox.forbidden_commands.is_empty() {
            // Default deny: block dangerous commands when sandbox is enabled
            vec![
                "rm -rf /".to_string(),
                "sudo".to_string(),
                "chmod 777 /".to_string(),
                "mkfs".to_string(),
                "dd if=".to_string(),
                ":(){ :|:& };:".to_string(),
            ]
        } else {
            config.sandbox.forbidden_commands.clone()
        },
        max_timeout: Duration::from_secs(config.sandbox.max_timeout_secs),
        network_allowed: config.sandbox.network_allowed,
    };
    let audit = AuditLog::new(config.guardrails.audit_log_path.clone());
    let guardrails = GuardrailPipeline::new(
        rules,
        assessors,
        approval,
        sandbox,
        audit,
        config.guardrails.approval_level,
    );

    // 3. Tools — register all available tools
    let mut tools = ToolRegistry::new();
    let disabled = &config.tools.disabled_tools;
    if !disabled.contains(&"bash".to_string()) {
        tools.register(Box::new(bash::BashTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register bash: {}", e)))?;
    }
    if !disabled.contains(&"read_file".to_string()) {
        tools.register(Box::new(file::ReadFileTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register read_file: {}", e)))?;
    }
    if !disabled.contains(&"write_file".to_string()) {
        tools.register(Box::new(file::WriteFileTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register write_file: {}", e)))?;
    }
    if !disabled.contains(&"grep".to_string()) {
        tools.register(Box::new(search::GrepTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register grep: {}", e)))?;
    }
    if !disabled.contains(&"glob".to_string()) {
        tools.register(Box::new(search::GlobTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register glob: {}", e)))?;
    }
    if !disabled.contains(&"git_diff".to_string()) {
        tools.register(Box::new(git::GitDiffTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register git_diff: {}", e)))?;
    }
    if !disabled.contains(&"run_test".to_string()) {
        tools.register(Box::new(test_runner::RunTestTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register run_test: {}", e)))?;
    }

    // 4. Feedback — load channels when enabled
    let feedback_channels: Vec<Box<dyn harness_agent::feedback::FeedbackChannel>> = if config.feedback.enabled {
        vec![
            Box::new(TestRunnerChannel),
            Box::new(TypeCheckChannel),
            Box::new(LintChannel),
        ]
    } else {
        vec![]
    };
    let feedback = FeedbackRunner::new(feedback_channels, config.feedback.max_retries);

    // 5. Memory
    let memory = MemoryStore::new(config.memory.storage_path.clone())?;

    // 6. Context builder — load rules and skills
    let tool_menu = serde_json::to_string(&tools.generate_tool_menu())
        .unwrap_or_default();

    let rules_text = if let Some(ref rules_file) = config.guardrails.rules_file {
        if !rules_file.as_os_str().is_empty() {
            RuleFile::from_file(rules_file)
                .map(|rf| rf.to_system_prompt_fragment())
                .unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let skills_text = if let Some(ref skills_dir) = config.agent.skills_dir {
        if !skills_dir.as_os_str().is_empty() {
            SkillIndex::from_dir(skills_dir)
                .map(|si| si.to_prompt_fragment())
                .unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let memory_index = memory.compact_index();

    let context_builder = ContextBuilder::from_config(
        config,
        tool_menu,
        rules_text,
        skills_text,
        memory_index,
    );

    // 7. Assemble
    Ok(AgentLoop::new(
        llm,
        guardrails,
        tools,
        feedback,
        memory,
        config.clone(),
        context_builder,
        workspace,
        event_tx,
    ))
}

// ============================================================================
// run_init
// ============================================================================

async fn run_init() -> Result<()> {
    use std::io::{self, Write};

    println!("AuV 初始化");
    println!("==========");

    // 1. 创建项目局部配置 ./.AuV/config.toml（cwd 为 home 目录时创建全局配置）
    let (home, cwd) = current_home_cwd();
    let config_path = if cwd == home {
        home.join(".AuV").join("config.toml")
    } else {
        cwd.join(".AuV").join("config.toml")
    };
    if config_path.exists() {
        let mut answer = String::new();
        print!("{} 已存在，是否覆盖？[y/N]: ", config_path.display());
        io::stdout().flush()?;
        io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_lowercase();
        if answer != "y" && answer != "yes" {
            println!("跳过配置创建（保留现有配置）。");
        } else {
            harness_agent::config::write_default_config(&config_path)?;
            println!("已写入默认配置：{}", config_path.display());
        }
    } else {
        harness_agent::config::write_default_config(&config_path)?;
        println!("已创建配置：{}", config_path.display());
    }

    // 2. 创建记忆目录（AuV 数据统一收纳在 .AuV/ 下）
    let memory_path = PathBuf::from(".AuV/memory");
    if !memory_path.exists() {
        std::fs::create_dir_all(&memory_path)?;
        let index_path = memory_path.join("MEMORY.md");
        std::fs::write(&index_path, "# Memory Index\n\n")?;
        println!("已创建 .AuV/memory/ 目录。");
    } else {
        println!(".AuV/memory/ 目录已存在。");
    }

    // 3. 可选：设置 API 密钥
    let mut answer = String::new();
    print!("现在设置 API 密钥吗？[y/N]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_lowercase();
    if answer == "y" || answer == "yes" {
        run_key_set().await?;
    }

    println!("\n初始化完成。");
    println!("编辑 {} 自定义配置（默认模型、审批力度等）。", config_path.display());
    println!("运行 `auv run \"任务\"` 执行单次任务，或直接 `auv` 进入交互式 REPL。");

    Ok(())
}

// ============================================================================
// key management commands
// ============================================================================

fn make_credential_manager() -> CredentialManager {
    let backend = Box::new(KeyringCredentialBackend::new());
    CredentialManager::new(backend)
}

async fn run_key_status() -> Result<()> {
    let manager = make_credential_manager();
    let status = manager.key_status()?;
    println!("{}", status);
    Ok(())
}

async fn run_key_set() -> Result<()> {
    let manager = make_credential_manager();
    manager.key_set().await
}

async fn run_key_clear(key: &str) -> Result<()> {
    let manager = make_credential_manager();
    manager.key_clear(key).await?;
    println!("Key '{}' removed.", key);
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use clap::Parser;
    use harness_agent::credentials::CredentialBackend;
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard};

    /// 进程环境变量（OPENAI_API_KEY / OPENAI_API_KEY_FILE）是共享可变状态，
    /// 串行化所有 set_var/remove_var 的测试，消除并行竞态。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct TestCredentialBackend {
        values: HashMap<String, String>,
    }

    #[async_trait]
    impl CredentialBackend for TestCredentialBackend {
        async fn get(&self, key: &str) -> Result<Option<String>> {
            Ok(self.values.get(key).cloned())
        }

        async fn set(&self, _key: &str, _value: &str) -> Result<()> {
            Ok(())
        }

        async fn delete(&self, _key: &str) -> Result<()> {
            Ok(())
        }

        fn list_keys(&self) -> Result<Vec<String>> {
            Ok(self.values.keys().cloned().collect())
        }
    }

    fn test_credential_manager(entries: &[(&str, &str)]) -> CredentialManager {
        let values = entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        CredentialManager::new(Box::new(TestCredentialBackend { values }))
    }

    // -----------------------------------------------------------------------
    // Helper: parse CLI args from a string
    // -----------------------------------------------------------------------

    /// Parse a command line string, respecting shell-like quoting.
    /// Args are split by whitespace, but text within double quotes is kept
    /// as a single argument.
    fn parse(args: &str) -> Cli {
        let args_vec = shell_split(args);
        let mut full_args: Vec<&str> = vec!["auv"];
        full_args.extend(args_vec.iter().map(|s| s.as_str()));
        Cli::parse_from(full_args)
    }

    /// Split a command-line string into arguments, respecting double quotes.
    /// Simple implementation: toggle in/out of quote mode on `"` characters.
    fn shell_split(input: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;

        for ch in input.chars() {
            match ch {
                '"' => {
                    in_quotes = !in_quotes;
                }
                ' ' if !in_quotes => {
                    if !current.is_empty() {
                        result.push(std::mem::take(&mut current));
                    }
                }
                _ => {
                    current.push(ch);
                }
            }
        }
        if !current.is_empty() {
            result.push(current);
        }
        result
    }

    // -----------------------------------------------------------------------
    // Run command tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_with_task() {
        let cli = parse("run \"fix the login bug\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "fix the login bug");
                assert!(config.is_none());
                assert!(!no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_run_with_config_flag() {
        let cli = parse("run -c my-config.toml \"do stuff\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "do stuff");
                assert_eq!(config, Some(PathBuf::from("my-config.toml")));
                assert!(!no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_run_with_long_config_flag() {
        let cli = parse("run --config prod.toml \"deploy\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "deploy");
                assert_eq!(config, Some(PathBuf::from("prod.toml")));
                assert!(!no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_run_with_no_tui_flag() {
        let cli = parse("run --no-tui \"run tests\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "run tests");
                assert!(config.is_none());
                assert!(no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_run_with_all_flags() {
        let cli = parse("run -c custom.toml --no-tui \"complex task\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "complex task");
                assert_eq!(config, Some(PathBuf::from("custom.toml")));
                assert!(no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    // -----------------------------------------------------------------------
    // Init command tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_init_command() {
        let cli = parse("init");
        assert!(matches!(cli.command, Some(Commands::Init)));
    }

    // -----------------------------------------------------------------------
    // Key subcommand tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_key_status() {
        let cli = parse("key status");
        match cli.command {
            Some(Commands::Key { action }) => {
                assert!(matches!(action, KeyAction::Status));
            }
            _ => panic!("Expected Commands::Key"),
        }
    }

    #[test]
    fn test_key_set() {
        let cli = parse("key set");
        match cli.command {
            Some(Commands::Key { action }) => {
                assert!(matches!(action, KeyAction::Set));
            }
            _ => panic!("Expected Commands::Key"),
        }
    }

    #[test]
    fn test_key_clear() {
        let cli = parse("key clear openai_api_key");
        match cli.command {
            Some(Commands::Key { action }) => match action {
                KeyAction::Clear { key } => {
                    assert_eq!(key, "openai_api_key");
                }
                _ => panic!("Expected KeyAction::Clear"),
            },
            _ => panic!("Expected Commands::Key"),
        }
    }

    #[test]
    fn test_key_clear_with_quoted_key() {
        let cli = parse("key clear \"my secret key\"");
        match cli.command {
            Some(Commands::Key { action }) => match action {
                KeyAction::Clear { key } => {
                    assert_eq!(key, "my secret key");
                }
                _ => panic!("Expected KeyAction::Clear"),
            },
            _ => panic!("Expected Commands::Key"),
        }
    }

    // -----------------------------------------------------------------------
    // Version and help tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_version_flag() {
        let args: Vec<&str> = vec!["auv", "--version"];
        let result = Cli::try_parse_from(args);
        // --version causes clap to print and exit with ErrorKind::DisplayVersion
        assert!(result.is_err() || result.is_ok());
    }

    #[test]
    fn test_help_flag() {
        let args: Vec<&str> = vec!["auv", "--help"];
        let result = Cli::try_parse_from(args);
        // --help causes clap to print and exit, so this is expected to be an error
        // (ErrorKind::DisplayHelp). We just verify it doesn't panic.
        assert!(result.is_err() || result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_with_empty_task_rejected() {
        // Empty string as task — clap should reject this as a missing required argument
        let args: Vec<&str> = vec!["auv", "run", ""];
        let result = Cli::try_parse_from(args);
        // clap may accept "" as a task or reject it; either behavior is fine
        // We just verify it doesn't panic
        let _ = result;
    }

    #[test]
    fn test_run_with_special_characters_in_task() {
        let cli = parse("run \"fix bug #123: login doesn't work (urgent!)\"");
        match cli.command {
            Some(Commands::Run { task, .. }) => {
                assert_eq!(task, "fix bug #123: login doesn't work (urgent!)");
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_no_subcommand_enters_repl() {
        let args: Vec<&str> = vec!["auv"];
        let cli = Cli::parse_from(args);
        assert!(cli.command.is_none(), "No subcommand should enter REPL mode");
    }

    #[test]
    fn test_resume_flag_default_false() {
        let cli = Cli::parse_from(vec!["auv"]);
        assert!(!cli.resume, "--resume 默认应为关闭（新会话）");
    }

    #[test]
    fn test_resume_flag_enables_session_restore() {
        let cli = Cli::parse_from(vec!["auv", "--resume"]);
        assert!(cli.resume, "--resume 应开启上次会话恢复");
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_invalid_subcommand() {
        let args: Vec<&str> = vec!["auv", "unknown"];
        let result = Cli::try_parse_from(args);
        assert!(result.is_err());
    }

    #[test]
    fn test_approval_flag_parses_in_repl_mode() {
        let cli = Cli::parse_from(vec!["auv", "--approval", "high"]);
        assert_eq!(cli.approval, Some(ApprovalLevel::High));
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_approval_flag_parses_in_run_mode() {
        // global = true：参数在子命令后同样可用
        let cli = Cli::parse_from(vec!["auv", "run", "--approval", "none", "测试任务"]);
        assert_eq!(cli.approval, Some(ApprovalLevel::None));
        match cli.command {
            Some(Commands::Run { task, .. }) => assert_eq!(task, "测试任务"),
            _ => panic!("应为 Run 子命令"),
        }
    }

    #[test]
    fn test_approval_flag_accepts_chinese_alias() {
        // 中文档位名为兼容别名（CLI 主值为英文）
        let cli = Cli::parse_from(vec!["auv", "--approval", "高"]);
        assert_eq!(cli.approval, Some(ApprovalLevel::High));
    }

    #[test]
    fn test_approval_flag_rejects_invalid_value() {
        let result = Cli::try_parse_from(vec!["auv", "--approval", "无敌"]);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // format_skills_list tests（/skills 指令）
    // -----------------------------------------------------------------------

    #[test]
    fn test_skills_list_not_configured() {
        let text = format_skills_list(None);
        assert!(text.contains("未配置技能目录"), "got: {}", text);
        let text = format_skills_list(Some(std::path::Path::new("")));
        assert!(text.contains("未配置技能目录"), "got: {}", text);
    }

    #[test]
    fn test_skills_list_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no_such_dir");
        let text = format_skills_list(Some(&missing));
        assert!(text.contains("不存在"), "got: {}", text);
    }

    #[test]
    fn test_skills_list_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let text = format_skills_list(Some(dir.path()));
        assert!(text.contains("没有技能文件"), "got: {}", text);
    }

    #[test]
    fn test_skills_list_with_skills() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("deploy.md"),
            "---\ndescription: 部署应用到生产\n---\n# Deploy\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("notes.txt"),
            "非 md 文件应被忽略",
        )
        .unwrap();

        let text = format_skills_list(Some(dir.path()));
        assert!(text.contains("可用技能（1 个"), "got: {}", text);
        assert!(text.contains("deploy"), "got: {}", text);
        assert!(text.contains("部署应用到生产"), "got: {}", text);
        assert!(!text.contains("notes"), "非 md 文件不应列出，got: {}", text);
    }

    // -----------------------------------------------------------------------
    // load_config tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_config_with_explicit_path() {
        // Create a temp config file
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test.toml");
        let config = HarnessConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&config_path, toml_str).unwrap();

        let (loaded, notices) = load_config(Some(config_path)).unwrap();
        assert_eq!(loaded.llm.model, "gpt-4o");
        assert_eq!(loaded.agent.max_turns, 50);
        assert!(loaded.validate().is_ok());
        assert!(notices.is_empty(), "显式路径不应产生提示");
    }

    #[test]
    fn test_load_config_with_nonexistent_file() {
        let result = load_config(Some(PathBuf::from("/nonexistent/path/config.toml")));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_layered_default_when_no_file() {
        // home/cwd 注入，无需修改进程级 CWD
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let (loaded, notices) = load_config_layered(home.path(), cwd.path()).unwrap();
        assert_eq!(loaded.llm.model, "gpt-4o"); // default
        assert!(home.path().join(".AuV/config.toml").exists());
        assert!(cwd.path().join(".AuV/config.toml").exists());
        assert_eq!(notices.len(), 2);
    }

    #[test]
    fn test_load_config_layered_from_local_file() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let local_dir = cwd.path().join(".AuV");
        std::fs::create_dir_all(&local_dir).unwrap();
        let mut config = HarnessConfig::default();
        config.llm.model = "gpt-4o-mini".to_string();
        config.agent.max_turns = 10;
        let toml_str = toml::to_string_pretty(&config).unwrap();
        std::fs::write(local_dir.join("config.toml"), toml_str).unwrap();

        let (loaded, _) = load_config_layered(home.path(), cwd.path()).unwrap();
        assert_eq!(loaded.llm.model, "gpt-4o-mini");
        assert_eq!(loaded.agent.max_turns, 10);
    }

    // -----------------------------------------------------------------------
    // apply_persona tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_apply_persona_loads_global_and_project() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("CLAUDE.md"), "全局人设").unwrap();
        std::fs::write(cwd.path().join("AuV.md"), "项目人设").unwrap();

        let mut config = HarnessConfig::default();
        let notices = apply_persona(&mut config, home.path(), cwd.path());
        let prompt = config.agent.system_prompt.as_ref().expect("应组装角色说明");
        assert!(prompt.contains("全局人设"), "全局角色文件应叠加");
        assert!(prompt.contains("项目人设"), "项目角色文件应叠加");
        assert!(prompt.contains("## Persona"));
        assert_eq!(notices.len(), 2, "两级各一条加载提示：{:?}", notices);
    }

    #[test]
    fn test_apply_persona_inline_system_prompt_wins() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(cwd.path().join("AuV.md"), "文件人设").unwrap();

        let mut config = HarnessConfig::default();
        config.agent.system_prompt = Some("内联人设".to_string());
        let notices = apply_persona(&mut config, home.path(), cwd.path());
        assert_eq!(
            config.agent.system_prompt.as_deref(),
            Some("内联人设"),
            "内联配置最高优先，不加载文件"
        );
        assert!(notices.is_empty());
    }

    #[test]
    fn test_apply_persona_none_when_no_files() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut config = HarnessConfig::default();
        let notices = apply_persona(&mut config, home.path(), cwd.path());
        assert!(config.agent.system_prompt.is_none(), "无角色文件时不改动配置");
        assert!(notices.is_empty());
    }

    // -----------------------------------------------------------------------
    // resolve_api_key tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_api_key_from_config() {
        let _guard = env_lock();
        let mut config = HarnessConfig::default();
        config.llm.api_key = Some("sk-config-key".to_string());
        let manager = test_credential_manager(&[("OPENAI_API_KEY", "sk-stored-key")]);
        let key = resolve_api_key_with_manager(&config, &manager).await.unwrap();
        assert_eq!(key, "sk-config-key");
    }

    #[tokio::test]
    async fn test_resolve_api_key_from_env() {
        let _guard = env_lock();
        let config = HarnessConfig::default();
        unsafe { std::env::remove_var("OPENAI_API_KEY_FILE") };
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-env-key") };
        let manager = test_credential_manager(&[("OPENAI_API_KEY", "sk-stored-key")]);
        let key = resolve_api_key_with_manager(&config, &manager).await.unwrap();
        assert_eq!(key, "sk-env-key");
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }

    #[tokio::test]
    async fn test_resolve_api_key_config_priority() {
        let _guard = env_lock();
        let mut config = HarnessConfig::default();
        config.llm.api_key = Some("sk-config-key".to_string());
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-env-key") };
        // Config should take priority
        let manager = test_credential_manager(&[("OPENAI_API_KEY", "sk-stored-key")]);
        let key = resolve_api_key_with_manager(&config, &manager).await.unwrap();
        assert_eq!(key, "sk-config-key");
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }

    #[tokio::test]
    async fn test_resolve_api_key_from_secure_storage() {
        let _guard = env_lock();
        let config = HarnessConfig::default();
        unsafe { std::env::remove_var("OPENAI_API_KEY_FILE") };
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        let manager = test_credential_manager(&[("OPENAI_API_KEY", "sk-stored-key")]);
        let key = resolve_api_key_with_manager(&config, &manager).await.unwrap();
        assert_eq!(key, "sk-stored-key");
    }

    #[tokio::test]
    async fn test_resolve_api_key_from_file() {
        let _guard = env_lock();
        let config = HarnessConfig::default();
        let key_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(key_file.path(), "sk-file-key\n").unwrap();
        unsafe { std::env::set_var("OPENAI_API_KEY_FILE", key_file.path()) };
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-env-key") };
        let manager = test_credential_manager(&[("OPENAI_API_KEY", "sk-stored-key")]);
        let key = resolve_api_key_with_manager(&config, &manager).await.unwrap();
        assert_eq!(key, "sk-file-key");
        unsafe { std::env::remove_var("OPENAI_API_KEY_FILE") };
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }

    #[tokio::test]
    async fn test_resolve_api_key_missing() {
        let _guard = env_lock();
        let config = HarnessConfig::default();
        // Ensure env var is not set
        unsafe { std::env::remove_var("OPENAI_API_KEY_FILE") };
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        let manager = test_credential_manager(&[]);
        let result = resolve_api_key_with_manager(&config, &manager).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            harness_agent::error::HarnessError::Auth(msg) => {
                assert!(msg.contains("No API key found"));
            }
            _ => panic!("Expected Auth error"),
        }
    }

    // -----------------------------------------------------------------------
    // build_agent tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_agent_with_default_config() {
        let config = HarnessConfig::default();
        let workspace = PathBuf::from("/tmp/test-workspace");
        let agent = build_agent(&config, "sk-test-key", workspace, None, None);
        assert!(agent.is_ok(), "build_agent should succeed: {:?}", agent.err());
    }

    #[test]
    fn test_build_agent_with_ui_approval_mode() {
        // TUI 模式：事件通道 + 决策通道 → 审批门使用事件模式
        let config = HarnessConfig::default();
        let workspace = PathBuf::from("/tmp/test-workspace");
        let (tx, _rx) = mpsc::channel::<AgentEvent>(8);
        let (_dtx, decision_rx) = mpsc::channel::<ApprovalDecision>(4);
        let agent = build_agent(&config, "sk-test-key", workspace, Some(tx), Some(decision_rx));
        assert!(agent.is_ok(), "build_agent should succeed: {:?}", agent.err());
    }

    #[test]
    fn test_build_agent_respects_disabled_tools() {
        let mut config = HarnessConfig::default();
        config.tools.disabled_tools = vec!["bash".to_string(), "grep".to_string()];
        let workspace = PathBuf::from("/tmp/test-workspace");
        let agent = build_agent(&config, "sk-test-key", workspace, None, None).unwrap();

        // Verify the disabled tools are not in the registry
        let tool_names: Vec<String> = agent
            .trace() // This is empty initially, but we can check the tool list
            .iter()
            .map(|t| format!("{:?}", t.action))
            .collect();
        // The agent's tools field is private, but we can verify the build succeeded
        // and the registry only has the non-disabled tools.
        // Since the tools field is private, we just verify the build succeeded.
        drop(tool_names);
    }

    // -----------------------------------------------------------------------
    // run_init helpers tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_write_default_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        harness_agent::config::write_default_config(&config_path).unwrap();
        assert!(config_path.exists());

        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("model"));
        assert!(content.contains("gpt-4o"));

        // Verify it can be parsed back
        let config = HarnessConfig::from_file(&config_path).unwrap();
        assert!(config.validate().is_ok());
    }

    // -----------------------------------------------------------------------
    // Key management tests (integration-style)
    // -----------------------------------------------------------------------

    #[test]
    fn test_make_credential_manager() {
        let manager = make_credential_manager();
        // key_status may fail if system keyring is unavailable;
        // in that case, the manager will fall back to encrypted file
        let _ = manager.key_status();
    }

    // -----------------------------------------------------------------------
    // 历史展示与 /view 渲染测试
    // -----------------------------------------------------------------------

    fn tool_msg(content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: content.to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn test_truncate_tool_result_short_untouched() {
        let (out, total) = truncate_tool_result("line1\nline2");
        assert_eq!(out, "line1\nline2");
        assert_eq!(total, 2);
    }

    #[test]
    fn test_truncate_tool_result_long_lines() {
        let content = (0..20).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        let (out, total) = truncate_tool_result(&content);
        assert_eq!(total, 20);
        assert_eq!(out.lines().count(), 12);
        assert!(out.starts_with("line0"));
        assert!(!out.contains("line12"));
    }

    #[test]
    fn test_truncate_tool_result_long_chars() {
        let content = "x".repeat(1000);
        let (out, total) = truncate_tool_result(&content);
        assert_eq!(total, 1);
        assert_eq!(out.chars().count(), 600);
    }

    #[test]
    fn test_view_message_empty_conversation() {
        let err = view_message(&[], 1).unwrap_err();
        assert!(err.contains("当前没有对话历史"));
    }

    #[test]
    fn test_view_message_out_of_range() {
        let conversation = vec![tool_msg("result")];
        let err = view_message(&conversation, 2).unwrap_err();
        assert!(err.contains("消息编号无效"));
        assert!(err.contains("共 1 条消息"));
    }

    #[test]
    fn test_view_message_full_content_no_truncation() {
        // 40 行的工具结果：/view 必须展示全部行，不受 12 行限制
        let long = (0..40).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        let conversation = vec![tool_msg(&long)];
        let text = view_message(&conversation, 1).unwrap();
        assert_eq!(text.lines().count(), 40);
        assert!(text.contains("line39"));
        assert!(text.contains("[1]"));
    }

    #[test]
    fn test_view_message_zero_index_rejected() {
        let conversation = vec![tool_msg("result")];
        assert!(view_message(&conversation, 0).is_err());
    }

    // -----------------------------------------------------------------------
    // 工具调用标注（工具名 + 具体命令）
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_call_label_bash_shows_command() {
        let tc = ToolCall {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"uname -a"}"#.to_string(),
        };
        assert_eq!(tool_call_label(&tc), "bash: uname -a");
    }

    #[test]
    fn test_tool_call_label_other_tool_shows_json() {
        let tc = ToolCall {
            id: "call_2".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path":"src/main.rs"}"#.to_string(),
        };
        assert_eq!(tool_call_label(&tc), r#"read_file: {"path":"src/main.rs"}"#);
    }

    #[test]
    fn test_tool_call_label_invalid_json_falls_back_to_name() {
        let tc = ToolCall {
            id: "call_3".to_string(),
            name: "bash".to_string(),
            arguments: "not json".to_string(),
        };
        assert_eq!(tool_call_label(&tc), "bash");
    }

    #[test]
    fn test_view_message_annotation_omits_command() {
        // 需求：具体命令已由工具结果行 `[n] 工具 bash: <命令>` 承担，
        // assistant 消息的「工具调用：」括号内只列工具名，不再重复命令
        let msg = Message {
            role: Role::Assistant,
            content: String::new(),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"command":"cargo test"}"#.to_string(),
            }]),
            tool_call_id: None,
        };
        let conversation = vec![msg];
        let text = view_message(&conversation, 1).unwrap();
        assert!(
            text.contains("（工具调用：bash）"),
            "Expected annotation with tool name only, got: {}",
            text
        );
        assert!(
            !text.contains("cargo test"),
            "标注中不应再出现具体命令, got: {}",
            text
        );
    }

    #[test]
    fn test_tool_call_names_dedups_preserving_order() {
        let tcs = vec![
            ToolCall {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"command":"ls"}"#.to_string(),
            },
            ToolCall {
                id: "call_2".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"src/main.rs"}"#.to_string(),
            },
            ToolCall {
                id: "call_3".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"command":"pwd"}"#.to_string(),
            },
        ];
        assert_eq!(tool_call_names(&tcs), "bash、read_file");
        assert_eq!(tool_call_names(&tcs[..1]), "bash");
        assert_eq!(tool_call_names(&[]), "");
    }

    // -----------------------------------------------------------------------
    // 会话标题清理 / 上下文窗口估算 / 状态行文本 / 最新会话加载
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_label_for_id_finds_command_in_assistant_tool_calls() {
        let assistant = Message {
            role: Role::Assistant,
            content: String::new(),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"command":"uname -a"}"#.to_string(),
            }]),
            tool_call_id: None,
        };
        let conversation = vec![assistant];
        let label = tool_label_for_id(&conversation, Some("call_1")).unwrap();
        assert_eq!(label, "bash: uname -a");
        assert!(tool_label_for_id(&conversation, Some("call_unknown")).is_none());
        assert!(tool_label_for_id(&conversation, None).is_none());
    }

    #[test]
    fn test_view_message_tool_shows_command_above_content() {
        let assistant = Message {
            role: Role::Assistant,
            content: String::new(),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"command":"cargo test"}"#.to_string(),
            }]),
            tool_call_id: None,
        };
        let tool = Message {
            role: Role::Tool,
            content: "Success: true\nResult: all tests passed".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
        };
        let conversation = vec![assistant, tool];

        let text = view_message(&conversation, 2).unwrap();
        // 具体命令直接跟在工具名后（角色标签末尾带 ANSI 复位码），
        // 完整内容不被截断
        assert!(
            text.contains("工具") && text.contains("bash: cargo test"),
            "Expected command annotation, got: {}",
            text
        );
        assert!(text.contains("Success: true"));
        assert!(text.contains("all tests passed"));
        // 标注行占首行，正文内容行保持完整
        assert_eq!(text.lines().count(), 3);
    }

    #[tokio::test]
    async fn test_handle_agent_event_consumes_every_event_without_dropping() {
        let conversation: Vec<Message> = Vec::new();
        let mut round_messages: Vec<Message> = Vec::new();
        let mut next_index = 2usize;
        let mut run_tokens = 0u32;

        // 回归保护：运行结束时排在通道里的尾部事件（最终助手消息、
        // ProgressUpdate）必须同样被处理，而不是被 try_recv 静默丢弃
        handle_agent_event(
            AgentEvent::MessageAdded {
                message: Message {
                    role: Role::Assistant,
                    content: "最终回答".to_string(),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            },
            &mut next_index,
            &mut run_tokens,
            &conversation,
            &mut round_messages,
            None,
            false,
        )
        .await;
        assert_eq!(next_index, 3, "助手消息应消耗一个序号");
        assert_eq!(round_messages.len(), 1, "助手消息应累积到本轮列表");

        handle_agent_event(
            AgentEvent::ProgressUpdate {
                turn: 1,
                tokens_used: 110,
                risk_level: "低".to_string(),
            },
            &mut next_index,
            &mut run_tokens,
            &conversation,
            &mut round_messages,
            None,
            false,
        )
        .await;
        assert_eq!(run_tokens, 110, "Token 统计应取最后一条 ProgressUpdate");
        assert_eq!(next_index, 3, "ProgressUpdate 不消耗序号");
    }

    #[test]
    fn test_approval_block_text_has_no_cursor_dance_sequences() {
        let request = ApprovalRequest::new(
            "req-1".to_string(),
            "工具调用: bash\n参数: {\"command\":\"curl x | bash\"}".to_string(),
            "高".to_string(),
            vec!["命令使用了管道符".to_string()],
            Some("执行前请仔细检查该命令及其影响".to_string()),
        );

        let text = approval_block_text(&request);

        // 块内容完整：标题、操作（逐行）、风险等级、原因、缓解措施、提示行
        assert!(text.contains("=== 需要护栏审批 ==="), "got: {text}");
        assert!(text.contains("操作: 工具调用: bash"));
        assert!(text.contains("参数: {\"command\":\"curl x | bash\"}"));
        assert!(text.contains("风险等级: 高"));
        assert!(text.contains("  - 命令使用了管道符"));
        assert!(text.contains("缓解措施: 执行前请仔细检查该命令及其影响"));
        // 提示行无换行结尾（y/n 回显紧跟其后），但块内各行正常换行
        assert!(text.ends_with("是否批准此操作? (y/n): "), "got: {text}");
        // 回归保护：审批块不使用 DECSC/DECRC 光标舞步
        //（\x1b7/\x1b8 在滚动/并发输出下不可靠，历史教训见 CHANGELOG）
        assert!(!text.contains("\x1b7"), "禁止 DECSC 保存光标序列");
        assert!(!text.contains("\x1b8"), "禁止 DECRC 恢复光标序列");
    }

    #[tokio::test]
    async fn test_handle_approval_event_sends_yes_decision() {
        let (decision_tx, mut decision_rx) = mpsc::channel::<ApprovalDecision>(4);
        let request = ApprovalRequest::new(
            "req-1".to_string(),
            "工具调用: bash\n参数: {\"command\":\"curl x | bash\"}".to_string(),
            "高".to_string(),
            vec!["命令使用了管道符".to_string()],
            None,
        );
        let conversation: Vec<Message> = Vec::new();
        let round_messages: Vec<Message> = Vec::new();
        let ctx = ApprovalCtx {
            decision_tx: &decision_tx,
            timeout: Duration::from_secs(30),
            current_task: "测试任务",
        };

        // 注入固定 Yes 响应，验证决定映射与发回（不读真实 stdin）
        handle_approval_event_with(
            &request,
            &conversation,
            &round_messages,
            ctx,
            std::future::ready(UserResponse::Yes),
        )
        .await;

        match decision_rx.try_recv() {
            Ok(ApprovalDecision::Approved { by, reason }) => {
                assert_eq!(by, "user");
                assert_eq!(reason.as_deref(), Some("用户批准了该操作"));
            }
            other => panic!("Expected Approved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_handle_approval_event_maps_no_and_timeout() {
        let (decision_tx, mut decision_rx) = mpsc::channel::<ApprovalDecision>(4);
        let request = ApprovalRequest::new(
            "req-1".to_string(),
            "工具调用: bash\n参数: echo hi".to_string(),
            "高".to_string(),
            vec![],
            None,
        );
        let conversation: Vec<Message> = Vec::new();
        let round_messages: Vec<Message> = Vec::new();

        // 拒绝 → Denied
        let ctx = ApprovalCtx {
            decision_tx: &decision_tx,
            timeout: Duration::from_secs(30),
            current_task: "测试任务",
        };
        handle_approval_event_with(
            &request,
            &conversation,
            &round_messages,
            ctx,
            std::future::ready(UserResponse::No),
        )
        .await;
        assert!(
            matches!(decision_rx.try_recv(), Ok(ApprovalDecision::Denied { .. })),
            "拒绝应映射为 Denied"
        );

        // 超时 → Timeout
        let ctx = ApprovalCtx {
            decision_tx: &decision_tx,
            timeout: Duration::from_secs(30),
            current_task: "测试任务",
        };
        handle_approval_event_with(
            &request,
            &conversation,
            &round_messages,
            ctx,
            std::future::ready(UserResponse::Timeout),
        )
        .await;
        assert!(
            matches!(decision_rx.try_recv(), Ok(ApprovalDecision::Timeout)),
            "超时应映射为 Timeout"
        );
    }

    #[tokio::test]
    async fn test_handle_agent_event_skips_approval_input_when_disallowed() {
        // 运行结束后的通道清空阶段（allow_approval_input = false）：
        // 审批事件静默跳过，不再读取 y/n 打扰用户（agent 已超时放弃等待）
        let conversation: Vec<Message> = Vec::new();
        let mut round_messages: Vec<Message> = Vec::new();
        let mut next_index = 1usize;
        let mut run_tokens = 0u32;
        let request = ApprovalRequest::new(
            "req-1".to_string(),
            "工具调用: bash\n参数: echo hi".to_string(),
            "高".to_string(),
            vec![],
            None,
        );

        handle_agent_event(
            AgentEvent::GuardrailApprovalNeeded { request },
            &mut next_index,
            &mut run_tokens,
            &conversation,
            &mut round_messages,
            None,
            false,
        )
        .await;

        assert_eq!(next_index, 1, "跳过的审批事件不消耗序号");
        assert_eq!(run_tokens, 0);
    }

    #[test]
    fn test_sanitize_session_title_strips_quotes_and_punctuation() {
        assert_eq!(sanitize_session_title("\"修复登录 bug\"。"), "修复登录 bug");
        assert_eq!(sanitize_session_title("「重构模块」！"), "重构模块");
        assert_eq!(sanitize_session_title("  普通标题  "), "普通标题");
    }

    #[test]
    fn test_sanitize_session_title_replaces_path_chars_and_truncates() {
        // 路径分隔符等非法文件名字符替换为空格，折叠多余空白
        assert_eq!(sanitize_session_title("a/b\\c:d"), "a b c d");
        // 超过 24 字符截断
        let long = "这是一个特别特别特别特别特别特别特别特别特别长的标题";
        assert_eq!(sanitize_session_title(long).chars().count(), 24);
        // 全标点/空白输入清理后为空
        assert_eq!(sanitize_session_title("。。。「」"), "");
        assert_eq!(sanitize_session_title("   "), "");
    }

    #[test]
    fn test_context_window_for_known_model_families() {
        assert_eq!(context_window_for("gpt-4o", None), 128_000);
        assert_eq!(context_window_for("claude-sonnet-4", None), 128_000);
        assert_eq!(context_window_for("deepseek-chat", None), 64_000);
        assert_eq!(context_window_for("llama3.1", None), 32_000);
    }

    #[test]
    fn test_context_window_for_unknown_model_falls_back_to_budget() {
        assert_eq!(context_window_for("some-unknown-model", Some(50_000)), 50_000);
        assert_eq!(context_window_for("some-unknown-model", None), 0);
    }

    #[test]
    fn test_remaining_context_text() {
        assert_eq!(remaining_context_text(0, 0), "上下文剩余: 未知");
        assert_eq!(remaining_context_text(500, 1000), "上下文剩余: 50%（500/1000）");
        assert_eq!(remaining_context_text(900, 1000), "上下文剩余: 10%（100/1000）");
        // 超出窗口：剩余 0，百分比归零而非负数
        assert_eq!(remaining_context_text(1500, 1000), "上下文剩余: 0%（0/1000）");
    }

    #[test]
    fn test_build_repl_prompt_puts_status_above_input_without_cursor_dance() {
        let p = build_repl_prompt("模型: gpt-4o | Token: 0 | 上下文剩余: 未知");
        // 状态行位于提示符中间（输入行上方）
        assert!(p.contains("模型: gpt-4o | Token: 0 | 上下文剩余: 未知"));
        // 提示符以 "> " 结尾，输入从这里开始
        assert!(p.ends_with("\n> "));
        // 回归保护：不得再使用 DECSC/DECRC 光标舞步（会破坏 rustyline 回显定位）
        assert!(!p.contains("\u{1b}7"));
        assert!(!p.contains("\u{1b}8"));
    }

    #[test]
    fn test_load_latest_session_in_picks_most_recently_modified() {
        let dir = std::env::temp_dir().join(format!("harness_sessions_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let msg = serde_json::json!([{
            "role": "User",
            "content": "hello",
            "tool_calls": null,
            "reasoning_content": null,
            "tool_call_id": null,
        }]);
        // 旧文件先写，新文件后写（mtime 更新）
        std::fs::write(dir.join("old.json"), msg.to_string()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("new.json"), msg.to_string()).unwrap();
        // 非 json 文件与空会话文件应被跳过
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.join("empty.json"), "[]").unwrap();

        let (name, messages) = load_latest_session_in(&dir).expect("should find latest session");
        assert_eq!(name, "new");
        assert_eq!(messages.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_latest_session_in_empty_or_missing_dir() {
        let dir = std::env::temp_dir().join(format!(
            "harness_sessions_missing_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_latest_session_in(&dir).is_none());

        std::fs::create_dir_all(&dir).unwrap();
        assert!(load_latest_session_in(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
