use std::collections::HashSet;
use std::fmt::Write as FmtWrite;
use std::io::{self, IsTerminal, Write as IoWrite};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::guardrails::assessor::RiskAssessment;
use crate::types::Action;

// ============================================================================
// ApprovalDecision
// ============================================================================

/// The outcome of a human-in-the-loop approval request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The action was approved.
    Approved {
        /// Who or what approved the action (e.g. "user", "session_whitelist").
        by: String,
        /// Optional human-readable reason for the approval.
        reason: Option<String>,
    },
    /// The action was explicitly denied.
    Denied {
        /// The reason the action was denied.
        reason: String,
    },
    /// The approval request timed out without a response.
    Timeout,
}

// ============================================================================
// ApprovalGate
// ============================================================================

/// A human-in-the-loop approval gate with session-scoped whitelisting.
///
/// The gate maintains a whitelist of action fingerprints that have been
/// approved during the current session.  When an action with a known
/// fingerprint is submitted it is auto-approved without prompting the user.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use harness_agent::guardrails::approval::ApprovalGate;
///
/// let gate = ApprovalGate::new(Duration::from_secs(30));
/// assert!(!gate.is_whitelisted("some-fingerprint"));
/// ```
pub struct ApprovalGate {
    /// How long to wait for user input before timing out.
    timeout: Duration,
    /// Set of action fingerprints that have been approved in this session.
    session_whitelist: HashSet<String>,
    /// UI 事件模式（TUI）：向 UI 发送审批请求并等待决定。
    /// None 表示 stdin 交互模式（REPL / 纯文本模式）。
    ui: Option<ApprovalUi>,
}

/// UI 事件模式的审批通道：审批请求以事件发给 UI 面板渲染，
/// 用户按键产生的决定通过决策通道返回。
struct ApprovalUi {
    timeout: Duration,
    event_tx: mpsc::Sender<crate::events::AgentEvent>,
    decision_rx: mpsc::Receiver<ApprovalDecision>,
}

impl ApprovalGate {
    /// Create a new approval gate with the given timeout (stdin 交互模式).
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            session_whitelist: HashSet::new(),
            ui: None,
        }
    }

    /// Create an approval gate in UI 事件模式.
    ///
    /// TUI 模式下 stdin 被 crossterm raw mode 接管，无法直接读取 y/n 输入，
    /// 因此审批请求以 `GuardrailApprovalNeeded` 事件发给 UI，并通过
    /// `decision_rx` 等待 UI 返回决定。
    pub fn with_ui_events(
        timeout: Duration,
        event_tx: mpsc::Sender<crate::events::AgentEvent>,
        decision_rx: mpsc::Receiver<ApprovalDecision>,
    ) -> Self {
        Self {
            timeout,
            session_whitelist: HashSet::new(),
            ui: Some(ApprovalUi {
                timeout,
                event_tx,
                decision_rx,
            }),
        }
    }

    /// Add a fingerprint to the session whitelist.
    ///
    /// Once whitelisted, future approval requests for the same fingerprint
    /// will be auto-approved without prompting.
    pub fn whitelist(&mut self, fingerprint: &str) {
        self.session_whitelist.insert(fingerprint.to_string());
    }

    /// Check whether a fingerprint is in the session whitelist.
    pub fn is_whitelisted(&self, fingerprint: &str) -> bool {
        self.session_whitelist.contains(fingerprint)
    }

    /// Request user approval for an action.
    ///
    /// The flow is:
    /// 1. Generate a deterministic fingerprint of the action.
    /// 2. If the fingerprint is whitelisted, return `Approved` immediately.
    /// 3. Ask the user: stdin 交互模式打印风险信息并读取 y/n；
    ///    UI 事件模式发送 `GuardrailApprovalNeeded` 事件并等待决定。
    /// 4. Wait for the response with the configured timeout.
    /// 5. If approved, add the fingerprint to the whitelist.
    ///
    /// This is an async function because it waits for user input with a
    /// tokio timeout.
    pub async fn request_approval(
        &mut self,
        action: &Action,
        assessment: &RiskAssessment,
    ) -> ApprovalDecision {
        // UI 事件模式（TUI）：不触碰 stdin，把审批请求发给 UI 并等待决定
        if let Some(ui) = self.ui.as_mut() {
            let fingerprint = fingerprint_action(action);
            // 会话白名单：已批准过的操作无需再次询问
            if self.session_whitelist.contains(&fingerprint) {
                return ApprovalDecision::Approved {
                    by: "session_whitelist".to_string(),
                    reason: Some("本会话中此前已批准".to_string()),
                };
            }
            let decision = await_ui_decision(ui, action, assessment).await;
            if matches!(&decision, ApprovalDecision::Approved { .. }) {
                self.whitelist(&fingerprint);
            }
            return decision;
        }

        // stdin 交互模式（纯文本/--no-tui；REPL 已改用 UI 事件模式，
        // 由 REPL 自身打印审批块并读取 y/n，见 main.rs handle_approval_event）
        let input = read_yes_no_with_timeout(self.timeout);
        self.request_approval_inner(action, assessment, input).await
    }

    /// Core approval logic shared with tests.
    ///
    /// Accepts a future that resolves to the user's response so that tests
    /// can inject a controlled input without touching real stdin.
    async fn request_approval_inner(
        &mut self,
        action: &Action,
        assessment: &RiskAssessment,
        input: impl std::future::Future<Output = UserResponse>,
    ) -> ApprovalDecision {
        let fingerprint = fingerprint_action(action);

        // Step 2: check whitelist
        if self.is_whitelisted(&fingerprint) {
            return ApprovalDecision::Approved {
                by: "session_whitelist".to_string(),
                reason: Some("本会话中此前已批准".to_string()),
            };
        }

        // Step 3: print risk info to stderr
        print_risk_info(action, assessment);

        // Step 4: wait for user input with timeout
        let decision = match input.await {
            UserResponse::Yes => {
                // Step 5: add to whitelist
                self.whitelist(&fingerprint);
                ApprovalDecision::Approved {
                    by: "user".to_string(),
                    reason: Some("用户批准了该操作".to_string()),
                }
            }
            UserResponse::No => ApprovalDecision::Denied {
                reason: "用户拒绝了该操作".to_string(),
            },
            UserResponse::Timeout => ApprovalDecision::Timeout,
        };

        // 清除审批提示块，让屏幕回到对话流（批准/拒绝/超时一致处理）
        clear_risk_info();

        decision
    }
}

