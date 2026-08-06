pub mod approval;
pub mod assessor;
pub mod audit;
pub mod config;
pub mod rules;
pub mod sandbox;

#[cfg(test)]
use std::time::Duration;

use crate::guardrails::approval::{ApprovalDecision, ApprovalGate};
use crate::guardrails::assessor::{RiskAssessment, RiskAssessor, RiskLevel};
use crate::guardrails::audit::{AuditEntry, AuditLog};
use crate::guardrails::rules::StaticRuleEngine;
use crate::guardrails::sandbox::SandboxBoundary;
use crate::types::{Action, GuardDecision, GuardResult};
use chrono::Utc;

/// Context passed to guardrail evaluation.
///
/// Carries information about the execution environment that rules may
/// consult when deciding whether an action is dangerous.
#[derive(Debug, Clone)]
pub struct GuardContext {
    /// Unique identifier for the current session.
    pub session_id: String,
    /// Root directory of the workspace.  All file operations are expected to
    /// target paths within this directory.
    pub workspace_root: std::path::PathBuf,
    /// Optional user identifier (e.g. login name).
    pub user_id: Option<String>,
}

// ============================================================================
// GuardrailPipeline
// ============================================================================

/// Orchestrates all four guardrail layers into a single check pipeline.
///
/// # Layers
///
/// 1. **Static rules** — Fast, deterministic pattern matching.  If a rule
///    returns `Denied`, the pipeline stops immediately.
/// 2. **Risk assessment** — All registered assessors evaluate the action and
///    their results are merged into a single risk level.
/// 3. **Approval** — If the merged risk level is `High` or `Critical`, the
///    human-in-the-loop approval gate is invoked.
/// 4. **Sandbox** — Hard boundary enforcement that cannot be overridden by
///    approval.
pub struct GuardrailPipeline {
    rules: StaticRuleEngine,
    assessors: Vec<Box<dyn RiskAssessor>>,
    approval: ApprovalGate,
    sandbox: SandboxBoundary,
    audit: AuditLog,
}

impl GuardrailPipeline {
    /// Create a new pipeline with the given components.
    pub fn new(
        rules: StaticRuleEngine,
        assessors: Vec<Box<dyn RiskAssessor>>,
        approval: ApprovalGate,
        sandbox: SandboxBoundary,
        audit: AuditLog,
    ) -> Self {
        Self {
            rules,
            assessors,
            approval,
            sandbox,
            audit,
        }
    }

    /// Convenience constructor for tests: build a pipeline with default
    /// components and a zero approval timeout (so escalation always times out
    /// unless the caller overrides the gate).
    #[cfg(test)]
    pub(crate) fn for_testing(
        rules: StaticRuleEngine,
        assessors: Vec<Box<dyn RiskAssessor>>,
        sandbox: SandboxBoundary,
    ) -> Self {
        Self {
            rules,
            assessors,
            approval: ApprovalGate::new(Duration::from_millis(1)),
            sandbox,
            audit: AuditLog::new(std::path::PathBuf::from("/dev/null")),
        }
    }

