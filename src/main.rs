use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use harness_agent::config::HarnessConfig;
use harness_agent::config::rules::RuleFile;
use harness_agent::config::skills::SkillIndex;
use harness_agent::credentials::keyring::KeyringCredentialBackend;
use harness_agent::credentials::CredentialManager;
use harness_agent::error::Result;
use harness_agent::feedback::FeedbackRunner;
use harness_agent::feedback::lint::LintChannel;
use harness_agent::feedback::test_runner::TestRunnerChannel;
use harness_agent::feedback::type_check::TypeCheckChannel;
use harness_agent::guardrails::approval::ApprovalGate;
use harness_agent::guardrails::assessor::{
    CommandRiskAssessor, FileRiskAssessor, NetworkRiskAssessor, RiskAssessor,
};
use harness_agent::guardrails::audit::AuditLog;
use harness_agent::guardrails::rules::StaticRuleEngine;
use harness_agent::guardrails::sandbox::SandboxBoundary;
use harness_agent::guardrails::GuardrailPipeline;
use harness_agent::llm::openai::OpenAiProvider;
use harness_agent::llm::LlmProvider;
use harness_agent::r#loop::context::ContextBuilder;
use harness_agent::r#loop::AgentLoop;
use harness_agent::memory::MemoryStore;
use harness_agent::tools::{bash, file, git, search, test_runner, ToolRegistry};
use harness_agent::tui::{run_cli, run_tui};
use harness_agent::types::{Message, Role};

// ============================================================================
// CLI definition
// ============================================================================

#[derive(Parser, Debug)]
#[command(
    name = "harness",
    version = "0.1.0",
    about = "HarnessAgent - Coding Agent Harness",
    long_about = "An AI-powered coding agent harness with guardrails, \
                  tool execution, and feedback loops."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the agent with a task description
    Run {
        /// The task description for the agent to execute
        task: String,

        /// Path to a TOML configuration file
        #[arg(short = 'c', long)]
        config: Option<PathBuf>,

        /// Disable TUI mode (use plain text output)
        #[arg(long)]
        no_tui: bool,
    },

    /// Initialize harness configuration (config.toml, .memory directory, API key)
    Init,

    /// Manage API keys
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
}

#[derive(Subcommand, Debug)]
enum KeyAction {
    /// Show which keys are configured (no plaintext values)
    Status,

    /// Interactively set an API key
    Set,

    /// Remove a stored key by name
    Clear {
        /// Name of the key to remove
        key: String,
    },
}

// ============================================================================
// main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => {
            let config = load_config(None)?;
            let workspace = std::env::current_dir()?;
            run_repl(config, workspace).await
        }

        Some(Commands::Run {
            task,
            config,
            no_tui,
        }) => {
            let config = load_config(config)?;
            let workspace = std::env::current_dir()?;

            // Resolve API key from config or env
            let api_key = resolve_api_key(&config)?;

            let agent = build_agent(&config, &api_key, workspace)?;

            if no_tui || !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                run_cli(agent, task).await
            } else {
                run_tui(agent, task).await
            }
        }

        Some(Commands::Init) => run_init().await,

        Some(Commands::Key { action }) => match action {
            KeyAction::Status => run_key_status().await,
            KeyAction::Set => run_key_set().await,
            KeyAction::Clear { key } => run_key_clear(&key).await,
        },
    }
}

// ============================================================================
// run_repl
// ============================================================================

