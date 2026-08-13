use async_trait::async_trait;
use std::sync::Mutex;
use crate::llm::LlmProvider;
use crate::types::{Message, LlmResponse, FinishReason, TokenUsage};
use crate::error::HarnessError;

pub struct MockLlmProvider {
    responses: Mutex<Vec<LlmResponse>>,
    call_count: Mutex<usize>,
}

impl MockLlmProvider {
    pub fn new(responses: Vec<LlmResponse>) -> Self {
        Self { responses: Mutex::new(responses), call_count: Mutex::new(0) }
    }

    pub fn call_count(&self) -> usize {
        *self.call_count.lock().unwrap()
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn complete(&self, _messages: &[Message], _tools: &[crate::types::ToolInfo]) -> Result<LlmResponse, HarnessError> {
        let mut count = self.call_count.lock().unwrap();
        let responses = self.responses.lock().unwrap();
        let idx = *count;
        *count += 1;
        if idx < responses.len() {
            Ok(responses[idx].clone())
        } else {
            Ok(LlmResponse {
                content: "Done".to_string(),
                reasoning_content: None,
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
                tool_calls: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCall;

    fn make_response(content: &str, finish_reason: FinishReason) -> LlmResponse {
        LlmResponse {
            content: content.to_string(),
            reasoning_content: None,
            finish_reason,
            usage: TokenUsage::default(),
            tool_calls: None,
        }
    }

    #[tokio::test]
    async fn test_mock_returns_programmed_responses() {
        let responses = vec![
            make_response("first", FinishReason::Stop),
            make_response("second", FinishReason::Stop),
        ];
        let mock = MockLlmProvider::new(responses);
        let empty_messages: Vec<Message> = vec![];

        let r1 = mock.complete(&empty_messages, &[]).await.unwrap();
        assert_eq!(r1.content, "first");
        assert_eq!(r1.finish_reason, FinishReason::Stop);

        let r2 = mock.complete(&empty_messages, &[]).await.unwrap();
        assert_eq!(r2.content, "second");
        assert_eq!(r2.finish_reason, FinishReason::Stop);

        assert_eq!(mock.call_count(), 2);
    }

    #[tokio::test]
    async fn test_mock_exhausted_returns_default() {
        let mock = MockLlmProvider::new(vec![]);
        let empty_messages: Vec<Message> = vec![];

        let r = mock.complete(&empty_messages, &[]).await.unwrap();
        assert_eq!(r.content, "Done");
        assert_eq!(r.finish_reason, FinishReason::Stop);
        assert_eq!(r.usage, TokenUsage::default());
        assert!(r.tool_calls.is_none());
        assert_eq!(mock.call_count(), 1);
    }

    #[tokio::test]
    async fn test_mock_returns_tool_calls() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command": "cargo build"}"#.to_string(),
        };
        let response = LlmResponse {
            content: String::new(),
            reasoning_content: None,
            finish_reason: FinishReason::ToolCalls,
            usage: TokenUsage::default(),
            tool_calls: Some(vec![tool_call.clone()]),
        };
        let mock = MockLlmProvider::new(vec![response]);
        let empty_messages: Vec<Message> = vec![];

        let r = mock.complete(&empty_messages, &[]).await.unwrap();
        assert_eq!(r.finish_reason, FinishReason::ToolCalls);
        assert!(r.tool_calls.is_some());

        let calls = r.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call-1");
        assert_eq!(calls[0].name, "bash");
        assert_eq!(calls[0].arguments, r#"{"command": "cargo build"}"#);
    }
}