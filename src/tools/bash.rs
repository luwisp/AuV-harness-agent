use crate::error::{HarnessError, Result};
use crate::tools::context::ToolContext;
use crate::tools::Tool;
use crate::types::ToolResult;
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Tool that executes shell commands via `sh -c` in the workspace root.
///
/// Captures stdout and stderr (merged), applies a timeout, and reports exit
/// status. The timeout is taken from the optional `timeout_secs` parameter,
/// falling back to the `command_timeout` in [`ToolContext`].
pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Executes a shell command via `sh -c` in the workspace root. \
         Captures stdout and stderr. Supports an optional `timeout_secs` parameter."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Maximum execution time in seconds (overrides the context default).",
                    "minimum": 1
                }
            },
            "required": ["command"]
        })
    }

    fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResult> {
        let command = params["command"]
            .as_str()
            .ok_or_else(|| HarnessError::ToolExecution("Missing 'command' parameter".to_string()))?;

        let timeout = params["timeout_secs"]
            .as_u64()
            .map(|s| Duration::from_secs(s))
            .unwrap_or(ctx.command_timeout);

        let child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.workspace_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                HarnessError::ToolExecution(format!("Failed to spawn command: {}", e))
            })?;

        // Capture the PID before moving the child into the thread, so we can
        // kill it if the timeout fires.
        let child_id = child.id();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let output = child.wait_with_output();
            let _ = tx.send(output);
        });

        match rx.recv_timeout(timeout) {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let content = if stderr.is_empty() {
                    stdout.to_string()
                } else if stdout.is_empty() {
                    stderr.to_string()
                } else {
                    format!("{}{}", stdout, stderr)
                };

                let success = output.status.success();
                Ok(ToolResult {
                    success,
                    content,
                    structured: Some(json!({
                        "exit_code": output.status.code(),
                        "stdout_len": output.stdout.len(),
                        "stderr_len": output.stderr.len(),
                        "timed_out": false,
                    })),
                    artifacts: vec![],
                })
            }
            Ok(Err(e)) => Err(HarnessError::ToolExecution(format!(
                "Failed to wait on process: {}",
                e
            ))),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Kill the child process by PID.
                let _ = Command::new("kill")
                    .arg("-9")
                    .arg(child_id.to_string())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                Err(HarnessError::ToolExecution(format!(
                    "Command timed out after {} seconds",
                    timeout.as_secs()
                )))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(HarnessError::ToolExecution(
                    "Process terminated unexpectedly".to_string(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_workspace() -> (TempDir, ToolContext) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let ctx = ToolContext {
            workspace_root: dir.path().to_path_buf(),
            ..ToolContext::default()
        };
        (dir, ctx)
    }

    #[test]
    fn test_bash_echo() {
        let (_dir, ctx) = setup_workspace();
        let tool = BashTool;
        let result = tool
            .execute(&json!({"command": "echo hello"}), &ctx)
            .expect("bash echo should succeed");
        assert!(result.success);
        assert!(result.content.contains("hello"));
    }

    #[test]
    fn test_bash_command_failure() {
        let (_dir, ctx) = setup_workspace();
        let tool = BashTool;
        let result = tool
            .execute(&json!({"command": "exit 1"}), &ctx)
            .expect("bash should not error — it returns success=false");
        assert!(!result.success);
        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["exit_code"], 1);
    }

    #[test]
    fn test_bash_timeout() {
        let (_dir, ctx) = setup_workspace();
        let tool = BashTool;
        let result = tool.execute(
            &json!({"command": "sleep 10", "timeout_secs": 1}),
            &ctx,
        );
        assert!(result.is_err());
        match result {
            Err(HarnessError::ToolExecution(msg)) => {
                assert!(
                    msg.contains("timed out"),
                    "expected timeout message, got: {}",
                    msg
                );
            }
            other => panic!("expected ToolExecution timeout error, got {:?}", other),
        }
    }
}