/// UI 事件模式：发送审批请求事件，等待 UI 返回决定（带超时）。
async fn await_ui_decision(
    ui: &mut ApprovalUi,
    action: &Action,
    assessment: &RiskAssessment,
) -> ApprovalDecision {
    // 清空陈旧决定：上一轮审批中 agent 先超时、UI 后补发的 Timeout
    // 决定会残留在通道里，不清空会被本轮审批立即消费，导致用户还没
    // 看到审批提示就"超时"（历史 bug，见 CHANGELOG 根因记录）。
    // 必须在发送事件**之前**清空：UI 只会收到事件后才发决定。
    while ui.decision_rx.try_recv().is_ok() {}

    let request = crate::events::ApprovalRequest::new(
        uuid::Uuid::new_v4().to_string(),
        action_display(action),
        risk_level_cn(&assessment.level).to_string(),
        assessment.reasons.clone(),
        assessment.suggested_mitigation.clone(),
    );
    // try_send：事件通道由 UI 事件循环非阻塞轮询，不等待接收方
    let _ = ui
        .event_tx
        .try_send(crate::events::AgentEvent::GuardrailApprovalNeeded { request });

    match tokio::time::timeout(ui.timeout, ui.decision_rx.recv()).await {
        Ok(Some(decision @ ApprovalDecision::Approved { .. })) => decision,
        Ok(Some(decision @ ApprovalDecision::Denied { .. })) => decision,
        // 超时、通道关闭、或 UI 明确返回 Timeout 一律按超时处理
        Ok(Some(ApprovalDecision::Timeout)) | Ok(None) | Err(_) => ApprovalDecision::Timeout,
    }
}

// ============================================================================
// Action fingerprinting
// ============================================================================

/// Generate a deterministic fingerprint for an action.
///
/// The fingerprint is a hex-encoded SHA-256 hash of the action's key
/// properties.  Two actions with the same semantic meaning (same tool name
/// and parameters, same final answer text, etc.) will produce the same
/// fingerprint.  The `id` field of `ToolCall` is intentionally excluded so
/// that identical tool calls issued with different call IDs still match.
///
/// # Determinism
///
/// For `ToolCall` variants, the params are serialized via `serde_json` which
/// produces a canonical (sorted-key) output, ensuring deterministic hashing.
pub fn fingerprint_action(action: &Action) -> String {
    let mut hasher = Sha256::new();

    match action {
        Action::ToolCall { name, params, .. } => {
            hasher.update(b"tool_call:");
            hasher.update(name.as_bytes());
            hasher.update(b":");
            // serde_json serializes objects with sorted keys by default,
            // giving us deterministic output for the same params.
            let params_str = serde_json::to_string(params).unwrap_or_default();
            hasher.update(params_str.as_bytes());
        }
        Action::FinalAnswer { summary } => {
            hasher.update(b"final_answer:");
            hasher.update(summary.as_bytes());
        }
        Action::AskUser { question } => {
            hasher.update(b"ask_user:");
            hasher.update(question.as_bytes());
        }
        Action::NoOp => {
            hasher.update(b"noop");
        }
    }

    bytes_to_hex(&hasher.finalize())
}

/// Convert a byte slice to a lowercase hex string.
fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut s, "{:02x}", byte).expect("writing to String never fails");
    }
    s
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Outcome of the user-input read with timeout.
///
/// 公开仅供 REPL（bin crate）复用：审批处理用它读取 y/n 并映射为审批决定。
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserResponse {
    Yes,
    No,
    Timeout,
}

