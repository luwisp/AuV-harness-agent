use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    /// 审批力度：无/低/中/高（默认「低」）。
    /// 支持中文档位名或英文别名（none/low/medium/high）。
    #[serde(default)]
    pub approval_level: crate::guardrails::ApprovalLevel,
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
            approval_level: crate::guardrails::ApprovalLevel::default(),
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

// ============================================================================
// 分层配置加载（AuV 两级目录）
// ============================================================================

/// 分层配置路径。
#[derive(Debug, Clone, PartialEq)]
pub struct LayeredPaths {
    /// 全局配置：`~/AuV/config.toml`
    pub global: PathBuf,
    /// 项目局部配置：`./AuV/config.toml`（cwd 为 home 目录时为 None）
    pub local: Option<PathBuf>,
    /// 旧版项目根 `./config.toml`（不再加载，仅用于迁移提示；cwd 为 home 时为 None）
    pub legacy: Option<PathBuf>,
}

/// 解析分层配置路径（纯函数，home/cwd 参数注入便于测试）。
pub fn resolve_config_paths(home: &Path, cwd: &Path) -> LayeredPaths {
    let global = home.join("AuV").join("config.toml");
    if cwd == home {
        LayeredPaths {
            global,
            local: None,
            legacy: None,
        }
    } else {
        LayeredPaths {
            global,
            local: Some(cwd.join("AuV").join("config.toml")),
            legacy: Some(cwd.join("config.toml")),
        }
    }
}

/// 递归合并两份 TOML 值：`overlay` 中的键覆盖 `base` 中的对应键。
/// 嵌套表递归合并，其余类型（含数组）整体替换。
pub fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(existing) => merge_toml(existing, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

/// 在指定路径创建默认配置文件（自动创建父目录）。
pub fn write_default_config(path: &Path) -> Result<(), crate::error::HarnessError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::error::HarnessError::Config(format!(
                "无法创建配置目录 {}：{}",
                parent.display(),
                e
            ))
        })?;
    }
    let content = toml::to_string_pretty(&HarnessConfig::default()).map_err(|e| {
        crate::error::HarnessError::Config(format!("序列化默认配置失败：{}", e))
    })?;
    std::fs::write(path, content).map_err(|e| {
        crate::error::HarnessError::Config(format!("无法写入配置文件 {}：{}", path.display(), e))
    })
}

/// 读取配置文件为 `toml::Value`（损坏时返回中文错误）。
fn read_toml_value(path: &Path) -> Result<toml::Value, crate::error::HarnessError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        crate::error::HarnessError::Config(format!("无法读取配置文件 {}：{}", path.display(), e))
    })?;
    toml::from_str(&content).map_err(|e| {
        crate::error::HarnessError::Config(format!("配置文件 {} 格式错误：{}", path.display(), e))
    })
}

/// 角色说明文件的候选名（按优先级）。
pub const PERSONA_FILE_NAMES: [&str; 3] = ["AuV.md", "CLAUDE.md", "AGENTS.md"];

/// 检测角色说明文件：返回 (全局角色文件, 项目角色文件)，取第一个存在的候选。
///
/// 全局候选：`~/AuV/AuV.md` → `~/CLAUDE.md` → `~/AGENTS.md`
/// 项目候选：`./AuV.md` → `./CLAUDE.md` → `./AGENTS.md`
/// 两级都不存在时零打扰（不创建任何文件）。
pub fn resolve_persona_files(home: &Path, cwd: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
        candidates.iter().find(|p| p.is_file()).cloned()
    }
    let global_candidates = [
        home.join("AuV").join("AuV.md"),
        home.join("CLAUDE.md"),
        home.join("AGENTS.md"),
    ];
    let project_candidates = [cwd.join("AuV.md"), cwd.join("CLAUDE.md"), cwd.join("AGENTS.md")];
    (
        first_existing(&global_candidates),
        first_existing(&project_candidates),
    )
}

/// 分层加载结果。
pub struct LayeredLoad {
    pub config: HarnessConfig,
    /// 用户可见的中文提示（创建/迁移提示等）。
    pub notices: Vec<String>,
}

