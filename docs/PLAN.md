# AuV harness agent 实施计划

> 项目：AI4SE 期末项目 · A · Coding Agent Harness
> 计划制定：2025-07-08 ｜ 最后更新：2026-08-14（完成状态与修订记录）
> 对应设计文档：[SPEC.md](SPEC.md) ｜ 过程记录：[SPEC_PROCESS.md](SPEC_PROCESS.md) ｜ Agent 日志：[AGENT_LOG.md](AGENT_LOG.md)

---

## 1. 目标与总体架构

**目标**：用 Rust 构建一个完整的 Coding Agent Harness——将 LLM 作为"CPU"封装成能稳定执行软件工程任务的完整系统，包含工具、护栏、反馈闭环、记忆与交互界面，全部机制可用 mock LLM 做确定性测试。

**架构决策**：全链路 trait 依赖注入——每个组件（LLM、工具、护栏、反馈、记忆）都在 trait 之后，用 mock 实现做确定性单元测试；Agent 主循环编排所有组件。**护栏管线是重点维度**，采用四层架构（静态规则 → 风险评估 → 沙箱边界 → 审批状态机；沙箱先于审批的最终顺序见 §7 修订记录）。

**技术栈**：Rust (edition 2024)、tokio、reqwest、clap、ratatui、crossterm、rustyline、serde、tracing、keyring（已接入，失败时加密文件回退）。

## 2. 全局约束

- Rust edition：2024；异步运行时：tokio（多线程）
- **所有机制必须能用 mock LLM 测试**——任何机制不得依赖真实 LLM 调用
- TDD：每个任务先红后绿再重构
- 真实 API key 不得出现在源码、git 历史或日志中（`config.toml`、`.AuV/` 已加入 `.gitignore`）
- 二进制名：`auv`（原计划 `harness`，2026-08-13 更名，见 §7）
- LLM 供应商：OpenAI 兼容 API 为主（DeepSeek/Groq/Ollama），Anthropic 预留
- TUI：基于 ratatui，无法启动时优雅降级为纯文本 CLI

## 3. 阶段与任务总览

| 阶段 | 任务 | 说明 | 预计耗时 | 状态 |
|------|------|------|---------|------|
| 1 | 1-3 | 项目脚手架 + 核心类型 | 30 分钟 | ✓ |
| 2 | 4-5 | LLM 抽象层 | 30 分钟 | ✓ |
| 3 | 6 | 配置系统 | 20 分钟 | ✓ |
| 4 | 7-11 | 工具系统 | 45 分钟 | ✓ |
| 5 | 12-18 | **护栏（重点维度）** | 90 分钟 | ✓ |
| 6 | 19-20 | 反馈闭环 | 30 分钟 | ✓ |
| 7 | 21 | 记忆系统 | 20 分钟 | ✓ |
| 8 | 22 | Agent 主循环 | 30 分钟 | ✓ |
| 9 | 23-24 | 可观测性 + 子 Agent | 20 分钟 | ✓ |
| 10 | 25 | 凭据管理 | 20 分钟 | ✓ |
| 11 | 26-28 | TUI | 45 分钟 | ✓ |
| 12 | 29 | CLI 入口 | 15 分钟 | ✓ |
| 13 | 30-31 | Docker + CI | 15 分钟 | ✓ |
| 14 | 32 | 机制演示 | 20 分钟 | ✓ |
| 15 | — | REPL 交互层（计划外新增，见 §5） | — | ✓ |
| **合计** | **32 个任务 + REPL 扩展** | | **约 7 小时（不含 REPL）** | |

---

## 4. 阶段任务明细

### 阶段 1：项目脚手架与核心类型（任务 1-3）

#### 任务 1：初始化 Cargo 项目与依赖

**文件**：`Cargo.toml`（修改）、`src/lib.rs`、`src/main.rs`（新建）

**产出**：`harness_agent` library crate、`auv` binary crate

**关键实现**：

```toml
[package]
name = "harnessAgent"
version = "0.1.0"
edition = "2024"

[lib]
name = "harness_agent"
path = "src/lib.rs"

[[bin]]
name = "auv"            # 原计划为 harness，2026-08-13 更名
path = "src/main.rs"
```

依赖包括 tokio、reqwest、serde、clap、ratatui、crossterm、rustyline、tracing、chrono、keyring、rpassword、async-trait、thiserror、regex、glob、uuid、toml、dirs 等；dev-dependencies 包括 tempfile、tokio-test、wiremock。

**测试计划**：`cargo build` 编译通过。

**完成状态**：✓ 已完成 — commit `3b665ea`（feat: add project scaffolding, core types, and module structure）

---

#### 任务 2：定义核心数据类型

**文件**：`src/types.rs`（修改）

**产出**：`Role`、`Message`、`ToolCall`、`Action`、`ToolResult`、`Artifact`、`LlmResponse`、`FinishReason`、`TokenUsage`、`GuardResult`、`GuardDecision`、`ToolInfo`、`FeedbackResult`、`FeedbackError`

**关键实现**（关键类型，完整定义见 SPEC.md §6）：

```rust
pub enum Role { System, User, Assistant, Tool }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Action {
    #[serde(rename = "tool_call")] ToolCall { id: String, name: String, params: serde_json::Value },
    #[serde(rename = "final_answer")] FinalAnswer { summary: String },
    #[serde(rename = "ask_user")] AskUser { question: String },
    #[serde(rename = "noop")] NoOp,
}

pub enum GuardResult {
    Allowed,
    Denied { reason: String, decision: GuardDecision },
    NeedsApproval { risk_level: String, reasons: Vec<String> },
}
```

**测试计划**：`Action` 全变体 JSON 往返、`GuardResult` 判定方法、`Message` 全字段构造。

