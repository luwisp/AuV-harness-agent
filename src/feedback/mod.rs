pub mod lint;
pub mod test_runner;
pub mod type_check;

use crate::error::HarnessError;
use crate::types::{Action, FeedbackError, FeedbackResult};
use async_trait::async_trait;
use std::path::PathBuf;

/// Trait for feedback channels that inspect the workspace after tool
/// execution and report failures, errors, and warnings.
#[async_trait]
pub trait FeedbackChannel: Send + Sync {
    /// A unique name identifying this channel (e.g. "test_runner", "type_check").
    fn name(&self) -> &str;

    /// Return `true` when this channel should execute for the given action and
    /// context (e.g. only when `.rs` files have changed).
    fn should_run(&self, action: &Action, context: &FeedbackContext) -> bool;

    /// Execute the feedback channel in the workspace and return a structured
    /// result indicating pass / fail together with any discovered errors.
    async fn run(&self, context: &FeedbackContext) -> Result<FeedbackResult, HarnessError>;
}

/// Context supplied to every feedback channel invocation.
#[derive(Debug, Clone)]
pub struct FeedbackContext {
    /// Root directory of the workspace being inspected.
    pub workspace_root: PathBuf,
    /// Paths (relative to `workspace_root`) of files that were changed by the
    /// preceding tool execution.
    pub changed_files: Vec<PathBuf>,
}

/// Orchestrates a collection of feedback channels and retry logic.
pub struct FeedbackRunner {
    channels: Vec<Box<dyn FeedbackChannel>>,
    max_retries: usize,
}

impl FeedbackRunner {
    /// Create a new runner with the given channels and retry budget.
    pub fn new(channels: Vec<Box<dyn FeedbackChannel>>, max_retries: usize) -> Self {
        Self {
            channels,
            max_retries,
        }
    }

    /// Run every channel whose `should_run` predicate is satisfied for the
    /// given action and context.  Channels that fail with an error are
    /// converted into a failed [`FeedbackResult`] so the caller always gets
    /// a complete picture.
    pub async fn run_all(
        &self,
        action: &Action,
        ctx: &FeedbackContext,
    ) -> Vec<FeedbackResult> {
        let mut results = Vec::new();

        for channel in &self.channels {
            if !channel.should_run(action, ctx) {
                continue;
            }

            match channel.run(ctx).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    results.push(FeedbackResult {
                        channel: channel.name().to_string(),
                        passed: false,
                        errors: vec![FeedbackError {
                            file: None,
                            line: None,
                            column: None,
                            error_type: "channel_error".to_string(),
                            message: e.to_string(),
                        }],
                        summary: format!(
                            "Channel '{}' failed with an internal error: {}",
                            channel.name(),
                            e
                        ),
                    });
                }
            }
        }

        results
    }

    /// Determine whether the agent should retry after seeing the given
    /// feedback results.  Returns `true` when at least one channel failed
    /// and the current attempt is still below `max_retries`.
    pub fn should_retry(&self, results: &[FeedbackResult], attempt: usize) -> bool {
        let all_passed = results.iter().all(|r| r.passed);
        !all_passed && attempt < self.max_retries
    }
}

