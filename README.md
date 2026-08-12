# HarnessAgent — Coding Agent Harness

> 将 LLM 封装成一台能稳定、可靠执行软件工程任务的完整系统。核心理念：**Agent = LLM + Harness**。

## 目录

- [三种运行模式](#三种运行模式)
- [快速开始](#快速开始)
- [1. 安装与初始化](#1-安装与初始化)
- [2. 配置 API](#2-配置-api)
- [3. 运行 Agent](#3-运行-agent)
- [4. 配置文件](#4-配置文件)
- [5. 规则与技能](#5-规则与技能)
- [6. 护栏系统](#6-护栏系统)
- [7. 目录结构](#7-目录结构)
- [8. 开发与测试](#8-开发与测试)
- [已知限制](#已知限制)

---

## 三种运行模式

`harness` 有三种截然不同的交互方式，按场景选择：

| 模式 | 命令 | 适用场景 |
|------|------|---------|
| **交互式 REPL** | `harness` | 日常使用，多轮对话，持续编码 |
| **TUI 模式** | `harness run "任务"` | 单次任务，需要可视化面板 |
| **纯文本模式** | `harness run --no-tui "任务"` | CI/脚本/管道，不需终端 |

### 模式 1：交互式 REPL（推荐日常使用）

```bash
harness
```

不带任何参数直接启动，进入类似 Claude Code 的交互式对话循环：

```
HarnessAgent REPL v0.1.0
Type a task for the agent, or /exit to quit.
Type /help for available commands.

> 帮我把 src/main.rs 里的 println! 换成 tracing::info!

⏳ Running agent for: "帮我把 src/main.rs 里的 println! 换成 tracing::info!"

✅ Result: 已将 3 处 println! 替换为 tracing::info!，修改文件 src/main.rs

> 现在给这些日志加上合适的日志级别

...
```

**REPL 内部命令：**

| 输入 | 功能 |
|------|------|
| 任意文本 | 作为任务发送给 agent |
| `/help` | 显示帮助 |
| `/exit` 或 `/quit` | 退出 |
| `Ctrl+D` | 退出（EOF） |
| `Ctrl+C` | 强制退出 |

**REPL 特性：** 会话历史会在多轮对话中保持，agent 记得之前做过的事。上一轮读过的文件、执行过的命令结果都在上下文中。

**当前限制：** REPL 模式下 agent 的工具调用过程不实时显示（与 Claude Code 不同），只显示最终结果。如需看到每一步工具调用，请使用 TUI 模式。

### 模式 2：TUI 模式（可视化面板）

```bash
harness run "你的任务"
```

在终端中启动时，默认进入 TUI 模式（除非加了 `--no-tui` 或 stdout 不是终端）：

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

**TUI 快捷键：**

| 按键 | 功能 |
|------|------|
| `q` / `Esc` / `Ctrl+C` | 退出 |
| `y` | 批准护栏请求（执行危险操作） |
| `n` | 拒绝护栏请求 |
| `Enter` | 确认 |
| `Tab` | 切换面板焦点 |

### 模式 3：纯文本模式（脚本/CI/管道）

```bash
harness run --no-tui "运行 cargo test 并修复失败的测试"
```

纯文本输出，不依赖终端能力。适合：
- CI/CD 流水线
- 脚本自动化
- 管道重定向（`harness run "task" > output.txt`）

---

## 快速开始

### 前置条件

- Rust 1.85+ (edition 2024)
- OpenAI API Key，或任何兼容 OpenAI 接口的服务（DeepSeek、Groq、Ollama、vLLM 等）

### 三步启动

```bash
# 1. 编译
cargo build --release

# 2. 初始化（首次运行，创建配置 + 录入 key）
./target/release/harness init

# 3. 开始使用
./target/release/harness
```

---

## 1. 安装与初始化

### 从源码构建

```bash
git clone <repo-url>
cd harnessAgent
cargo build --release
./target/release/harness --help
```

### Docker

```bash
docker build -t harness-agent .
docker run -it harness-agent
```

### 初始化

```bash
harness init
```

交互式引导会：
1. 创建 `config.toml` — 可选的配置文件
2. 创建 `.memory/` — 记忆存储目录
3. 引导你输入 API Key（隐藏回显）

---

## 2. 配置 API

### 密钥管理

```bash
harness key status       # 查看已配置哪些 key（不显示明文）
harness key set          # 交互式录入（隐藏回显）
harness key clear <名称>  # 删除存储的 key
```

存储方式：优先 OS 钥匙串（Linux Secret Service / macOS Keychain），不可用时自动降级到 AES-256-GCM 加密文件。

### 设置 API Key 的其他方式

除了 `harness key set`，也可以：

```bash
# 环境变量（优先级低于 config.toml）
export OPENAI_API_KEY="sk-your-key"
```

```toml
# config.toml（优先级最高）
[llm]
api_key = "sk-your-key"
```

### 使用兼容 API（DeepSeek / Groq / Ollama / vLLM）

任何兼容 OpenAI Chat Completions 格式的服务都能用，只需改 `config.toml`：

**DeepSeek：**
```toml
[llm]
model = "deepseek-chat"
base_url = "https://api.deepseek.com/v1"
api_key = "sk-your-deepseek-key"
```

**Groq：**
```toml
[llm]
model = "llama-3.1-70b-versatile"
base_url = "https://api.groq.com/openai/v1"
api_key = "gsk-your-groq-key"
```

**本地 Ollama：**
```toml
[llm]
model = "llama3.1"
base_url = "http://localhost:11434/v1"
api_key = "ollama"   # Ollama 不需要真实 key，但字段不能为空
```

---

## 3. 运行 Agent

### 所有 CLI 入口总览

```
harness                       交互式 REPL（推荐）
harness run "task"            TUI 模式单次任务
harness run --no-tui "task"   纯文本单次任务（CI/脚本）
harness run -c cfg.toml "t"   使用自定义配置
harness init                  初始化配置
harness key status            查看 key 配置
harness key set               录入 key
harness key clear <name>     删除 key
harness --help                查看所有命令
harness --version             查看版本
```

### 示例

```bash
# REPL — 日常编程
harness

# 单次 — 查看并修复 bug
harness run "src/auth.rs 中的登录函数在空密码时 panic，帮我修复"

# 单次 — 重构
harness run --no-tui "把 src/ 下所有的 unwrap() 替换成 ? 操作符"

# 用 DeepSeek
harness run -c deepseek.toml "解释一下这个项目的架构"

# 写 Dockerfile 然后构建
harness run "写一个 Dockerfile 用于部署这个 Rust 项目"
```

---

## 4. 配置文件

`harness init` 会在当前目录创建 `config.toml`。所有字段都有默认值，不创建配置文件也能正常运行。

```toml
[llm]
provider = "openai"                          # LLM 供应商
model = "gpt-4o"                             # 模型名
base_url = "https://api.openai.com/v1"       # API 地址（兼容服务改这里）
api_key = "sk-..."                           # API key（可选，也可用环境变量）
max_tokens = 4096
temperature = 0.0
timeout_secs = 120

[agent]
max_turns = 50                               # 最大循环轮次
token_budget = 100000                        # Token 上限（可选）
system_prompt = ""                           # 自定义 system prompt（可选）
skills_dir = ".skills"                       # 技能文件目录

[guardrails]
enabled = true                               # 启用护栏
rules_file = "rules.md"                      # 规则文件路径
approval_timeout_secs = 120                  # 审批超时
audit_log_path = ".harness/audit.jsonl"     # 审计日志路径

[sandbox]
enabled = true
network_allowed = true                       # 允许网络访问
max_timeout_secs = 300                       # 命令执行超时
allowed_commands = []                        # 命令白名单
forbidden_commands = [                       # 命令黑名单
    "rm -rf /",
    "sudo",
    "mkfs",
]

[tools]
disabled_tools = []                          # 禁用某些工具（如 ["bash"]）

[memory]
storage_path = ".memory"                     # 记忆存储路径

[feedback]
enabled = true
max_retries = 3                              # 最多自我修正轮次
```

---

## 5. 规则与技能

### 规则文件（`rules.md`）

声明式约束 agent 行为，一行一条规则，`#` 开头为注释：

```markdown
# Coding Rules
- 所有函数必须写 doc comment
- 使用 async/await，不写阻塞代码
- 禁止使用 unwrap() 和 expect()
- 错误类型要提供可操作的信息
- 修改代码后必须运行 cargo test
```

规则会被注入到 system prompt 中，每次 LLM 调用都会看到。

### 技能文件（`.skills/` 目录）

将领域知识打包成技能文件，agent 按需加载：

```markdown
---
description: 部署应用到 Kubernetes 集群
---

# K8s Deploy Skill

## pre-deploy check
- kubectl cluster-info
- kubectl get pods -n production

## deploy steps
1. docker build -t app:latest .
2. kubectl apply -f k8s/deployment.yaml
3. kubectl rollout status deployment/app
```

Agent 会在 relevant 时自动读取完整 skill 内容。

---

## 6. 护栏系统

四层管线确保危险操作被拦截：

| 层级 | 名称 | 机制 | 示例 |
|------|------|------|------|
| L1 | 静态规则 | glob 模式匹配，Allow/Deny/Escalate | `rm -rf /` → 直接 Deny |
| L2 | 风险评估 | 命令/文件/网络三维度打分 | `curl \| bash` → High |
| L3 | 人工审批 | High 风险暂停，等待 y/n | 审批超时自动拒绝 |
| L4 | 沙箱边界 | 工作目录限制、命令黑白名单 | 写 `/etc/` → 拒绝 |

所有护栏决策写入 `.harness/audit.jsonl`。

---

## 7. 目录结构

```
harnessAgent/
├── src/
│   ├── main.rs              # CLI 入口（6 个子命令 + REPL）
│   ├── lib.rs               # 库根
│   ├── types.rs             # 核心数据类型
│   ├── error.rs             # 错误类型
│   ├── llm/                 # LLM 抽象层（trait + openai + mock）
│   ├── config/              # 配置（HarnessConfig + rules + skills）
│   ├── loop/                # Agent 主循环（AgentLoop + parser + context）
│   ├── tools/               # 工具系统（8 个内置工具）
│   ├── guardrails/          # 护栏系统（四层管线）
│   ├── feedback/            # 反馈闭环（test/lint/typeck）
│   ├── memory/              # 记忆系统（文件级）
│   ├── credentials/         # 凭据管理（keyring + enc file）
│   ├── observability/       # 可观测性（trace log）
│   ├── subagent/            # 子 Agent 派发
│   └── tui/                 # 终端 UI（ratatui）
├── docs/superpowers/        # 设计文档与计划
├── tests/mechanism_demo.rs  # 机制演示测试
├── config.toml              # 配置文件（可选）
├── rules.md                 # 规则文件（可选）
├── Cargo.toml
└── Dockerfile
```

---

## 8. 开发与测试

```bash
# 运行所有 362 个测试（不依赖网络，全部用 mock LLM）
cargo test

# 运行机制演示
cargo test --test mechanism_demo

# 编译 release
cargo build --release

# 查看日志（设置 log level）
RUST_LOG=debug cargo run
```

---

## 已知限制

- **REPL 不展示中间步骤**：当前 REPL 模式下看不到 agent 的工具调用过程，只显示最终结果。如需可视化每一步，请使用 `harness run "task"` TUI 模式。后续版本计划在 REPL 中加入流式输出。
- **LSP 诊断**：降级为解析 `cargo check` 输出，未实现完整 LSP 协议。
- **Linux 钥匙串**：需 `gnome-keyring` 或 `kwallet`，无桌面环境自动降级到加密文件。
- **平台**：主要测试 Linux x86_64；macOS ARM64 可编译但未充分测试。
- **流式输出**：LLM 响应不是流式的，大任务时等待时间较长。