**完成状态**：✓ 已完成 — commit `3b665ea`。实现期扩展：`Message` 与 `LlmResponse` 增加 `reasoning_content` 字段（DeepSeek 思考模式回传，commit `efa9f4d`）。

---

#### 任务 3：定义错误类型

**文件**：`src/error.rs`（修改）

**产出**：`HarnessError` 枚举、`Result<T>` 类型别名

**关键实现**：

```rust
#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("LLM error: {0}")] Llm(String),
    #[error("Network error: {0}")] Network(#[from] reqwest::Error),
    #[error("Auth error: {0}")] Auth(String),
    #[error("Tool not found: {0}")] ToolNotFound(String),
    #[error("Tool execution error: {0}")] ToolExecution(String),
    #[error("Guardrail blocked: {0}")] GuardrailBlocked(String),
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
```

**测试计划**：编译通过 + 既有测试不回归。

**完成状态**：✓ 已完成 — commit `3939700`（feat: define error types with HarnessError enum）

---

### 阶段 2：LLM 抽象层（任务 4-5）

#### 任务 4：LlmProvider trait 与 MockLlmProvider

**文件**：`src/llm/mod.rs`（修改）、`src/llm/mock.rs`（新建）

**产出**：`LlmProvider` trait、`MockLlmProvider`

**关键实现**：

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, messages: &[Message]) -> Result<LlmResponse, HarnessError>;
}
```

`MockLlmProvider` 持有预设响应序列（`Mutex<Vec<LlmResponse>>`），按调用顺序返回；序列耗尽后返回默认 "Done" 响应。**这是全部机制离线测试的基石**——移除真实 LLM 后整个系统仍可单测。

**测试计划**：预设响应按序返回、耗尽回退默认、tool_calls 响应透传。

**完成状态**：✓ 已完成 — commit `334300b`（feat: add LlmProvider trait and MockLlmProvider）

---

#### 任务 5：OpenAI provider

**文件**：`src/llm/openai.rs`（修改）

**产出**：`OpenAiProvider`

**关键实现**：构造函数 `new(api_key, model, base_url)`；`build_request()` 将消息列表转为 Chat Completions JSON；`complete()` 发送 POST 并解析；401 → `HarnessError::Auth`、429 → 限流错误；`finish_reason` 字符串映射为枚举。

**测试计划**（wiremock）：文本响应解析、tool_calls 响应解析、401 认证错误映射。

**完成状态**：✓ 已完成 — commit `df8818e`（feat: add OpenAI provider implementation with wiremock tests）。实现期扩展：DeepSeek 思考模式 `reasoning_content` 原样回传（commit `efa9f4d`，修复 HTTP 400）。

---

### 阶段 3：配置系统（任务 6）

#### 任务 6：配置加载、规则与技能

**文件**：`src/config/mod.rs`（修改）、`src/config/rules.rs`、`src/config/skills.rs`（新建）

**产出**：`HarnessConfig`、`RuleFile`、`SkillIndex`

**关键实现**：`HarnessConfig` 含 `LlmConfig`/`GuardConfig`/`SandboxConfig`/`ToolConfig`/`MemoryConfig`/`FeedbackConfig`/`AgentConfig` 子配置；`Default` 实现合理默认值（max_turns: 50、审批超时 120s）；`from_file()` 读取 TOML 并验证（模型非空、max_turns > 0）。`RuleFile` 纯文本规则（一行一条，跳过空行与 `#` 注释），格式化为 system prompt 片段。`SkillIndex` 扫描技能目录 `.md` 文件，提取 frontmatter `description` 生成"名片"提示片段。

**测试计划**：默认配置有效、空模型拒绝、零 max_turns 拒绝、TOML 加载往返。

**完成状态**：✓ 已完成 — commit `41808f6`（feat: add configuration system with rules and skills）。实现期扩展：两级配置系统（`~/.AuV/config.toml` 全局 + `./.AuV/config.toml` 项目，字段级递归合并，启动幂等创建）与 AuV.md 角色说明两级检测，commit `a30bba0`；配置目录更正为隐藏目录 `.AuV`，commit `29e5270`。收尾回归将自动创建策略明确为「全局写完整默认值、项目写稀疏覆盖模板」，避免第二次启动时项目默认值遮蔽全局模型/API 地址。

---

### 阶段 4：工具系统（任务 7-11）

#### 任务 7：Tool trait、ToolContext 与 ToolRegistry

**文件**：`src/tools/mod.rs`（修改）、`src/tools/context.rs`（新建）

**产出**：`Tool` trait、`ToolContext`、`ToolRegistry`

**关键实现**：

```rust
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub workspace_root: std::path::PathBuf,
    pub command_timeout: std::time::Duration,
    pub network_allowed: bool,
}
```

`Tool` trait 定义 `name()`/`description()`/`parameters()`/`execute()`；`ToolRegistry` 提供 `register()`/`get()`/`list_tools()`/`generate_tool_menu()`/`execute()`。

**测试计划**：注册与查找、工具列表、执行分发。

**完成状态**：✓ 已完成 — commit `1966818`（feat: add Tool trait, ToolContext, and ToolRegistry）

---

#### 任务 8：文件工具（read_file、write_file）

**文件**：`src/tools/file.rs`（新建）

**产出**：`ReadFileTool`、`WriteFileTool`

**关键实现**：`read_file` 支持行号范围（offset/limit），返回带行号内容；`write_file` 自动创建父目录，返回字节数。路径均基于 `workspace_root` 解析，受沙箱路径限制。

**测试计划**：基础读取、行范围、不存在文件报错；基础写入、父目录创建。

**完成状态**：✓ 已完成 — commit `64d55be`（feat: add read_file and write_file tools）

---

#### 任务 9：Bash 工具

