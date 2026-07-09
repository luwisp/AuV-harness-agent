use crate::error::HarnessError;
use crate::feedback::{FeedbackChannel, FeedbackContext};
use crate::types::{FeedbackError, FeedbackResult};
use async_trait::async_trait;
use regex::Regex;
use std::sync::LazyLock;
use tokio::process::Command;

/// Feedback channel that runs `cargo test` in the workspace and parses test
/// failures into structured [`FeedbackError`] values with file:line
/// information.
pub struct TestRunnerChannel;

/// Regex for extracting panic locations from test output.
/// Matches lines like:
///   thread 'test_name' panicked at src/lib.rs:42:5:
static PANIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"panicked at ([^:]+):(\d+):(\d+)").expect("failed to compile PANIC_RE")
});

/// Regex for detecting that the test suite failed overall.
static TEST_FAILED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"test result: FAILED").expect("failed to compile TEST_FAILED_RE")
});

#[async_trait]
impl FeedbackChannel for TestRunnerChannel {
    fn name(&self) -> &str {
        "test_runner"
    }

    fn should_run(&self, _action: &crate::types::Action, context: &FeedbackContext) -> bool {
        context
            .changed_files
            .iter()
            .any(|f| f.extension().map_or(false, |ext| ext == "rs"))
    }

    async fn run(&self, context: &FeedbackContext) -> Result<FeedbackResult, HarnessError> {
        let output = Command::new("cargo")
            .arg("test")
            .current_dir(&context.workspace_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                HarnessError::ToolExecution(format!("Failed to run cargo test: {}", e))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let passed = !TEST_FAILED_RE.is_match(&combined);

        let errors: Vec<FeedbackError> = PANIC_RE
            .captures_iter(&combined)
            .map(|cap| FeedbackError {
                file: Some(cap[1].to_string()),
                line: cap[2].parse().ok(),
                column: cap[3].parse().ok(),
                error_type: "test_failure".to_string(),
                message: {
                    // Try to grab the panic message on the same line.
                    let full = cap.get(0).unwrap().as_str();
                    full.to_string()
                },
            })
            .collect();

        let summary = if passed {
            "All tests passed".to_string()
        } else {
            format!(
                "{} test(s) failed, {} failure(s) detected",
                errors.len(),
                errors.len()
            )
        };

        Ok(FeedbackResult {
            channel: self.name().to_string(),
            passed,
            errors,
            summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::FeedbackContext;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Helper: create a temporary Rust project with the given `lib.rs` content.
    fn setup_rust_project(name: &str, lib_content: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let src_dir = dir.path().join("src");
        std::fs::create_dir_all(&src_dir).expect("failed to create src dir");

        let cargo_toml = format!(
            r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"
"#,
            name
        );
        std::fs::write(dir.path().join("Cargo.toml"), cargo_toml)
            .expect("failed to write Cargo.toml");
        std::fs::write(src_dir.join("lib.rs"), lib_content).expect("failed to write lib.rs");

        let workspace = dir.path().to_path_buf();
        (dir, workspace)
    }

    #[tokio::test]
    async fn test_runner_parses_pass() {
        let lib = r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }
}
"#;
        let (_dir, workspace) = setup_rust_project("test_pass", lib);
        let ctx = FeedbackContext {
            workspace_root: workspace,
            changed_files: vec![PathBuf::from("src/lib.rs")],
        };

        let channel = TestRunnerChannel;
        let result = channel.run(&ctx).await.expect("channel should not error");
        assert!(
            result.passed,
            "expected tests to pass, got: {}",
            result.summary
        );
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_runner_parses_failures() {
        let lib = r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_fails() {
        assert_eq!(add(2, 3), 6); // deliberately wrong on line 11
    }
}
"#;
        let (_dir, workspace) = setup_rust_project("test_fail", lib);
        let ctx = FeedbackContext {
            workspace_root: workspace,
            changed_files: vec![PathBuf::from("src/lib.rs")],
        };

        let channel = TestRunnerChannel;
        let result = channel.run(&ctx).await.expect("channel should not error");
        assert!(
            !result.passed,
            "expected tests to fail, got: {}",
            result.summary
        );
        assert!(!result.errors.is_empty(), "expected at least one parsed error");

        // The panic should reference src/lib.rs with a line number.
        let error = &result.errors[0];
        assert_eq!(error.file.as_deref(), Some("src/lib.rs"));
        assert!(error.line.is_some(), "expected a line number in the panic");
    }

    #[tokio::test]
    async fn test_runner_should_run_only_when_rs_files_changed() {
        let channel = TestRunnerChannel;

        let ctx_with_rs = FeedbackContext {
            workspace_root: PathBuf::from("/tmp"),
            changed_files: vec![PathBuf::from("src/main.rs")],
        };
        assert!(channel.should_run(
            &crate::types::Action::NoOp,
            &ctx_with_rs
        ));

        let ctx_without_rs = FeedbackContext {
            workspace_root: PathBuf::from("/tmp"),
            changed_files: vec![PathBuf::from("README.md"), PathBuf::from("Cargo.toml")],
        };
        assert!(!channel.should_run(
            &crate::types::Action::NoOp,
            &ctx_without_rs
        ));
    }
}