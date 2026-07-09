use std::collections::HashSet;
use std::fmt::Write as FmtWrite;
use std::io::{self, Write as IoWrite};
use std::time::Duration;

use sha2::{Digest, Sha256};

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
}

impl ApprovalGate {
    /// Create a new approval gate with the given timeout.
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            session_whitelist: HashSet::new(),
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
    /// 3. Print risk information to stderr and prompt the user (y/n).
    /// 4. Wait for input with the configured timeout.
    /// 5. If approved, add the fingerprint to the whitelist.
    ///
    /// This is an async function because it waits for user input with a
    /// tokio timeout.
    pub async fn request_approval(
        &mut self,
        action: &Action,
        assessment: &RiskAssessment,
    ) -> ApprovalDecision {
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
                reason: Some("Previously approved in this session".to_string()),
            };
        }

        // Step 3: print risk info to stderr
        print_risk_info(action, assessment);

        // Step 4: wait for user input with timeout
        match input.await {
            UserResponse::Yes => {
                // Step 5: add to whitelist
                self.whitelist(&fingerprint);
                ApprovalDecision::Approved {
                    by: "user".to_string(),
                    reason: Some("User approved the action".to_string()),
                }
            }
            UserResponse::No => ApprovalDecision::Denied {
                reason: "User denied the action".to_string(),
            },
            UserResponse::Timeout => ApprovalDecision::Timeout,
        }
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
enum UserResponse {
    Yes,
    No,
    Timeout,
}

/// Print risk information about the action to stderr.
fn print_risk_info(action: &Action, assessment: &RiskAssessment) {
    let mut stderr = io::stderr().lock();

    let _ = writeln!(stderr);
    let _ = writeln!(stderr, "=== Guardrail Approval Required ===");
    let _ = writeln!(stderr, "Action: {action:?}");
    let _ = writeln!(stderr, "Risk Level: {:?}", assessment.level);

    if !assessment.reasons.is_empty() {
        let _ = writeln!(stderr, "Reasons:");
        for reason in &assessment.reasons {
            let _ = writeln!(stderr, "  - {reason}");
        }
    }

    if let Some(ref mitigation) = assessment.suggested_mitigation {
        let _ = writeln!(stderr, "Mitigation: {mitigation}");
    }

    let _ = writeln!(stderr, "===================================");
    let _ = write!(stderr, "Approve this action? (y/n): ");
    let _ = stderr.flush();
}

/// Read a yes/no answer from stdin with a timeout.
///
/// Spawns a blocking task to read from stdin, then wraps it with
/// `tokio::time::timeout`.  Returns `UserResponse::Timeout` if the user does
/// not respond within the deadline.
async fn read_yes_no_with_timeout(timeout: Duration) -> UserResponse {
    let result = tokio::time::timeout(timeout, async {
        tokio::task::spawn_blocking(move || {
            let mut input = String::new();
            match io::stdin().read_line(&mut input) {
                Ok(_) => {
                    let trimmed = input.trim().to_lowercase();
                    if trimmed == "y" || trimmed == "yes" {
                        Some(true)
                    } else {
                        Some(false)
                    }
                }
                Err(_) => None,
            }
        })
        .await
        .unwrap_or(None)
    })
    .await;

    match result {
        Ok(Some(true)) => UserResponse::Yes,
        Ok(Some(false)) => UserResponse::No,
        Ok(None) => UserResponse::No, // stdin error → deny
        Err(_elapsed) => UserResponse::Timeout,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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