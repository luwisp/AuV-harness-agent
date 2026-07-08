# HarnessAgent — Coding Agent Harness 设计文档

> 日期：2025-07-08
> 项目：AI4SE 期末项目 · A · Coding Agent Harness
> 语言：Rust (edition 2024)

---

## 1. 问题陈述

### 1.1 要解决什么问题

构建一个 **Coding Agent Harness**——将 LLM 从"只能产生下一步设想"的推理引擎，封装成一台能稳定、可靠执行软件工程任务的完整系统。Harness 是 LLM（CPU）与外部世界之间的全部工程：工具分发、治理护栏、反馈闭环、记忆管理、上下文工程。

### 1.2 目标用户

- 需要在编码场景中使用 AI agent 的开发者
- 需要安全边界（审批、沙箱）的团队 / 企业场景
- 对 agent 内部机制有研究需求的 AI4SE 学习者

### 1.3 为什么值得做

现有 agent 框架（LangChain、AutoGen 等）将核心循环内置于框架中，用户只能做配置级定制。本项目从零实现 harness 内核，让每个机制（护栏、反馈、记忆）都是可验证、可单测的独立代码，而非依赖 LLM 智能的提示词。这既是工程训练，也是对"Agent = LLM + Harness"这一等式的第一手验证。

---

## 2. 用户故事

1. **US1**：作为一个开发者，我希望 agent 能读取我的代码、运行 shell 命令、执行测试，并根据测试结果自动修正代码。
2. **US2**：作为一个开发者，我希望 agent 在执行危险操作（如 `rm -rf`、删除数据库）时被拦截，并需要我人工确认后才继续。
3. **US3**：作为一个开发者，我希望 agent 能记住项目约定和我的偏好，跨会话保持这些信息。
4. **US4**：作为一个安全管理员，我希望所有 agent 的决策和动作都有完整审计日志，可事后回放。
5. **US5**：作为一个开发者，我希望我的 API key 被安全存储（不硬编码、不进 git），首次运行时能安全录入。
6. **US6**：作为一个开发者，我希望通过声明式规则文件（类似 CLAUDE.md）约束 agent 的行为，而不需修改代码。
7. **US7**：作为一个用户，我希望能通过 Docker 或二进制文件一键运行 harness，无需复杂的依赖安装。

---

## 3. 功能规约

### 3.1 Agent 主循环

- **输入**：用户任务描述、配置、记忆
- **行为**：组织上下文 → 调用 LLM → 解析动作 → 护栏检查 → 分发执行 → 反馈收集 → 回灌结果 → 停机判断 → 循环
- **输出**：任务完成或终止
- **边界条件**：最大轮次（默认 50）、token 预算耗尽、用户中断信号
- **错误处理**：LLM 调用失败重试 3 次（指数退避）、工具执行失败将错误回灌给 LLM

### 3.2 LLM 抽象层

- **输入**：消息列表 `Vec<Message>`
- **行为**：将消息发送给 LLM 供应商，返回原始响应
- **输出**：`LlmResponse { content, finish_reason, usage }`
- **边界条件**：超时（默认 120s）、速率限制、空响应
- **错误处理**：网络错误重试、认证错误立即失败并提示用户检查 key

**Mock 实现**：`MockLlmProvider` 支持预设回复序列，用于所有机制的确定性单元测试。

### 3.3 工具系统

| 工具 | 功能 | 安全边界 |
|------|------|---------|
| `read_file` | 读取文件（支持行号范围） | 沙箱路径限制 |
| `write_file` | 创建/覆盖文件 | 沙箱 + 护栏 |
| `bash` | 执行 shell 命令 | 护栏 + 超时 + 沙箱 |
| `grep` | 内容搜索 | 只读 + 沙箱路径 |
| `glob` | 文件名模式匹配 | 只读 |
| `lsp_diagnostic` | 查询 LSP 诊断 | 只读 |
| `git_diff` | 查看工作区变更 | 只读 |
| `run_test` | 执行测试命令 | 超时 + 输出截断 |

### 3.4 治理护栏（重点维度）

四层管线架构：

```
Action → [静态规则] → [风险评估] → [审批状态机] → [沙箱校验] → 执行/拒绝
```

