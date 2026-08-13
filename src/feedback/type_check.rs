use crate::error::HarnessError;
use crate::feedback::{FeedbackChannel, FeedbackContext};
use crate::types::{FeedbackError, FeedbackResult};
use async_trait::async_trait;
use regex::Regex;
use std::sync::LazyLock;
use tokio::process::Command;

/// Feedback channel that runs `cargo check` in the workspace and parses
/// compilation errors into structured [`FeedbackError`] values.
pub struct TypeCheckChannel;

/// Regex for extracting compilation errors from `cargo check` output.
/// Matches blocks like:
///   error[E0308]: mismatched types
///    --> src/lib.rs:2:15
static ERROR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"error\[E\d{4}\]:\s*(.+?)\n\s*-->\s*([^:]+):(\d+):(\d+)",
    )
    .expect("failed to compile ERROR_RE")
});

#[async_trait]
impl FeedbackChannel for TypeCheckChannel {
    fn name(&self) -> &str {
        "type_check"
    }

    fn should_run(&self, _action: &crate::types::Action, context: &FeedbackContext) -> bool {
        context
            .changed_files
            .iter()
            .any(|f| f.extension().map_or(false, |ext| ext == "rs"))
    }

    async fn run(&self, context: &FeedbackContext) -> Result<FeedbackResult, HarnessError> {
        let output = Command::new("cargo")
            .arg("check")
            .arg("--color")
            .arg("never")
            .current_dir(&context.workspace_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                HarnessError::ToolExecution(format!("Failed to run cargo check: {}", e))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Compilation errors go to stderr, so prefer stderr but include stdout
        // for completeness.
        let combined = format!("{}{}", stdout, stderr);

        let mut errors: Vec<FeedbackError> = ERROR_RE
            .captures_iter(&combined)
            .map(|cap| FeedbackError {
                file: Some(cap[2].to_string()),
                line: cap[3].parse().ok(),
                column: cap[4].parse().ok(),
                error_type: format!("compile_error[{}]", &cap[1]),
                message: cap[1].to_string(),
            })
            .collect();

        if !output.status.success() && errors.is_empty() {
            let message = combined
                .lines()
                .find(|line| line.trim_start().starts_with("error:"))
                .map(str::trim)
                .unwrap_or("cargo check failed without a structured diagnostic")
                .to_string();
            errors.push(FeedbackError {
                file: None,
                line: None,
                column: None,
                error_type: "cargo_check_failed".to_string(),
                message,
            });
        }

        let passed = output.status.success() && errors.is_empty();

        let summary = if passed {
            "Type check passed — no errors".to_string()
        } else {
            format!(
                "Type check found {} error(s): {}",
                errors.len(),
                errors
                    .iter()
                    .map(|e| format!(
                        "{}:{} — {}",
                        e.file.as_deref().unwrap_or("?"),
                        e.line.map_or("?".to_string(), |l| l.to_string()),
                        e.message
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
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
    async fn test_type_check_parses_errors() {
        // Deliberate type error: assign &str to u32
        let lib = r#"
pub fn broken() -> u32 {
    let x: u32 = "hello";
    x
}
"#;
        let (_dir, workspace) = setup_rust_project("test_type_err", lib);
        let cargo_dir = workspace.join(".cargo");
        std::fs::create_dir_all(&cargo_dir).expect("failed to create .cargo dir");
        std::fs::write(cargo_dir.join("config.toml"), "[term]\ncolor = 'always'\n")
            .expect("failed to write Cargo config");
        let ctx = FeedbackContext {
            workspace_root: workspace,
            changed_files: vec![PathBuf::from("src/lib.rs")],
        };

        let channel = TypeCheckChannel;
        let result = channel.run(&ctx).await.expect("channel should not error");

        assert!(
            !result.passed,
            "expected type check to fail, got: {}",
            result.summary
        );
        assert!(!result.errors.is_empty(), "expected at least one parsed error");

        let error = &result.errors[0];
        assert_eq!(error.file.as_deref(), Some("src/lib.rs"));
        assert!(error.line.is_some(), "expected a line number in the error");
        assert!(
            error.message.contains("mismatched types")
                || error.message.contains("expected")
                || error.error_type.contains("E0308"),
            "expected a type-mismatch error, got: {}",
            error.message
        );
    }

    #[tokio::test]
    async fn test_type_check_nonzero_without_numbered_diagnostic_fails() {
        let lib = r#"compile_error!("plain compile failure");"#;
        let (_dir, workspace) = setup_rust_project("test_plain_err", lib);
        let ctx = FeedbackContext {
            workspace_root: workspace,
            changed_files: vec![PathBuf::from("src/lib.rs")],
        };

        let result = TypeCheckChannel
            .run(&ctx)
            .await
            .expect("channel should not error");

        assert!(!result.passed, "a non-zero cargo exit must fail the check");
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].error_type, "cargo_check_failed");
        assert!(result.errors[0].message.contains("plain compile failure"));
    }

    #[tokio::test]
    async fn test_type_check_passes_clean_code() {
        let lib = r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
        let (_dir, workspace) = setup_rust_project("test_type_ok", lib);
        let ctx = FeedbackContext {
            workspace_root: workspace,
            changed_files: vec![PathBuf::from("src/lib.rs")],
        };

        let channel = TypeCheckChannel;
        let result = channel.run(&ctx).await.expect("channel should not error");

        assert!(
            result.passed,
            "expected type check to pass, got: {}",
            result.summary
        );
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_type_check_should_run_only_when_rs_files_changed() {
        let channel = TypeCheckChannel;

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
            changed_files: vec![PathBuf::from("README.md")],
        };
        assert!(!channel.should_run(
            &crate::types::Action::NoOp,
            &ctx_without_rs
        ));
    }

    #[test]
    fn test_error_regex_parses_typical_rustc_output() {
        let input = "\
error[E0308]: mismatched types
 --> src/lib.rs:2:15
  |
2 |     let x: u32 = \"hello\";
  |               ^^^^^^^ expected `u32`, found `&str`
";
        let caps: Vec<_> = ERROR_RE.captures_iter(input).collect();
        assert_eq!(caps.len(), 1);
        let cap = &caps[0];
        assert_eq!(&cap[1], "mismatched types");
        assert_eq!(&cap[2], "src/lib.rs");
        assert_eq!(&cap[3], "2");
        assert_eq!(&cap[4], "15");
    }
}
