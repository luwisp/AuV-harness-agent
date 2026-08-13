use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::types::Action;

// ============================================================================
// SandboxViolation
// ============================================================================

/// The category of a sandbox violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationType {
    /// An action targeted a file path outside the workspace root.
    PathOutsideWorkspace,
    /// A command was executed that is not in the allowed list.
    CommandNotAllowed,
    /// A command was executed that is in the forbidden list.
    CommandForbidden,
    /// The requested timeout exceeds the maximum allowed timeout.
    TimeoutExceeded,
    /// Network access was attempted when network is disabled.
    NetworkAccessBlocked,
}

/// A violation detected by the sandbox boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxViolation {
    /// Human-readable description of the violation.
    pub message: String,
    /// The category of violation.
    pub violation_type: ViolationType,
}

impl SandboxViolation {
    pub fn new(message: impl Into<String>, violation_type: ViolationType) -> Self {
        Self {
            message: message.into(),
            violation_type,
        }
    }
}

// ============================================================================
// SandboxBoundary
// ============================================================================

/// The outermost guardrail layer (Layer 4) that enforces hard boundaries on
/// what actions can be performed within the harness.
///
/// Unlike the rule engine (Layer 1) or risk assessor (Layer 2), the sandbox
/// does not make permissive judgments — it applies rigid constraints that
/// cannot be overridden by approval or escalation.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use std::time::Duration;
/// use harness_agent::guardrails::sandbox::SandboxBoundary;
///
/// let boundary = SandboxBoundary {
///     workspace_root: PathBuf::from("/home/user/project"),
///     allowed_commands: vec![],
///     forbidden_commands: vec!["rm".into(), "sudo".into()],
///     max_timeout: Duration::from_secs(300),
///     network_allowed: false,
/// };
/// ```
pub struct SandboxBoundary {
    /// The root directory of the workspace.  All file operations must target
    /// paths within this directory.
    pub workspace_root: PathBuf,
    /// If non-empty, only commands whose base name appears in this list are
    /// permitted.  When empty, all commands are allowed (subject to the
    /// forbidden list).
    pub allowed_commands: Vec<String>,
    /// Commands whose base name appears in this list are always rejected,
    /// regardless of the allowed list.
    pub forbidden_commands: Vec<String>,
    /// The maximum duration a bash command may run.  Actions that request a
    /// longer timeout are rejected.
    pub max_timeout: Duration,
    /// Whether network access is permitted.  When `false`, actions that attempt
    /// to make network requests (curl, wget, fetch tool, etc.) are rejected.
    pub network_allowed: bool,
}

impl SandboxBoundary {
    /// Validate an action against the sandbox boundaries.
    ///
    /// Returns `Ok(())` if the action passes all checks, or a
    /// `SandboxViolation` describing the first boundary that was breached.
    pub fn validate(&self, action: &Action) -> Result<(), SandboxViolation> {
        // 1. Check file paths are within workspace_root
        self.check_file_paths(action)?;

        // 2. Check bash commands against allowed/forbidden lists
        self.check_command_lists(action)?;

        // 3. Check timeout against max_timeout
        self.check_timeout(action)?;

        // 4. Check network access if network_allowed is false
        if !self.network_allowed {
            self.check_network_access(action)?;
        }

        Ok(())
    }

    /// Wrap a shell command with a timeout prefix so it cannot run longer than
    /// `max_timeout`.
    ///
    /// The returned string is suitable for passing to a shell.
    pub fn wrap_command(&self, cmd: &str) -> String {
        let seconds = self.max_timeout.as_secs();
        format!("timeout {} {}", seconds, cmd)
    }

    // ------------------------------------------------------------------
    // Internal checks
    // ------------------------------------------------------------------