**第一层 · 静态规则引擎**：基于正则 / glob 的模式匹配。内置危险命令规则（`rm -rf /`、`DROP TABLE`、`curl | bash` 等），支持 Allow / Deny / Escalate 三种动作。规则可通过配置文件扩展。

**第二层 · 风险评估器**：多评估器组合打分（`CommandRiskAssessor`、`FileRiskAssessor`、`NetworkRiskAssessor`），输出风险等级：Low（自动放行）、Medium（日志记录但放行）、High（需审批）、Critical（硬拒绝）。

**第三层 · HITL 审批状态机**：High 风险操作暂停，等待人工审批。支持 Approve / Deny / Timeout（默认 120s 超时自动拒绝）。会话级白名单（已批准的操作指纹在同一会话内自动放行）。

**第四层 · 沙箱边界**：限制工作目录、命令白名单/黑名单、超时上限、网络开关。在工具执行前做最终校验。

**审计日志**：每次护栏决策写入 JSONL 文件，包含时间戳、动作、风险评估、决策、审批人。

### 3.5 反馈闭环

- **通道**：`test_runner`（`cargo test`）、`type_check`（`cargo check`）、`lint`（`cargo clippy`）、`lsp_diagnostic`
- **触发条件**：代码变更时自动触发相关通道
- **回灌**：结构化错误信息（文件、行号、错误类型）回灌给 LLM
- **自我修正**：最多 3 轮，每轮 agent 看到反馈后修改代码，再跑验证

### 3.6 记忆系统

文件级记忆，存储结构：
```
.memory/
  MEMORY.md            # 索引文件
  <name>.md            # 每个记忆一个文件，含 frontmatter
```

- **写**：agent 调用 `write_memory` 工具，写入带 frontmatter 的 Markdown 文件 + 更新索引
- **读**：启动时加载索引，运行时按需搜索（关键词匹配）
- **索引注入**：每次主循环构建上下文时，将记忆索引（仅标题+描述）注入 system prompt

### 3.7 配置系统

- **规则文件**（`rules.md`）：启动时加载，注入 system prompt 作为声明式约束
- **技能文件**（`SKILL.md`）：以"名片"模式载入（仅 description），命中时读全文
- **主配置**（`config.toml`）：LLM、护栏、沙箱、记忆、反馈的所有参数

### 3.8 子 Agent 系统

- 递归调用 `agent_loop`，子 agent 在独立上下文中运行
- 子 agent 只返回摘要给主循环
- 支持 SameProcess 和 Worktree 两种隔离模式
- 防护：递归深度上限（默认 3）、总 agent 数上限（默认 10）

### 3.9 可观测性

- 每次循环记录一条 `TraceEntry`（JSONL），包含：轮次、消息快照、LLM 响应、解析动作、护栏决策、工具结果、反馈结果
- 支持事后回放审计

### 3.10 凭据管理

- **安全存储**：OS 钥匙串（Linux Secret Service / macOS Keychain / Windows Credential Manager）或带主密码的加密文件
- **首次录入**：`rpassword` 隐藏输入引导
- **命令**：`harness key status`（不回显明文）、`harness key set`、`harness key clear`
- **威胁模型**：key 绝不硬编码、不提交 git、不写入日志、不进入 shell history。`.env` 文件作为备选方案但文档化其明文风险

---

## 4. 领域与机制设计

### 4.1 该领域的反馈信号

- 测试结果（`cargo test` 的 pass/fail + 具体失败用例）
- 类型检查（`cargo check` 的编译错误）
- Lint 警告（`cargo clippy`）
- LSP 诊断（语法错误、未定义变量、类型不匹配）

### 4.2 危险动作

- 破坏性 shell 命令（`rm -rf`、`dd`、`mkfs`）
- 数据库删除操作（`DROP TABLE`、`DROP DATABASE`）
- 权限变更（`chmod 777`、`chown`）
- 对外发布（`git push --force`、`scp`、`rsync` 到远程）
- 关键配置覆盖（`/etc/`、`~/.ssh/`、`.env`）

### 4.3 所需工具

见 §3.3 工具系统。

### 4.4 记忆需求

- 项目约定（代码风格、命名规范、技术栈选择）
- 设计决策与理由
- 用户偏好（如"不要自动 git commit"）
- 常见陷阱与已知问题

### 4.5 重点维度：治理护栏

