# AuV harness agent — Coding Agent Harness

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

`auv` 有三种截然不同的交互方式，按场景选择：

| 模式 | 命令 | 适用场景 |
|------|------|---------|
| **交互式 REPL** | `auv` | 日常使用，多轮对话，持续编码 |
| **TUI 模式** | `auv run "任务"` | 单次任务，需要可视化面板 |
| **纯文本模式** | `auv run --no-tui "任务"` | CI/脚本/管道，不需终端 |

### 模式 1：交互式 REPL（推荐日常使用）

```bash
auv
```

不带任何参数直接启动，进入类似 Claude Code 的交互式对话循环。**默认开启新会话**；需要接着上次的对话时用 `auv --resume` 启动（恢复自动保存的会话）。

```
AuV harness agent REPL v0.1.0
输入任务开始对话，/help 查看命令，/exit 退出
（暂无对话历史）
────────────────────────────────────────────────
模型: gpt-4o | Token: 0 | 上下文剩余: 100%（128000/128000）
> 帮我把 src/main.rs 里的 println! 换成 tracing::info!
[1]  用户 › 帮我把 src/main.rs 里的 println! 换成 tracing::info!
[2]  助手 › 我来帮你把 println! 替换为 tracing 宏...
         （工具调用：read_file: {"path":"src/main.rs"}）
[3]  工具 › read_file: use tracing::{info, warn, error};
[4]  助手 › 已将 3 处 println! 替换为 tracing::info!

────────────────────────────────────────────────
模型: gpt-4o | Token: 1,234 | 上下文剩余: 99%（126,766/128,000）
> /view 5
[5]  工具 › 完整内容……（不限行数、不限字符）
```

实时消息与历史使用完全相同的消息块：所有消息（含实时新消息）都带灰色序号 `[n]`，assistant 的「工具调用：」标注直接附上具体命令（如 `（工具调用：bash: uname -a）`），`/cls` 重绘后同样可见。**工具结果行同样紧跟工具名直接显示具体命令**（如 `[3] 工具 bash: uname -a`，跳过 `Success: true` 包装行），实时输出、历史回显、`/view` 查看三处一致。

**彩色输出：** 角色标签为彩色背景块——用户蓝底白字、助手绿底白字、工具紫底白字、系统灰底白字；消息序号 `[n]`、工具调用标注（`（工具调用：…）`）、分隔线等辅助信息为灰色，失败内容红色显示。标签采用 24 位真彩色（如 `48;2;37;99;235`），不依赖终端浅色/深色主题，任何背景下均清晰可读。历史展示与实时输出使用完全相同的消息块风格。

**完整历史：** `auv --resume` 启动与 `/resume` 命令恢复会话后**完整打印全部历史**（用户/助手消息不截断，工具结果限 12 行、超出标注省略），靠终端滚动条回看——对标 Claude Code 转录模式与 Codex resume。`/history` 显示全部消息（`/history 5` 只看最近 5 条）；`/view <编号>` 查看单条消息全文。

**REPL 输入体验（rustyline）：**

- 输入行上方常驻**状态行**：`模型: gpt-4o | Token: 1,234 | 上下文剩余: 99%（126,766/128,000）`，模型与上下文窗口按当前配置显示，Token 为全会话累计值（每轮结束后刷新）
- `↑` / `↓`：浏览输入历史（跨会话持久化到 `.harness/repl_history.txt`）
- `←` / `→`：行内移动光标编辑
- `Ctrl+A` / `Ctrl+E`：跳到行首 / 行尾
- `Ctrl+C`：退出 REPL
- `Ctrl+D`：退出 REPL（EOF）

**REPL 内部命令：**