**文件**：`src/tools/bash.rs`（新建）

**产出**：`BashTool`

**关键实现**：`execute()` 在 workspace_root 中运行 `sh -c "<command>"`，合并 stdout+stderr，支持超时（`timeout_secs` 参数）。

**测试计划**：echo 回显、命令失败、超时（`sleep 10` + 1s 超时）。

**完成状态**：✓ 已完成 — commit `d336272`（feat: add bash tool）

---

#### 任务 10：搜索工具（grep、glob）

**文件**：`src/tools/search.rs`（新建）

**产出**：`GrepTool`、`GlobTool`

**关键实现**：`grep` 基于 `regex` crate 遍历工作区搜索，返回 `file:line` 前缀的匹配行；`glob` 基于 `glob` crate 列出匹配文件。

**测试计划**：命中/未命中；文件列表/空结果。

**完成状态**：✓ 已完成 — commit `e0f2f1a`（feat: add grep and glob search tools）

---

#### 任务 11：Git 与测试运行工具

**文件**：`src/tools/git.rs`、`src/tools/test_runner.rs`（新建）

**产出**：`GitDiffTool`、`RunTestTool`

**关键实现**：`git_diff` 运行 `git diff`（可选 `--cached`）；`run_test` 运行测试命令（默认 `cargo test`），按退出码判定成功。

**测试计划**：临时 git 仓库 diff、测试命令成功/失败路径。

**完成状态**：✓ 已完成 — commit `81a395b`（feat: add git_diff and run_test tools）

---

### 阶段 5：护栏——重点维度（任务 12-18）

#### 任务 12：静态规则引擎（第一层）

**文件**：`src/guardrails/mod.rs`（修改）、`src/guardrails/rules.rs`（新建）

**产出**：`StaticRuleEngine`、`GuardRule`、`RulePattern`、`RuleAction`

**关键实现**：

```rust
pub enum RulePattern {
    CommandGlob { globs: Vec<String> },
    FilePath { paths: Vec<String>, op: FileOp },
    NetworkDest { hosts: Vec<String> },
    Composite { all: Vec<RulePattern>, any: Vec<RulePattern> },
}

pub enum RuleAction { Allow, Deny(String), Escalate }
```

内置危险规则：`rm -rf /*` → Deny、`rm -rf ~` → Escalate、`DROP TABLE` → Escalate、`DROP DATABASE` → Deny、`curl ... | bash` → Escalate、`git push --force` → Escalate、`chmod 777` → Escalate、`dd if=` → Deny、`mkfs.*` → Deny、写 `/etc/*` → Escalate、写 `~/.ssh/*` → Escalate、写 `.env` → Escalate。规则按优先级排序，首个命中生效。**L1 Deny 始终硬拦截，不受审批力度影响**（实现期约束，commit `dc6052a`）。

**测试计划**：`rm -rf /` 拦截、`DROP TABLE` 升级、正常命令放行、`/etc` 写入升级、工作区内写入放行、优先级顺序。

**完成状态**：✓ 已完成 — commit `368f72f`（feat: add static rule engine with built-in dangerous rules）；规则名称与拦截原因中文化（commit `dc6052a`）

---

#### 任务 13：风险评估器（第二层）

**文件**：`src/guardrails/assessor.rs`（新建）

**产出**：`RiskAssessor` trait、`CommandRiskAssessor`、`FileRiskAssessor`、`NetworkRiskAssessor`、`RiskAssessment`、`RiskLevel`

**关键实现**：

```rust
pub enum RiskLevel { Low, Medium, High, Critical }

pub struct RiskAssessment {
    pub level: RiskLevel,
    pub reasons: Vec<String>,
    pub suggested_mitigation: Option<String>,
}
```

三类评估器：命令评估器（`sudo` → 高、管道/重定向/链式操作符/`curl`/`wget` → 中、多因子叠加 → 高）；文件评估器（工作区外路径 → 高、系统目录 → 严重、隐藏文件 → 低）；网络评估器（外发 HTTP → 中、数据外流模式如 `curl POST`/`scp` → 高）。多评估器结果按"取最高等级 + 合并原因"归并。

**测试计划**：`sudo` 高/`echo` 低、工作区外高/内低、`curl` 中、归并取最高。

**完成状态**：✓ 已完成 — commit `52493ab`（feat: add risk assessment layer with three assessors）；评估原因与缓解措施中文化（commit `dc6052a`）

---

#### 任务 14：审批状态机（原计划第三层，最终管线中为第四层）

**文件**：`src/guardrails/approval.rs`（新建）

**产出**：`ApprovalGate`、`ApprovalDecision`、`ApprovalRequest`

**关键实现**：

```rust
pub struct ApprovalGate {
    timeout: Duration,
    session_whitelist: HashSet<String>,
}

pub enum ApprovalDecision {
    Approved { by: String, reason: Option<String> },
    Denied { reason: String },
    Timeout,
}
```

动作指纹（工具名 + 参数哈希）用于会话级白名单——同一会话内已批准的操作自动放行。支持 Approve / Deny / Timeout（默认 120s 超时自动拒绝）。

**测试计划**：白名单自动批准、超时判定、不同动作不受白名单影响。

**完成状态**：✓ 已完成 — 基础实现落地于 commit `8f2e55b`；后续大幅强化（commit `dc6052a` / `efa9f4d` / `04684ff`）：
- UI 事件模式（`with_ui_events`）：审批请求经 `GuardrailApprovalNeeded` 事件发 UI，决定经通道发回——打通 TUI 与 REPL 的 y/n 审批
- 可取消轮询读取（`libc::poll` 探测 + 50ms 轮询 + `libc::read`），根治 `spawn_blocking` 阻塞线程泄漏
- 审批期间 Ctrl+C 视为拒绝（`tokio::select!` 三分支竞争），不再杀死进程
- 行结束同时接受 `\n` 与 `\r`（raw 环境容错）
- 审批力度四档（`ApprovalLevel`：无/低/中/高，见 commit `dc6052a`）