/// Print risk information about the action to stderr.
fn print_risk_info(action: &Action, assessment: &RiskAssessment) {
    let mut stderr = io::stderr().lock();

    // 保存光标位置（DECSC）：审批结束后整块清除，不留在对话流里
    if stderr.is_terminal() {
        let _ = write!(stderr, "\x1b7");
    }

    let _ = writeln!(stderr);
    let _ = writeln!(stderr, "=== 需要护栏审批 ===");
    let _ = writeln!(stderr, "操作: {}", action_display(action));
    let _ = writeln!(stderr, "风险等级: {}", risk_level_cn(&assessment.level));

    if !assessment.reasons.is_empty() {
        let _ = writeln!(stderr, "原因:");
        for reason in &assessment.reasons {
            let _ = writeln!(stderr, "  - {reason}");
        }
    }

    if let Some(ref mitigation) = assessment.suggested_mitigation {
        let _ = writeln!(stderr, "缓解措施: {mitigation}");
    }

    let _ = writeln!(stderr, "===================================");
    let _ = write!(stderr, "是否批准此操作? (y/n): ");
    let _ = stderr.flush();
}

/// 清除审批提示块：恢复打印前保存的光标位置（DECRC）并清除到屏幕底。
///
/// 依赖终端的光标保存/恢复能力；非终端（stderr 重定向）时不做任何事，
/// 避免在日志文件中写入转义序列。
fn clear_risk_info() {
    let mut stderr = io::stderr().lock();
    if stderr.is_terminal() {
        let _ = write!(stderr, "\x1b8\x1b[J");
        let _ = stderr.flush();
    }
}

/// Render an action as a human-readable Chinese description.
fn action_display(action: &Action) -> String {
    match action {
        Action::ToolCall { name, params, .. } => {
            format!("工具调用: {name}\n参数: {params}")
        }
        Action::FinalAnswer { summary } => format!("最终回答: {summary}"),
        Action::AskUser { question } => format!("询问用户: {question}"),
        Action::NoOp => "空操作".to_string(),
    }
}

/// Chinese name for a risk level.
fn risk_level_cn(level: &crate::guardrails::assessor::RiskLevel) -> &'static str {
    use crate::guardrails::assessor::RiskLevel;
    match level {
        RiskLevel::Low => "低",
        RiskLevel::Medium => "中",
        RiskLevel::High => "高",
        RiskLevel::Critical => "严重",
    }
}

/// Read a yes/no answer from stdin with a timeout.
///
/// 可取消的轮询式读取：审批期间临时把 stdin 切换为非规范输入，使用
/// `poll(2)` 检查可读性，再由 `read(2)` 读取。这样即使调用方留下了
/// `ICRNL`/`ISIG` 关闭的混合终端模式，Enter 和 Ctrl+C 仍然有效。
/// 终端属性通过 RAII 在所有返回路径完整恢复，不残留阻塞线程。
///
/// 审批期间按 Ctrl+C 视为「拒绝」：临时关闭 `ISIG` 后把 `0x03` 当作
/// 输入字节处理，同时监听外部 SIGINT，二者都会拒绝而不终止 REPL。
///
/// 历史 bug：旧实现 `spawn_blocking` + 阻塞 `read_line` 在超时后
/// 无法取消——阻塞线程永久泄漏在 stdin read 上，与 rustyline
/// 竞争同一个 fd，用户输入被随机吞掉（"输入文字难输入"），
/// 每次审批超时泄漏一个线程，见 CHANGELOG 根因记录。
///
/// 公开仅供 REPL（bin crate）复用：REPL 收到 `GuardrailApprovalNeeded`
/// 事件后同样用它读取 y/n（REPL 模式下 stdin 审批不再由 ApprovalGate
/// 内部的 stdin 分支承担）。
#[doc(hidden)]
pub async fn read_yes_no_with_timeout(timeout: Duration) -> UserResponse {
    read_yes_no_from_fd_with_timeout(0, timeout).await
}

async fn read_yes_no_from_fd_with_timeout(
    fd: libc::c_int,
    timeout: Duration,
) -> UserResponse {
    let terminal_mode = ApprovalTerminalMode::enter(fd);
    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::new();
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                discard_pending_input(fd, &terminal_mode);
                return UserResponse::Timeout;
            }
            r = &mut ctrl_c => {
                let _ = r;
                discard_pending_input(fd, &terminal_mode);
                return UserResponse::No;
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        match poll_fd_readable(fd) {
            StdinState::Readable => {
                let mut chunk = [0u8; 128];
                let n = unsafe {
                    libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len())
                };
                if n > 0 {
                    let bytes = &chunk[..n as usize];
                    if bytes.contains(&b'\x03') {
                        discard_pending_input(fd, &terminal_mode);
                        return UserResponse::No;
                    }
                    buf.extend_from_slice(bytes);
                    if line_complete(&buf) {
                        break;
                    }
                } else if n == 0 {
                    // stdin 关闭（EOF）→ 拒绝
                    return UserResponse::No;
                }
                // n < 0（EAGAIN 等）继续轮询
            }
            StdinState::Closed => return UserResponse::No,
            StdinState::NotReady => {}
        }
    }
    discard_pending_input(fd, &terminal_mode);
    let text = String::from_utf8_lossy(&buf);
    let trimmed = text.trim().to_lowercase();
    if trimmed == "y" || trimmed == "yes" {
        UserResponse::Yes
    } else {
        UserResponse::No
    }
}

