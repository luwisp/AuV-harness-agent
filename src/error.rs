use thiserror::Error;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Tool execution error: {0}")]
    ToolExecution(String),

    #[error("Guardrail blocked: {0}")]
    GuardrailBlocked(String),

    #[error("Guardrail needs approval: {0}")]
    GuardrailNeedsApproval(String),

    #[error("Approval timeout")]
    ApprovalTimeout,

    #[error("Approval denied: {0}")]
    ApprovalDenied(String),

    #[error("Sandbox violation: {0}")]
    SandboxViolation(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Max turns reached")]
    MaxTurnsReached,

    #[error("Token budget exhausted")]
    TokenBudgetExhausted,

    #[error("Subagent limit reached")]
    SubagentLimitReached,

    #[error("Recursion depth exceeded")]
    RecursionDepthExceeded,

    #[error("User interrupted")]
    UserInterrupted,

    #[error("Credential error: {0}")]
    Credential(String),
}

pub type Result<T> = std::result::Result<T, HarnessError>;