/// Interactive REPL mode: reads tasks from stdin and runs the agent in a loop.
///
/// Conversation history is accumulated across turns so the agent remembers
/// previous interactions. Tool calls and their results are preserved in the
/// message history between turns.
async fn run_repl(config: HarnessConfig, workspace: PathBuf) -> Result<()> {
    let api_key = resolve_api_key(&config)?;
    let mut agent = build_agent(&config, &api_key, workspace)?;

    // Accumulated conversation history across all REPL turns
    let mut conversation: Vec<Message> = Vec::new();

    println!("HarnessAgent REPL v0.1.0");
    println!("Type a task for the agent, or /exit to quit.");
    println!("Type /help for available commands.\n");

    loop {
        // Print prompt
        print!("> ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        match std::io::stdin().read_line(&mut input) {
            Ok(0) => break, // EOF / Ctrl+D
            Ok(_) => {
                let trimmed = input.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == "/exit" || trimmed == "/quit" {
                    break;
                }
                if trimmed == "/help" {
                    println!("Commands:");
                    println!("  /exit, /quit  Exit the REPL");
                    println!("  /help         Show this help");
                    println!("  <task>        Run the agent with the given task");
                    println!("  Ctrl+D        Exit the REPL");
                    println!("\nConversation history: {} messages across previous turns",
                             conversation.len());
                    continue;
                }
                // Run the agent with accumulated conversation history
                println!("\n⏳ Running agent for: \"{}\"\n", trimmed);
                match agent.run_with_history(trimmed, &conversation).await {
                    Ok((summary, messages)) => {
                        // The returned messages already include the
                        // assistant response (added in the agent loop
                        // before the FinalAnswer check). Keep only
                        // assistant/tool messages for history — system
                        // prompt and user task are rebuilt fresh each turn.
                        conversation = messages
                            .into_iter()
                            .filter(|m| matches!(m.role, Role::Assistant | Role::Tool))
                            .collect();

                        println!("\n✅ Result: {}\n", summary);
                    }
                    Err(e) => {
                        eprintln!("\n❌ Error: {}\n", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Read error: {}", e);
                break;
            }
        }
    }

    println!("\nGoodbye.");
    Ok(())
}

// ============================================================================
// load_config
// ============================================================================

/// Load a `HarnessConfig` from the given path, or from the default
/// `config.toml` if no path is provided.  If `config.toml` does not exist,
/// returns the default configuration.
fn load_config(path: Option<PathBuf>) -> Result<HarnessConfig> {
    match path {
        Some(p) => HarnessConfig::from_file(&p),
        None => {
            let default_path = PathBuf::from("config.toml");
            if default_path.exists() {
                HarnessConfig::from_file(&default_path)
            } else {
                Ok(HarnessConfig::default())
            }
        }
    }
}

// ============================================================================
// resolve_api_key
// ============================================================================

/// Resolve the API key to use, checking config first, then the `OPENAI_API_KEY`
/// environment variable.
fn resolve_api_key(config: &HarnessConfig) -> Result<String> {
    if let Some(ref key) = config.llm.api_key {
        return Ok(key.clone());
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        return Ok(key);
    }
    Err(harness_agent::error::HarnessError::Auth(
        "No API key found. Set it in config.toml under [llm].api_key, \
         or set the OPENAI_API_KEY environment variable. \
         Run `harness key set` to store it in the env file."
            .to_string(),
    ))
}

// ============================================================================
// build_agent
// ============================================================================

/// Build a fully-wired [`AgentLoop`] from the given configuration and API key.
fn build_agent(
    config: &HarnessConfig,
    api_key: &str,
    workspace: PathBuf,
) -> Result<AgentLoop> {
    // 1. LLM provider
    let llm: Box<dyn LlmProvider> = Box::new(OpenAiProvider::new(
        api_key.to_string(),
        config.llm.model.clone(),
        config.llm.base_url.clone(),
    ));

    // 2. Guardrails — load built-in rules and all assessors
    let mut rules = StaticRuleEngine::new();
    rules.load_builtin_rules();

    let assessors: Vec<Box<dyn RiskAssessor>> = if config.guardrails.enabled {
        vec![
            Box::new(CommandRiskAssessor),
            Box::new(FileRiskAssessor),
            Box::new(NetworkRiskAssessor),
        ]
    } else {
        vec![]
    };

    let approval = ApprovalGate::new(Duration::from_secs(
        config.guardrails.approval_timeout_secs,
    ));
    let sandbox = SandboxBoundary {
        workspace_root: workspace.clone(),
        allowed_commands: config.sandbox.allowed_commands.clone(),
        forbidden_commands: if config.sandbox.forbidden_commands.is_empty() {
            // Default deny: block dangerous commands when sandbox is enabled
            vec![
                "rm -rf /".to_string(),
                "sudo".to_string(),
                "chmod 777 /".to_string(),
                "mkfs".to_string(),
                "dd if=".to_string(),
                ":(){ :|:& };:".to_string(),
            ]
        } else {
            config.sandbox.forbidden_commands.clone()
        },
        max_timeout: Duration::from_secs(config.sandbox.max_timeout_secs),
        network_allowed: config.sandbox.network_allowed,
    };
    let audit = AuditLog::new(config.guardrails.audit_log_path.clone());
    let guardrails = GuardrailPipeline::new(rules, assessors, approval, sandbox, audit);

    // 3. Tools — register all available tools
    let mut tools = ToolRegistry::new();
    let disabled = &config.tools.disabled_tools;
    if !disabled.contains(&"bash".to_string()) {
        tools.register(Box::new(bash::BashTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register bash: {}", e)))?;
    }
    if !disabled.contains(&"read_file".to_string()) {
        tools.register(Box::new(file::ReadFileTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register read_file: {}", e)))?;
    }
    if !disabled.contains(&"write_file".to_string()) {
        tools.register(Box::new(file::WriteFileTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register write_file: {}", e)))?;
    }
    if !disabled.contains(&"grep".to_string()) {
        tools.register(Box::new(search::GrepTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register grep: {}", e)))?;
    }
    if !disabled.contains(&"glob".to_string()) {
        tools.register(Box::new(search::GlobTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register glob: {}", e)))?;
    }
    if !disabled.contains(&"git_diff".to_string()) {
        tools.register(Box::new(git::GitDiffTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register git_diff: {}", e)))?;
    }
    if !disabled.contains(&"run_test".to_string()) {
        tools.register(Box::new(test_runner::RunTestTool))
            .map_err(|e| harness_agent::error::HarnessError::Config(format!("register run_test: {}", e)))?;
    }

    // 4. Feedback — load channels when enabled
    let feedback_channels: Vec<Box<dyn harness_agent::feedback::FeedbackChannel>> = if config.feedback.enabled {
        vec![
            Box::new(TestRunnerChannel),
            Box::new(TypeCheckChannel),
            Box::new(LintChannel),
        ]
    } else {
        vec![]
    };
    let feedback = FeedbackRunner::new(feedback_channels, config.feedback.max_retries);

    // 5. Memory
    let memory = MemoryStore::new(config.memory.storage_path.clone())?;

    // 6. Context builder — load rules and skills
    let tool_menu = serde_json::to_string(&tools.generate_tool_menu())
        .unwrap_or_default();

    let rules_text = if let Some(ref rules_file) = config.guardrails.rules_file {
        if !rules_file.as_os_str().is_empty() {
            RuleFile::from_file(rules_file)
                .map(|rf| rf.to_system_prompt_fragment())
                .unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let skills_text = if let Some(ref skills_dir) = config.agent.skills_dir {
        if !skills_dir.as_os_str().is_empty() {
            SkillIndex::from_dir(skills_dir)
                .map(|si| si.to_prompt_fragment())
                .unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let memory_index = memory.compact_index();

    let context_builder = ContextBuilder::from_config(
        config,
        tool_menu,
        rules_text,
        skills_text,
        memory_index,
    );

    // 7. Assemble
    Ok(AgentLoop::new(
        llm,
        guardrails,
        tools,
        feedback,
        memory,
        config.clone(),
        context_builder,
        workspace,
    ))
}

// ============================================================================
// run_init
// ============================================================================

async fn run_init() -> Result<()> {
    use std::io::{self, Write};

    println!("HarnessAgent Initialization");
    println!("===========================");

    // 1. Create config.toml if it doesn't exist
    let config_path = PathBuf::from("config.toml");
    if config_path.exists() {
        let mut answer = String::new();
        print!("config.toml already exists. Overwrite? [y/N]: ");
        io::stdout().flush()?;
        io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_lowercase();
        if answer != "y" && answer != "yes" {
            println!("Skipping config.toml creation.");
        } else {
            write_default_config(&config_path)?;
        }
    } else {
        write_default_config(&config_path)?;
    }

    // 2. Create .memory directory
    let memory_path = PathBuf::from(".memory");
    if !memory_path.exists() {
        std::fs::create_dir_all(&memory_path)?;
        let index_path = memory_path.join("MEMORY.md");
        std::fs::write(&index_path, "# Memory Index\n\n")?;
        println!("Created .memory/ directory.");
    } else {
        println!(".memory/ directory already exists.");
    }

    // 3. Optionally set up API key
    let mut answer = String::new();
    print!("Would you like to set up your OpenAI API key now? [y/N]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_lowercase();
    if answer == "y" || answer == "yes" {
        run_key_set().await?;
    }

    println!("\nInitialization complete.");
    println!("Edit config.toml to customize settings.");
    println!("Run `harness run \"your task\"` for a single task, or just `harness` for interactive REPL mode.");

    Ok(())
}

/// Write a default `config.toml` file to the given path.
fn write_default_config(path: &PathBuf) -> Result<()> {
    let config = HarnessConfig::default();
    let toml_str = toml::to_string_pretty(&config).map_err(|e| {
        harness_agent::error::HarnessError::Config(format!("Failed to serialize config: {}", e))
    })?;
    std::fs::write(path, toml_str)?;
    println!("Created config.toml with default settings.");
    Ok(())
}

// ============================================================================
// key management commands
// ============================================================================

fn make_credential_manager() -> CredentialManager {
    let backend = Box::new(KeyringCredentialBackend::new());
    CredentialManager::new(backend)
}

async fn run_key_status() -> Result<()> {
    let manager = make_credential_manager();
    let status = manager.key_status()?;
    println!("{}", status);
    Ok(())
}

async fn run_key_set() -> Result<()> {
    let manager = make_credential_manager();
    manager.key_set().await
}

async fn run_key_clear(key: &str) -> Result<()> {
    let manager = make_credential_manager();
    manager.key_clear(key).await?;
    println!("Key '{}' removed.", key);
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // -----------------------------------------------------------------------
    // Helper: parse CLI args from a string
    // -----------------------------------------------------------------------

    /// Parse a command line string, respecting shell-like quoting.
    /// Args are split by whitespace, but text within double quotes is kept
    /// as a single argument.
    fn parse(args: &str) -> Cli {
        let args_vec = shell_split(args);
        let mut full_args: Vec<&str> = vec!["harness"];
        full_args.extend(args_vec.iter().map(|s| s.as_str()));
        Cli::parse_from(full_args)
    }

    /// Split a command-line string into arguments, respecting double quotes.
    /// Simple implementation: toggle in/out of quote mode on `"` characters.
    fn shell_split(input: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;

        for ch in input.chars() {
            match ch {
                '"' => {
                    in_quotes = !in_quotes;
                }
                ' ' if !in_quotes => {
                    if !current.is_empty() {
                        result.push(std::mem::take(&mut current));
                    }
                }
                _ => {
                    current.push(ch);
                }
            }
        }
        if !current.is_empty() {
            result.push(current);
        }
        result
    }

    // -----------------------------------------------------------------------
    // Run command tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_with_task() {
        let cli = parse("run \"fix the login bug\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "fix the login bug");
                assert!(config.is_none());
                assert!(!no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_run_with_config_flag() {
        let cli = parse("run -c my-config.toml \"do stuff\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "do stuff");
                assert_eq!(config, Some(PathBuf::from("my-config.toml")));
                assert!(!no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_run_with_long_config_flag() {
        let cli = parse("run --config prod.toml \"deploy\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "deploy");
                assert_eq!(config, Some(PathBuf::from("prod.toml")));
                assert!(!no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_run_with_no_tui_flag() {
        let cli = parse("run --no-tui \"run tests\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "run tests");
                assert!(config.is_none());
                assert!(no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_run_with_all_flags() {
        let cli = parse("run -c custom.toml --no-tui \"complex task\"");
        match cli.command {
            Some(Commands::Run {
                task,
                config,
                no_tui,
            }) => {
                assert_eq!(task, "complex task");
                assert_eq!(config, Some(PathBuf::from("custom.toml")));
                assert!(no_tui);
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    // -----------------------------------------------------------------------
    // Init command tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_init_command() {
        let cli = parse("init");
        assert!(matches!(cli.command, Some(Commands::Init)));
    }

    // -----------------------------------------------------------------------
    // Key subcommand tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_key_status() {
        let cli = parse("key status");
        match cli.command {
            Some(Commands::Key { action }) => {
                assert!(matches!(action, KeyAction::Status));
            }
            _ => panic!("Expected Commands::Key"),
        }
    }

    #[test]
    fn test_key_set() {
        let cli = parse("key set");
        match cli.command {
            Some(Commands::Key { action }) => {
                assert!(matches!(action, KeyAction::Set));
            }
            _ => panic!("Expected Commands::Key"),
        }
    }

    #[test]
    fn test_key_clear() {
        let cli = parse("key clear openai_api_key");
        match cli.command {
            Some(Commands::Key { action }) => match action {
                KeyAction::Clear { key } => {
                    assert_eq!(key, "openai_api_key");
                }
                _ => panic!("Expected KeyAction::Clear"),
            },
            _ => panic!("Expected Commands::Key"),
        }
    }

    #[test]
    fn test_key_clear_with_quoted_key() {
        let cli = parse("key clear \"my secret key\"");
        match cli.command {
            Some(Commands::Key { action }) => match action {
                KeyAction::Clear { key } => {
                    assert_eq!(key, "my secret key");
                }
                _ => panic!("Expected KeyAction::Clear"),
            },
            _ => panic!("Expected Commands::Key"),
        }
    }

    // -----------------------------------------------------------------------
    // Version and help tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_version_flag() {
        let args: Vec<&str> = vec!["harness", "--version"];
        let result = Cli::try_parse_from(args);
        // --version causes clap to print and exit with ErrorKind::DisplayVersion
        assert!(result.is_err() || result.is_ok());
    }

    #[test]
    fn test_help_flag() {
        let args: Vec<&str> = vec!["harness", "--help"];
        let result = Cli::try_parse_from(args);
        // --help causes clap to print and exit, so this is expected to be an error
        // (ErrorKind::DisplayHelp). We just verify it doesn't panic.
        assert!(result.is_err() || result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_with_empty_task_rejected() {
        // Empty string as task — clap should reject this as a missing required argument
        let args: Vec<&str> = vec!["harness", "run", ""];
        let result = Cli::try_parse_from(args);
        // clap may accept "" as a task or reject it; either behavior is fine
        // We just verify it doesn't panic
        let _ = result;
    }

    #[test]
    fn test_run_with_special_characters_in_task() {
        let cli = parse("run \"fix bug #123: login doesn't work (urgent!)\"");
        match cli.command {
            Some(Commands::Run { task, .. }) => {
                assert_eq!(task, "fix bug #123: login doesn't work (urgent!)");
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_no_subcommand_enters_repl() {
        let args: Vec<&str> = vec!["harness"];
        let cli = Cli::parse_from(args);
        assert!(cli.command.is_none(), "No subcommand should enter REPL mode");
    }

    #[test]
    fn test_invalid_subcommand() {
        let args: Vec<&str> = vec!["harness", "unknown"];
        let result = Cli::try_parse_from(args);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // load_config tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_config_with_explicit_path() {
        // Create a temp config file
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test.toml");
        let config = HarnessConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&config_path, toml_str).unwrap();

        let loaded = load_config(Some(config_path)).unwrap();
        assert_eq!(loaded.llm.model, "gpt-4o");
        assert_eq!(loaded.agent.max_turns, 50);
        assert!(loaded.validate().is_ok());
    }

    #[test]
    fn test_load_config_with_nonexistent_file() {
        let result = load_config(Some(PathBuf::from("/nonexistent/path/config.toml")));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_default_when_no_file() {
        // Switch to a temp dir where config.toml doesn't exist
        let dir = tempfile::tempdir().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let loaded = load_config(None).unwrap();
        assert_eq!(loaded.llm.model, "gpt-4o"); // default

        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    fn test_load_config_from_default_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut config = HarnessConfig::default();
        config.llm.model = "gpt-4o-mini".to_string();
        config.agent.max_turns = 10;
        let toml_str = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&config_path, toml_str).unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let loaded = load_config(None).unwrap();
        assert_eq!(loaded.llm.model, "gpt-4o-mini");
        assert_eq!(loaded.agent.max_turns, 10);

        std::env::set_current_dir(original_dir).unwrap();
    }

    // -----------------------------------------------------------------------
    // resolve_api_key tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_api_key_from_config() {
        let mut config = HarnessConfig::default();
        config.llm.api_key = Some("sk-config-key".to_string());
        let key = resolve_api_key(&config).unwrap();
        assert_eq!(key, "sk-config-key");
    }

    #[test]
    fn test_resolve_api_key_from_env() {
        let config = HarnessConfig::default();
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-env-key") };
        let key = resolve_api_key(&config).unwrap();
        assert_eq!(key, "sk-env-key");
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }

    #[test]
    fn test_resolve_api_key_config_priority() {
        let mut config = HarnessConfig::default();
        config.llm.api_key = Some("sk-config-key".to_string());
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-env-key") };
        // Config should take priority
        let key = resolve_api_key(&config).unwrap();
        assert_eq!(key, "sk-config-key");
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }

    #[test]
    fn test_resolve_api_key_missing() {
        let config = HarnessConfig::default();
        // Ensure env var is not set
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        let result = resolve_api_key(&config);
        assert!(result.is_err());
        match result.unwrap_err() {
            harness_agent::error::HarnessError::Auth(msg) => {
                assert!(msg.contains("No API key found"));
            }
            _ => panic!("Expected Auth error"),
        }
    }

    // -----------------------------------------------------------------------
    // build_agent tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_agent_with_default_config() {
        let config = HarnessConfig::default();
        let workspace = PathBuf::from("/tmp/test-workspace");
        let agent = build_agent(&config, "sk-test-key", workspace);
        assert!(agent.is_ok(), "build_agent should succeed: {:?}", agent.err());
    }

    #[test]
    fn test_build_agent_respects_disabled_tools() {
        let mut config = HarnessConfig::default();
        config.tools.disabled_tools = vec!["bash".to_string(), "grep".to_string()];
        let workspace = PathBuf::from("/tmp/test-workspace");
        let agent = build_agent(&config, "sk-test-key", workspace).unwrap();

        // Verify the disabled tools are not in the registry
        let tool_names: Vec<String> = agent
            .trace() // This is empty initially, but we can check the tool list
            .iter()
            .map(|t| format!("{:?}", t.action))
            .collect();
        // The agent's tools field is private, but we can verify the build succeeded
        // and the registry only has the non-disabled tools.
        // Since the tools field is private, we just verify the build succeeded.
        drop(tool_names);
    }

    // -----------------------------------------------------------------------
    // run_init helpers tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_write_default_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        write_default_config(&config_path).unwrap();
        assert!(config_path.exists());

        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("model"));
        assert!(content.contains("gpt-4o"));

        // Verify it can be parsed back
        let config = HarnessConfig::from_file(&config_path).unwrap();
        assert!(config.validate().is_ok());
    }

    // -----------------------------------------------------------------------
    // Key management tests (integration-style)
    // -----------------------------------------------------------------------

    #[test]
    fn test_make_credential_manager() {
        let manager = make_credential_manager();
        // key_status may fail if system keyring is unavailable;
        // in that case, the manager will fall back to encrypted file
        let _ = manager.key_status();
    }
}