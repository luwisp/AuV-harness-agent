use crate::error::{HarnessError, Result};
use crate::tools::context::ToolContext;
use crate::tools::Tool;
use crate::types::ToolResult;
use serde_json::{json, Value};
use std::process::Command;
use std::time::Duration;

/// Tool that runs a test command in the workspace root.
///
/// Defaults to `cargo test`. Captures stdout and stderr, reports success
/// based on the command's exit code. Supports an optional `timeout_secs`
/// parameter that overrides the context default.
pub struct RunTestTool;

impl Tool for RunTestTool {
    fn name(&self) -> &str {
        "run_test"
    }

    fn description(&self) -> &str {
        "Runs a test command in the workspace root. Defaults to `cargo test`. \
         Captures stdout and stderr. Supports an optional `timeout_secs` parameter."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The test command to execute. Default: 'cargo test'."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Maximum execution time in seconds (overrides the context default).",
                    "minimum": 1
                }
            }
        })
    }

    fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResult> {
        let command_str = params["command"]
            .as_str()
            .unwrap_or("cargo test");

        let timeout = params["timeout_secs"]
            .as_u64()
            .map(|s| Duration::from_secs(s))
            .unwrap_or(ctx.command_timeout);

        // Use sh -c to support complex commands with arguments
        let child = Command::new("sh")
            .arg("-c")
            .arg(command_str)
            .current_dir(&ctx.workspace_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                HarnessError::ToolExecution(format!("Failed to spawn test command: {}", e))
            })?;

        let child_id = child.id();

        let (tx, rx) = std::sync::mpsc::channel();
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
                "Failed to wait on test process: {}",
                e
            ))),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Kill the child process by PID.
                let _ = Command::new("kill")
                    .arg("-9")
                    .arg(child_id.to_string())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                Err(HarnessError::ToolExecution(format!(
                    "Test command timed out after {} seconds",
                    timeout.as_secs()
                )))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(HarnessError::ToolExecution(
                    "Test process terminated unexpectedly".to_string(),
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
    fn test_run_test_default_command() {
        let (_dir, ctx) = setup_workspace();
        // Use a simple command that simulates a successful test run.
        // We avoid running the actual `cargo test` in the project workspace
        // because it would be too slow and could time out.
        let tool = RunTestTool;
        let result = tool
            .execute(&json!({"command": "echo test result: ok; exit 0"}), &ctx)
            .expect("run_test should succeed");
        assert!(result.success);
        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["exit_code"], 0);
        assert_eq!(structured["timed_out"], false);
    }

    #[test]
    fn test_run_test_custom_command_success() {
        let (_dir, ctx) = setup_workspace();
        let tool = RunTestTool;
        let result = tool
            .execute(&json!({"command": "echo all tests passed"}), &ctx)
            .expect("run_test should succeed");
        assert!(result.success);
        assert!(result.content.contains("all tests passed"));
        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["exit_code"], 0);
    }

    #[test]
    fn test_run_test_failure() {
        let (_dir, ctx) = setup_workspace();
        let tool = RunTestTool;
        let result = tool
            .execute(&json!({"command": "exit 1"}), &ctx)
            .expect("run_test should not error — it returns success=false");
        assert!(!result.success);
        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["exit_code"], 1);
    }

    #[test]
    fn test_run_test_with_stderr() {
        let (_dir, ctx) = setup_workspace();
        let tool = RunTestTool;
        let result = tool
            .execute(&json!({"command": "echo stdout; echo stderr >&2"}), &ctx)
            .expect("run_test should succeed");
        assert!(result.success);
        assert!(result.content.contains("stdout"));
        assert!(result.content.contains("stderr"));
    }

    #[test]
    fn test_run_test_timeout() {
        let (_dir, ctx) = setup_workspace();
        let tool = RunTestTool;
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

    #[test]
    fn test_run_test_structured_output() {
        let (_dir, ctx) = setup_workspace();
        let tool = RunTestTool;
        let result = tool
            .execute(&json!({"command": "echo ok"}), &ctx)
            .expect("run_test should succeed");
        assert!(result.success);
        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["exit_code"], 0);
        assert_eq!(structured["timed_out"], false);
        assert!(structured["stdout_len"].as_u64().unwrap() > 0);
    }
}