选择治理护栏作为 main contribution，理由：
1. 护栏天生由代码构成（规则引擎、风险评估、审批状态机、沙箱），天然满足"机制必须是代码"的要求
2. 四层管线架构有足够的工程深度（每层独立可测，组合后覆盖全链路）
3. 在 AI 安全日益重要的背景下，护栏是 harness 最具实际价值的组件
4. 可以写出丰富的确定性单元测试（mock LLM 下测试每一层的拦截/放行/审批逻辑）

### 4.6 机制编码实现方式

所有机制均为确定性 Rust 代码，通过 trait 抽象实现可测试性：

- **护栏**：`GuardrailPipeline` 的每一层都是独立可测的代码单元。测试时用 mock LLM 构造 Action，断言拦截/放行/审批结果。
- **反馈闭环**：`FeedbackRunner` 不依赖 LLM——它执行命令、解析输出、返回结构化结果。测试时用 mock 命令输出。
- **主循环**：将 `LlmProvider` 替换为 mock，控制模型回复序列，验证循环的完整行为（包含护栏拦截、反馈回灌、停机判断）。
- **记忆**：文件系统操作，测试时用临时目录。

---

## 5. 系统架构

### 5.1 组件图

```
┌─────────────────────────────────────────────────────────┐
│                    HarnessAgent                          │
│                                                          │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────────┐  │
│  │  Config  │  │  Memory  │  │   Observability      │  │
│  │  (rules) │  │  (file)  │  │   (tracing/audit)    │  │
│  └────┬─────┘  └────┬─────┘  └──────────┬───────────┘  │
│       │             │                    │               │
│       └──────────┬──┴────────────────────┘               │
│                  ▼                                       │
│  ┌──────────────────────────────────────┐               │
│  │         Agent Main Loop               │              │
│  │  build_context → llm.complete()       │              │
│  │  → parse_action → guardrail.check()   │              │
│  │  → dispatch_tool → feedback.run()     │              │
│  │  → stop? → loop                        │              │
│  └───┬──────────┬──────────┬─────────────┘              │
│      │          │          │                             │
│  ┌───▼──┐  ┌────▼────┐ ┌──▼──────────┐                 │
│  │ LLM  │  │Guardrail│ │ Tool Registry│                 │
│  │Layer │  │Pipeline │ │ (files/bash/ │                 │
│  │(mock │  │(rules/  │ │  grep/lsp/   │                 │
│  │/real)│  │ approval│ │  git/test)   │                 │
│  └──────┘  │ /sandbox│ └──────────────┘                 │
│            │ /audit) │                                    │
│            └─────────┘                                    │
│  ┌──────────┐  ┌──────────────┐  ┌────────────────┐     │
│  │ Feedback │  │  Subagent    │  │  Credentials   │     │
│  │  Runner  │  │  Spawner     │  │  (keyring/     │     │
│  │(test/lint│  │  (isolate)   │  │   encrypted)   │     │
│  │ /typeck) │  │              │  │                │     │
│  └──────────┘  └──────────────┘  └────────────────┘     │
└─────────────────────────────────────────────────────────┘
```

### 5.2 数据流

```
用户输入 → AgentLoop
           ├── load config + memory
           ├── build context (system prompt + tools + memory index)
           ├── [loop]
           │   ├── LlmProvider::complete(messages)
           │   ├── ActionParser::parse(response)
           │   ├── GuardrailPipeline::check(action)
           │   │   ├── StaticRuleEngine::evaluate()
           │   │   ├── RiskAssessor::assess()
           │   │   ├── ApprovalGate::request_approval() [if High]
           │   │   └── SandboxBoundary::validate()
           │   ├── ToolRegistry::execute(action)
           │   ├── FeedbackRunner::run_all(action)
           │   ├── inject results → messages
           │   └── stop_judgment? → exit | continue
           └── return final response
```

### 5.3 外部依赖

- **LLM 供应商**：Anthropic Messages API（主要），OpenAI Chat Completions API（预留接口）
- **OS 钥匙串**：`keyring` crate（跨平台凭据存储）
- **LSP**：通过 `lsp-server` / `lsp-types` crate 与语言服务器通信
- **Git**：通过 `git2` crate 或命令行调用
- **HTTP**：`reqwest` crate（LLM API 调用）

### 5.4 Crate 结构