    /// Check that every file path referenced by the action is within the
    /// workspace root.
    fn check_file_paths(&self, action: &Action) -> Result<(), SandboxViolation> {
        if let Some(path) = extract_file_path(action) {
            let resolved = resolve_path(&path, &self.workspace_root);
            if !resolved.starts_with(&self.workspace_root) {
                return Err(SandboxViolation::new(
                    format!(
                        "文件路径 '{}'（解析为 '{}'）位于工作区根目录 '{}' 之外",
                        path,
                        resolved.display(),
                        self.workspace_root.display()
                    ),
                    ViolationType::PathOutsideWorkspace,
                ));
            }
        }
        Ok(())
    }

    /// Check that any bash command in the action is allowed.
    fn check_command_lists(&self, action: &Action) -> Result<(), SandboxViolation> {
        let command = match extract_command(action) {
            Some(cmd) => cmd,
            None => return Ok(()),
        };

        let base = command_base(&command);

        // Check forbidden list first — it takes priority.
        for forbidden in &self.forbidden_commands {
            if base == *forbidden {
                return Err(SandboxViolation::new(
                    format!(
                        "命令 '{}' 被禁止（基础命令: '{}'）",
                        command, base
                    ),
                    ViolationType::CommandForbidden,
                ));
            }
        }

        // If allowed list is non-empty, the command must be in it.
        if !self.allowed_commands.is_empty() {
            if !self.allowed_commands.iter().any(|a| *a == base) {
                return Err(SandboxViolation::new(
                    format!(
                        "命令 '{}' 不在允许列表中（基础命令: '{}'）",
                        command, base
                    ),
                    ViolationType::CommandNotAllowed,
                ));
            }
        }

        Ok(())
    }

    /// Check that the action's timeout (if specified) does not exceed
    /// `max_timeout`.
    fn check_timeout(&self, action: &Action) -> Result<(), SandboxViolation> {
        if let Some(requested) = extract_timeout(action) {
            if requested > self.max_timeout {
                return Err(SandboxViolation::new(
                    format!(
                        "请求的超时时间 {:?} 超过最大允许值 {:?}",
                        requested, self.max_timeout
                    ),
                    ViolationType::TimeoutExceeded,
                ));
            }
        }
        Ok(())
    }

