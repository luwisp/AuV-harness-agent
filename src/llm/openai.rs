use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use crate::llm::LlmProvider;
use crate::types::{Message, LlmResponse, FinishReason, TokenUsage, ToolCall, Role};
use crate::error::HarnessError;

pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: String, base_url: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
        }
    }
}

// --- Request types ---

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
    /// DeepSeek 思考模式：assistant 消息必须把上轮返回的
    /// reasoning_content 原样传回，否则 API 返回 400。
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ChatToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct ChatToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: FunctionCall,
}

#[derive(Serialize)]
struct FunctionCall {
    name: String,
    arguments: String,
}

// --- Response types ---

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<UsageData>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ResponseToolCall>>,
}

#[derive(Deserialize)]
struct ResponseToolCall {
    id: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    call_type: String,
    function: FunctionCallData,
}

#[derive(Deserialize)]
struct FunctionCallData {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct UsageData {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

// --- Helpers ---

fn role_to_string(role: &Role) -> String {
    match role {
        Role::System => "system".to_string(),
        Role::User => "user".to_string(),
        Role::Assistant => "assistant".to_string(),
        Role::Tool => "tool".to_string(),
    }
}

fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "tool_calls" => FinishReason::ToolCalls,
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Stop,
    }
}

fn messages_to_chat_messages(messages: &[Message]) -> Vec<ChatMessage> {
    messages.iter().map(|m| ChatMessage {
        role: role_to_string(&m.role),
        content: m.content.clone(),
        reasoning_content: m.reasoning_content.clone(),
        tool_calls: m.tool_calls.as_ref().map(|tcs| {
            tcs.iter().map(|tc| ChatToolCall {
                id: tc.id.clone(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                },
            }).collect()
        }),
        tool_call_id: m.tool_call_id.clone(),
    }).collect()
}

fn parse_tool_calls(tool_calls: Option<Vec<ResponseToolCall>>) -> Option<Vec<ToolCall>> {
    tool_calls.map(|tcs| {
        tcs.into_iter().map(|tc| ToolCall {
            id: tc.id,
            name: tc.function.name,
            arguments: tc.function.arguments,
        }).collect()
    })
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete(&self, messages: &[Message], tools: &[crate::types::ToolInfo]) -> Result<LlmResponse, HarnessError> {
        let openai_tools: Option<Vec<serde_json::Value>> = if tools.is_empty() {
            None
        } else {
            Some(tools.iter().map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            }).collect())
        };

        let request = ChatRequest {
            model: self.model.clone(),
            messages: messages_to_chat_messages(messages),
            tools: openai_tools,
            tool_choice: if tools.is_empty() { None } else { Some("auto".to_string()) },
            max_tokens: 4096,
            temperature: 0.7,
        };

        let response = self.client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(HarnessError::Auth("Invalid API key — check your credentials".to_string()));
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(HarnessError::Llm("Rate limited — retry after a short wait".to_string()));
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            // Log the failing request for debugging
            let req_json = serde_json::to_string_pretty(&request).unwrap_or_default();
            tracing::debug!(
                status = status.as_u16(),
                body = %body,
                request = %req_json,
                "LLM API error"
            );
            return Err(HarnessError::Llm(format!("HTTP {}: {}", status.as_u16(), body)));
        }

        let chat_response: ChatResponse = response.json().await?;

        let choice = chat_response.choices.into_iter().next()
            .ok_or_else(|| HarnessError::Llm("Empty response — no choices returned".to_string()))?;

        let finish_reason = map_finish_reason(&choice.finish_reason.unwrap_or_default());
        let content = choice.message.content.unwrap_or_default();
        let reasoning_content = choice.message.reasoning_content;
        let tool_calls = parse_tool_calls(choice.message.tool_calls);

        let usage = chat_response.usage.map(|u| TokenUsage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        }).unwrap_or_default();

        Ok(LlmResponse {
            content,
            reasoning_content,
            finish_reason,
            usage,
            tool_calls,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path, header};
    use serde_json::json;

    #[tokio::test]
    async fn test_openai_handles_text_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "content": "Hello from OpenAI!" },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5,
                    "total_tokens": 15
                }
            })))
            .mount(&mock_server)
            .await;

        let provider = OpenAiProvider::new(
            "test-key".to_string(),
            "gpt-4o".to_string(),
            Some(mock_server.uri()),
        );

        let messages = vec![Message {
            role: Role::User,
            content: "hi".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }];

        let response = provider.complete(&messages, &[]).await.unwrap();
        assert_eq!(response.content, "Hello from OpenAI!");
        assert_eq!(response.finish_reason, FinishReason::Stop);
        assert_eq!(response.usage.total_tokens, 15);
        assert!(response.tool_calls.is_none());
    }

    #[tokio::test]
    async fn test_openai_handles_tool_calls() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": {
                        "content": null,
                        "tool_calls": [{
                            "id": "call_abc123",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"src/main.rs\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": { "prompt_tokens": 20, "completion_tokens": 15, "total_tokens": 35 }
            })))
            .mount(&mock_server)
            .await;

        let provider = OpenAiProvider::new(
            "test-key".to_string(),
            "gpt-4o".to_string(),
            Some(mock_server.uri()),
        );

        let messages = vec![Message {
            role: Role::User,
            content: "read main.rs".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }];

        let response = provider.complete(&messages, &[]).await.unwrap();
        assert_eq!(response.finish_reason, FinishReason::ToolCalls);
        assert!(response.tool_calls.is_some());

        let calls = response.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_abc123");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, "{\"path\":\"src/main.rs\"}");
    }

    #[tokio::test]
    async fn test_openai_handles_auth_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&mock_server)
            .await;

        let provider = OpenAiProvider::new(
            "bad-key".to_string(),
            "gpt-4o".to_string(),
            Some(mock_server.uri()),
        );

        let messages = vec![Message {
            role: Role::User,
            content: "hi".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }];

        let result = provider.complete(&messages, &[]).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            HarnessError::Auth(_) => {} // expected
            other => panic!("Expected Auth error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_openai_passes_reasoning_content_through() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": {
                        "content": "Let me think...",
                        "reasoning_content": "思考过程：先读取文件，再决定下一步"
                    },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
            })))
            .mount(&mock_server)
            .await;

        let provider = OpenAiProvider::new(
            "test-key".to_string(),
            "deepseek-reasoner".to_string(),
            Some(mock_server.uri()),
        );

        // 第一次调用：解析响应中的 reasoning_content
        let messages = vec![Message {
            role: Role::User,
            content: "hi".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        let response = provider.complete(&messages, &[]).await.unwrap();
        assert_eq!(
            response.reasoning_content.as_deref(),
            Some("思考过程：先读取文件，再决定下一步")
        );

        // 第二次调用：assistant 消息携带 reasoning_content，必须原样回传
        let messages = vec![
            Message {
                role: Role::User,
                content: "hi".to_string(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: Role::Assistant,
                content: "Let me think...".to_string(),
                reasoning_content: response.reasoning_content,
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        provider.complete(&messages, &[]).await.unwrap();

        // 校验收到的请求体：assistant 消息包含原样的 reasoning_content，
        // 而 user 消息不带该字段
        let received = mock_server.received_requests().await.unwrap().pop().unwrap();
        let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
        let user_msg = &body["messages"][0];
        let assistant_msg = &body["messages"][1];
        assert!(user_msg.get("reasoning_content").is_none());
        assert_eq!(assistant_msg["role"], "assistant");
        assert_eq!(
            assistant_msg["reasoning_content"],
            "思考过程：先读取文件，再决定下一步"
        );
    }
}