```
harnessAgent/
  src/
    main.rs                   # CLI 入口
    lib.rs                    # 库根
    config/
      mod.rs                  # 配置加载与验证
      rules.rs                # 规则文件解析
      skills.rs               # 技能文件解析
    llm/
      mod.rs                  # LlmProvider trait + 工厂
      anthropic.rs            # Anthropic API 实现
      openai.rs               # OpenAI API 实现（预留）
      mock.rs                 # MockLlmProvider
    loop/
      mod.rs                  # AgentLoop 主循环
      parser.rs               # ActionParser 动作解析
      context.rs              # 上下文构建
    tools/
      mod.rs                  # Tool trait + ToolRegistry
      file.rs                 # read_file / write_file
      bash.rs                 # bash 执行
      search.rs               # grep / glob
      lsp.rs                  # LSP 诊断查询
      git.rs                  # git_diff
      test.rs                 # run_test
    guardrails/
      mod.rs                  # GuardrailPipeline
      rules.rs                # StaticRuleEngine
      assessor.rs             # RiskAssessor trait + 内置评估器
      approval.rs             # ApprovalGate 状态机
      sandbox.rs              # SandboxBoundary
      audit.rs                # AuditLog
    feedback/
      mod.rs                  # FeedbackRunner
      test_runner.rs          # cargo test 通道
      type_check.rs           # cargo check 通道
      lint.rs                 # cargo clippy 通道
      lsp_diag.rs             # LSP 诊断通道
    memory/
      mod.rs                  # MemoryStore
      entry.rs                # MemoryEntry 数据结构
    subagent/
      mod.rs                  # SubagentSpawner
    observability/
      mod.rs                  # TraceLog
    credentials/
      mod.rs                  # CredentialManager
      keyring.rs              # KeyringCredentialBackend
      env.rs                  # EnvCredentialBackend
```

---

## 6. 数据模型

### 6.1 Message

```rust
pub struct Message {
    pub role: Role,       // System | User | Assistant | Tool
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
}
```

### 6.2 Action

```rust
pub enum Action {
    ToolCall { id: String, name: String, params: serde_json::Value },
    FinalAnswer { summary: String },
    AskUser { question: String },
    NoOp,
}
```

### 6.3 GuardRule

```rust
pub struct GuardRule {
    pub id: String,
    pub name: String,
    pub pattern: RulePattern,
    pub action: RuleAction,
    pub priority: u8,
}
```

### 6.4 MemoryEntry

```rust
pub struct MemoryEntry {
    pub name: String,           // kebab-case slug
    pub description: String,    // 一行摘要
    pub file_path: PathBuf,
    pub metadata: MemoryMetadata,
}

pub struct MemoryMetadata {
    pub mem_type: MemoryType,   // User | Feedback | Project | Reference
}
```

### 6.5 TraceEntry

```rust
pub struct TraceEntry {
    pub turn: usize,
    pub timestamp: DateTime<Utc>,
    pub messages_snapshot: Vec<Message>,
    pub llm_response: String,
    pub parsed_action: Action,
    pub guard_result: GuardResult,
    pub tool_result: Option<ToolResult>,
    pub feedback_results: Vec<FeedbackResult>,
}
```

---

## 7. 非功能性需求

### 7.1 性能

- 主循环单轮延迟（不含 LLM 调用）< 100ms
- 记忆索引加载 < 500ms（1000 条记忆规模）
- 审计日志追加写入，不阻塞主循环

### 7.2 安全（凭据威胁模型）

- **威胁**：key 泄露（硬编码、git 提交、日志记录、shell history）
- **对策**：key 仅通过 OS 钥匙串或加密文件存储；`.env` 文件需用户确认风险；代码中无任何硬编码 key；git pre-commit hook 扫描敏感模式
- **威胁**：进程环境变量暴露
- **对策**：key 从钥匙串按需读取后立即注入内存，不通过环境变量传递
- **威胁**：中间人攻击
- **对策**：所有 LLM API 调用强制 HTTPS

### 7.3 可用性

- 首次运行引导：`harness init` 交互式设置 key 和基本配置
- 错误信息可操作：网络错误提示检查网络、认证错误提示检查 key
- 护栏审批提示清晰：显示危险操作的具体内容和风险原因

### 7.4 可观测性