    /// Check that the action does not attempt network access.
    fn check_network_access(&self, action: &Action) -> Result<(), SandboxViolation> {
        // Check for network-related tool calls (fetch, web_fetch, etc.)
        if is_network_tool_call(action) {
            return Err(SandboxViolation::new(
                "网络访问已禁用；网络工具调用被拦截".to_string(),
                ViolationType::NetworkAccessBlocked,
            ));
        }

        // Check for network-related commands (curl, wget)
        if let Some(cmd) = extract_command(action) {
            if has_network_command(&cmd) {
                return Err(SandboxViolation::new(
                    format!(
                        "网络访问已禁用；命令 '{}' 会发起网络请求",
                        cmd
                    ),
                    ViolationType::NetworkAccessBlocked,
                ));
            }
        }

        Ok(())
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Extract a file path from a tool-call action.
fn extract_file_path(action: &Action) -> Option<String> {
    match action {
        Action::ToolCall { name, params, .. } => {
            let file_tools = [
                "read_file",
                "write_file",
                "edit_file",
                "create_file",
                "delete_file",
                "read",
                "write",
                "edit",
                "delete",
                "remove_file",
                "rm",
                "open_file",
                "save_file",
                "replace",
            ];
            if file_tools.contains(&name.as_str()) {
                params
                    .get("path")
                    .or_else(|| params.get("file_path"))
                    .or_else(|| params.get("file"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Resolve a path relative to the workspace root.
///
/// If `path` is absolute it is used as-is.  If it is relative it is resolved
/// against `workspace_root` and canonicalized where possible.
fn resolve_path(path: &str, workspace_root: &Path) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        // Resolve relative path against workspace root, then clean up.
        let joined = workspace_root.join(p);
        // Attempt canonicalization; fall back to the joined path if it fails
        // (e.g. the file does not exist yet).
        joined.canonicalize().unwrap_or_else(|_| clean_path(&joined))
    }
}

/// Clean a path by removing `.` and `..` components without touching the
/// filesystem.
fn clean_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                components.pop();
            }
            other => components.push(other),
        }
    }
    components.into_iter().collect()
}

/// Extract a command string from a tool-call action.
fn extract_command(action: &Action) -> Option<String> {
    match action {
        Action::ToolCall { name, params, .. } => {
            let command_tools = [
                "bash", "execute", "run_command", "shell", "sh", "cmd",
            ];
            if command_tools.contains(&name.as_str()) {
                params
                    .get("command")
                    .or_else(|| params.get("cmd"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract a timeout duration from an action's params, if present.
fn extract_timeout(action: &Action) -> Option<Duration> {
    match action {
        Action::ToolCall { params, .. } => {
            params
                .get("timeout")
                .or_else(|| params.get("timeout_ms"))
                .or_else(|| params.get("timeout_secs"))
                .and_then(|v| {
                    if let Some(ms) = v.as_u64() {
                        // If the key is timeout_ms, treat as milliseconds.
                        // Otherwise default to seconds.
                        if params.get("timeout_ms").is_some() {
                            Some(Duration::from_millis(ms))
                        } else {
                            Some(Duration::from_secs(ms))
                        }
                    } else if let Some(f) = v.as_f64() {
                        if params.get("timeout_ms").is_some() {
                            Some(Duration::from_millis(f as u64))
                        } else {
                            Some(Duration::from_secs(f as u64))
                        }
                    } else {
                        None
                    }
                })
        }
        _ => None,
    }
}

/// Extract the base command name (first word) from a command string, ignoring
/// leading variable assignments and environment overrides.
fn command_base(command: &str) -> String {
    let trimmed = command.trim();
    // Skip leading variable assignments like `VAR=val cmd`
    let rest = trimmed
        .split_whitespace()
        .skip_while(|w| w.contains('=') && !w.starts_with('='))
        .collect::<Vec<_>>();

    rest.first()
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Check whether a command string contains network-related commands.
fn has_network_command(command: &str) -> bool {
    let lower = command.to_lowercase();
    let network_commands = [
        "curl", "wget", "nc", "netcat", "ncat", "telnet", "ssh", "scp", "ftp", "sftp",
        "rsync", "http", "https",
    ];
    let base = command_base(command).to_lowercase();
    network_commands.contains(&base.as_str())
        || lower.contains("curl ")
        || lower.contains("wget ")
        || lower.contains("nc ")
        || lower.contains("telnet ")
}

/// Check whether an action is a network-related tool call.
fn is_network_tool_call(action: &Action) -> bool {
    match action {
        Action::ToolCall { name, .. } => {
            let network_tools = [
                "fetch",
                "web_fetch",
                "http_request",
                "web_request",
                "download",
                "upload",
                "api_call",
                "request",
                "web_search",
                "websearch",
            ];
            network_tools.contains(&name.as_str())
        }
        _ => false,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::Duration;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn default_boundary() -> SandboxBoundary {
        SandboxBoundary {
            workspace_root: PathBuf::from("/home/user/project"),
            allowed_commands: vec![],
            forbidden_commands: vec!["rm".into(), "sudo".into()],
            max_timeout: Duration::from_secs(300),
            network_allowed: false,
        }
    }

    fn bash_action(command: &str) -> Action {
        Action::ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            params: json!({"command": command}),
        }
    }

    fn write_file_action(path: &str) -> Action {
        Action::ToolCall {
            id: "call-1".into(),
            name: "write_file".into(),
            params: json!({"path": path}),
        }
    }

    fn read_file_action(path: &str) -> Action {
        Action::ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            params: json!({"path": path}),
        }
    }

    fn fetch_action(url: &str) -> Action {
        Action::ToolCall {
            id: "call-1".into(),
            name: "fetch".into(),
            params: json!({"url": url}),
        }
    }

    // -----------------------------------------------------------------------
    // Path validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_path_outside_workspace_rejected() {
        let boundary = default_boundary();

        // Write to /etc/passwd is outside the workspace
        let result = boundary.validate(&write_file_action("/etc/passwd"));
        assert!(result.is_err(), "write to /etc/passwd should be rejected");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::PathOutsideWorkspace);
        assert!(violation.message.contains("/etc/passwd"));
    }

    #[test]
    fn test_path_inside_workspace_allowed() {
        let boundary = default_boundary();

        // Relative path resolves inside workspace
        let result = boundary.validate(&write_file_action("src/main.rs"));
        assert!(result.is_ok(), "write to src/main.rs should be allowed");

        // Absolute path inside workspace
        let result = boundary.validate(&write_file_action("/home/user/project/src/main.rs"));
        assert!(result.is_ok(), "write to absolute path inside workspace should be allowed");
    }

    #[test]
    fn test_path_traversal_blocked() {
        let boundary = default_boundary();

        // Path traversal attempt: ../../etc/passwd from workspace should resolve outside
        let result = boundary.validate(&write_file_action("../../etc/passwd"));
        assert!(result.is_err(), "path traversal should be rejected");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::PathOutsideWorkspace);
    }

    #[test]
    fn test_read_file_inside_workspace_allowed() {
        let boundary = default_boundary();

        let result = boundary.validate(&read_file_action("src/lib.rs"));
        assert!(result.is_ok(), "read from inside workspace should be allowed");
    }

    #[test]
    fn test_read_file_outside_workspace_rejected() {
        let boundary = default_boundary();

        let result = boundary.validate(&read_file_action("/etc/shadow"));
        assert!(result.is_err(), "read from /etc/shadow should be rejected");
    }

    #[test]
    fn test_non_file_action_skips_path_check() {
        let boundary = default_boundary();

        // Non-file actions should not trigger path validation
        let result = boundary.validate(&Action::NoOp);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Command list tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_forbidden_command_rejected() {
        let boundary = default_boundary();

        // "rm" is in the forbidden list
        let result = boundary.validate(&bash_action("rm file.txt"));
        assert!(result.is_err(), "rm should be rejected");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::CommandForbidden);
        assert!(violation.message.contains("rm"));
    }

    #[test]
    fn test_sudo_command_rejected() {
        let boundary = default_boundary();

        // "sudo" is in the forbidden list
        let result = boundary.validate(&bash_action("sudo ls"));
        assert!(result.is_err(), "sudo should be rejected");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::CommandForbidden);
    }

    #[test]
    fn test_allowed_command_passes() {
        let boundary = default_boundary();

        let result = boundary.validate(&bash_action("cargo test"));
        assert!(result.is_ok(), "cargo test should be allowed");
    }

    #[test]
    fn test_allowed_list_restricts_commands() {
        let boundary = SandboxBoundary {
            workspace_root: PathBuf::from("/home/user/project"),
            allowed_commands: vec!["cargo".into(), "git".into(), "echo".into()],
            forbidden_commands: vec![],
            max_timeout: Duration::from_secs(300),
            network_allowed: true,
        };

        // Allowed commands pass
        assert!(boundary.validate(&bash_action("cargo build")).is_ok());
        assert!(boundary.validate(&bash_action("git status")).is_ok());
        assert!(boundary.validate(&bash_action("echo hello")).is_ok());

        // Non-allowed command fails
        let result = boundary.validate(&bash_action("ls -la"));
        assert!(result.is_err(), "ls should be rejected by allowed list");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::CommandNotAllowed);
    }

    #[test]
    fn test_forbidden_takes_priority_over_allowed() {
        let boundary = SandboxBoundary {
            workspace_root: PathBuf::from("/home/user/project"),
            allowed_commands: vec!["cargo".into(), "rm".into()],
            forbidden_commands: vec!["rm".into()],
            max_timeout: Duration::from_secs(300),
            network_allowed: true,
        };

        // Even though "rm" is in the allowed list, forbidden takes priority
        let result = boundary.validate(&bash_action("rm file.txt"));
        assert!(result.is_err(), "rm should be rejected despite being in allowed list");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::CommandForbidden);
    }

    #[test]
    fn test_non_bash_action_skips_command_check() {
        let boundary = default_boundary();

        let result = boundary.validate(&write_file_action("src/main.rs"));
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Timeout tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_timeout_within_limit_allowed() {
        let boundary = default_boundary();

        let action = Action::ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            params: json!({"command": "cargo build", "timeout": 60}),
        };
        let result = boundary.validate(&action);
        assert!(result.is_ok(), "timeout 60s within 300s max should be allowed");
    }

    #[test]
    fn test_timeout_exceeds_max_rejected() {
        let boundary = default_boundary();

        let action = Action::ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            params: json!({"command": "cargo build", "timeout": 600}),
        };
        let result = boundary.validate(&action);
        assert!(result.is_err(), "timeout 600s should exceed 300s max");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::TimeoutExceeded);
    }

