use async_trait::async_trait;
use crate::types::{Message, LlmResponse};
use crate::error::HarnessError;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, messages: &[Message]) -> Result<LlmResponse, HarnessError>;
}

pub mod mock;
pub mod openai;