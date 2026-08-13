//! AuV harness agent — 带护栏、工具执行与反馈回路的 AI 编码代理

pub mod config;
pub mod credentials;
pub mod error;
pub mod events;
pub mod feedback;
pub mod guardrails;
pub mod llm;
pub mod r#loop;
pub mod memory;
pub mod observability;
pub mod subagent;
pub mod tools;
pub mod tui;
pub mod types;
