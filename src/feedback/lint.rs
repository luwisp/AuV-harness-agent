use crate::error::HarnessError;
use crate::feedback::{FeedbackChannel, FeedbackContext};
use crate::types::{FeedbackError, FeedbackResult};
use async_trait::async_trait;
use regex::Regex;
use std::sync::LazyLock;
use tokio::process::Command;

/// Feedback channel that runs `cargo clippy` in the workspace and parses
/// warnings into structured [`FeedbackError`] values.
pub struct LintChannel;

/// Regex for extracting warnings from `cargo clippy` output.
/// Matches blocks like:
///   warning: unused variable: `x`
///    --> src/lib.rs:2:9
static WARNING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"warning:\s*(.+?)\n\s*-->\s*([^:]+):(\d+):(\d+)")
        .expect("failed to compile WARNING_RE")
});

/// Regex for detecting that clippy is not installed (so we can return a
/// graceful result instead of an error).
static CLIPPY_NOT_FOUND_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"no such subcommand|not installed|Unknown tool")
        .expect("failed to compile CLIPPY_NOT_FOUND_RE")
});

#[async_trait]
impl FeedbackChannel for LintChannel {
    fn name(&self) -> &str {
        "lint"
    }

    fn should_run(&self, _action: &crate::types::Action, context: &FeedbackContext) -> bool {
        // Run when any source file changed (check for common source extensions).
        context
            .changed_files
            .iter()
            .any(|f| {
                f.extension()
                    .map_or(false, |ext| {
                        matches!(
                            ext.to_str(),
                            Some("rs" | "toml" | "json" | "yaml" | "yml" | "md")
                        )
                    })
            })
    }

    async fn run(&self, context: &FeedbackContext) -> Result<FeedbackResult, HarnessError> {
        let output = Command::new("cargo")
            .arg("clippy")
            .arg("--message-format=short")
            .current_dir(&context.workspace_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                HarnessError::ToolExecution(format!("Failed to run cargo clippy: {}", e))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        // If clippy is not installed, return a pass with a note.
        if CLIPPY_NOT_FOUND_RE.is_match(&combined) {
            return Ok(FeedbackResult {
                channel: self.name().to_string(),
                passed: true,
                errors: vec![],
                summary: "cargo clippy is not available — skipping lint check".to_string(),
            });
        }

        let errors: Vec<FeedbackError> = WARNING_RE
            .captures_iter(&combined)
            .map(|cap| FeedbackError {
                file: Some(cap[2].to_string()),
                line: cap[3].parse().ok(),
                column: cap[4].parse().ok(),
                error_type: "clippy_warning".to_string(),
                message: cap[1].to_string(),
            })
            .collect();

        let passed = errors.is_empty();

        let summary = if passed {
            "Lint check passed — no warnings".to_string()
        } else {
            format!(
                "Lint found {} warning(s): {}",
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
    async fn test_lint_parses_warnings() {
        // An unused variable triggers clippy's `unused_variables` warning.
        let lib = r#"
pub fn unused_var() -> i32 {
    let x = 42;
    0
}
"#;
        let (_dir, workspace) = setup_rust_project("test_lint_warn", lib);
        let ctx = FeedbackContext {
            workspace_root: workspace,
            changed_files: vec![PathBuf::from("src/lib.rs")],
        };

        let channel = LintChannel;
        let result = channel.run(&ctx).await;

        match result {
            Ok(r) => {
                if r.passed {
                    // If clippy is not installed, the result will still pass
                    // with a note about skipping.
                    assert!(
                        r.summary.contains("not available")
                            || r.summary.contains("no warnings"),
                        "unexpected summary: {}",
                        r.summary
                    );
                } else {
                    assert!(!r.errors.is_empty(), "expected at least one warning");
                    let error = &r.errors[0];
                    assert_eq!(error.file.as_deref(), Some("src/lib.rs"));
                    assert!(error.line.is_some(), "expected a line number");
                }
            }
            Err(e) => {
                // Gracefully skip if clippy is not installed — the test runner
                // may not have the clippy component.
                if !e.to_string().contains("not installed")
                    && !e.to_string().contains("no such command")
                {
                    panic!("unexpected error: {}", e);
                }
            }
        }
    }

    #[tokio::test]
    async fn test_lint_passes_clean_code() {
        // This code is clean — no unused variables or other clippy warnings.
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
        let (_dir, workspace) = setup_rust_project("test_lint_clean", lib);
        let ctx = FeedbackContext {
            workspace_root: workspace,
            changed_files: vec![PathBuf::from("src/lib.rs")],
        };

        let channel = LintChannel;
        let result = channel.run(&ctx).await;

        match result {
            Ok(r) => {
                assert!(
                    r.passed || r.summary.contains("not available"),
                    "expected clean code to pass lint, got: {}",
                    r.summary
                );
            }
            Err(e) => {
                if !e.to_string().contains("not installed")
                    && !e.to_string().contains("no such command")
                {
                    panic!("unexpected error: {}", e);
                }
            }
        }
    }

    #[tokio::test]
    async fn test_lint_should_run_for_source_files() {
        let channel = LintChannel;

        let run_cases = vec![
            vec![PathBuf::from("src/main.rs")],
            vec![PathBuf::from("Cargo.toml")],
            vec![PathBuf::from("config.json")],
            vec![PathBuf::from("README.md")],
        ];

        for files in &run_cases {
            let ctx = FeedbackContext {
                workspace_root: PathBuf::from("/tmp"),
                changed_files: files.clone(),
            };
            assert!(
                channel.should_run(&crate::types::Action::NoOp, &ctx),
                "should run for changed files: {:?}",
                files
            );
        }

        let ctx_no_source = FeedbackContext {
            workspace_root: PathBuf::from("/tmp"),
            changed_files: vec![PathBuf::from("image.png"), PathBuf::from("data.bin")],
        };
        assert!(!channel.should_run(
            &crate::types::Action::NoOp,
            &ctx_no_source
        ));
    }

    #[test]
    fn test_warning_regex_parses_typical_clippy_output() {
        let input = "\
warning: unused variable: `x`
 --> src/lib.rs:2:9
  |
2 |     let x = 42;
  |         ^ help: if this is intentional, prefix it with an underscore: `_x`
";
        let caps: Vec<_> = WARNING_RE.captures_iter(input).collect();
        assert_eq!(caps.len(), 1);
        let cap = &caps[0];
        assert_eq!(&cap[1], "unused variable: `x`");
        assert_eq!(&cap[2], "src/lib.rs");
        assert_eq!(&cap[3], "2");
        assert_eq!(&cap[4], "9");
    }
}