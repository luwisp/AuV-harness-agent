use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// DeepSeek 思考模式要求：assistant 消息的 reasoning_content 必须在
    /// 后续请求中原样传回，否则返回 HTTP 400。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Action {
    #[serde(rename = "tool_call")]
    ToolCall {
        id: String,
        name: String,
        params: serde_json::Value,
    },
    #[serde(rename = "final_answer")]
    FinalAnswer { summary: String },
    #[serde(rename = "ask_user")]
    AskUser { question: String },
    #[serde(rename = "noop")]
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub content: String,
    pub structured: Option<serde_json::Value>,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub path: std::path::PathBuf,
    pub content_type: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    pub finish_reason: FinishReason,
    pub usage: TokenUsage,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardDecision {
    Allowed,
    Blocked,
    Escalated,
    Approved,
    Denied,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardResult {
    Allowed,
    Denied {
        reason: String,
        decision: GuardDecision,
    },
    NeedsApproval {
        risk_level: String,
        reasons: Vec<String>,
    },
}

impl GuardResult {
    pub fn is_allowed(&self) -> bool {
        matches!(self, GuardResult::Allowed)
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, GuardResult::Denied { .. })
    }

    pub fn needs_approval(&self) -> bool {
        matches!(self, GuardResult::NeedsApproval { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackError {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub error_type: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackResult {
    pub channel: String,
    pub passed: bool,
    pub errors: Vec<FeedbackError>,
    pub summary: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn action_json_roundtrips_for_all_variants() {
        let actions = vec![
            Action::ToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                params: json!({ "path": "src/main.rs" }),
            },
            Action::FinalAnswer {
                summary: "done".to_string(),
            },
            Action::AskUser {
                question: "Continue?".to_string(),
            },
            Action::NoOp,
        ];

        for action in actions {
            let encoded = serde_json::to_string(&action).expect("action serializes");
            let decoded: Action = serde_json::from_str(&encoded).expect("action deserializes");
            assert_eq!(decoded, action);
        }
    }

    #[test]
    fn guard_result_helpers_report_expected_state() {
        let allowed = GuardResult::Allowed;
        assert!(allowed.is_allowed());
        assert!(!allowed.is_denied());
        assert!(!allowed.needs_approval());

        let denied = GuardResult::Denied {
            reason: "blocked".to_string(),
            decision: GuardDecision::Blocked,
        };
        assert!(!denied.is_allowed());
        assert!(denied.is_denied());
        assert!(!denied.needs_approval());

        let approval = GuardResult::NeedsApproval {
            risk_level: "High".to_string(),
            reasons: vec!["destructive command".to_string()],
        };
        assert!(!approval.is_allowed());
        assert!(!approval.is_denied());
        assert!(approval.needs_approval());
    }

    #[test]
    fn message_creation_with_all_fields() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            arguments: "{\"command\":\"cargo test\"}".to_string(),
        };
        let message = Message {
            role: Role::Assistant,
            content: "Running tests".to_string(),
            reasoning_content: None,
            tool_calls: Some(vec![tool_call.clone()]),
            tool_call_id: Some("call-1".to_string()),
        };

        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content, "Running tests");
        assert_eq!(message.tool_calls, Some(vec![tool_call]));
        assert_eq!(message.tool_call_id.as_deref(), Some("call-1"));
    }
}