---

#### 任务 15：沙箱边界（原计划第四层，最终管线中为第三层）

**文件**：`src/guardrails/sandbox.rs`（新建）

**产出**：`SandboxBoundary`、`SandboxViolation`

**关键实现**：

```rust
pub struct SandboxBoundary {
    pub workspace_root: PathBuf,
    pub allowed_commands: Vec<String>,
    pub forbidden_commands: Vec<String>,
    pub max_timeout: Duration,
    pub network_allowed: bool,
}
```

校验内容：文件路径必须在 workspace_root 内、命令黑白名单、超时上限、网络开关；`wrap_command()` 为命令加超时包装。

**测试计划**：工作区外路径拒绝、区内放行、禁用命令拒绝、网络关闭时 curl 拒绝、命令包装。

**完成状态**：✓ 已完成 — 落地于 commit `8f2e55b`。管线顺序修复（commit `8c102ec`）：**沙箱硬校验先于审批**——参数级违规在审批前直接 Blocked，修复"用户批准后仍被沙箱拦截"的假批准。沙箱违规消息中文化（commit `dc6052a`）。

---

#### 任务 16：GuardrailPipeline 编排

**文件**：`src/guardrails/mod.rs`（修改）

**产出**：`GuardrailPipeline`、`GuardContext`

**关键实现**（最终管线顺序，沙箱先于审批；原计划为"先审批后沙箱"，修订见 §7）：

```rust
pub struct GuardrailPipeline {
    rules: StaticRuleEngine,
    assessors: Vec<Box<dyn RiskAssessor>>,
    approval: ApprovalGate,
    sandbox: SandboxBoundary,
    audit_log: AuditLog,
}

impl GuardrailPipeline {
    pub async fn check(&mut self, action: &Action, ctx: &GuardContext)
        -> Result<GuardResult, HarnessError> {
        // 第一层：静态规则——Deny 直接拦截，Escalate 继续评估
        // 第二层：风险评估——多评估器归并
        // 第三层：沙箱硬校验——参数级违规直接 Blocked（先于审批）
        // 第四层：审批（High 风险）——Approve / Deny / Timeout
        // 审计：每条动作恰好一条记录
    }
}
```

**管线不变量**（实现期通过回归测试固化，commit `8c102ec`）：
1. 沙箱硬校验先于审批——用户永远不会批准一个必然被沙箱拒绝的操作
2. 每条动作恰好一条审计记录——风险等级使用真实评估值，不硬编码
3. 拒绝注入反馈回路——`GuardResult::Denied` 不终止 run，拒绝原因作为 Tool 消息注入对话，LLM 调整后重试（`max_turns` 兜底）

**测试计划**：管线拦截 `rm -rf /`、放行正常命令、升级到审批、沙箱拒绝工作区外路径；回归测试 `test_pipeline_sandbox_blocks_before_approval_single_audit_entry`、`test_pipeline_approval_single_audit_entry`。

**完成状态**：✓ 已完成 — 落地于 commit `8f2e55b`，管线不变量修复 commit `8c102ec`

---

#### 任务 17：审计日志

**文件**：`src/guardrails/audit.rs`（新建）

**产出**：`AuditLog`、`AuditEntry`

**关键实现**：

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
```

JSONL 逐条追加写盘（默认 `.AuV/audit.jsonl`，commit `227e1d5` 统一收纳），支持事后回放审计。

**测试计划**：写盘验证、多条目、时间戳。

**完成状态**：✓ 已完成 — 落地于 commit `8f2e55b`；单记录不变量修复 commit `8c102ec`

---

#### 任务 18：护栏配置文件解析

**文件**：`src/guardrails/config.rs`（新建）

**产出**：护栏规则 TOML/JSON 解析函数

**关键实现**：`parse_rules_from_file()` 读取自定义规则（id/name/pattern/action/priority），与内置规则合并，高优先级覆盖。

**测试计划**：JSON 自定义规则解析、自定义覆盖内置。

**完成状态**：✓ 已完成 — commit `4049924`（feat: add guardrail config file parsing）

---

### 阶段 6：反馈闭环（任务 19-20）

#### 任务 19：反馈通道 trait 与 FeedbackRunner

**文件**：`src/feedback/mod.rs`（修改）、`src/feedback/test_runner.rs`、`src/feedback/type_check.rs`、`src/feedback/lint.rs`（新建）

**产出**：`FeedbackChannel` trait、`FeedbackRunner`、`TestRunnerChannel`、`TypeCheckChannel`、`LintChannel`

**关键实现**：

```rust
#[async_trait]
pub trait FeedbackChannel: Send + Sync {
    fn name(&self) -> &str;
    fn should_run(&self, action: &Action, context: &FeedbackContext) -> bool;
    async fn run(&self, context: &FeedbackContext) -> Result<FeedbackResult, HarnessError>;
}
```

三个通道：`test_runner`（`.rs` 变更时跑 `cargo test`，解析失败用例）、`type_check`（`.rs` 变更时跑 `cargo check`）、`lint`（源码变更时跑 `cargo clippy`）。`FeedbackRunner::run_all()` 运行全部适用通道；`should_retry()` 判断是否继续自我修正（默认最多 3 轮）。**反馈闭环不依赖 LLM**——执行命令、解析输出、返回结构化结果，全部可 mock 命令输出测试。

**测试计划**：test_runner 通过/失败解析、type_check 错误解析、lint 警告解析。

**完成状态**：✓ 已完成 — 落地于 commit `8f2e55b`

---

#### 任务 20：反馈结果格式化

**文件**：`src/feedback/mod.rs`（修改）

**产出**：`format_feedback_for_llm()`

**关键实现**：结构化错误信息（通道名、通过/失败、`文件:行:消息`）格式化为注入 LLM 上下文的文本。

**测试计划**：全通过、部分失败（含错误细节）、空结果。

**完成状态**：✓ 已完成 — commit `e63249e`（feat: add feedback result formatting for LLM injection）

---

### 阶段 7：记忆系统（任务 21）

#### 任务 21：文件级持久化 MemoryStore

**文件**：`src/memory/mod.rs`（修改）、`src/memory/entry.rs`（新建）

**产出**：`MemoryStore`、`MemoryEntry`、`MemoryMetadata`、`MemoryType`

**关键实现**：

```rust
pub enum MemoryType { User, Feedback, Project, Reference }