/// Format a slice of [`FeedbackResult`] into structured text suitable for
/// injection into an LLM context window.
///
/// The output includes the channel name, PASSED/FAILED status, and each
/// error rendered as `file:line:column: message`.
pub fn format_feedback_for_llm(results: &[FeedbackResult]) -> String {
    let mut out = String::from("## Feedback Results\n");

    if results.is_empty() {
        out.push_str("(no feedback channels ran)\n");
        return out;
    }

    for result in results {
        let status = if result.passed { "PASSED" } else { "FAILED" };
        out.push_str(&format!("**{}**: {}\n", result.channel, status));

        for err in &result.errors {
            out.push_str("  - ");
            // file
            match &err.file {
                Some(f) => out.push_str(f),
                None => out.push_str("<?>"),
            }
            out.push(':');
            // line
            match err.line {
                Some(l) => out.push_str(&l.to_string()),
                None => out.push('?'),
            }
            out.push(':');
            // column
            match err.column {
                Some(c) => out.push_str(&c.to_string()),
                None => out.push('?'),
            }
            out.push_str(": ");
            out.push_str(&err.message);
            out.push('\n');
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Action;
    use std::path::PathBuf;

    /// A stub channel that always passes.
    struct AlwaysPassChannel;

    #[async_trait]
    impl FeedbackChannel for AlwaysPassChannel {
        fn name(&self) -> &str {
            "always_pass"
        }

        fn should_run(&self, _action: &Action, _context: &FeedbackContext) -> bool {
            true
        }

        async fn run(&self, _context: &FeedbackContext) -> Result<FeedbackResult, HarnessError> {
            Ok(FeedbackResult {
                channel: "always_pass".to_string(),
                passed: true,
                errors: vec![],
                summary: "All good".to_string(),
            })
        }
    }

    /// A stub channel that always fails.
    struct AlwaysFailChannel;

    #[async_trait]
    impl FeedbackChannel for AlwaysFailChannel {
        fn name(&self) -> &str {
            "always_fail"
        }

        fn should_run(&self, _action: &Action, _context: &FeedbackContext) -> bool {
            true
        }

        async fn run(&self, _context: &FeedbackContext) -> Result<FeedbackResult, HarnessError> {
            Ok(FeedbackResult {
                channel: "always_fail".to_string(),
                passed: false,
                errors: vec![FeedbackError {
                    file: Some("src/main.rs".to_string()),
                    line: Some(10),
                    column: None,
                    error_type: "test_failure".to_string(),
                    message: "assertion failed".to_string(),
                }],
                summary: "1 test failed".to_string(),
            })
        }
    }

    /// A stub channel that only runs when .rs files changed.
    struct ConditionalChannel;

    #[async_trait]
    impl FeedbackChannel for ConditionalChannel {
        fn name(&self) -> &str {
            "conditional"
        }

        fn should_run(&self, _action: &Action, context: &FeedbackContext) -> bool {
            context
                .changed_files
                .iter()
                .any(|f| f.extension().map_or(false, |ext| ext == "rs"))
        }

        async fn run(&self, _context: &FeedbackContext) -> Result<FeedbackResult, HarnessError> {
            Ok(FeedbackResult {
                channel: "conditional".to_string(),
                passed: true,
                errors: vec![],
                summary: "Ran conditionally".to_string(),
            })
        }
    }

    /// A channel whose `run` method returns an error.
    struct ErrorChannel;

    #[async_trait]
    impl FeedbackChannel for ErrorChannel {
        fn name(&self) -> &str {
            "error_channel"
        }

        fn should_run(&self, _action: &Action, _context: &FeedbackContext) -> bool {
            true
        }

        async fn run(&self, _context: &FeedbackContext) -> Result<FeedbackResult, HarnessError> {
            Err(HarnessError::ToolExecution(
                "simulated channel failure".to_string(),
            ))
        }
    }

    fn test_context() -> FeedbackContext {
        FeedbackContext {
            workspace_root: PathBuf::from("/tmp/test"),
            changed_files: vec![PathBuf::from("src/main.rs")],
        }
    }

    #[tokio::test]
    async fn test_runner_run_all_executes_channels() {
        let runner = FeedbackRunner::new(
            vec![
                Box::new(AlwaysPassChannel),
                Box::new(AlwaysFailChannel),
            ],
            3,
        );
        let ctx = test_context();
        let results = runner.run_all(&Action::NoOp, &ctx).await;

        assert_eq!(results.len(), 2);
        let pass = results.iter().find(|r| r.channel == "always_pass").unwrap();
        assert!(pass.passed);
        let fail = results
            .iter()
            .find(|r| r.channel == "always_fail")
            .unwrap();
        assert!(!fail.passed);
    }

    #[tokio::test]
    async fn test_runner_conditional_channel_skips_when_no_rs_files() {
        let runner = FeedbackRunner::new(vec![Box::new(ConditionalChannel)], 3);
        let ctx = FeedbackContext {
            workspace_root: PathBuf::from("/tmp/test"),
            changed_files: vec![PathBuf::from("README.md")],
        };
        let results = runner.run_all(&Action::NoOp, &ctx).await;
        assert!(
            results.is_empty(),
            "conditional channel should not run when no .rs files changed"
        );
    }

    #[tokio::test]
    async fn test_runner_conditional_channel_runs_when_rs_files_changed() {
        let runner = FeedbackRunner::new(vec![Box::new(ConditionalChannel)], 3);
        let ctx = test_context(); // has src/main.rs
        let results = runner.run_all(&Action::NoOp, &ctx).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[tokio::test]
    async fn test_runner_error_channel_converted_to_failed_result() {
        let runner = FeedbackRunner::new(vec![Box::new(ErrorChannel)], 3);
        let ctx = test_context();
        let results = runner.run_all(&Action::NoOp, &ctx).await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert_eq!(results[0].channel, "error_channel");
        assert!(results[0].summary.contains("simulated channel failure"));
    }

    #[test]
    fn test_should_retry_all_passed() {
        let runner = FeedbackRunner::new(vec![], 3);
        let results = vec![FeedbackResult {
            channel: "check".to_string(),
            passed: true,
            errors: vec![],
            summary: "ok".to_string(),
        }];
        assert!(!runner.should_retry(&results, 0));
        assert!(!runner.should_retry(&results, 2));
    }

    #[test]
    fn test_should_retry_with_failures() {
        let runner = FeedbackRunner::new(vec![], 3);
        let results = vec![FeedbackResult {
            channel: "check".to_string(),
            passed: false,
            errors: vec![FeedbackError {
                file: Some("src/lib.rs".to_string()),
                line: Some(5),
                column: None,
                error_type: "test_failure".to_string(),
                message: "boom".to_string(),
            }],
            summary: "fail".to_string(),
        }];

        assert!(runner.should_retry(&results, 0));
        assert!(runner.should_retry(&results, 1));
        assert!(runner.should_retry(&results, 2));
        assert!(!runner.should_retry(&results, 3));
        assert!(!runner.should_retry(&results, 4));
    }

    #[test]
    fn test_should_retry_no_results() {
        let runner = FeedbackRunner::new(vec![], 3);
        // No results means all passed (vacuous truth).
        assert!(!runner.should_retry(&[], 0));
    }

    #[test]
    fn test_feedback_context_debug() {
        let ctx = test_context();
        let debug_str = format!("{:?}", ctx);
        assert!(debug_str.contains("src/main.rs"));
        assert!(debug_str.contains("/tmp/test"));
    }

    // -----------------------------------------------------------------
    // Tests for format_feedback_for_llm
    // -----------------------------------------------------------------

    #[test]
    fn test_format_passing_results() {
        let results = vec![
            FeedbackResult {
                channel: "test_runner".to_string(),
                passed: true,
                errors: vec![],
                summary: "All tests passed".to_string(),
            },
            FeedbackResult {
                channel: "type_check".to_string(),
                passed: true,
                errors: vec![],
                summary: "Type check passed — no errors".to_string(),
            },
        ];

        let output = format_feedback_for_llm(&results);

        assert!(output.contains("## Feedback Results"));
        assert!(output.contains("**test_runner**: PASSED"));
        assert!(output.contains("**type_check**: PASSED"));
        assert!(!output.contains("FAILED"));
    }

    #[test]
    fn test_format_failing_results() {
        let results = vec![
            FeedbackResult {
                channel: "test_runner".to_string(),
                passed: false,
                errors: vec![FeedbackError {
                    file: Some("src/main.rs".to_string()),
                    line: Some(42),
                    column: Some(5),
                    error_type: "test_failure".to_string(),
                    message: "assertion failed: expected true, got false".to_string(),
                }],
                summary: "1 test failed".to_string(),
            },
            FeedbackResult {
                channel: "type_check".to_string(),
                passed: true,
                errors: vec![],
                summary: "Type check passed".to_string(),
            },
        ];

        let output = format_feedback_for_llm(&results);

        assert!(output.contains("## Feedback Results"));
        assert!(output.contains("**test_runner**: FAILED"));
        assert!(output.contains("**type_check**: PASSED"));
        // Error details should be present with file:line:column:message
        assert!(output.contains("src/main.rs:42:5: assertion failed: expected true, got false"));
    }

    #[test]
    fn test_format_failing_results_with_missing_fields() {
        let results = vec![
            FeedbackResult {
                channel: "lint".to_string(),
                passed: false,
                errors: vec![
                    FeedbackError {
                        file: Some("src/lib.rs".to_string()),
                        line: Some(10),
                        column: None,
                        error_type: "clippy_warning".to_string(),
                        message: "unused variable: `x`".to_string(),
                    },
                    FeedbackError {
                        file: None,
                        line: None,
                        column: None,
                        error_type: "unknown".to_string(),
                        message: "something went wrong".to_string(),
                    },
                ],
                summary: "2 warnings".to_string(),
            },
        ];

        let output = format_feedback_for_llm(&results);

        assert!(output.contains("**lint**: FAILED"));
        // Error with partial location info
        assert!(output.contains("src/lib.rs:10:?: unused variable: `x`"));
        // Error with no location info at all — uses placeholders
        assert!(output.contains("<?>:?:?: something went wrong"));
    }

    #[test]
    fn test_format_empty_results() {
        let results: Vec<FeedbackResult> = vec![];
        let output = format_feedback_for_llm(&results);

        assert!(output.contains("## Feedback Results"));
        assert!(output.contains("(no feedback channels ran)"));
        assert!(!output.contains("PASSED"));
        assert!(!output.contains("FAILED"));
    }
}