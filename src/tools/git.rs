use crate::error::{HarnessError, Result};
use crate::tools::context::ToolContext;
use crate::tools::Tool;
use crate::types::ToolResult;
use serde_json::{json, Value};
use std::process::Command;

/// Tool that runs `git diff` in the workspace root.
///
/// Returns the diff output. When the `staged` parameter is `true`,
/// `git diff --cached` is used instead to show staged changes.
pub struct GitDiffTool;

impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Runs `git diff` in the workspace root. Set `staged: true` to show staged changes \
         (uses `git diff --cached`). Returns the diff output."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "staged": {
                    "type": "boolean",
                    "description": "If true, show staged changes (git diff --cached). Default: false."
                }
            }
        })
    }

    fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResult> {
        let staged = params["staged"].as_bool().unwrap_or(false);

        let mut cmd = Command::new("git");
        cmd.env("LC_ALL", "C"); // Force English messages for predictable error output
        cmd.arg("diff");
        if staged {
            cmd.arg("--cached");
        }
        cmd.current_dir(&ctx.workspace_root);

        let output = cmd.output().map_err(|e| {
            HarnessError::ToolExecution(format!("Failed to run git diff: {}", e))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            let msg = if stderr.is_empty() {
                format!("git diff failed with exit code {:?}", output.status.code())
            } else {
                stderr
            };
            return Err(HarnessError::ToolExecution(msg));
        }

        Ok(ToolResult {
            success: true,
            content: if stdout.is_empty() {
                "(no changes)".to_string()
            } else {
                stdout
            },
            structured: Some(json!({
                "staged": staged,
                "output_len": output.stdout.len(),
            })),
            artifacts: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;

    /// Initialize a temporary git repo, return the temp dir and its ToolContext.
    fn setup_git_repo() -> (TempDir, ToolContext) {
        let dir = TempDir::new().expect("failed to create temp dir");

        // Initialize git repo
        let status = StdCommand::new("git")
            .arg("init")
            .current_dir(dir.path())
            .status()
            .expect("failed to git init");
        assert!(status.success(), "git init should succeed");

        // Configure git user for the test repo (needed for commits)
        StdCommand::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir.path())
            .status()
            .expect("failed to set git user.email");
        StdCommand::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir.path())
            .status()
            .expect("failed to set git user.name");

        // Create and commit an initial file
        fs::write(dir.path().join("initial.txt"), "initial content\n")
            .expect("failed to write initial file");
        StdCommand::new("git")
            .args(["add", "initial.txt"])
            .current_dir(dir.path())
            .status()
            .expect("failed to git add");
        StdCommand::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(dir.path())
            .status()
            .expect("failed to git commit");

        let ctx = ToolContext {
            workspace_root: dir.path().to_path_buf(),
            ..ToolContext::default()
        };
        (dir, ctx)
    }

    #[test]
    fn test_git_diff_no_changes() {
        let (_dir, ctx) = setup_git_repo();
        let tool = GitDiffTool;
        let result = tool
            .execute(&json!({}), &ctx)
            .expect("git diff should succeed");
        assert!(result.success);
        assert_eq!(result.content, "(no changes)");
    }

    #[test]
    fn test_git_diff_unstaged_changes() {
        let (_dir, ctx) = setup_git_repo();
        // Modify the initial file
        fs::write(ctx.workspace_root.join("initial.txt"), "modified content\n")
            .expect("failed to write modified file");

        let tool = GitDiffTool;
        let result = tool
            .execute(&json!({}), &ctx)
            .expect("git diff should succeed");
        assert!(result.success);
        assert!(!result.content.is_empty());
        assert_ne!(result.content, "(no changes)");
        // The diff should mention the file path
        assert!(result.content.contains("initial.txt"));
    }

    #[test]
    fn test_git_diff_staged_changes() {
        let (_dir, ctx) = setup_git_repo();
        // Modify the initial file and stage it
        fs::write(ctx.workspace_root.join("initial.txt"), "staged content\n")
            .expect("failed to write staged file");
        StdCommand::new("git")
            .args(["add", "initial.txt"])
            .current_dir(&ctx.workspace_root)
            .status()
            .expect("failed to git add staged change");

        // Unstaged diff should show nothing
        let tool = GitDiffTool;
        let unstaged = tool
            .execute(&json!({}), &ctx)
            .expect("unstaged diff should succeed");
        assert_eq!(unstaged.content, "(no changes)");

        // Staged diff should show the change
        let staged = tool
            .execute(&json!({"staged": true}), &ctx)
            .expect("staged diff should succeed");
        assert!(staged.success);
        assert!(!staged.content.is_empty());
        assert_ne!(staged.content, "(no changes)");
        assert!(staged.content.contains("initial.txt"));
    }

    #[test]
    fn test_git_diff_new_file() {
        let (_dir, ctx) = setup_git_repo();
        // Create a new untracked file (should not appear in diff)
        fs::write(ctx.workspace_root.join("new.txt"), "new file\n")
            .expect("failed to write new file");

        let tool = GitDiffTool;
        let result = tool
            .execute(&json!({}), &ctx)
            .expect("git diff should succeed");
        assert_eq!(result.content, "(no changes)");
    }

    #[test]
    fn test_git_diff_not_a_repo() {
        let dir = TempDir::new().expect("failed to create temp dir");
        let ctx = ToolContext {
            workspace_root: dir.path().to_path_buf(),
            ..ToolContext::default()
        };
        let tool = GitDiffTool;
        let result = tool.execute(&json!({}), &ctx);
        assert!(result.is_err());
        match result {
            Err(HarnessError::ToolExecution(msg)) => {
                assert!(
                    msg.contains("Not a git repository")
                        || msg.contains("not a git repository"),
                    "expected git error message, got: {}",
                    msg
                );
            }
            other => panic!("expected ToolExecution error, got {:?}", other),
        }
    }

    #[test]
    fn test_git_diff_structured_output() {
        let (_dir, ctx) = setup_git_repo();
        fs::write(ctx.workspace_root.join("initial.txt"), "changed\n")
            .expect("failed to write changed file");

        let tool = GitDiffTool;
        let result = tool
            .execute(&json!({}), &ctx)
            .expect("git diff should succeed");
        assert!(result.success);
        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["staged"], false);
        assert!(structured["output_len"].as_u64().unwrap() > 0);
    }
}