    #[test]
    fn test_timeout_ms_exceeds_max_rejected() {
        let boundary = default_boundary();

        // 400000ms = 400s > 300s max
        let action = Action::ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            params: json!({"command": "cargo build", "timeout_ms": 400000}),
        };
        let result = boundary.validate(&action);
        assert!(result.is_err(), "timeout_ms 400000 should exceed 300s max");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::TimeoutExceeded);
    }

    #[test]
    fn test_no_timeout_is_allowed() {
        let boundary = default_boundary();

        let result = boundary.validate(&bash_action("cargo test"));
        assert!(result.is_ok(), "no timeout should be allowed");
    }

    // -----------------------------------------------------------------------
    // Network access tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_network_blocked_curl_rejected() {
        let boundary = default_boundary(); // network_allowed: false

        let result = boundary.validate(&bash_action("curl https://example.com"));
        assert!(result.is_err(), "curl should be rejected when network is blocked");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::NetworkAccessBlocked);
    }

    #[test]
    fn test_network_blocked_wget_rejected() {
        let boundary = default_boundary();

        let result = boundary.validate(&bash_action("wget https://example.com/file.tar.gz"));
        assert!(result.is_err(), "wget should be rejected when network is blocked");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::NetworkAccessBlocked);
    }

    #[test]
    fn test_network_blocked_fetch_tool_rejected() {
        let boundary = default_boundary();

        let result = boundary.validate(&fetch_action("https://example.com"));
        assert!(result.is_err(), "fetch tool should be rejected when network is blocked");
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::NetworkAccessBlocked);
    }

    #[test]
    fn test_network_allowed_curl_passes() {
        let boundary = SandboxBoundary {
            workspace_root: PathBuf::from("/home/user/project"),
            allowed_commands: vec![],
            forbidden_commands: vec![],
            max_timeout: Duration::from_secs(300),
            network_allowed: true,
        };

        let result = boundary.validate(&bash_action("curl https://example.com"));
        assert!(result.is_ok(), "curl should be allowed when network is enabled");
    }

    #[test]
    fn test_network_allowed_fetch_passes() {
        let boundary = SandboxBoundary {
            workspace_root: PathBuf::from("/home/user/project"),
            allowed_commands: vec![],
            forbidden_commands: vec![],
            max_timeout: Duration::from_secs(300),
            network_allowed: true,
        };

        let result = boundary.validate(&fetch_action("https://example.com"));
        assert!(result.is_ok(), "fetch should be allowed when network is enabled");
    }

    #[test]
    fn test_network_blocked_ssh_rejected() {
        let boundary = default_boundary();

        let result = boundary.validate(&bash_action("ssh user@remote"));
        assert!(result.is_err(), "ssh should be rejected when network is blocked");
    }

    #[test]
    fn test_network_blocked_rsync_rejected() {
        let boundary = default_boundary();

        let result = boundary.validate(&bash_action("rsync -avz /src /dest"));
        assert!(result.is_err(), "rsync should be rejected when network is blocked");
    }

    // -----------------------------------------------------------------------
    // Command wrapping tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_command_wrapping_adds_timeout() {
        let boundary = default_boundary();

        let wrapped = boundary.wrap_command("cargo build");
        assert_eq!(wrapped, "timeout 300 cargo build");
    }

    #[test]
    fn test_command_wrapping_with_short_timeout() {
        let boundary = SandboxBoundary {
            workspace_root: PathBuf::from("/home/user/project"),
            allowed_commands: vec![],
            forbidden_commands: vec![],
            max_timeout: Duration::from_secs(30),
            network_allowed: true,
        };

        let wrapped = boundary.wrap_command("ls -la");
        assert_eq!(wrapped, "timeout 30 ls -la");
    }

    #[test]
    fn test_command_wrapping_with_complex_command() {
        let boundary = default_boundary();

        let wrapped = boundary.wrap_command("cargo build --release 2>&1 | tee build.log");
        assert_eq!(
            wrapped,
            "timeout 300 cargo build --release 2>&1 | tee build.log"
        );
    }

    // -----------------------------------------------------------------------
    // Multiple violations: first one wins
    // -----------------------------------------------------------------------

    #[test]
    fn test_first_violation_reported() {
        let boundary = default_boundary();

        // Action with both path violation and network violation.
        // Path check runs first, so it should be reported.
        let action = Action::ToolCall {
            id: "call-1".into(),
            name: "write_file".into(),
            params: json!({"path": "/etc/passwd", "url": "https://evil.com"}),
        };
        let result = boundary.validate(&action);
        assert!(result.is_err());
        let violation = result.unwrap_err();
        assert_eq!(violation.violation_type, ViolationType::PathOutsideWorkspace);
    }

    // -----------------------------------------------------------------------
    // SandboxViolation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_sandbox_violation_new() {
        let v = SandboxViolation::new("test message", ViolationType::CommandForbidden);
        assert_eq!(v.message, "test message");
        assert_eq!(v.violation_type, ViolationType::CommandForbidden);
    }

    #[test]
    fn test_sandbox_violation_equality() {
        let a = SandboxViolation::new("msg", ViolationType::PathOutsideWorkspace);
        let b = SandboxViolation::new("msg", ViolationType::PathOutsideWorkspace);
        assert_eq!(a, b);

        let c = SandboxViolation::new("msg", ViolationType::CommandForbidden);
        assert_ne!(a, c);
    }

    // -----------------------------------------------------------------------
    // command_base tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_command_base_simple() {
        assert_eq!(command_base("cargo build"), "cargo");
        assert_eq!(command_base("ls -la"), "ls");
        assert_eq!(command_base("git status"), "git");
    }

    #[test]
    fn test_command_base_with_env() {
        assert_eq!(command_base("VAR=val cargo build"), "cargo");
        assert_eq!(command_base("FOO=bar BAZ=qux git push"), "git");
    }

    #[test]
    fn test_command_base_empty() {
        assert_eq!(command_base(""), "");
        assert_eq!(command_base("   "), "");
    }

    // -----------------------------------------------------------------------
    // Combined boundary tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_all_checks_pass_for_safe_action() {
        let boundary = SandboxBoundary {
            workspace_root: PathBuf::from("/home/user/project"),
            allowed_commands: vec!["cargo".into(), "git".into(), "echo".into()],
            forbidden_commands: vec!["rm".into()],
            max_timeout: Duration::from_secs(300),
            network_allowed: true,
        };

        // A safe cargo build inside the workspace should pass all checks
        let result = boundary.validate(&bash_action("cargo test"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_write_file_hidden_inside_workspace_allowed() {
        let boundary = default_boundary();

        // Hidden files within workspace should be allowed
        let result = boundary.validate(&write_file_action(".env"));
        assert!(result.is_ok());
    }
}