| 输入 | 功能 |
|------|------|
| 任意文本 | 作为任务发送给 agent（以用户消息块进入对话流） |
| `/help` | 显示帮助 |
| `/history` | 显示全部对话消息（`/history 5` 只看最近 5 条） |
| `/view <编号>` | 查看单条消息全文（编号见消息前的 `[n]`） |
| `/cls` | 清屏重绘，清理 `/view`、`/help` 等命令输出（等同重新进入会话） |
| `/clear` | 重置对话历史（同时删除当前会话的自动保存文件） |
| `/save <名称>` | 保存当前会话为命名快照 |
| `/resume` | 恢复会话（不带参数时列出所有会话交互选择，可直接回车或输入 `q` 取消返回原对话） |
| `/resume <名称>` | 恢复指定会话 |
| `/sessions` | 列出所有已保存的会话（当前会话带 `（当前会话）` 标记） |
| `/rename <标题>` | 更改当前会话标题（同步重命名自动保存文件） |
| `/model` | 查看当前模型信息（模型、API 端点、上下文窗口、累计 Token） |
| `/model <名称>` | 切换模型（运行时生效，下轮任务起使用新模型，失败自动回滚） |
| `/approval` | 查看当前审批力度与四档说明 |
| `/approval <无\|低\|中\|高>` | 调整审批力度（运行时生效，下轮任务起按新档位审批） |
| `/skills` | 查看可用技能列表（`.skills` 目录下的技能文件与描述） |
| `/exit` 或 `/quit` | 退出 |
| `Ctrl+C` / `Ctrl+D` | 退出 |

**REPL 特性：**

- **自动保存（模型起名）**：发送第一条任务、本轮运行结束后，由模型根据任务内容生成简短标题（不超过 12 字），会话自动保存到 `.harness/sessions/<标题>.json`；标题生成失败时回退到 `autosave.json`。后续每轮对话自动按当前标题续存。默认启动开启新会话，`auv --resume` 启动时恢复最近修改的会话，无需手动 `/save`。
- **界面重绘**：启动、`/resume`、`/clear` 后自动清屏并重绘界面；清屏同时清空终端滚动缓冲区（`\x1b[3J`），向上滚动不会再看到重复的历史记录；`/resume` 交互选择完成后清理选择列表，不留过期信息。
- **用户消息块**：发送任务后输入行原位替换为彩色用户消息块，任务以对话消息形式留在屏幕与历史中。
- **完整历史**：恢复会话时完整打印全部消息（带灰色序号 `[n]`），工具结果限 12 行并标注 `/view` 提示；`/history` 显示全部，`/view <编号>` 查看单条消息全文。
- **审批提示清除**：护栏审批由 REPL 自身渲染（事件模式，与 TUI 同机制）——审批块独立成行打印（助手消息不再与 `(y/n):` 提示同行），y/n 确认或超时后**全屏重绘**整块从屏幕清除，对话流不被审批信息打断，已打印的助手消息经重绘保留。
- **交互式 /resume**：不带参数时列出所有会话，按编号或名称选择恢复；空输入或 `q` 取消返回原对话，无效编号提示后重新选择。
- **对话记忆**：会话历史在多轮对话中保持（按时间顺序），agent 记得之前做过的事。
- **实时进度**：agent 执行过程中 assistant 消息块实时显示（「工具调用：」标注直接附具体命令，如 `（工具调用：bash: uname -a）`），随后工具结果显示首行；全部实时消息带序号，与历史编号一致。
- **中文界面**：所有系统提示均为中文，护栏审批信息同样本地化（`=== 需要护栏审批 ===`、风险等级、原因、缓解措施）。

### 模式 2：TUI 模式（可视化面板）

```bash
auv run "你的任务"
```

在终端中启动时，默认进入 TUI 模式（除非加了 `--no-tui` 或 stdout 不是终端）。**Agent 运行完毕后 TUI 保持打开**，你可查看完整的工具调用记录和对话历史，按 `q` 退出：

```
┌──────────────────────────────┬───────────────────────┐
│                              │      工具面板           │
│      对话面板 (70%)           │  bash: uname -a        │
│  User: 任务描述               ├───────────────────────┤
│  Assistant: 工具调用...       │      护栏面板          │
│  Tool: 执行结果               │ 按 y 批准 / n 拒绝，   │
│                              │ Esc 或 q 退出          │
├──────────────────────────────┴───────────────────────┤
│ 轮次: 3 | Token: 1234 | 风险: 低 | 模型: gpt-4o | 运行中 │
└──────────────────────────────────────────────────────┘
```