pub struct MemoryEntry {
    pub name: String,           // kebab-case 短名
    pub description: String,    // 一行摘要
    pub file_path: PathBuf,
    pub metadata: MemoryMetadata,
}
```

`MemoryStore`：`load_all()` 解析 `MEMORY.md` 索引（`- [name](file.md) — description` 格式）+ 逐文件读 frontmatter；`search()` 按 name + description 关键词匹配；`write()` 写带 frontmatter 的 Markdown 文件并更新索引；`compact_index()` 生成注入 system prompt 的记忆目录文本。存储路径默认 `.AuV/memory/`（commit `227e1d5` 统一收纳）。

**测试计划**：写入后读取、关键词搜索、索引格式、跨实例持久化。

**完成状态**：✓ 已完成 — commit `3d45dee`（feat: add file-level memory system with index）。**已知缺口**：读侧（索引注入 + 每轮加载）已启用，写侧 `MemoryStore::write()` 无调用点——agent 目前无法保存新记忆，为后续可选任务（见 SPEC.md §11 风险 7）。

---

### 阶段 8：Agent 主循环（任务 22）

#### 任务 22：AgentLoop + ActionParser + ContextBuilder

**文件**：`src/loop/mod.rs`（修改）、`src/loop/parser.rs`、`src/loop/context.rs`（新建）

**产出**：`AgentLoop`、`ActionParser`、`ContextBuilder`

**关键实现**：

```rust
pub struct AgentLoop {
    llm: Box<dyn LlmProvider>,
    guardrails: GuardrailPipeline,
    tools: ToolRegistry,
    feedback: FeedbackRunner,
    memory: MemoryStore,
    config: HarnessConfig,
    parser: ActionParser,
    context_builder: ContextBuilder,
    event_tx: Option<mpsc::Sender<AgentEvent>>,   // 实现期新增：UI 事件通道
}
```

- `ActionParser::parse()`：优先解析 tool_calls，其次 `<tool_call>` XML 标签，`FINAL ANSWER:` 前缀识别为 FinalAnswer，默认按文本回答处理
- `ContextBuilder::build()`：`[System, ...history, User(task)]` 时间顺序排列（system prompt + 工具菜单 + 规则 + 记忆索引在前，历史消息按时间，当前任务最后）
- `run_with_history()`：加载记忆 → 构建上下文 → 循环（LLM 调用 → 解析 → 护栏 → 分发 → 反馈 → 注入结果 → 停机判断）
- 停机判断：FinalAnswer / `max_turns`（默认 50）/ token 预算耗尽

**集成测试计划**（MockLlmProvider 控制回复序列）：单轮完成、工具调用执行、护栏拦截（**Denied 注入 Tool 消息后第二轮 FinalAnswer 恢复**）、max_turns 停机、反馈闭环全流程。多 tool_calls 响应的 tool_call_id 配对不变量回归测试（commit `dc6052a`）。

**完成状态**：✓ 已完成 — 落地于 commit `8f2e55b`；后续关键修复：
- 对话历史时间顺序修复（commit `fdaec41` / `213f44b`）
- 护栏拒绝反馈回路注入（commit `8c102ec`）
- 事件发射（MessageAdded/ToolCallStarted/ToolCallCompleted/ProgressUpdate/Finished，commit `efa9f4d`）
- 失败路径保存会话（commit `8c102ec`）

---

### 阶段 9：可观测性与子 Agent（任务 23-24）

#### 任务 23：TraceLog

**文件**：`src/observability/mod.rs`（修改）

**产出**：`TraceLog`、`TraceEntry`

**关键实现**：`TraceEntry` 记录轮次、时间戳、消息快照、LLM 响应、解析动作、护栏决策、工具结果、反馈结果；JSONL 追加写盘，支持事后回放。

**测试计划**：JSONL 写入、回放迭代。

**完成状态**：✓ 已完成 — commit `5744502`（feat: add trace log for observability）

---

#### 任务 24：SubagentSpawner

**文件**：`src/subagent/mod.rs`（修改）

**产出**：`SubagentSpawner`、`IsolationMode`、`SubagentResult`

**关键实现**：递归调用 `agent_loop`，子 agent 在独立上下文运行、只返回摘要；`IsolationMode`（SameProcess / Worktree）；防护：递归深度上限（默认 3）+ 总 agent 数上限（默认 10，`AtomicUsize` 计数）。

**测试计划**：摘要返回、深度上限、总数上限。

**完成状态**：✓ 已完成 — commit `7db8c06`（feat: add subagent spawner with depth and count limits）

**生产接入（2026-08 补充）**：早期实现仅自测通过、生产零引用（孤岛模块）。现已在 10 步计划中接入生产：`[subagent]` 配置段（max_depth/max_total_agents）→ spawner 深度传播（`for_child` 链）→ `SubagentTool` 委派工具 → `AgentLoopRunner` 生产 Runner（工厂闭包构建子 loop）→ main.rs 装配（REPL/CLI/TUI 全部启用）→ 审批上下文预览 → TUI 子审批路由（`SubagentApprovalNeeded` 事件 + 子专属回发通道）→ 第四项机制演示（父委派子 agent 汇总结果）。SameProcess 已实现；Worktree 保持预留（调用返回明确错误）。已知限制：REPL 冻结期无实时子状态行；超时孤儿线程结果丢弃。commits：`980b593`→`16db698`→`a2937e8`→`b21f8b5`→`300710b`→`dc3f0da`→`e68e2b2`→`143745d`→`d9e4283`。

---

### 阶段 10：凭据管理（任务 25）

#### 任务 25：CredentialManager 与 keyring/env 后端

**文件**：`src/credentials/mod.rs`（修改）、`src/credentials/keyring.rs`、`src/credentials/env.rs`（新建）

**产出**：`CredentialBackend` trait、`KeyringCredentialBackend`、`EnvCredentialBackend`、`CredentialManager`

**关键实现**：

```rust
#[async_trait]
pub trait CredentialBackend: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, HarnessError>;
    async fn set(&self, key: &str, value: &str) -> Result<(), HarnessError>;
    async fn delete(&self, key: &str) -> Result<(), HarnessError>;
    fn list_keys(&self) -> Result<Vec<String>, HarnessError>;
}
```

**测试计划**：env 后端读写删、key 状态不回显明文。

**完成状态**：✓ 已完成 — 初版落地于 commit `8f2e55b`，生产读取于 commit `7079391` 接通。当前主方案为 `auv key set`（系统钥匙串不可用时回退到权限 `0600` 的 AES-256-GCM 文件）；容器使用 `OPENAI_API_KEY_FILE`；`[llm] api_key` 与环境变量仅作明文兼容来源。

---

### 阶段 11：TUI（任务 26-28）

#### 任务 26：TUI 应用状态与事件循环

**文件**：`src/tui/mod.rs`（修改）、`src/tui/app.rs`（新建）

**产出**：`App`、`AppState`、TUI 事件循环

**关键实现**：`AppState` 持有消息列表、当前工具、工具结果、审批请求、状态信息（轮次/token/风险等级/模型）；事件循环用 `try_recv` 轮询 + `event::poll` 输入 + 持续重绘。**agent 完成后 TUI 不自动退出**（channel 断开不是退出信号），用户读完结果按 `q`/`Esc`/`Ctrl+C` 退出（修复"一闪而过"，commit `efa9f4d`）。

**测试计划**：编译 + 既有测试。

**完成状态**：✓ 已完成 — 落地于 commit `ff8e33a`（feat: add TUI app state and event loop）

---

#### 任务 27：TUI 面板（对话、工具、护栏、状态）

**文件**：`src/tui/panels/mod.rs`、`conversation.rs`、`tools.rs`、`guardrails.rs`、`status.rs`（新建）

**产出**：四个面板渲染器

**关键实现**：对话面板角色配色自动滚动到底；工具面板显示当前调用与最近结果（含具体命令 detail）；护栏面板高亮待审批请求、y/n 操作提示置顶（黄底黑字加粗，请求多时不被裁掉）；状态栏单行显示轮次/token/模型/风险等级（24 位真彩色，主题无关可读；去边框修复"内容高度归零不可见"）。

**测试计划**：渲染测试使用真实布局尺寸（状态栏 1 行高）防止掩盖问题。

**完成状态**：✓ 已完成 — 落地于 commit `8f2e55b`；面板修复 commit `dc6052a`（y/n 提示置顶、中文化标签）；真彩色 commit `efa9f4d`

---

#### 任务 28：TUI 布局与终端设置

**文件**：`src/tui/mod.rs`（修改）

**产出**：完整布局 TUI

**关键实现**：ratatui Layout——左侧 70% 对话面板，右侧 30% 上下分割（工具 + 护栏），底部 1 行状态栏。按键：`q`/`Ctrl+C` 退出、`y`/`n` 审批决定（经决定通道传回审批门，commit `efa9f4d`）、`Tab` 焦点切换。agent 循环在独立 tokio 任务中运行，经 mpsc 通道向 TUI 发事件。

**测试计划**：编译 + 既有测试。

**完成状态**：✓ 已完成 — 落地于 commit `8f2e55b`

---

### 阶段 12：CLI 入口（任务 29）

#### 任务 29：clap CLI

**文件**：`src/main.rs`（修改）

**产出**：`auv`（无子命令进入 REPL）、`auv run "task"`、`auv run --no-tui`、`auv init`、`auv --resume`、`--approval <档位>`、`--config <路径>`

**关键实现**：

```rust
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,   // None → 进入 REPL
    // --resume、--approval 为全局参数
}

