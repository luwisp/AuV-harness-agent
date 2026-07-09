# HarnessAgent — Coding Agent Harness

> 将 LLM 封装成一台能稳定、可靠执行软件工程任务的完整系统。

## 概述

HarnessAgent 是一个从零实现的 Coding Agent Harness，核心理念是 **Agent = LLM + Harness**。LLM 只负责"下一步做什么"的任务决策，其余全部是 harness 的工程：

- **决策封装**：主循环组织上下文 → 调用 LLM → 解析动作 → 分发执行 → 回灌结果
- **工具系统**：8 个内置工具（文件读写、bash、grep、glob、git diff、测试运行、LSP 诊断）
- **治理护栏**（重点维度）：四层管线（静态规则 → 风险评估 → 审批状态机 → 沙箱边界）
- **反馈闭环**：自动运行测试/lint/类型检查，将结果回灌给 LLM 自我修正
- **记忆系统**：文件级跨会话记忆，启动时加载索引，运行时按需检索
- **TUI 界面**：基于 ratatui 的终端 UI，实时显示对话、工具调用、护栏审批

## 快速开始

### 前置依赖

- Rust 1.85+ (edition 2024)
- 可选：Docker（容器分发）
- 可选：OpenAI API Key（或兼容 API）

### 从源码构建

```bash
git clone <repo-url>
cd harnessAgent
cargo build --release
./target/release/harness --help
```

### 二进制分发

```bash
# 下载 Release 中的二进制文件
chmod +x harness
./harness --help
```

### Docker 分发

```bash
docker build -t harness-agent .
docker run -it harness-agent run "你的任务描述"
```

## 使用方法

### 初始化

首次运行需要初始化配置和 API Key：

```bash
harness init
```

交互式引导会：
1. 创建 `config.toml` 配置文件
2. 创建 `.memory/` 记忆目录
3. 引导你安全录入 API Key（隐藏输入，不显示明文）

### 管理 API Key

```bash
harness key status    # 查看已配置的 key（不显示明文）
harness key set       # 交互式录入 API key
harness key clear     # 删除存储的 key
```

### 运行 Agent

```bash
# TUI 模式（默认）
harness run "在 src/main.rs 中添加一个 hello world 函数"

# 纯文本模式（适合 CI/管道）
harness run --no-tui "运行 cargo test 并修复失败的测试"

# 使用自定义配置文件
harness run -c custom-config.toml "你的任务"
```

### TUI 交互

TUI 启动后，界面分为四个面板：

```
┌──────────────────────────────┬───────────────────────┐
│                              │      工具面板           │
│      对话面板 (70%)           │  当前工具调用及结果     │
│  User: 任务描述               ├───────────────────────┤
│  Assistant: 工具调用...       │    护栏面板            │
│  Tool: 执行结果               │  审批请求 (y/n 交互)   │
├──────────────────────────────┴───────────────────────┤
│  轮次: 3  │  Token: 1234  │  风险: Low  │  模型: gpt-4o │
└──────────────────────────────────────────────────────┘
```

**快捷键：**
| 按键 | 功能 |
|------|------|
| `q` / `Esc` / `Ctrl+C` | 退出 |
| `y` | 批准护栏请求 |
| `n` | 拒绝护栏请求 |
| `Enter` | 确认 |
| `Tab` | 切换面板焦点 |

### 运行测试

```bash
# 运行所有测试（含 mock LLM 测试，不依赖网络）
cargo test

# 运行机制演示
cargo test --test mechanism_demo
```

## 安全边界

### API Key 安全

- Key **绝不**硬编码进源码、**绝不**提交 git、**绝不**写入日志
- 首选存储：OS 钥匙串（Linux Secret Service / macOS Keychain / Windows Credential Manager）
- 备选方案：AES-256-GCM 加密文件（主密码派生密钥）
- 最低方案：`.env` 文件（明文风险，仅开发环境，需自行确认风险）

### 治理护栏

四层管线确保危险操作被拦截：

| 层级 | 机制 | 示例 |
|------|------|------|
| 第1层 | 静态规则引擎 | `rm -rf /` → 直接拒绝 |
| 第2层 | 风险评估器 | `sudo cmd` → High 风险 |
| 第3层 | HITL 审批 | High 风险暂停，等待人工 y/n |
| 第4层 | 沙箱边界 | 限制工作目录、命令白名单 |

### 审计日志

每次护栏决策写入 `.harness/audit.jsonl`，包含时间戳、动作、风险评估、决策、审批人，可事后回放。

## 配置文件

`config.toml`（可选的覆盖配置）：