    /// Run all four guardrail layers against the given action.
    ///
    /// Returns the final `GuardResult` after traversing static rules, risk
    /// assessment, approval (if needed), and sandbox validation.
    pub async fn check(&mut self, action: &Action, ctx: &GuardContext) -> GuardResult {
        // Layer 1: Static rules
        let rule_result = self.rules.evaluate(action, ctx);
        if rule_result.is_denied() {
            tracing::info!(
                action = ?action,
                session_id = %ctx.session_id,
                result = ?rule_result,
                "GuardrailPipeline[L1]: static rule denied action"
            );
            let _ = self.audit.record(AuditEntry {
                timestamp: Utc::now(),
                session_id: ctx.session_id.clone(),
                action_summary: action_summary(action),
                risk_level: "Low".to_string(),
                decision: "Denied".to_string(),
                approver: None,
                reasons: vec!["Blocked by static rule".to_string()],
            });
            return rule_result;
        }

        // If L1 escalated, seed the risk assessment at High so that L3
        // approval is triggered even when no assessors are registered.
        let initial_assessment = if rule_result.needs_approval() {
            tracing::info!(
                action = ?action,
                session_id = %ctx.session_id,
                "GuardrailPipeline[L1]: static rule escalated action"
            );
            if let GuardResult::NeedsApproval { reasons, .. } = &rule_result {
                RiskAssessment {
                    level: RiskLevel::High,
                    reasons: reasons.clone(),
                    suggested_mitigation: Some(
                        "Static rule escalation — requires human approval".to_string(),
                    ),
                }
            } else {
                RiskAssessment::low()
            }
        } else {
            RiskAssessment::low()
        };

        // Layer 2: Risk assessment — merge all assessors
        let assessment = self.assessors.iter().fold(initial_assessment, |acc, a| {
            acc.merge(a.assess(action, ctx))
        });

        tracing::info!(
            action = ?action,
            session_id = %ctx.session_id,
            risk_level = ?assessment.level,
            reasons = ?assessment.reasons,
            "GuardrailPipeline[L2]: risk assessment complete"
        );

        // Layer 3: Approval — if risk is High or Critical
        if assessment.level >= RiskLevel::High {
            tracing::info!(
                action = ?action,
                session_id = %ctx.session_id,
                risk_level = ?assessment.level,
                "GuardrailPipeline[L3]: requesting approval"
            );
            let risk_level_str = format!("{:?}", assessment.level);
            let assessment_reasons = assessment.reasons.clone();
            let decision = self.approval.request_approval(action, &assessment).await;
            match decision {
                ApprovalDecision::Approved { by, reason } => {
                    tracing::info!(
                        session_id = %ctx.session_id,
                        approved_by = %by,
                        reason = ?reason,
                        "GuardrailPipeline[L3]: action approved"
                    );
                    let _ = self.audit.record(AuditEntry {
                        timestamp: Utc::now(),
                        session_id: ctx.session_id.clone(),
                        action_summary: action_summary(action),
                        risk_level: risk_level_str,
                        decision: "Approved".to_string(),
                        approver: Some(by.clone()),
                        reasons: assessment_reasons,
                    });
                }
                ApprovalDecision::Denied { reason } => {
                    tracing::info!(
                        session_id = %ctx.session_id,
                        reason = %reason,
                        "GuardrailPipeline[L3]: action denied by user"
                    );
                    let _ = self.audit.record(AuditEntry {
                        timestamp: Utc::now(),
                        session_id: ctx.session_id.clone(),
                        action_summary: action_summary(action),
                        risk_level: risk_level_str,
                        decision: "Denied".to_string(),
                        approver: None,
                        reasons: assessment_reasons,
                    });
                    return GuardResult::Denied {
                        reason,
                        decision: GuardDecision::Denied,
                    };
                }
                ApprovalDecision::Timeout => {
                    tracing::info!(
                        session_id = %ctx.session_id,
                        "GuardrailPipeline[L3]: approval timed out"
                    );
                    let _ = self.audit.record(AuditEntry {
                        timestamp: Utc::now(),
                        session_id: ctx.session_id.clone(),
                        action_summary: action_summary(action),
                        risk_level: risk_level_str,
                        decision: "Timeout".to_string(),
                        approver: None,
                        reasons: assessment_reasons,
                    });
                    return GuardResult::Denied {
                        reason: "Approval request timed out".to_string(),
                        decision: GuardDecision::Timeout,
                    };
                }
            }
        }

        // Layer 4: Sandbox — hard boundary enforcement
        if let Err(violation) = self.sandbox.validate(action) {
            tracing::info!(
                action = ?action,
                session_id = %ctx.session_id,
                violation = %violation.message,
                violation_type = ?violation.violation_type,
                "GuardrailPipeline[L4]: sandbox violation"
            );
            let _ = self.audit.record(AuditEntry {
                timestamp: Utc::now(),
                session_id: ctx.session_id.clone(),
                action_summary: action_summary(action),
                risk_level: "Low".to_string(),
                decision: "Blocked".to_string(),
                approver: None,
                reasons: vec![violation.message.clone()],
            });
            return GuardResult::Denied {
                reason: violation.message,
                decision: GuardDecision::Blocked,
            };
        }

        tracing::info!(
            action = ?action,
            session_id = %ctx.session_id,
            "GuardrailPipeline: action allowed"
        );
        let _ = self.audit.record(AuditEntry {
            timestamp: Utc::now(),
            session_id: ctx.session_id.clone(),
            action_summary: action_summary(action),
            risk_level: "Low".to_string(),
            decision: "Allowed".to_string(),
            approver: None,
            reasons: Vec::new(),
        });
        GuardResult::Allowed
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Create a human-readable summary of an action for audit logging.
fn action_summary(action: &Action) -> String {
    match action {
        Action::ToolCall { name, params, .. } => {
            let args = match serde_json::to_string(params) {
                Ok(s) => s,
                Err(_) => "?".to_string(),
            };
            format!("{name}: {args}")
        }
        Action::FinalAnswer { summary } => {
            format!("final_answer: {summary}")
        }
        Action::AskUser { question } => {
            format!("ask_user: {question}")
        }
        Action::NoOp => "noop".to_string(),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guardrails::assessor::{CommandRiskAssessor, FileRiskAssessor, NetworkRiskAssessor};
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::Duration;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn test_context() -> GuardContext {
        GuardContext {
            session_id: "test-session-1".to_string(),
            workspace_root: PathBuf::from("/home/user/project"),
            user_id: Some("test-user".to_string()),
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

    fn engine_with_builtins() -> StaticRuleEngine {
        let mut engine = StaticRuleEngine::new();
        engine.load_builtin_rules();
        engine
    }

    fn permissive_sandbox() -> SandboxBoundary {
        SandboxBoundary {
            workspace_root: PathBuf::from("/home/user/project"),
            allowed_commands: vec![],
            forbidden_commands: vec![],
            max_timeout: Duration::from_secs(300),
            network_allowed: true,
        }
    }

    fn restricted_sandbox() -> SandboxBoundary {
        SandboxBoundary {
            workspace_root: PathBuf::from("/home/user/project"),
            allowed_commands: vec![],
            forbidden_commands: vec!["rm".into(), "sudo".into()],
            max_timeout: Duration::from_secs(300),
            network_allowed: false,
        }
    }

    // -----------------------------------------------------------------------
    // Integration tests
    // -----------------------------------------------------------------------

    /// Pipeline with builtin rules should deny `rm -rf /`.
    #[tokio::test]
    async fn test_pipeline_denies_rm_rf() {
        let rules = engine_with_builtins();
        let assessors: Vec<Box<dyn RiskAssessor>> = vec![];
        let sandbox = permissive_sandbox();

        let mut pipeline = GuardrailPipeline::for_testing(rules, assessors, sandbox);
        let ctx = test_context();

        let result = pipeline.check(&bash_action("rm -rf /"), &ctx).await;
        assert!(
            result.is_denied(),
            "rm -rf / should be Denied by static rules, got {:?}",
            result
        );
    }

    /// Pipeline should allow a normal command like `cargo build`.
    #[tokio::test]
    async fn test_pipeline_allows_normal_command() {
        let rules = engine_with_builtins();
        let assessors: Vec<Box<dyn RiskAssessor>> = vec![];
        let sandbox = permissive_sandbox();

        let mut pipeline = GuardrailPipeline::for_testing(rules, assessors, sandbox);
        let ctx = test_context();

        let result = pipeline.check(&bash_action("cargo build"), &ctx).await;
        assert!(
            result.is_allowed(),
            "cargo build should be Allowed, got {:?}",
            result
        );
    }

    /// Pipeline with a mock approval gate (tiny timeout → auto-deny) should
    /// deny an action that triggers escalation in the static rules.
    #[tokio::test]
    async fn test_pipeline_escalates_to_approval() {
        let rules = engine_with_builtins();
        let assessors: Vec<Box<dyn RiskAssessor>> = vec![];
        let sandbox = permissive_sandbox();

        // Use a 1ms timeout so the approval gate always times out, simulating
        // a "deny" from the human-in-the-loop stage.
        let mut pipeline = GuardrailPipeline::new(
            rules,
            assessors,
            ApprovalGate::new(Duration::from_millis(1)),
            sandbox,
            AuditLog::new(std::path::PathBuf::from("/dev/null")),
        );
        let ctx = test_context();

        // rm -rf ~ triggers the "escalate-rm-rf-home" rule → NeedsApproval →
        // pipeline continues to L3 approval, which times out → Denied
        let result = pipeline.check(&bash_action("rm -rf ~"), &ctx).await;
        assert!(
            result.is_denied(),
            "rm -rf ~ should escalate to approval and be denied (timeout), got {:?}",
            result
        );
    }

    /// Pipeline with a restricted sandbox should deny writing to a path
    /// outside the workspace root.
    #[tokio::test]
    async fn test_pipeline_sandbox_rejects_outside_path() {
        let rules = engine_with_builtins();
        let assessors: Vec<Box<dyn RiskAssessor>> = vec![];
        let sandbox = restricted_sandbox();

        let mut pipeline = GuardrailPipeline::for_testing(rules, assessors, sandbox);
        let ctx = test_context();

        // Writing to /etc is outside the workspace root, sandbox should reject
        let result = pipeline
            .check(&write_file_action("/etc/passwd"), &ctx)
            .await;
        assert!(
            result.is_denied(),
            "write to /etc/passwd should be denied by sandbox, got {:?}",
            result
        );
    }

    /// Pipeline with all assessors should merge risk and escalate if risk is High.
    #[tokio::test]
    async fn test_pipeline_assessors_contribute_to_risk() {
        let rules = engine_with_builtins();
        let assessors: Vec<Box<dyn RiskAssessor>> = vec![
            Box::new(CommandRiskAssessor),
            Box::new(FileRiskAssessor),
            Box::new(NetworkRiskAssessor),
        ];
        let sandbox = permissive_sandbox();

        // Use 1ms approval timeout so escalation results in deny
        let mut pipeline = GuardrailPipeline::new(
            rules,
            assessors,
            ApprovalGate::new(Duration::from_millis(1)),
            sandbox,
            AuditLog::new(std::path::PathBuf::from("/dev/null")),
        );
        let ctx = test_context();

        // sudo echo > /etc/passwd: CommandRiskAssessor → High (sudo),
        // FileRiskAssessor → Critical (/etc), NetworkRiskAssessor → Low
        // Merged → Critical → triggers approval → timeout → Denied
        let result = pipeline
            .check(&bash_action("sudo echo config > /etc/app.conf"), &ctx)
            .await;
        assert!(
            result.is_denied(),
            "sudo + write to /etc should be Critical and escalate to approval, got {:?}",
            result
        );
    }

    /// Pipeline with a permissive sandbox should allow a safe file write.
    #[tokio::test]
    async fn test_pipeline_allows_safe_file_write() {
        let rules = engine_with_builtins();
        let assessors: Vec<Box<dyn RiskAssessor>> = vec![];
        let sandbox = permissive_sandbox();

        let mut pipeline = GuardrailPipeline::for_testing(rules, assessors, sandbox);
        let ctx = test_context();

        let result = pipeline
            .check(&write_file_action("src/main.rs"), &ctx)
            .await;
        assert!(
            result.is_allowed(),
            "write to src/main.rs should be Allowed, got {:?}",
            result
        );
    }

    /// Pipeline with a network-disabled sandbox should deny network commands.
    #[tokio::test]
    async fn test_pipeline_sandbox_blocks_network() {
        let rules = engine_with_builtins();
        let assessors: Vec<Box<dyn RiskAssessor>> = vec![];
        let sandbox = restricted_sandbox(); // network_allowed: false

        let mut pipeline = GuardrailPipeline::for_testing(rules, assessors, sandbox);
        let ctx = test_context();

        let result = pipeline
            .check(&bash_action("curl https://example.com"), &ctx)
            .await;
        assert!(
            result.is_denied(),
            "curl should be denied by sandbox when network is disabled, got {:?}",
            result
        );
    }

    /// Verify that L1 deny takes priority and stops the pipeline before L2-L4.
    #[tokio::test]
    async fn test_pipeline_l1_deny_stops_early() {
        let rules = engine_with_builtins();
        let assessors: Vec<Box<dyn RiskAssessor>> = vec![];
        let sandbox = permissive_sandbox();

        let mut pipeline = GuardrailPipeline::for_testing(rules, assessors, sandbox);
        let ctx = test_context();

        // dd if= is blocked by L1 (priority 100), should return Denied
        // without ever reaching L2-L4
        let result = pipeline
            .check(&bash_action("dd if=/dev/sda of=/dev/sdb"), &ctx)
            .await;
        assert!(
            result.is_denied(),
            "dd if= should be Denied by L1 static rules, got {:?}",
            result
        );
    }
}