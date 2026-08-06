use crate::types::{Action, LlmResponse};
use crate::error::HarnessError;

/// Parses an [`LlmResponse`] into a concrete [`Action`] that the agent loop
/// can dispatch to tools, guardrails, and feedback channels.
///
/// # Parsing priority
///
/// 1. If the response carries `tool_calls` (native function-calling), the
///    first tool call is parsed into `Action::ToolCall`.
/// 2. XML-style: `<tool_call>{"name": "...", "arguments": {...}}</tool_call>`
/// 3. LangChain-style: `Action: <name>\nAction Input: <json>`
/// 4. If the response content starts with `"FINAL ANSWER:"`, the remainder
///    is treated as `Action::FinalAnswer`.
/// 5. Otherwise the entire content is returned as `Action::FinalAnswer`.
pub struct ActionParser;

impl ActionParser {
    /// Parse a raw LLM response into a structured action.
    pub fn parse(response: &LlmResponse) -> Result<Action, HarnessError> {
        // 1. Native tool calls (function-calling)
        if let Some(ref tool_calls) = response.tool_calls {
            if let Some(tc) = tool_calls.first() {
                let params: serde_json::Value = serde_json::from_str(&tc.arguments)
                    .unwrap_or(serde_json::Value::Null);
                return Ok(Action::ToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    params,
                });
            }
        }

        let content = response.content.trim();

        // 2. XML-style tool call: <tool_call>{"name": "...", "arguments": {...}}</tool_call>
        if let Some(action) = Self::parse_xml_tool_call(content) {
            return Ok(action);
        }

        // 3. LangChain-style: Action: <name>\nAction Input: <json>
        if let Some(action) = Self::parse_langchain_tool_call(content) {
            return Ok(action);
        }

        // 4. Content-based "FINAL ANSWER:" marker
        if let Some(after) = content.strip_prefix("FINAL ANSWER:") {
            return Ok(Action::FinalAnswer {
                summary: after.trim().to_string(),
            });
        }

        // 5. Default: treat content as final answer
        Ok(Action::FinalAnswer {
            summary: content.to_string(),
        })
    }

    /// Try to parse an XML-style tool call like:
    /// `<tool_call>{"name": "read_file", "arguments": {"path": "src/main.rs"}}</tool_call>`
    fn parse_xml_tool_call(content: &str) -> Option<Action> {
        let inner = content
            .strip_prefix("<tool_call>")
            .and_then(|s| s.strip_suffix("</tool_call>"))?;
        let v: serde_json::Value = serde_json::from_str(inner).ok()?;
        let name = v.get("name")?.as_str()?.to_string();
        let params = v.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
        Some(Action::ToolCall {
            id: format!("text-{}", uuid::Uuid::new_v4()),
            name,
            params,
        })
    }

    /// Try to parse LangChain-style:
    /// ```text
    /// Action: read_file
    /// Action Input: {"path": "src/main.rs"}
    /// ```
    fn parse_langchain_tool_call(content: &str) -> Option<Action> {
        let lines: Vec<&str> = content.lines().collect();
        let action_line = lines.iter().find(|l| l.trim().starts_with("Action:"))?;
        let tool_name = action_line
            .trim()
            .strip_prefix("Action:")?
            .trim()
            .to_string();

        if tool_name.is_empty() {
            return None;
        }

        let params = lines
            .iter()
            .find(|l| l.trim().starts_with("Action Input:"))
            .and_then(|l| {
                let json_str = l.trim().strip_prefix("Action Input:")?.trim();
                serde_json::from_str(json_str).ok()
            })
            .unwrap_or(serde_json::Value::Null);

        Some(Action::ToolCall {
            id: format!("text-{}", uuid::Uuid::new_v4()),
            name: tool_name,
            params,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FinishReason, TokenUsage, ToolCall};

    fn make_response(content: &str, tool_calls: Option<Vec<ToolCall>>) -> LlmResponse {
        LlmResponse {
            content: content.to_string(),
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
            tool_calls,
        }
    }

    #[test]
    fn test_parse_native_tool_call() {
        let tc = ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command": "cargo test"}"#.to_string(),
        };
        let response = make_response("", Some(vec![tc]));

        let action = ActionParser::parse(&response).unwrap();
        match action {
            Action::ToolCall { id, name, params } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "bash");
                assert_eq!(params["command"], "cargo test");
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_final_answer_marker() {
        let response = make_response("FINAL ANSWER: The task is complete.", None);

        let action = ActionParser::parse(&response).unwrap();
        match action {
            Action::FinalAnswer { summary } => {
                assert_eq!(summary, "The task is complete.");
            }
            other => panic!("expected FinalAnswer, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_final_answer_with_whitespace() {
        let response = make_response("  FINAL ANSWER:   Done!  ", None);

        let action = ActionParser::parse(&response).unwrap();
        match action {
            Action::FinalAnswer { summary } => {
                assert_eq!(summary, "Done!");
            }
            other => panic!("expected FinalAnswer, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_default_to_final_answer() {
        let response = make_response("Here is your code:\n\nfn main() {}", None);

        let action = ActionParser::parse(&response).unwrap();
        match action {
            Action::FinalAnswer { summary } => {
                assert_eq!(summary, "Here is your code:\n\nfn main() {}");
            }
            other => panic!("expected FinalAnswer, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_empty_content() {
        let response = make_response("", None);

        let action = ActionParser::parse(&response).unwrap();
        match action {
            Action::FinalAnswer { summary } => {
                assert_eq!(summary, "");
            }
            other => panic!("expected FinalAnswer, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_tool_calls_take_priority_over_content() {
        // Even if content says "FINAL ANSWER:", tool_calls take priority
        let tc = ToolCall {
            id: "call-2".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path": "src/main.rs"}"#.to_string(),
        };
        let response = make_response("FINAL ANSWER: Done", Some(vec![tc]));

        let action = ActionParser::parse(&response).unwrap();
        assert!(
            matches!(action, Action::ToolCall { .. }),
            "tool_calls should take priority over FINAL ANSWER content, got {:?}",
            action
        );
    }

    #[test]
    fn test_parse_malformed_tool_call_arguments_defaults_to_null() {
        let tc = ToolCall {
            id: "call-3".to_string(),
            name: "echo".to_string(),
            arguments: "not valid json".to_string(),
        };
        let response = make_response("", Some(vec![tc]));

        let action = ActionParser::parse(&response).unwrap();
        match action {
            Action::ToolCall { params, .. } => {
                assert_eq!(params, serde_json::Value::Null);
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }
}