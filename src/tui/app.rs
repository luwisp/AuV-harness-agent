use crate::types::{Message, ToolResult};

/// Which panel in the TUI has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// The conversation/messages panel.
    #[default]
    Conversation,
    /// The tools panel.
    Tools,
    /// The guardrails/approval panel.
    Guardrails,
    /// The status bar.
    Status,
}

impl Focus {
    /// Cycle to the next focus target.
    pub fn next(self) -> Self {
        match self {
            Focus::Conversation => Focus::Tools,
            Focus::Tools => Focus::Guardrails,
            Focus::Guardrails => Focus::Status,
            Focus::Status => Focus::Conversation,
        }
    }
}

/// Represents a guardrail approval request displayed to the user in the TUI.
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
}

impl ApprovalRequest {
    /// Create a new approval request.
    pub fn new(
        id: String,
        action_summary: String,
        risk_level: String,
        reasons: Vec<String>,
    ) -> Self {
        Self {
            id,
            action_summary,
            risk_level,
            reasons,
        }
    }
}

/// Status information about the current agent run, displayed in the status bar.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusInfo {
    /// The current turn number (0-indexed).
    pub turn: usize,
    /// Cumulative tokens used so far.
    pub tokens_used: u32,
    /// The current risk level (e.g., "Low", "Medium", "High", "Critical").
    pub risk_level: String,
    /// The model being used (e.g., "gpt-4o").
    pub model: String,
}

impl StatusInfo {
    /// Create a new status info with the given model and zeroed counters.
    pub fn new(model: &str) -> Self {
        Self {
            turn: 0,
            tokens_used: 0,
            risk_level: "Low".to_string(),
            model: model.to_string(),
        }
    }
}

impl Default for StatusInfo {
    fn default() -> Self {
        Self {
            turn: 0,
            tokens_used: 0,
            risk_level: "Low".to_string(),
            model: String::new(),
        }
    }
}

/// The full application state for the TUI.
///
/// Holds all data that the TUI renders: messages from the agent, the current
/// tool being executed, tool results, pending guardrail approval requests,
/// and status information.
#[derive(Debug, Clone)]
pub struct AppState {
    /// All messages exchanged between the agent, tools, and user.
    pub messages: Vec<Message>,
    /// The name of the tool currently being executed (if any).
    pub current_tool: Option<String>,
    /// Results from completed tool executions.
    pub tool_results: Vec<ToolResult>,
    /// Pending guardrail approval requests.
    pub guard_requests: Vec<ApprovalRequest>,
    /// Status information for the status bar.
    pub status: StatusInfo,
    /// Whether the agent loop is still running.
    pub running: bool,
    /// Which panel currently has keyboard focus.
    pub focus: Focus,
}

impl AppState {
    /// Create a new application state with the given model name.
    pub fn new(model: &str) -> Self {
        Self {
            messages: Vec::new(),
            current_tool: None,
            tool_results: Vec::new(),
            guard_requests: Vec::new(),
            status: StatusInfo::new(model),
            running: true,
            focus: Focus::Conversation,
        }
    }

    /// Add a guardrail approval request to the pending list.
    pub fn add_guard_request(&mut self, request: ApprovalRequest) {
        self.guard_requests.push(request);
    }

    /// Remove a guardrail approval request by ID.
    ///
    /// Returns `true` if a request was removed.
    pub fn remove_guard_request(&mut self, id: &str) -> bool {
        let len_before = self.guard_requests.len();
        self.guard_requests.retain(|r| r.id != id);
        self.guard_requests.len() < len_before
    }

    /// Record a tool execution result.
    pub fn add_tool_result(&mut self, result: ToolResult) {
        self.tool_results.push(result);
    }

    /// Set the currently executing tool.
    pub fn set_current_tool(&mut self, tool_name: Option<String>) {
        self.current_tool = tool_name;
    }

    /// Mark the agent as stopped.
    pub fn stop(&mut self) {
        self.running = false;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Artifact, Message, Role, ToolResult};

    fn make_message(content: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn make_tool_result() -> ToolResult {
        ToolResult {
            success: true,
            content: "echo: hello".to_string(),
            structured: None,
            artifacts: vec![Artifact {
                path: std::path::PathBuf::from("output.txt"),
                content_type: "text/plain".to_string(),
                size_bytes: 42,
            }],
        }
    }

    // -----------------------------------------------------------------------
    // StatusInfo tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_status_info_new() {
        let status = StatusInfo::new("gpt-4o");
        assert_eq!(status.turn, 0);
        assert_eq!(status.tokens_used, 0);
        assert_eq!(status.risk_level, "Low");
        assert_eq!(status.model, "gpt-4o");
    }