```toml
[llm]
provider = "openai"       # LLM 供应商
model = "gpt-4o"          # 模型名称
base_url = "https://api.openai.com/v1"  # 可选，兼容 API
timeout_secs = 120        # 请求超时

[agent]
max_turns = 50            # 最大轮次
token_budget = 100000     # Token 预算上限（可选）

[guardrails]
enabled = true
approval_timeout_secs = 120  # 审批超时

[sandbox]
enabled = true
network_allowed = true
max_timeout_secs = 300
```

### 规则文件

创建 `rules.md` 来声明式约束 agent 行为：

```markdown
# 项目规则
- 始终使用 async/await 而非阻塞调用
- 不要在代码中使用 unwrap()
- 测试文件放在 tests/ 目录下
```

### 技能文件

将 `.md` 文件放入 `.skills/` 目录，每个文件一个 skill：

```markdown
---
description: 部署应用到生产环境
---
# Deploy Skill
...（skill 指令内容）
```

Agent 会在需要时自动读取匹配的 skill 文件。

## 目录结构

```
harnessAgent/
├── src/
│   ├── main.rs              # CLI 入口
│   ├── lib.rs               # 库根
│   ├── types.rs             # 核心数据类型
│   ├── error.rs             # 错误类型
│   ├── llm/                 # LLM 抽象层
│   │   ├── mod.rs           # LlmProvider trait
│   │   ├── mock.rs          # Mock LLM（测试用）
│   │   └── openai.rs        # OpenAI 实现
│   ├── config/              # 配置系统
│   │   ├── mod.rs           # HarnessConfig
│   │   ├── rules.rs         # 规则文件解析
│   │   └── skills.rs        # 技能索引
│   ├── loop/                # Agent 主循环
│   │   ├── mod.rs           # AgentLoop
│   │   ├── parser.rs        # 动作解析器
│   │   └── context.rs       # 上下文构建器
│   ├── tools/               # 工具系统
│   │   ├── mod.rs           # Tool trait + ToolRegistry
│   │   ├── context.rs       # ToolContext
│   │   ├── file.rs          # 文件读写
│   │   ├── bash.rs          # Shell 执行
│   │   ├── search.rs        # grep/glob 搜索
│   │   ├── git.rs           # git diff
│   │   └── test_runner.rs   # 测试运行
│   ├── guardrails/          # 护栏系统（重点维度）
│   │   ├── mod.rs           # GuardrailPipeline
│   │   ├── rules.rs         # 静态规则引擎
│   │   ├── assessor.rs      # 风险评估器
│   │   ├── approval.rs      # 审批状态机
│   │   ├── sandbox.rs       # 沙箱边界
│   │   ├── audit.rs         # 审计日志
│   │   └── config.rs        # 护栏配置解析
│   ├── feedback/            # 反馈闭环
│   │   ├── mod.rs           # FeedbackRunner
│   │   ├── test_runner.rs   # cargo test 通道
│   │   ├── type_check.rs    # cargo check 通道
│   │   └── lint.rs          # cargo clippy 通道
│   ├── memory/              # 记忆系统
│   │   ├── mod.rs           # MemoryStore
│   │   └── entry.rs         # MemoryEntry
│   ├── subagent/            # 子 Agent
│   ├── observability/       # 可观测性
│   ├── credentials/         # 凭据管理
│   │   ├── mod.rs           # CredentialManager
│   │   ├── keyring.rs       # OS 钥匙串
│   │   └── env.rs           # .env 文件
│   └── tui/                 # 终端 UI
│       ├── mod.rs           # TUI 主入口
│       ├── app.rs           # App 状态
│       └── panels/          # 四个面板
├── tests/
│   └── mechanism_demo.rs    # 机制演示
├── Cargo.toml
├── Dockerfile
└── .github/workflows/ci.yml
```

## 已知限制

- **LSP 诊断**：当前降级为解析 `cargo check` 输出，未实现完整 LSP 协议
- **OS 钥匙串**：Linux 下需 `gnome-keyring` 或 `kwallet`，无桌面环境自动降级到加密文件
- **子 Agent Worktree 隔离**：SameProcess 模式可用，Worktree 模式需 git 支持
- **MCP 客户端**：未实现，作为 stretch goal
- **平台**：主要支持 Linux x86_64，macOS ARM64 可编译但未充分测试

## 技术选型

| 选择 | 理由 |
|------|------|
| Rust (edition 2024) | trait 系统天然适合可注入 mock 架构；静态编译适合二进制分发 |
| OpenAI API | 兼容接口最广，第三方服务广泛兼容 |
| tokio | 异步运行时，支持并发 LLM 调用和子 agent |
| ratatui | 终端 TUI 框架，轻量高效 |
| clap | CLI 参数解析，derive 模式简洁 |
| keyring | 跨平台 OS 钥匙串抽象 |