/// 审批读取期间使用的终端模式。只接管输入语义，不改变输出处理和回显：
/// - 关闭 `ICANON`，避免异常的 CR 无法提交规范行；
/// - 开启 `ICRNL`，让 Enter 统一成为 `\n`；
/// - 关闭 `ISIG`，让 Ctrl+C 作为可识别的 `0x03` 字节到达读取器。
struct ApprovalTerminalMode {
    fd: libc::c_int,
    original: Option<libc::termios>,
}

impl ApprovalTerminalMode {
    fn enter(fd: libc::c_int) -> Self {
        // Some libc layouts contain reserved bytes that tcgetattr leaves untouched.
        let mut original = std::mem::MaybeUninit::<libc::termios>::zeroed();
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Self { fd, original: None };
        }

        let original = unsafe { original.assume_init() };
        let mut approval = original;
        approval.c_lflag &= !(libc::ICANON | libc::ISIG);
        approval.c_iflag &= !(libc::IGNCR | libc::INLCR);
        approval.c_iflag |= libc::ICRNL;
        approval.c_cc[libc::VMIN] = 1;
        approval.c_cc[libc::VTIME] = 0;

        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &approval) } != 0 {
            return Self { fd, original: None };
        }

        Self {
            fd,
            original: Some(original),
        }
    }

    fn controls_terminal(&self) -> bool {
        self.original.is_some()
    }
}

impl Drop for ApprovalTerminalMode {
    fn drop(&mut self) {
        if let Some(original) = self.original.as_ref() {
            unsafe {
                libc::tcflush(self.fd, libc::TCIFLUSH);
                libc::tcsetattr(self.fd, libc::TCSANOW, original);
            }
        }
    }
}

fn discard_pending_input(fd: libc::c_int, mode: &ApprovalTerminalMode) {
    if mode.controls_terminal() {
        unsafe {
            libc::tcflush(fd, libc::TCIFLUSH);
        }
    } else {
        drain_fd(fd);
    }
}

/// 行结束判定：`\n`（cooked 模式 Enter 经 ICRNL 翻译的结果）或
/// `\r`（意外 raw 模式下 ICRNL 未翻译）均视为行已完整。
///
/// 只认 `\n` 时，raw 环境（ICRNL off）下 Enter 永远无法完成行，
/// 审批读取会一直轮询到超时、用户输入完全无效（历史 bug）。
fn line_complete(buf: &[u8]) -> bool {
    buf.contains(&b'\n') || buf.contains(&b'\r')
}

/// stdin 可读状态（`poll(2)` 0 超时非阻塞检查）。
enum StdinState {
    /// 有数据可读（cooked 模式下即整行已到达，read 不会阻塞）。
    Readable,
    /// 无数据。
    NotReady,
    /// stdin 已关闭（EOF）。
    Closed,
}

/// 非阻塞检查 stdin（fd 0）是否可读。
fn poll_fd_readable(fd: libc::c_int) -> StdinState {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ret = unsafe { libc::poll(&mut pfd, 1, 0) };
    if ret > 0 {
        if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            StdinState::Closed
        } else if pfd.revents & libc::POLLIN != 0 {
            StdinState::Readable
        } else {
            StdinState::NotReady
        }
    } else {
        StdinState::NotReady
    }
}

/// 丢弃 stdin 中所有待读字节。
///
/// 审批读取结束/超时后调用：用户补输的 y 等残留输入不能留给
/// rustyline（会被读成下一轮任务）。cooked 模式下只丢弃已完成的
/// 整行；未回车的半行由内核行缓冲保留，属正常输入流程。
#[doc(hidden)]
pub fn drain_stdin() {
    drain_fd(0);
}

