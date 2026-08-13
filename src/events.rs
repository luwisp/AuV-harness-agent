//! Agent event types shared between the agent loop, TUI, and REPL.
//!
//! The `AgentEvent` enum is the primary communication channel from the
//! background agent loop to the foreground UI (TUI or REPL task consumer).

use crate::guardrails::approval::ApprovalDecision;
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
    /// 子 agent 的护栏审批请求（TUI 模式）。
    ///
    /// 子 loop 运行在父工具执行期间的子线程里，其审批门无法读 stdin
    /// （crossterm raw mode），因此请求以事件路由到父界面面板渲染；
    /// y/n 决定经事件自带的 `decision_tx`（子专属通道）回发，
    /// 不与父审批的全局决策通道混用。
    SubagentApprovalNeeded {
        request: ApprovalRequest,
        /// 审批来源标签（如「子 agent」），供面板区分父子审批。
        label: String,
        /// 决定回发通道：UI 按键产生的决定直接发回子 loop 的审批门。
        decision_tx: tokio::sync::mpsc::Sender<ApprovalDecision>,
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