    #[test]
    fn test_status_info_default() {
        let status = StatusInfo::default();
        assert_eq!(status.turn, 0);
        assert_eq!(status.tokens_used, 0);
        assert_eq!(status.risk_level, "Low");
        assert_eq!(status.model, "");
    }

    #[test]
    fn test_status_info_clone() {
        let status = StatusInfo::new("claude-sonnet-4-20250514");
        let cloned = status.clone();
        assert_eq!(status, cloned);
    }

    // -----------------------------------------------------------------------
    // AppState tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_app_state_new() {
        let state = AppState::new("gpt-4o");
        assert!(state.messages.is_empty());
        assert_eq!(state.current_tool, None);
        assert!(state.tool_results.is_empty());
        assert!(state.guard_requests.is_empty());
        assert_eq!(state.status.turn, 0);
        assert_eq!(state.status.model, "gpt-4o");
        assert!(state.running);
    }

    #[test]
    fn test_app_state_add_guard_request() {
        let mut state = AppState::new("gpt-4o");
        let req = ApprovalRequest::new(
            "req-1".to_string(),
            "bash: rm -rf /tmp/test".to_string(),
            "High".to_string(),
            vec!["Destructive command".to_string()],
        );
        state.add_guard_request(req.clone());
        assert_eq!(state.guard_requests.len(), 1);
        assert_eq!(state.guard_requests[0], req);
    }

    #[test]
    fn test_app_state_remove_guard_request() {
        let mut state = AppState::new("gpt-4o");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "action 1".to_string(),
            "High".to_string(),
            vec!["reason".to_string()],
        ));
        state.add_guard_request(ApprovalRequest::new(
            "req-2".to_string(),
            "action 2".to_string(),
            "Medium".to_string(),
            vec!["reason 2".to_string()],
        ));

        assert!(state.remove_guard_request("req-1"));
        assert_eq!(state.guard_requests.len(), 1);
        assert_eq!(state.guard_requests[0].id, "req-2");

        assert!(!state.remove_guard_request("nonexistent"));
        assert_eq!(state.guard_requests.len(), 1);
    }

    #[test]
    fn test_app_state_add_tool_result() {
        let mut state = AppState::new("gpt-4o");
        let result = make_tool_result();
        state.add_tool_result(result.clone());
        assert_eq!(state.tool_results.len(), 1);
        assert_eq!(state.tool_results[0], result);
    }

    #[test]
    fn test_app_state_set_current_tool() {
        let mut state = AppState::new("gpt-4o");
        assert_eq!(state.current_tool, None);

        state.set_current_tool(Some("bash".to_string()));
        assert_eq!(state.current_tool, Some("bash".to_string()));

        state.set_current_tool(None);
        assert_eq!(state.current_tool, None);
    }

    #[test]
    fn test_app_state_stop() {
        let mut state = AppState::new("gpt-4o");
        assert!(state.running);
        state.stop();
        assert!(!state.running);
    }

    #[test]
    fn test_app_state_messages_accumulate() {
        let mut state = AppState::new("gpt-4o");
        state.messages.push(make_message("Hello"));
        state.messages.push(make_message("World"));
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].content, "Hello");
        assert_eq!(state.messages[1].content, "World");
    }

    // -----------------------------------------------------------------------
    // ApprovalRequest tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_approval_request_new() {
        let req = ApprovalRequest::new(
            "req-1".to_string(),
            "bash: rm -rf /tmp".to_string(),
            "Critical".to_string(),
            vec!["Destructive command".to_string(), "Outside workspace".to_string()],
        );
        assert_eq!(req.id, "req-1");
        assert_eq!(req.action_summary, "bash: rm -rf /tmp");
        assert_eq!(req.risk_level, "Critical");
        assert_eq!(req.reasons.len(), 2);
    }

    #[test]
    fn test_approval_request_clone_eq() {
        let req = ApprovalRequest::new(
            "req-1".to_string(),
            "action".to_string(),
            "High".to_string(),
            vec!["reason".to_string()],
        );
        let cloned = req.clone();
        assert_eq!(req, cloned);
    }

    // -----------------------------------------------------------------------
    // Focus tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_focus_default_is_conversation() {
        assert_eq!(Focus::default(), Focus::Conversation);
    }

    #[test]
    fn test_focus_cycles_through_all_panels() {
        let mut focus = Focus::Conversation;
        focus = focus.next();
        assert_eq!(focus, Focus::Tools);
        focus = focus.next();
        assert_eq!(focus, Focus::Guardrails);
        focus = focus.next();
        assert_eq!(focus, Focus::Status);
        focus = focus.next();
        assert_eq!(focus, Focus::Conversation);
    }

    #[test]
    fn test_app_state_has_focus_field() {
        let state = AppState::new("gpt-4o");
        assert_eq!(state.focus, Focus::Conversation);
    }
}