fn drain_fd(fd: libc::c_int) {
    loop {
        match poll_fd_readable(fd) {
            StdinState::Readable => {
                let mut chunk = [0u8; 128];
                let n = unsafe {
                    libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len())
                };
                if n <= 0 {
                    break;
                }
            }
            _ => break,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    struct TestPty {
        master: libc::c_int,
        slave: libc::c_int,
    }

    #[cfg(unix)]
    impl TestPty {
        fn with_hybrid_input_mode() -> Self {
            let mut master = -1;
            let mut slave = -1;
            let result = unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            assert_eq!(result, 0, "openpty failed: {}", io::Error::last_os_error());

            let mut mode = terminal_mode(slave);
            mode.c_lflag |= libc::ICANON | libc::ECHO;
            mode.c_lflag &= !libc::ISIG;
            mode.c_iflag &= !libc::ICRNL;
            let result = unsafe { libc::tcsetattr(slave, libc::TCSANOW, &mode) };
            assert_eq!(
                result,
                0,
                "tcsetattr failed: {}",
                io::Error::last_os_error()
            );

            Self { master, slave }
        }
    }

    #[cfg(unix)]
    impl Drop for TestPty {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.master);
                libc::close(self.slave);
            }
        }
    }

    #[cfg(unix)]
    fn terminal_mode(fd: libc::c_int) -> libc::termios {
        let mut mode = std::mem::MaybeUninit::<libc::termios>::zeroed();
        let result = unsafe { libc::tcgetattr(fd, mode.as_mut_ptr()) };
        assert_eq!(
            result,
            0,
            "tcgetattr failed: {}",
            io::Error::last_os_error()
        );
        unsafe { mode.assume_init() }
    }

    #[cfg(unix)]
    fn assert_terminal_mode_eq(expected: &libc::termios, actual: &libc::termios) {
        assert_eq!(actual.c_iflag, expected.c_iflag, "input flags changed");
        assert_eq!(actual.c_oflag, expected.c_oflag, "output flags changed");
        assert_eq!(actual.c_cflag, expected.c_cflag, "control flags changed");
        assert_eq!(actual.c_lflag, expected.c_lflag, "local flags changed");
        assert_eq!(actual.c_cc, expected.c_cc, "control characters changed");
        assert_eq!(
            unsafe { libc::cfgetispeed(actual) },
            unsafe { libc::cfgetispeed(expected) },
            "input speed changed"
        );
        assert_eq!(
            unsafe { libc::cfgetospeed(actual) },
            unsafe { libc::cfgetospeed(expected) },
            "output speed changed"
        );
    }

    #[cfg(unix)]
    async fn write_pty_after(master: libc::c_int, bytes: &'static [u8]) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let written = unsafe {
            libc::write(
                master,
                bytes.as_ptr() as *const libc::c_void,
                bytes.len(),
            )
        };
        assert_eq!(written, bytes.len() as isize);
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn tool_call_action(name: &str, params: serde_json::Value) -> Action {
        Action::ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            params,
        }
    }

    fn final_answer_action(summary: &str) -> Action {
        Action::FinalAnswer {
            summary: summary.to_string(),
        }
    }

    fn ask_user_action(question: &str) -> Action {
        Action::AskUser {
            question: question.to_string(),
        }
    }

    fn low_risk_assessment() -> RiskAssessment {
        RiskAssessment::low()
    }

    fn high_risk_assessment() -> RiskAssessment {
        RiskAssessment {
            level: crate::guardrails::assessor::RiskLevel::High,
            reasons: vec!["Dangerous command detected".to_string()],
            suggested_mitigation: Some("Review the command carefully".to_string()),
        }
    }

    // -----------------------------------------------------------------------
    // Whitelist tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_whitelist_is_whitelisted() {
        let mut gate = ApprovalGate::new(Duration::from_secs(30));

        assert!(!gate.is_whitelisted("fp-abc"));
        gate.whitelist("fp-abc");
        assert!(gate.is_whitelisted("fp-abc"));
        assert!(!gate.is_whitelisted("fp-xyz"));
    }

    #[tokio::test]
    async fn test_whitelist_auto_approves() {
        let action = tool_call_action("bash", json!({"command": "cargo test"}));
        let fingerprint = fingerprint_action(&action);

        let mut gate = ApprovalGate::new(Duration::from_secs(30));
        gate.whitelist(&fingerprint);

        let assessment = low_risk_assessment();
        let decision = gate.request_approval(&action, &assessment).await;

        match decision {
            ApprovalDecision::Approved { by, reason } => {
                assert_eq!(by, "session_whitelist");
                assert!(reason.is_some());
            }
            other => panic!("Expected Approved, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Timeout tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_read_yes_no_eof_returns_no_without_blocking() {
        // 非交互环境（测试）stdin 为 EOF（/dev/null 或已关闭管道）：
        // poll 报告关闭/read 返回 0 → 立即返回 No，不阻塞整个 timeout
        let start = tokio::time::Instant::now();
        let response = read_yes_no_with_timeout(Duration::from_secs(5)).await;
        assert_eq!(response, UserResponse::No);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "EOF 应立即返回，不应阻塞到超时"
        );
    }

    #[test]
    fn test_line_complete_accepts_lf_and_cr() {
        // cooked 模式：Enter 经 ICRNL 翻译为 \n
        assert!(line_complete(b"y\n"));
        // 意外 raw 模式：ICRNL 未翻译，Enter 为 \r，同样应完成行
        assert!(line_complete(b"y\r"));
        assert!(line_complete(b"yes\r"));
        // 行未完成
        assert!(!line_complete(b"y"));
        assert!(!line_complete(b""));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_hybrid_terminal_accepts_y_carriage_return_and_restores_mode() {
        let pty = TestPty::with_hybrid_input_mode();
        let original = terminal_mode(pty.slave);
        let writer = tokio::spawn(write_pty_after(pty.master, b"y\r"));

        let response =
            read_yes_no_from_fd_with_timeout(pty.slave, Duration::from_secs(1)).await;

        writer.await.unwrap();
        assert_eq!(response, UserResponse::Yes);
        assert_terminal_mode_eq(&original, &terminal_mode(pty.slave));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_hybrid_terminal_treats_ctrl_c_byte_as_no_and_restores_mode() {
        let pty = TestPty::with_hybrid_input_mode();
        let original = terminal_mode(pty.slave);
        let writer = tokio::spawn(write_pty_after(pty.master, b"\x03"));

        let response =
            read_yes_no_from_fd_with_timeout(pty.slave, Duration::from_secs(1)).await;

        writer.await.unwrap();
        assert_eq!(response, UserResponse::No);
        assert_terminal_mode_eq(&original, &terminal_mode(pty.slave));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_hybrid_terminal_timeout_discards_partial_input_and_restores_mode() {
        let pty = TestPty::with_hybrid_input_mode();
        let original = terminal_mode(pty.slave);
        let writer = tokio::spawn(write_pty_after(pty.master, b"y"));

        let response =
            read_yes_no_from_fd_with_timeout(pty.slave, Duration::from_millis(150)).await;

        writer.await.unwrap();
        assert_eq!(response, UserResponse::Timeout);
        assert_terminal_mode_eq(&original, &terminal_mode(pty.slave));

        let mut inspect_mode = original;
        inspect_mode.c_lflag &= !libc::ICANON;
        inspect_mode.c_cc[libc::VMIN] = 1;
        inspect_mode.c_cc[libc::VTIME] = 0;
        assert_eq!(
            unsafe { libc::tcsetattr(pty.slave, libc::TCSANOW, &inspect_mode) },
            0
        );
        assert!(matches!(poll_fd_readable(pty.slave), StdinState::NotReady));
        assert_eq!(
            unsafe { libc::tcsetattr(pty.slave, libc::TCSANOW, &original) },
            0
        );
    }

    #[tokio::test]
    async fn test_read_yes_no_ctrl_c_returns_no_without_killing_process() {
        // 审批期间按 Ctrl+C（SIGINT）：cooked 模式（ISIG）下默认动作会
        // 直接终止进程。修复后把 Ctrl+C 解释为「拒绝」，进程存活、
        // 界面得以重绘（历史 bug：审批中误按 Ctrl+C 整个 REPL 退出）
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            unsafe { libc::raise(libc::SIGINT) };
        });
        let response = read_yes_no_with_timeout(Duration::from_secs(5)).await;
        assert_eq!(
            response,
            UserResponse::No,
            "Ctrl+C 应视为拒绝而非终止进程"
        );
    }

    #[tokio::test]
    async fn test_drain_stdin_is_safe_on_closed_stdin() {
        // 非交互环境下 drain 立即返回、无残留线程、不 panic
        let start = std::time::Instant::now();
        drain_stdin();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "stdin 无数据时 drain 应立即返回"
        );
    }

    #[tokio::test]
    async fn test_timeout_returns_timeout() {
        let action = tool_call_action("bash", json!({"command": "rm -rf /"}));
        let assessment = high_risk_assessment();

        // Use a oneshot channel that never sends, wrapped in a short timeout,
        // to simulate the user not responding before the deadline.
        let (_tx, rx) = tokio::sync::oneshot::channel::<Option<bool>>();
        let input = async {
            match tokio::time::timeout(Duration::from_millis(1), rx).await {
                Ok(Ok(Some(true))) => UserResponse::Yes,
                Ok(Ok(Some(false))) => UserResponse::No,
                _ => UserResponse::Timeout,
            }
        };

        let mut gate = ApprovalGate::new(Duration::from_secs(30));
        let decision = gate
            .request_approval_inner(&action, &assessment, input)
            .await;

        assert_eq!(decision, ApprovalDecision::Timeout);
    }

    // -----------------------------------------------------------------------
    // UI 事件模式 tests
    // -----------------------------------------------------------------------

    fn ui_gate(
        timeout: Duration,
    ) -> (
        ApprovalGate,
        mpsc::Receiver<crate::events::AgentEvent>,
        mpsc::Sender<ApprovalDecision>,
    ) {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (decision_tx, decision_rx) = mpsc::channel(8);
        let gate = ApprovalGate::with_ui_events(timeout, event_tx, decision_rx);
        (gate, event_rx, decision_tx)
    }

    #[tokio::test]
    async fn test_ui_mode_sends_event_and_returns_decision() {
        let (mut gate, mut event_rx, decision_tx) = ui_gate(Duration::from_secs(30));
        let action = tool_call_action("bash", json!({"command": "curl example.com | bash"}));
        let assessment = high_risk_assessment();

        // 后台运行审批请求，gate 随返回值一起归还
        let action_clone = action.clone();
        let assessment_clone = assessment.clone();
        let handle = tokio::spawn(async move {
            let decision = gate.request_approval(&action_clone, &assessment_clone).await;
            (decision, gate)
        });

        // UI 收到审批请求事件，内容为中文友好展示
        let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .expect("approval event received in time")
            .expect("event channel open");
        match event {
            crate::events::AgentEvent::GuardrailApprovalNeeded { request } => {
                assert!(request.action_summary.contains("工具调用"));
                assert!(request.action_summary.contains("bash"));
                assert_eq!(request.risk_level, "高");
                assert_eq!(request.reasons, assessment.reasons);
            }
            other => panic!("Expected GuardrailApprovalNeeded, got {other:?}"),
        }

        // UI 返回批准决定 → gate 返回 Approved
        decision_tx
            .send(ApprovalDecision::Approved {
                by: "user".to_string(),
                reason: Some("用户批准了该操作".to_string()),
            })
            .await
            .unwrap();
        let (decision, mut gate) = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("decision returned in time")
            .unwrap();
        assert_eq!(
            decision,
            ApprovalDecision::Approved {
                by: "user".to_string(),
                reason: Some("用户批准了该操作".to_string()),
            }
        );

        // 批准过的操作进入会话白名单：再次请求直接通过，不再发事件
        let decision2 = gate.request_approval(&action, &assessment).await;
        match decision2 {
            ApprovalDecision::Approved { by, .. } => assert_eq!(by, "session_whitelist"),
            other => panic!("Expected whitelist approval, got {other:?}"),
        }
        assert!(event_rx.try_recv().is_err(), "白名单命中不应再发事件");
    }

    #[tokio::test]
    async fn test_ui_mode_deny() {
        let (mut gate, mut event_rx, decision_tx) = ui_gate(Duration::from_secs(30));
        let action = tool_call_action("bash", json!({"command": "rm -rf /tmp/x"}));
        let assessment = high_risk_assessment();

        let action_clone = action.clone();
        let assessment_clone = assessment.clone();
        let handle = tokio::spawn(async move {
            let decision = gate.request_approval(&action_clone, &assessment_clone).await;
            (decision, gate)
        });

        let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .expect("approval event received in time");
        assert!(matches!(
            event,
            Some(crate::events::AgentEvent::GuardrailApprovalNeeded { .. })
        ));

        decision_tx
            .send(ApprovalDecision::Denied {
                reason: "用户拒绝了该操作".to_string(),
            })
            .await
            .unwrap();

        let (decision, _gate) = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("decision returned in time")
            .unwrap();
        assert_eq!(
            decision,
            ApprovalDecision::Denied {
                reason: "用户拒绝了该操作".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn test_ui_mode_drains_stale_decision_before_waiting() {
        let (mut gate, mut event_rx, decision_tx) = ui_gate(Duration::from_millis(200));
        let action = tool_call_action("bash", json!({"command": "sudo reboot"}));
        let assessment = high_risk_assessment();

        // 上一轮审批遗留的陈旧 Timeout 决定：若不清空，本轮审批的
        // recv() 会立即消费它，用户还没看到提示就"超时"（历史 bug）
        decision_tx.send(ApprovalDecision::Timeout).await.unwrap();

        let action_clone = action.clone();
        let assessment_clone = assessment.clone();
        let handle = tokio::spawn(async move {
            let decision = gate.request_approval(&action_clone, &assessment_clone).await;
            (decision, gate)
        });

        // UI 收到本轮审批请求事件
        let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .expect("approval event received in time");
        assert!(matches!(
            event,
            Some(crate::events::AgentEvent::GuardrailApprovalNeeded { .. })
        ));

        // 用户批准：修复后应等到真实决定，而不是立即消费陈旧 Timeout
        decision_tx
            .send(ApprovalDecision::Approved {
                by: "user".to_string(),
                reason: Some("用户批准了该操作".to_string()),
            })
            .await
            .unwrap();

        let (decision, _gate) = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("decision returned in time")
            .unwrap();
        assert_eq!(
            decision,
            ApprovalDecision::Approved {
                by: "user".to_string(),
                reason: Some("用户批准了该操作".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn test_ui_mode_timeout() {
        let (mut gate, mut event_rx, _decision_tx) = ui_gate(Duration::from_millis(50));
        let action = tool_call_action("bash", json!({"command": "sudo reboot"}));
        let assessment = high_risk_assessment();

        let action_clone = action.clone();
        let assessment_clone = assessment.clone();
        let handle = tokio::spawn(async move {
            let decision = gate.request_approval(&action_clone, &assessment_clone).await;
            (decision, gate)
        });

        let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .expect("approval event received in time");
        assert!(matches!(
            event,
            Some(crate::events::AgentEvent::GuardrailApprovalNeeded { .. })
        ));

        // 不发送决定 → gate 在 50ms 后超时
        let (decision, _gate) = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("decision returned in time")
            .unwrap();
        assert_eq!(decision, ApprovalDecision::Timeout);
    }

    // -----------------------------------------------------------------------
    // Different actions not whitelisted
    // -----------------------------------------------------------------------

    #[test]
    fn test_different_actions_not_whitelisted() {
        let action_a = tool_call_action("bash", json!({"command": "cargo build"}));
        let action_b = tool_call_action("bash", json!({"command": "cargo test"}));

        let fp_a = fingerprint_action(&action_a);
        let fp_b = fingerprint_action(&action_b);

        let mut gate = ApprovalGate::new(Duration::from_secs(30));
        gate.whitelist(&fp_a);

        assert!(gate.is_whitelisted(&fp_a));
        assert!(!gate.is_whitelisted(&fp_b));
    }

    #[test]
    fn test_different_tool_names_not_whitelisted() {
        let action_a = tool_call_action("bash", json!({"command": "ls"}));
        let action_b = tool_call_action("read_file", json!({"path": "src/main.rs"}));

        let fp_a = fingerprint_action(&action_a);
        let fp_b = fingerprint_action(&action_b);

        let mut gate = ApprovalGate::new(Duration::from_secs(30));
        gate.whitelist(&fp_a);

        assert!(gate.is_whitelisted(&fp_a));
        assert!(!gate.is_whitelisted(&fp_b));
    }

    // -----------------------------------------------------------------------
    // Fingerprinting tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_fingerprint_is_deterministic() {
        let action = tool_call_action("bash", json!({"command": "cargo test"}));

        let fp1 = fingerprint_action(&action);
        let fp2 = fingerprint_action(&action);

        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_different_params_different_hash() {
        let action_a = tool_call_action("bash", json!({"command": "cargo build"}));
        let action_b = tool_call_action("bash", json!({"command": "cargo test"}));

        let fp_a = fingerprint_action(&action_a);
        let fp_b = fingerprint_action(&action_b);

        assert_ne!(fp_a, fp_b);
    }

    #[test]
    fn test_fingerprint_different_tool_names_different_hash() {
        let action_a = tool_call_action("bash", json!({"command": "ls"}));
        let action_b = tool_call_action("read_file", json!({"path": "src/main.rs"}));

        let fp_a = fingerprint_action(&action_a);
        let fp_b = fingerprint_action(&action_b);

        assert_ne!(fp_a, fp_b);
    }

    #[test]
    fn test_fingerprint_same_params_different_id_same_hash() {
        // The id field is intentionally excluded from the fingerprint so that
        // identical tool calls with different call IDs are treated as the same
        // action.
        let action_a = Action::ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            params: json!({"command": "cargo test"}),
        };
        let action_b = Action::ToolCall {
            id: "call-999".to_string(),
            name: "bash".to_string(),
            params: json!({"command": "cargo test"}),
        };

        assert_eq!(fingerprint_action(&action_a), fingerprint_action(&action_b));
    }

    #[test]
    fn test_fingerprint_noop_is_deterministic() {
        let fp1 = fingerprint_action(&Action::NoOp);
        let fp2 = fingerprint_action(&Action::NoOp);

        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_final_answer_is_deterministic() {
        let action = final_answer_action("Task completed successfully");
        let fp1 = fingerprint_action(&action);
        let fp2 = fingerprint_action(&action);

        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_ask_user_is_deterministic() {
        let action = ask_user_action("Continue?");
        let fp1 = fingerprint_action(&action);
        let fp2 = fingerprint_action(&action);

        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_different_action_types_different_hash() {
        let fp_tool = fingerprint_action(&tool_call_action("bash", json!({"cmd": "ls"})));
        let fp_noop = fingerprint_action(&Action::NoOp);
        let fp_final = fingerprint_action(&final_answer_action("done"));
        let fp_ask = fingerprint_action(&ask_user_action("go?"));

        assert_ne!(fp_tool, fp_noop);
        assert_ne!(fp_tool, fp_final);
        assert_ne!(fp_tool, fp_ask);
        assert_ne!(fp_noop, fp_final);
        assert_ne!(fp_noop, fp_ask);
        assert_ne!(fp_final, fp_ask);
    }

    // -----------------------------------------------------------------------
    // ApprovalDecision tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_approval_decision_equality() {
        let a = ApprovalDecision::Approved {
            by: "user".to_string(),
            reason: Some("ok".to_string()),
        };
        let b = ApprovalDecision::Approved {
            by: "user".to_string(),
            reason: Some("ok".to_string()),
        };
        assert_eq!(a, b);

        let d1 = ApprovalDecision::Denied {
            reason: "no".to_string(),
        };
        let d2 = ApprovalDecision::Denied {
            reason: "no".to_string(),
        };
        assert_eq!(d1, d2);

        assert_eq!(ApprovalDecision::Timeout, ApprovalDecision::Timeout);
    }

    // -----------------------------------------------------------------------
    // Bytes-to-hex helper
    // -----------------------------------------------------------------------

    #[test]
    fn test_bytes_to_hex() {
        assert_eq!(bytes_to_hex(&[0x00]), "00");
        assert_eq!(bytes_to_hex(&[0xff]), "ff");
        assert_eq!(bytes_to_hex(&[0x0a, 0x1b, 0x2c]), "0a1b2c");
        assert_eq!(bytes_to_hex(&[]), "");
    }
}