#[derive(Subcommand)]
enum Commands {
    Run { task: String, #[arg(long)] no_tui: bool },
    Init,
    Key { #[command(subcommand)] action: KeyAction },
}
```

`main()` 装配：分层配置加载（全局 + 项目合并，`--config` 单文件）→ 角色说明叠加 → tracing 初始化（默认 warn、写 stderr、不污染 REPL 输出）→ 按模式分发（REPL / TUI / 纯文本）。

**测试计划**：CLI 解析测试（无子命令进 REPL、`--resume`、`--approval` 英文主值 + 中文别名）、既有测试不回归。

**完成状态**：✓ 已完成 — 落地于 commit `8f2e55b`；REPL 入口 commit `d34516d`；两级配置与品牌化 commit `a30bba0`

---

### 阶段 13：Docker 与 CI（任务 30-31）

#### 任务 30：Dockerfile

**文件**：`Dockerfile`、`.dockerignore`（新建）

**关键实现**：多阶段构建——`rust:1.88-alpine` 构建 + `alpine:3.21` 精简运行层；补齐 OpenSSL 静态链接依赖，只复制 `Cargo.toml`、`Cargo.lock` 与 `src/`；运行时使用非 root 用户、`tini`、`auv` ENTRYPOINT。`.dockerignore` 排除 git 元数据、构建产物、Agent 状态、凭据与开发文档。

**验证**：Podman 等价执行 `docker build` 成功；`auv --version` 输出 `auv 0.1.0`；容器 UID/GID 1000，工作目录与配置目录可写；运行镜像约 45 MB。

**完成状态**：✓ 初版 commit `83eadc5`；发布收尾与凭据链路修复 commit `7079391`

---

#### 任务 31：CI 工作流

**文件**：`.github/workflows/ci.yml`、`.github/workflows/publish.yml`、`.gitlab-ci.yml`

**关键实现**：GitHub CI 在每次 push/PR 执行 Rust 1.88 全目标检查、Clippy、全部测试与 doctest、release 构建/烟雾测试、Docker 构建/烟雾测试，并上传 Linux x86_64 GNU 二进制；Publish 流水线先验收测试，再向 GHCR 发布 `linux/amd64` 镜像，`v*` 标签同时创建带 SHA-256 的 GitHub Release；GitLab CI 提供课程指定的 `unit-test` job。

**验证**：CI/CD 初次收尾时本地 449 项测试通过（371 lib + 71 bin + 3 mechanism demo + 4 doctest）；后续功能与回归测试增加后为 480 项（见 AGENT_LOG 当前统计）。Rust 1.88 下 `cargo check` 与 Clippy 成功。GitHub Actions 实跑还发现并修复了 `CARGO_TERM_COLOR=always` 使反馈通道漏判编译错误的问题。仓库存在约 300 条既有 Clippy 风格建议，因此 Clippy 以建议模式运行，`cargo check`、测试、构建与容器烟雾测试为阻断门。

**完成状态**：✓ 初版 commit `83eadc5`；完整 CI/CD 与分发收尾 commit `7079391`

---

### 阶段 14：机制演示（任务 32）

#### 任务 32：机制演示测试

**文件**：`tests/mechanism_demo.rs`（新建）

**产出**：四项确定性演示测试（`cargo test --test mechanism_demo -- --nocapture --test-threads=1`）

**演示内容**：
1. **护栏拦截危险动作**：构造 `bash: rm -rf /` 工具调用，断言 `GuardResult::Denied` 并打印拦截原因
2. **反馈闭环驱动自我修正**：脚本 LLM 预设序列（写入有缺陷代码 → 看到反馈后修复 → FinalAnswer）；第二次调用必须先在 Tool 消息中看到具体反馈文本，否则测试失败
3. **护栏管线全流程（重点维度）**：四层按最终顺序独立验证（静态规则 Escalate、风险评估 High、沙箱拒绝越界写、审批白名单自动批准）+ 全管线组合（`curl | bash` → 升级 → 高风险 → 沙箱放行 → 需审批），打印各层决策轨迹
4. **子 agent 委派与汇总**：父 agent 委派「计算 2+2」，子 loop 独立完成，父第二次调用必须先在 Tool 消息中看到子结果，再生成汇总回答

**完成状态**：✓ 已完成 — 落地于 commit `8f2e55b`；适配审批力度新参数 commit `dc6052a`；评估后修正 commit `35a1cdc`；subagent 接入时增至四项，收尾评测补充反馈/子结果的因果断言并修正 L3/L4 展示顺序。

---

## 5. REPL 交互层扩展

> 实现，将一次性 `auv run "task"` 扩展为交互式 REPL（对标 Claude Code 对话体验）。设计文档原文见 history/specs/ 与 history/PLAN_REPL_CN.md。

### 5.1 设计决策

**1. 对话历史管理**：消息严格按时间顺序 `[System, ...history, User(task)]`。早期实现把新任务插在历史之前（`[System, User(task), ...history]`），导致 LLM 无法识别多轮上下文（commit `fdaec41` 修复）；REPL 对话累积保留全部非 System 消息（含 User 消息）。

**2. 会话管理**：会话序列化为 `.AuV/sessions/<标题>.json`。`/save <名称>`（快照）、`/resume`（交互选择、可取消、无效编号循环重试）、`/sessions`（列表 + 当前会话标记）、`/rename <标题>`（标题清洗：控制字符剔除、路径字符转空格、引号去除、24 字截断）、`/clear`（清空并删除会话文件）、`/model [名称]`（查看/运行时切换，失败回滚）。

**3. 事件通道**：`AgentEvent` 经 mpsc 通道实时推送，REPL 用 `tokio::select!` 并发等待任务与事件流。**运行结束时通道残留事件必须处理完**——直接丢弃导致最终回答与 Token 统计静默丢失（commit `efa9f4d` 修复的历史 bug）。

**4. 界面设计**：全中文、无 emoji、正式专业风格。角色标签彩色背景块（用户蓝底白字、助手绿底黑字、工具紫底白字、系统灰底黑字）；消息带全局序号 `[n]`；工具结果行直接显示具体命令（`[3] 工具 bash: uname -a`，事件 detail 字段 + 历史 tool_call_id 反查，实时/历史/view 五处渲染一致）；输入区上方常驻状态行（模型/累计 Token/上下文剩余）；清屏重绘统一走 `\x1b[2J\x1b[H\x1b[3J`（`3J` 同时清滚动缓冲区，根治向上滚动出现重复历史）。

**5. 输入编辑器**：rustyline 15——`↑`/`↓` 历史导航（持久化 `.AuV/repl_history.txt`）、行内光标编辑、`Ctrl+C`/`Ctrl+D` 退出。

**6. 自动保存（模型起名）**：首条任务结束后模型生成 ≤12 字标题（单轮调用 + 标题 system prompt，失败回退 `autosave`），保存 `<标题>.json`；`auv --resume` 启动恢复最近修改会话（按 mtime）；`/clear` 删除当前会话文件；`/save` 保留命名快照。

**7. 护栏审批 REPL 事件模式**：AgentLoop 只发 `GuardrailApprovalNeeded` 事件，REPL 打印审批块（独立行、`(y/n): ` 提示独立行）、读 y/n、决定经 `decision_tx` 发回；审批结束全屏重绘清除审批块。**不使用 DECSC/DECRC 光标舞步**（滚动/并发输出下不可靠，与 rustyline 刷新模型互斥——回归测试禁止该转义序列）。

### 5.2 实现清单（全部完成）

- CLI 无子命令进入 REPL；多轮对话历史（时间顺序正确）
- 实时事件渲染（助手消息块、工具结果行、护栏审批块、全局序号）
- 会话管理全命令（/save /resume /sessions /rename /clear /model /history /view /cls /approval /skills /help /exit）
- 自动保存（模型起名）+ `--resume` 恢复最近会话
- 历史完整展示（用户/助手消息不截断，工具结果限 12 行/600 字符，`/view <编号>` 看全文）
- 中文界面全量（横幅、帮助、状态、角色标签、护栏审批、评估原因、风险等级、沙箱违规）
- 审批期间 Ctrl+C 视为拒绝；行结束 `\n`/`\r` 容错；审批竞态残留补重绘
- 多 tool_calls 配对（tool_call_id 不变量）、DeepSeek `reasoning_content` 回传
- 审批力度四档（CLI/配置/REPL 三途径）、英文主值 + 中文别名
- 上下文窗口按模型家族识别（gpt-4/5/o 与 claude 128k、deepseek 64k、llama/qwen/glm 32k）
- 项目更名 AuV（二进制/横幅/默认提示词）；两级配置 + AuV.md 角色说明
- 数据目录统一收纳 `.AuV/`（sessions/repl_history/audit.jsonl/memory，commit `227e1d5`）

**完成状态**：✓ 已完成 — 对应 commit `d34516d` 起至 `227e1d5` 的连续迭代（详见 AGENT_LOG.md 时间线）

---

## 6. 依赖关系图

```
阶段 1（任务 1-3）：基础——无依赖
    ↓
阶段 2（任务 4-5）：LLM 层——依赖阶段 1
阶段 3（任务 6）：配置——依赖阶段 1
    ↓（并行）
阶段 4（任务 7-11）：工具——依赖阶段 1、2
阶段 5（任务 12-18）：护栏——依赖阶段 1（重点维度）
    ↓（并行）
阶段 6（任务 19-20）：反馈——依赖阶段 1
阶段 7（任务 21）：记忆——依赖阶段 1
    ↓
阶段 8（任务 22）：Agent 主循环——依赖以上全部
    ↓
阶段 9（任务 23-24）：可观测性 + 子 Agent——依赖阶段 8
阶段 10（任务 25）：凭据——独立（任意时间可做）
阶段 11（任务 26-28）：TUI——依赖阶段 8
阶段 12（任务 29）：CLI——依赖以上全部
    ↓
阶段 13（任务 30-31）：Docker + CI——依赖阶段 12
阶段 14（任务 32）：机制演示——依赖阶段 8、5（护栏）、6（反馈）
阶段 15：REPL 扩展——依赖阶段 8、12（计划外新增）
```

**并行工作机会**：阶段 2 + 3 + 5 可在阶段 1 后并行；阶段 4 + 6 + 7 + 10 可并行；阶段 9 + 11 在阶段 8 后并行。

---

## 7. 实施过程中的计划修订

| 修订 | 内容 | 依据 |
|------|------|------|
| 模块布局冲突 | 原计划 Task 1 为所有模块创建 `src/<module>/mod.rs`，Task 2/3 又要求修改 `src/types.rs`/`src/error.rs`——同一模块不能同时有两种表示。修订为混合布局：叶子模块用 `src/<module>.rs`，含子模块的用 `src/<module>/mod.rs`（用户确认） | SPEC_PROCESS.md 冷启动验证 |
| Cargo 目标声明 | 原 Cargo 示例缺 `[lib]`/`[[bin]]` 声明，补充后产物名称与计划一致 | SPEC_PROCESS.md 冷启动验证 |
| 护栏管线顺序 | 原计划「审批 → 沙箱」，实现中发现审批后仍被沙箱拦造成"假批准"，修订为「沙箱硬校验先于审批」 | commit `8c102ec`（真实会话审计日志定位） |
| 凭据主方案 | 保留 OS 钥匙串主路径；无桌面环境自动回退到 `0600` 加密文件，容器走 secret file；明文配置/环境变量仅作兼容来源 | SPEC.md §8.1、commit `7079391` |
| 二进制更名 | `harness` → `auv`（项目更名 AuV harness agent），lib/包名与内部类型名保持 | commit `a30bba0` |
| 数据目录 | `.harness/`、`.memory/` 统一收纳到 `.AuV/`（一次性手动迁移） | commit `227e1d5` |
| 主循环 API | 原计划 `AgentLoop::run()` 一次性运行，实现为 `run_with_history(task, history)` 支持跨轮对话 + UI 事件发射 | REPL 扩展阶段 |
| L3/L4 编号 | 沙箱与审批的层号随管线顺序互换（本文件按最终顺序叙述） | commit `8c102ec` |
| 新增阶段 15 | REPL 交互层为计划外新增阶段（用户需求驱动），包含会话管理、事件系统、审批交互、自动保存等 | §5 |