**面板说明：** 状态栏为底部单行（白字深灰底），显示轮次、Token、风险等级、**模型（当前配置的真实模型）**与运行状态；有待审批请求时追加 `待审批: N（按 y 批准 / n 拒绝）`。护栏面板的 y/n 操作提示固定在面板顶部（黄底黑字加粗），审批请求多时也不会被裁掉；风险等级显示为中文（低/中/高/严重）。工具面板展示具体调用命令（如 `bash: uname -a`）。**所有面板颜色均为 24 位真彩色**（不依赖终端浅色/深色主题，浅色终端下同样清晰可读）。

**TUI 快捷键：**

| 按键 | 功能 |
|------|------|
| `q` / `Esc` / `Ctrl+C` | 退出 |
| `y` | 批准护栏请求（决定实时传回护栏管线，危险操作继续执行） |
| `n` | 拒绝护栏请求（决定实时传回护栏管线，操作被拦截） |
| `Enter` | 确认 |
| `Tab` | 切换面板焦点 |

### 模式 3：纯文本模式（脚本/CI/管道）

```bash
auv run --no-tui "运行 cargo test 并修复失败的测试"
```

纯文本输出，不依赖终端能力。适合：
- CI/CD 流水线
- 脚本自动化
- 管道重定向（`auv run "task" > output.txt`）

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
./target/release/auv init

# 3. 开始使用
./target/release/auv
```

---

## 1. 安装与初始化

### 从源码构建

```bash
git clone <repo-url>
cd harnessAgent
cargo build --release
./target/release/auv --help
```

### Docker

```bash
docker build -t auv-harness-agent .
docker run -it auv-harness-agent
```

### 初始化

```bash
auv init
```

交互式引导会：
1. 创建 `./.AuV/config.toml` — 项目局部配置（当前目录为 home 时创建全局配置）
2. 创建 `.memory/` — 记忆存储目录
3. 引导你输入 API Key（隐藏回显）

### 两级配置

AuV 使用「全局 + 项目」两级配置，**启动时自动创建，已存在则绝不改动**：

| 层级 | 路径 | 作用 |
|------|------|------|
| 全局 | `~/.AuV/config.toml` | 用户级默认值（默认模型、默认审批力度等） |
| 项目 | `./.AuV/config.toml` | 项目级覆盖（当前目录为 home 时跳过） |

- **字段级合并**：项目配置写了哪个字段就覆盖哪个，未写的继承全局
- **显式指定**：`auv run -c <路径> "任务"` 只读指定文件，不走分层
- **旧版迁移**：项目根 `config.toml` 不再加载，启动时提示迁移
- **角色说明**：`AuV.md` 用于加载角色说明——项目内按 `AuV.md` → `CLAUDE.md` → `AGENTS.md` 取第一个存在的文件（已有这些文件的项目无需改名）；全局对应 `~/.AuV/AuV.md` → `~/CLAUDE.md` → `~/AGENTS.md`。两级叠加到默认提示词之后，配置内联 `[agent] system_prompt` 优先级最高

---

## 2. 配置 API

### 密钥管理

```bash
auv key status       # 查看已配置哪些 key（不显示明文）
auv key set          # 交互式录入（隐藏回显）
auv key clear <名称>  # 删除存储的 key
```

存储方式：优先 OS 钥匙串（Linux Secret Service / macOS Keychain），不可用时自动降级到 AES-256-GCM 加密文件。

### 设置 API Key 的其他方式

除了 `auv key set`，也可以：

```bash
# 环境变量（优先级低于 ./.AuV/config.toml）
export OPENAI_API_KEY="sk-your-key"
```

```toml
# ./.AuV/config.toml（优先级最高）
[llm]
api_key = "sk-your-key"
```

### 使用兼容 API（DeepSeek / Groq / Ollama / vLLM）

任何兼容 OpenAI Chat Completions 格式的服务都能用，只需改 `./.AuV/config.toml` 或 `~/.AuV/config.toml`：

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
auv                       交互式 REPL（默认新会话）
auv --resume              交互式 REPL（恢复上次自动保存的会话）
auv run "task"            TUI 模式单次任务
auv run --no-tui "task"   纯文本单次任务（CI/脚本）
auv run -c cfg.toml "t"   使用自定义配置
auv init                  初始化配置
auv key status            查看 key 配置
auv key set               录入 key
auv key clear <name>     删除 key
auv --help                查看所有命令
auv --version             查看版本
```

