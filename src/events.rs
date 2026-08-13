//! Agent event types shared between the agent loop, TUI, and REPL.
//!
//! The `AgentEvent` enum is the primary communication channel from the
//! background agent loop to the foreground UI (TUI or REPL task consumer).

use crate::types::Message;

// ============================================================================
// AgentEvent
// ============================================================================

/// Messages sent from the background agent task to the foreground UI.
///
/// The agent loop emits these events at key points during execution:
/// message received, tool call started/completed, progress update, completion.
#[derive(Debug)]
pub enum AgentEvent {
    /// A new message was added to the conversation.
    MessageAdded {
        message: Message,
    },
    /// A tool call has started executing.
    ToolCallStarted {
        name: String,
        /// 具体命令/参数摘要（如 bash 的命令行），供界面直接展示。
        detail: String,
    },
    /// A tool call has completed.
    ToolCallCompleted {
        name: String,
        /// 具体命令/参数摘要（如 bash 的命令行），供界面直接展示。
        detail: String,
        /// Content of the tool result.
        result_content: String,
        /// Whether the tool execution succeeded.
        success: bool,
    },
    /// A guardrail approval is needed.
    GuardrailApprovalNeeded {
        request: ApprovalRequest,
    },
    /// Progress update (turn, tokens, risk level).
    ProgressUpdate {
        turn: usize,
        tokens_used: u32,
        risk_level: String,
    },
    /// The agent loop has finished executing.
    Finished {
        /// Final result — Ok(summary) on success, Err(message) on failure.
        result: Result<String, String>,
    },
}

// ============================================================================
// AgentEventSender — convenience type alias
// ============================================================================

/// A sender for agent events, used to push events from the agent loop to a UI.
pub type AgentEventSender = tokio::sync::mpsc::Sender<AgentEvent>;

// ============================================================================
// ApprovalRequest
// ============================================================================

/// Represents a guardrail approval request displayed to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    /// Unique identifier for this approval request.
    pub id: String,
    /// Human-readable summary of the action requiring approval.
    pub action_summary: String,
    /// The risk level associated with the action (e.g., "High", "Critical").
    pub risk_level: String,
    /// Reasons explaining why the action requires approval.
    pub reasons: Vec<String>,
    /// Suggested mitigation for the risk, if the assessor provided one.
    pub suggested_mitigation: Option<String>,
}

impl ApprovalRequest {
    /// Create a new approval request.
    pub fn new(
        id: String,
        action_summary: String,
        risk_level: String,
        reasons: Vec<String>,
        suggested_mitigation: Option<String>,
    ) -> Self {
        Self {
            id,
            action_summary,
            risk_level,
            reasons,
            suggested_mitigation,
        }
    }
}