- 每次循环的 JSONL 追踪日志
- 护栏决策的审计日志
- 支持 `RUST_LOG` 环境变量控制日志级别

---

## 8. 凭据与分发设计

### 8.1 Key 存储方案

- **主方案**：OS 钥匙串（`keyring` crate）
  - Linux：Secret Service API（需 `gnome-keyring` 或 `kwallet`）
  - macOS：Keychain Services
  - Windows：Credential Manager
- **备选方案**：AES-256-GCM 加密文件（主密码派生密钥，`ring` crate）
- **最低方案**：`.env` 文件（文档化明文风险，仅用于开发环境）

### 8.2 Key 录入 / 更新 / 清除

```bash
harness key status    # 显示已配置的 key（不显示明文）
harness key set       # 交互式录入（隐藏输入）
harness key update    # 更新已有 key
harness key clear     # 删除存储的 key
```

### 8.3 分发形态

**二进制分发**（主要）：
- `cargo build --release` 产出单文件二进制
- CI 中构建 `x86_64-unknown-linux-musl` 静态链接版本
- 作为 GitHub Release asset 发布

**Docker 分发**（辅助）：
- 多阶段构建：`rust:alpine` 构建 + `alpine` 运行
- 推送到 GitHub Container Registry
- `docker pull ghcr.io/<user>/harness-agent && docker run -it ...`

**目标平台**：Linux x86_64（主要）、macOS ARM64（次要）

---

## 9. 技术选型与理由

| 选择 | 理由 |
|------|------|
| **Rust** | 零成本抽象、trait 系统天然适合可注入的 mock 架构；静态编译适合二进制分发；类型系统在编译期拦截大量错误 |
| **Anthropic API**（主要） | Claude 在编码任务上表现最优；Messages API 的 tool_use 格式规范清晰 |
| **OpenAI API**（预留） | 兼容接口广，便于扩展 |
| **`reqwest`** | Rust 生态最成熟的 HTTP 客户端 |
| **`keyring`** | 跨平台 OS 钥匙串抽象 |
| **`serde` + `serde_json`** | JSON Schema 生成与解析 |
| **`tokio`** | 异步运行时，支持并发 LLM 调用和子 agent |
| **`clap`** | CLI 参数解析 |
| **`tracing`** | 结构化日志框架 |

---

## 10. 验收标准

1. **主循环**：mock LLM 下，主循环能完成"接收任务 → 假动作 → 停机"的完整流程
2. **护栏拦截**：mock LLM 下，构造 `rm -rf /` 动作被静态规则引擎拦截
3. **审批流程**：mock LLM 下，High 风险动作触发审批状态机，超时自动拒绝
4. **反馈闭环**：mock 命令输出下，测试失败信息被正确解析并回灌
5. **记忆读写**：写入记忆后重启，记忆仍可检索
6. **凭据安全**：key 不在源码/git/日志中；`key status` 不显示明文
7. **一键测试**：`cargo test` 运行所有单元测试（含 mock LLM 测试），不依赖网络
8. **Docker 运行**：`docker build && docker run` 可启动
9. **CI 通过**：GitHub Actions 中 `cargo test` + `cargo build --release` 全部通过
10. **机制演示**：§A.6 的三项行为在 mock LLM 下可复现

---

## 11. 风险与未决问题

1. **Anthropic API 速率限制**：课程演示时可能遇到 429。对策：mock LLM 演示机制，真实 API 调用保留但非必需。
2. **LSP 集成复杂度**：LSP 协议较复杂，可能在实现中简化。对策：可将 LSP 诊断通道降级为"解析 `cargo check` 输出"。
3. **OS 钥匙串可用性**：Linux 下 Secret Service 不一定可用（无桌面环境）。对策：自动降级到加密文件方案。
4. **子 agent 递归深度**：fork 炸弹风险。对策：硬编码深度上限 + 总 agent 数上限。
5. **MCP 客户端**：作为 stretch goal，如果时间不足则移除。

---

## 12. Stretch Goals（时间允许时）

1. MCP 客户端协议实现
2. 更多 LLM 供应商（Ollama 本地模型）
3. 技能系统的热加载（文件变更自动重载）
4. 交互式 TUI（基于 `ratatui`）
5. Web Dashboard（基于 `axum` + 前端）