/// 按「全局 → 局部」分层加载配置：
///
/// - 配置文件不存在 → 创建目录并写入默认配置（已存在则绝不改动）
/// - 局部配置字段级覆盖全局配置
/// - 旧版项目根 `config.toml` 不再加载，仅提示迁移
pub fn load_layered(home: &Path, cwd: &Path) -> Result<LayeredLoad, crate::error::HarnessError> {
    let paths = resolve_config_paths(home, cwd);
    let mut notices: Vec<String> = Vec::new();
    let mut merged = toml::Value::Table(toml::map::Map::new());

    // 1. 全局配置
    if paths.global.exists() {
        merge_toml(&mut merged, read_toml_value(&paths.global)?);
    } else {
        match write_default_config(&paths.global) {
            Ok(()) => notices.push(format!("已创建全局配置：{}", paths.global.display())),
            Err(e) => notices.push(format!("{}（本次运行使用默认配置）", e)),
        }
    }

    // 2. 项目局部配置（cwd 为 home 目录时跳过）
    if let Some(local) = &paths.local {
        if local.exists() {
            merge_toml(&mut merged, read_toml_value(local)?);
        } else {
            match write_default_config(local) {
                Ok(()) => notices.push(format!("已创建项目配置：{}", local.display())),
                Err(e) => notices.push(format!("{}（本次运行使用默认配置）", e)),
            }
        }
    }

    // 3. 旧版配置迁移提示
    if let Some(legacy) = &paths.legacy {
        if legacy.exists() {
            notices.push(format!(
                "检测到旧版 {}，已不再加载；请将配置移至 ./AuV/config.toml",
                legacy.display()
            ));
        }
    }

    // 4. 合并结果反序列化并校验
    let config: HarnessConfig = if merged.as_table().is_some_and(|t| t.is_empty()) {
        HarnessConfig::default()
    } else {
        merged
            .try_into()
            .map_err(|e| crate::error::HarnessError::Config(format!("合并配置无效：{}", e)))?
    };
    config.validate()?;
    Ok(LayeredLoad { config, notices })
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

    #[test]
    fn test_approval_level_defaults_to_low() {
        let toml_content = r#"
[llm]
model = "gpt-4o-mini"

[guardrails]
approval_timeout_secs = 5
"#;
        let config: HarnessConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.guardrails.approval_level,
            crate::guardrails::ApprovalLevel::Low,
            "未配置 approval_level 时应默认「低」"
        );
    }

    #[test]
    fn test_approval_level_parses_english_primary_and_chinese_alias() {
        // 英文主值
        let config: HarnessConfig = toml::from_str(
            "[llm]\nmodel = \"m\"\n[guardrails]\napproval_level = \"high\"\n",
        )
        .unwrap();
        assert_eq!(
            config.guardrails.approval_level,
            crate::guardrails::ApprovalLevel::High
        );

        // 中文别名（兼容旧配置）
        let config: HarnessConfig = toml::from_str(
            "[llm]\nmodel = \"m\"\n[guardrails]\napproval_level = \"无\"\n",
        )
        .unwrap();
        assert_eq!(
            config.guardrails.approval_level,
            crate::guardrails::ApprovalLevel::None
        );
    }

    #[test]
    fn test_approval_level_serializes_as_english() {
        let config = HarnessConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        assert!(
            toml_str.contains("approval_level = \"low\""),
            "序列化应输出英文主值 low，实际：{}",
            toml_str
        );
    }

    // ===== 分层配置 =====

    /// 创建独立临时目录（home/cwd 注入，避免测试并行环境竞态）。
    fn tempdir() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("创建临时目录失败")
    }

    #[test]
    fn test_resolve_config_paths_basic() {
        let paths = resolve_config_paths(
            Path::new("/home/user"),
            Path::new("/home/user/projects/demo"),
        );
        assert_eq!(paths.global, PathBuf::from("/home/user/AuV/config.toml"));
        assert_eq!(
            paths.local,
            Some(PathBuf::from("/home/user/projects/demo/AuV/config.toml"))
        );
        assert_eq!(
            paths.legacy,
            Some(PathBuf::from("/home/user/projects/demo/config.toml"))
        );
    }

    #[test]
    fn test_resolve_config_paths_cwd_is_home_skips_local_and_legacy() {
        let paths = resolve_config_paths(Path::new("/home/user"), Path::new("/home/user"));
        assert_eq!(paths.global, PathBuf::from("/home/user/AuV/config.toml"));
        assert!(paths.local.is_none());
        assert!(paths.legacy.is_none());
    }

    #[test]
    fn test_merge_toml_recursive_field_override() {
        let mut base: toml::Value = toml::from_str("[a]\nx = 1\ny = 2\n").unwrap();
        let overlay: toml::Value = toml::from_str("[a]\ny = 3\nz = 4\nb = [1, 2]\n").unwrap();
        merge_toml(&mut base, overlay);
        let expected: toml::Value = toml::from_str("[a]\nx = 1\ny = 3\nz = 4\nb = [1, 2]\n").unwrap();
        assert_eq!(base, expected, "嵌套表递归合并、局部键覆盖、数组整体替换");
    }

    #[test]
    fn test_load_layered_creates_global_and_local_idempotent() {
        let home = tempdir();
        let cwd = tempdir();
        let load = load_layered(home.path(), cwd.path()).unwrap();
        assert_eq!(load.config.llm.model, "gpt-4o");
        assert!(home.path().join("AuV/config.toml").exists(), "应创建全局配置");
        assert!(cwd.path().join("AuV/config.toml").exists(), "应创建局部配置");
        assert_eq!(load.notices.len(), 2, "两个创建提示：{:?}", load.notices);

        // 幂等：二次加载不再创建/改动，也无提示
        let before = std::fs::read_to_string(home.path().join("AuV/config.toml")).unwrap();
        let load2 = load_layered(home.path(), cwd.path()).unwrap();
        assert!(load2.notices.is_empty(), "已存在时不应产生提示：{:?}", load2.notices);
        assert_eq!(
            std::fs::read_to_string(home.path().join("AuV/config.toml")).unwrap(),
            before,
            "已有配置绝不被改动"
        );
    }

    #[test]
    fn test_load_layered_local_overrides_global_field_wise() {
        let home = tempdir();
        let cwd = tempdir();
        // 全局：model + 审批超时
        let gdir = home.path().join("AuV");
        std::fs::create_dir_all(&gdir).unwrap();
        std::fs::write(
            gdir.join("config.toml"),
            "[llm]\nmodel = \"global-model\"\n[guardrails]\napproval_timeout_secs = 60\n",
        )
        .unwrap();
        // 局部：只覆盖 model 与审批力度
        let ldir = cwd.path().join("AuV");
        std::fs::create_dir_all(&ldir).unwrap();
        std::fs::write(
            ldir.join("config.toml"),
            "[llm]\nmodel = \"local-model\"\n[guardrails]\napproval_level = \"high\"\n",
        )
        .unwrap();

        let load = load_layered(home.path(), cwd.path()).unwrap();
        assert_eq!(load.config.llm.model, "local-model", "局部覆盖全局");
        assert_eq!(
            load.config.guardrails.approval_level,
            crate::guardrails::ApprovalLevel::High,
            "局部覆盖审批力度"
        );
        assert_eq!(
            load.config.guardrails.approval_timeout_secs, 60,
            "未写的字段继承全局"
        );
        assert_eq!(load.config.llm.provider, "openai", "两级都没写的字段用默认");
    }

    #[test]
    fn test_load_layered_cwd_is_home_creates_only_global() {
        let home = tempdir();
        let load = load_layered(home.path(), home.path()).unwrap();
        assert!(home.path().join("AuV/config.toml").exists());
        assert_eq!(load.notices.len(), 1, "只应有全局创建提示：{:?}", load.notices);
    }

    #[test]
    fn test_load_layered_legacy_config_not_loaded_with_notice() {
        let home = tempdir();
        let cwd = tempdir();
        std::fs::write(cwd.path().join("config.toml"), "[llm]\nmodel = \"legacy-model\"\n")
            .unwrap();
        let load = load_layered(home.path(), cwd.path()).unwrap();
        assert_eq!(load.config.llm.model, "gpt-4o", "旧版 config.toml 不再加载");
        assert!(
            load.notices.iter().any(|n| n.contains("旧版")),
            "应有迁移提示：{:?}",
            load.notices
        );
    }

    #[test]
    fn test_load_layered_corrupt_global_errors() {
        let home = tempdir();
        let cwd = tempdir();
        let gdir = home.path().join("AuV");
        std::fs::create_dir_all(&gdir).unwrap();
        std::fs::write(gdir.join("config.toml"), "not [valid toml").unwrap();
        assert!(load_layered(home.path(), cwd.path()).is_err(), "损坏配置应报错退出");
    }

    // ===== 角色说明文件检测 =====

    #[test]
    fn test_resolve_persona_files_priority_order() {
        let home = tempdir();
        let cwd = tempdir();
        // 全局：CLAUDE.md 与 AGENTS.md 都在 → 取 CLAUDE.md（优先级更高）
        std::fs::write(home.path().join("CLAUDE.md"), "global persona").unwrap();
        std::fs::write(home.path().join("AGENTS.md"), "ignored").unwrap();
        // 项目：AuV.md 与 CLAUDE.md 都在 → 取 AuV.md
        std::fs::write(cwd.path().join("AuV.md"), "project persona").unwrap();
        std::fs::write(cwd.path().join("CLAUDE.md"), "ignored").unwrap();

        let (global, project) = resolve_persona_files(home.path(), cwd.path());
        assert_eq!(global, Some(home.path().join("CLAUDE.md")));
        assert_eq!(project, Some(cwd.path().join("AuV.md")));
    }

    #[test]
    fn test_resolve_persona_files_none_when_absent() {
        let home = tempdir();
        let cwd = tempdir();
        let (global, project) = resolve_persona_files(home.path(), cwd.path());
        assert!(global.is_none() && project.is_none(), "两级都不存在时不创建文件");
    }
}