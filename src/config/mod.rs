use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod rules;
pub mod skills;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessConfig {
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub guardrails: GuardConfig,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub tools: ToolConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub feedback: FeedbackConfig,
    #[serde(default)]
    pub agent: AgentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub rules_file: Option<PathBuf>,
    #[serde(default = "default_approval_timeout")]
    pub approval_timeout_secs: u64,
    #[serde(default = "default_audit_log")]
    pub audit_log_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub workspace_root: Option<PathBuf>,
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    #[serde(default)]
    pub forbidden_commands: Vec<String>,
    #[serde(default = "default_max_timeout")]
    pub max_timeout_secs: u64,
    #[serde(default = "default_true")]
    pub network_allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    #[serde(default)]
    pub disabled_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_memory_path")]
    pub storage_path: PathBuf,
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_retries")]
    pub max_retries: usize,
    #[serde(default = "default_channels")]
    pub channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
    pub token_budget: Option<u32>,
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub rules_files: Vec<PathBuf>,
    pub skills_dir: Option<PathBuf>,
}

// Default value functions

fn default_provider() -> String { "openai".to_string() }
fn default_model() -> String { "gpt-4o".to_string() }
fn default_max_tokens() -> u32 { 4096 }
fn default_temperature() -> f32 { 0.7 }
fn default_timeout() -> u64 { 120 }
fn default_true() -> bool { true }
fn default_approval_timeout() -> u64 { 120 }
fn default_audit_log() -> PathBuf { PathBuf::from(".harness/audit.jsonl") }
fn default_max_timeout() -> u64 { 300 }
fn default_memory_path() -> PathBuf { PathBuf::from(".memory") }
fn default_max_entries() -> usize { 1000 }
fn default_max_retries() -> usize { 3 }
fn default_max_turns() -> usize { 50 }
fn default_channels() -> Vec<String> {
    vec!["test".to_string(), "type_check".to_string(), "lint".to_string()]
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            guardrails: GuardConfig::default(),
            sandbox: SandboxConfig::default(),
            tools: ToolConfig::default(),
            memory: MemoryConfig::default(),
            feedback: FeedbackConfig::default(),
            agent: AgentConfig::default(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            model: default_model(),
            api_key: None,
            base_url: None,
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            timeout_secs: default_timeout(),
        }
    }
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rules_file: None,
            approval_timeout_secs: default_approval_timeout(),
            audit_log_path: default_audit_log(),
        }
    }
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            workspace_root: None,
            allowed_commands: vec![],
            forbidden_commands: vec![],
            max_timeout_secs: default_max_timeout(),
            network_allowed: true,
        }
    }
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self { disabled_tools: vec![] }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            storage_path: default_memory_path(),
            max_entries: default_max_entries(),
        }
    }
}

impl Default for FeedbackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: default_max_retries(),
            channels: default_channels(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: default_max_turns(),
            token_budget: None,
            system_prompt: None,
            rules_files: vec![],
            skills_dir: None,
        }
    }
}

impl HarnessConfig {
    pub fn from_file(path: &PathBuf) -> Result<Self, crate::error::HarnessError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::error::HarnessError::Config(format!("Cannot read config file: {}", e)))?;
        let config: HarnessConfig = toml::from_str(&content)
            .map_err(|e| crate::error::HarnessError::Config(format!("Invalid TOML: {}", e)))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), crate::error::HarnessError> {
        if self.llm.model.is_empty() {
            return Err(crate::error::HarnessError::Config("llm.model must not be empty".to_string()));
        }
        if self.agent.max_turns == 0 {
            return Err(crate::error::HarnessError::Config("agent.max_turns must be > 0".to_string()));
        }
        if self.feedback.max_retries > 10 {
            return Err(crate::error::HarnessError::Config("feedback.max_retries must be <= 10".to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = HarnessConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.agent.max_turns, 50);
        assert_eq!(config.guardrails.approval_timeout_secs, 120);
    }

    #[test]
    fn test_config_validation_rejects_empty_model() {
        let mut config = HarnessConfig::default();
        config.llm.model = "".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_rejects_zero_max_turns() {
        let mut config = HarnessConfig::default();
        config.agent.max_turns = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_load_from_toml() {
        let toml_content = r#"
[llm]
model = "gpt-4o-mini"

[agent]
max_turns = 10
"#;
        let config: HarnessConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.llm.model, "gpt-4o-mini");
        assert_eq!(config.agent.max_turns, 10);
        // Defaults should fill in
        assert_eq!(config.llm.provider, "openai");
        assert!(config.validate().is_ok());
    }
}