### 示例

```bash
# REPL — 日常编程
auv

# 单次 — 查看并修复 bug
auv run "src/auth.rs 中的登录函数在空密码时 panic，帮我修复"

# 单次 — 重构
auv run --no-tui "把 src/ 下所有的 unwrap() 替换成 ? 操作符"

# 调低审批力度：高风险命令也自动批准（仅严重需审批）
auv --approval none
auv run --approval none "升级项目依赖"

# 用 DeepSeek
auv run -c deepseek.toml "解释一下这个项目的架构"

# 写 Dockerfile 然后构建
auv run "写一个 Dockerfile 用于部署这个 Rust 项目"
```

---

## 4. 配置文件

`auv init` 会在当前目录创建 `./.AuV/config.toml`（cwd 为 home 时创建 `~/.AuV/config.toml`）。所有字段都有默认值，不创建配置文件也能正常运行（首次启动会自动创建两级配置）。

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
approval_level = "low"                       # 审批力度：none/low/medium/high（见护栏章节）
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

Agent 会在 relevant 时自动读取完整 skill 内容。REPL 中用 `/skills` 查看当前已加载的技能列表（名称 + 描述）。

---

## 6. 护栏系统

四层管线确保危险操作被拦截：

| 层级 | 名称 | 机制 | 示例 |
|------|------|------|------|
| L1 | 静态规则 | glob 模式匹配，Allow/Deny/Escalate | `rm -rf /` → 直接 Deny |
| L2 | 风险评估 | 命令/文件/网络三维度打分 | `curl \| bash` → High |
| L3 | 人工审批 | 工具调用风险超过审批力度阈值时暂停，等待 y/n | 审批超时自动拒绝 |
| L4 | 沙箱边界 | 工作目录限制、命令黑白名单 | 写 `/etc/` → 拒绝 |

所有护栏决策写入 `.harness/audit.jsonl`。

### 审批力度（L3 阈值）

`approval_level` 四档（默认「低」），控制「风险等级**低于等于**对应阈值时自动批准命令」：

| 档位 | 自动批准阈值 | 行为 |
|------|-------------|------|
| 无   | 高          | 仅严重（Critical）需审批 |
| 低   | 中          | 高/严重需审批（默认，与历史行为一致） |
| 中   | 低          | 中及以上需审批 |
| 高   | 无          | 所有风险等级的工具调用都需审批 |

- 静态规则 L1 的 Deny 始终硬拦截，不受力度影响；L1 Escalate 把评估等级抬到高，在「无」档下同样自动批准。
- 审批只针对**工具调用**；`final_answer` 等无副作用动作永不审批。
- 三种设置途径（命令行参数优先于配置文件，REPL 指令运行时切换）：
  - 命令行：`auv --approval high`、`auv run --approval none "任务"`（REPL/TUI/CLI 所有模式可用，主值为英文 `none`/`low`/`medium`/`high`，中文「无/低/中/高」为兼容别名）
  - 配置文件：`[guardrails] approval_level = "medium"`（主值英文，中文档位名为兼容别名）
  - REPL：`/approval` 查看、`/approval <无|低|中|高>` 切换（中英文档位名均可，下轮任务生效）

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
├── .AuV/config.toml         # 项目局部配置（可选）
├── rules.md                 # 规则文件（可选）
├── Cargo.toml
└── Dockerfile
```

---

## 8. 开发与测试

```bash
# 运行所有 425 个测试（不依赖网络，全部用 mock LLM）
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

- **LSP 诊断**：降级为解析 `cargo check` 输出，未实现完整 LSP 协议。
- **Linux 钥匙串**：需 `gnome-keyring` 或 `kwallet`，无桌面环境自动降级到加密文件。
- **平台**：主要测试 Linux x86_64；macOS ARM64 可编译但未充分测试。
- **流式输出**：LLM 响应不是流式的，大任务时等待时间较长。
