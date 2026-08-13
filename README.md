# AuV harness agent — Coding Agent Harness

AuV 是一个用 Rust 从零实现的编码智能体执行框架（Coding Agent Harness）：主循环、工具分发、四层治理护栏、反馈闭环、记忆与子 agent 委派均为自研 harness 内核，不依赖任何现成 agent 框架。全部核心机制由 mock LLM 驱动、可离线确定性测试（当前 478 项测试全通过、零编译警告），并配有一键复现的机制演示。支持交互式 REPL、可视化 TUI、纯文本三种运行方式。

**核心机制速览**

| 机制 | 说明 |
|------|------|
| 四层护栏 | 静态规则 → 风险评估 → 人工审批 → 沙箱边界，危险操作在工具执行前被拦截，全部决策写入审计日志 |
| 反馈闭环 | 工具产出经测试/静态检查校验，失败结果注入下一轮上下文，驱动 agent 自我修正 |
| 子 agent 委派 | `subagent` 工具把任务委派给独立上下文的子 agent，深度传播限制递归，审批路由回父界面 |
| 规则与技能 | `rules.md` 声明式规则注入 system prompt；`.skills/` 技能按需加载 |
| 记忆 | 文件级持久记忆，多轮对话保持上下文 |
| 凭据安全 | OS 钥匙串 / AES-256-GCM 加密文件 / secret file 三种存储，密钥绝不明文落盘 |

## 目录

- [快速开始](#快速开始)
- [课程交付物对应位置](#课程交付物对应位置)
- [三种运行模式](#三种运行模式)
- [配置](#配置)
- [核心机制](#核心机制)
- [机制演示](#机制演示)
- [目录结构](#目录结构)
- [开发与测试](#开发与测试)
- [CI/CD 与发布](#cicd-与发布)
- [已知限制](#已知限制)

---

## 快速开始

### 前置条件

- Rust 1.88+（edition 2024；当前锁文件中的依赖要求至少 1.88）
- 一个 OpenAI 兼容 API 的 key（OpenAI、DeepSeek 等均可）

### 三步启动

```bash
# 1. 编译
cargo build --release

# 2. 初始化（首次运行：创建配置 + 引导录入 key，隐藏回显）
./target/release/auv init

# 3. 开始使用（进入交互式 REPL）
./target/release/auv
```

单次任务也可以直接一句话：

```bash
./target/release/auv run "运行 cargo test 并修复失败的测试"
```

### 获取方式（三选一）

**从源码构建**

```bash
git clone git@github.com:luwisp/AuV-harness-agent.git
cd AuV-harness-agent
cargo build --release --locked
./target/release/auv --help
```

**Docker**（本地构建或 GHCR 公开镜像；`main` 随主分支更新，`latest` 随 `v*` Release 标签更新）：

```bash
docker build -t auv-harness-agent .
docker run --rm -it \
  --mount type=bind,src="$PWD",dst=/workspace \
  auv-harness-agent
```

容器中推荐用只读 secret file 提供 key（不把 key 写进镜像、命令行或进程环境）：

```bash
mkdir -p "$HOME/.config/auv" && chmod 700 "$HOME/.config/auv"
touch "$HOME/.config/auv/openai-api-key" && chmod 600 "$HOME/.config/auv/openai-api-key"
# 用编辑器把 key 写入上面的文件，不要在命令行中直接写 key。

docker run --rm -it \
  --mount type=bind,src="$PWD",dst=/workspace \
  --mount type=bind,src="$HOME/.config/auv/openai-api-key",dst=/run/secrets/openai_api_key,readonly \
  -e OPENAI_API_KEY_FILE=/run/secrets/openai_api_key \
  auv-harness-agent
```

访问宿主机 Ollama 时，Linux Docker 还需加 `--network host`；Docker Desktop 请把 `base_url` 中的 `localhost` 改为 `host.docker.internal`。

**GitHub Release 二进制**：推送 `v*` 标签后，发布流水线会在 [GitHub Releases](https://github.com/luwisp/AuV-harness-agent/releases) 生成 Linux x86_64 GNU 二进制及 SHA-256 校验文件（未做代码签名）：

```bash
sha256sum -c auv-v*-SHA256SUMS
chmod +x auv-v*-x86_64-unknown-linux-gnu
```

---

## 课程交付物对应位置

AI4SE 期末项目提交清单（课程通用要求 §五 + A 类项目额外要求）与本仓库文件的对应关系：

| 交付物 | 仓库位置 |
|--------|---------|
| 1. `SPEC.md`、`PLAN.md`、`SPEC_PROCESS.md` | `docs/SPEC.md`、`docs/PLAN.md`、`docs/SPEC_PROCESS.md` |
| 2. 完整源代码（自研 harness 内核 + mock-LLM 单测） | `src/`（内核）、`tests/`（集成测试）、`src/llm/mock.rs`（mock LLM） |
| 3. 分发产物与说明 | `Dockerfile`；获取、运行、key 安全配置、已知限制见本 README |
| 4. `README.md` | `README.md`（本文件） |
| 5. `AGENT_LOG.md` | `docs/AGENT_LOG.md` |
| 6. CI 配置（含 `unit-test` job） | `.gitlab-ci.yml`（另有 `.github/workflows/ci.yml`） |
| 7. CI/CD 执行记录（最后一次须为 pass） | GitHub Actions：https://github.com/luwisp/AuV-harness-agent/actions |
| 8. `REFLECTION.md`（1500–2500 字反思报告） | `docs/REFLECTION.md` |
| 9. 线上部署 URL（WebUI 接口） | 不适用：本项目为 CLI/TUI 形态，SPEC 决策已移除 Web 仪表盘 |
| 机制演示（A 类项目额外要求 §A.6） | `tests/mechanism_demo.rs`（四项，运行方式见[机制演示](#机制演示)） |

课程项目要求原文（通用要求 + A 类项目说明）位于 `doc/` 目录。

---

## 三种运行模式

| 模式 | 命令 | 适用场景 |
|------|------|---------|
| 交互式 REPL | `auv` | 日常使用，多轮对话，持续编码 |
| 管道TUI 可视化面板 | `auv run "任务"` | 单次任务，可视化查看工具调用与审批 |
| 管道纯文本 | `auv run --no-tui "任务"` | CI/脚本/管道，不需终端能力 |

### 模式 1：交互式 REPL（推荐日常使用）

```bash
auv              # 默认开启新会话
auv --resume     # 恢复最近自动保存的会话（完整打印历史）
```

进入类似 Claude Code 的交互式对话循环，输入任务即开始；输入行上方常驻状态行（模型 / Token / 上下文剩余）。**常用命令：**

| 输入 | 功能 |
|------|------|
| `/help` | 显示帮助 |
| `/history [N]` | 显示全部对话消息（带 N 只看最近 N 条） |
| `/view <编号>` | 查看单条消息全文（编号见消息前的 `[n]`） |
| `/cls` | 清屏重绘 |
| `/clear` | 重置对话历史 |
| `/save <名称>` / `/resume [名称]` / `/sessions` | 会话快照保存 / 恢复 / 列表 |
| `/rename <标题>` | 更改当前会话标题 |
| `/model [名称]` | 查看 / 切换模型（运行时生效） |
| `/approval [无\|低\|中\|高]` | 查看 / 调整审批力度（运行时生效） |
| `/skills` | 查看可用技能列表 |
| `/exit` / `Ctrl+C` / `Ctrl+D` | 退出 |

主要特性：

- **自动保存**：首轮任务结束后由模型生成标题（不超过 12 字），会话自动保存到 `.AuV/sessions/`；后续每轮自动续存
- **对话记忆**：多轮对话保持完整历史，agent 记得之前做过的事
- **实时进度**：assistant 消息实时显示，工具调用标注直接附具体命令（如 `（工具调用：bash: uname -a）`）；全部消息带序号，与历史一致
- **审批**：护栏审批由 REPL 自身渲染（中文审批块：风险等级 / 原因 / 缓解措施），y/n 确认或超时后整块清除，对话流不被打断
- **中文界面**：所有系统提示与护栏信息均为中文
- 编辑体验：`↑`/`↓` 浏览历史输入（持久化到 `.AuV/repl_history.txt`），`←`/`→` 移动光标，`Ctrl+A`/`Ctrl+E` 跳行首/行尾

### 模式 2：管道TUI 可视化面板

```bash
auv run "你的任务"
```

终端中默认进入 TUI（除非加 `--no-tui` 或 stdout 不是终端）。**运行完毕后 TUI 保持打开**，可查看完整工具调用记录与对话历史。左侧对话面板 + 右侧工具面板 / 护栏面板；底部状态栏显示轮次、Token、风险等级、模型、运行状态，有待审批请求时追加 `待审批: N（按 y 批准 / n 拒绝）`。

| 按键 | 功能 |
|------|------|
| `y` / `n` | 批准 / 拒绝护栏请求（决定实时传回护栏管线） |
| `q` / `Esc` / `Ctrl+C` | 退出 |
| `Tab` | 切换面板焦点 |

### 模式 3：纯文本（脚本 / CI / 管道）

```bash
auv run --no-tui "运行 cargo test 并修复失败的测试"
auv run "task" > output.txt
```

---

## 配置

### 录入 API Key

```bash
auv key status       # 查看已配置哪些 key（不显示明文）
auv key set          # 交互式录入（隐藏回显）
auv key clear <名称>  # 删除存储的 key
```

存储方式：优先 OS 钥匙串（Linux Secret Service / macOS Keychain），不可用时自动降级到权限 `0600` 的 AES-256-GCM 加密文件（密钥由机器标识派生）。录入时 key 名称请使用 `OPENAI_API_KEY`。Agent 的读取优先级：配置文件 `[llm] api_key` → `OPENAI_API_KEY_FILE` → `OPENAI_API_KEY` → 安全存储。

### 两级配置

启动时自动创建、已存在则绝不改动；项目配置字段级覆盖全局配置：

| 层级 | 路径 | 作用 |
|------|------|------|
| 全局 | `~/.AuV/config.toml` | 用户级默认值 |
| 项目 | `./.AuV/config.toml` | 项目级覆盖（cwd 为 home 时跳过） |

### 使用兼容 API（DeepSeek / Groq / Ollama / vLLM）

任何兼容 OpenAI Chat Completions 格式的服务都能用，只需改配置文件：

| 服务 | `[llm] model` | `[llm] base_url` | 备注 |
|------|--------------|------------------|------|
| DeepSeek | `deepseek-chat` | `https://api.deepseek.com/v1` | 需真实 key |
| Groq | `llama-3.1-70b-versatile` | `https://api.groq.com/openai/v1` | 需真实 key |
| Ollama（本地） | `llama3.1` | `http://localhost:11434/v1` | key 填任意值即可 |

### 配置文件参考

`auv init` 会创建 `./.AuV/config.toml`；所有字段都有默认值。常用字段：

```toml
[llm]
model = "gpt-4o"                             # 模型名
base_url = "https://api.openai.com/v1"       # API 地址（兼容服务改这里）
api_key = "sk-..."                           # 可选，也可用环境变量 / 安全存储
timeout_secs = 120

[agent]
max_turns = 50                               # 最大循环轮次
system_prompt = ""                           # 自定义 system prompt（可选）

[guardrails]
approval_timeout_secs = 120                  # 审批超时（超时自动拒绝）
approval_level = "low"                       # none/low/medium/high（见核心机制）
audit_log_path = ".AuV/audit.jsonl"          # 审计日志路径

[sandbox]
network_allowed = true                       # 允许网络访问
max_timeout_secs = 300                       # 命令执行超时
forbidden_commands = ["rm -rf /", "sudo", "mkfs"]

[tools]
disabled_tools = []                          # 禁用工具（如 ["bash"]）

[memory]
storage_path = ".AuV/memory"                 # 记忆存储路径

[feedback]
max_retries = 3                              # 最多自我修正轮次

[subagent]
max_depth = 3                                # 子 agent 递归深度上限
max_total_agents = 10                        # 同时活跃子 agent 数上限
```

---

## 核心机制

### 四层护栏管线

| 层级 | 名称 | 机制 | 示例 |
|------|------|------|------|
| L1 | 静态规则 | glob 模式匹配，Allow/Deny/Escalate | `rm -rf /` → 直接 Deny |
| L2 | 风险评估 | 命令/文件/网络三维度打分 | `curl \| bash` → High |
| L3 | 人工审批 | 风险超过审批力度阈值时暂停，等待 y/n | 审批超时自动拒绝 |
| L4 | 沙箱边界 | 工作目录限制、命令黑白名单 | 写 `/etc/` → 拒绝 |

所有护栏决策写入 `.AuV/audit.jsonl`。审批力度四档（默认「低」）控制「风险等级**低于等于**对应阈值时自动批准命令」：

| 档位 | 自动批准阈值 | 行为 |
|------|-------------|------|
| 无 | 高 | 仅严重（Critical）需审批 |
| 低 | 中 | 高/严重需审批（默认） |
| 中 | 低 | 中及以上需审批 |
| 高 | 无 | 所有风险等级的工具调用都需审批 |

L1 的 Deny 始终硬拦截，不受力度影响；审批只针对工具调用。三种设置途径：命令行 `--approval <none|low|medium|high>`（中文「无/低/中/高」为兼容别名）、配置文件 `[guardrails] approval_level`、REPL `/approval` 指令。

### 反馈闭环

工具产出经测试 / 静态检查校验，失败结果（文件、行号、错误信息）注入下一轮 LLM 上下文，驱动 agent 自我修正，最多 `max_retries` 轮。Demo 2 演示了这一过程（写 bug 代码 → 反馈失败 → 修正）。

### 子 agent 委派

`subagent` 工具把任务委派给独立对话上下文与独立工具集的子 agent，只返回摘要给主循环。子 agent 递归深度受 `max_depth` 约束、活跃数量受 `max_total_agents` 约束；审批请求路由回父界面（REPL 审批块标注「子 agent」，TUI 护栏面板标注来源）。隔离模式当前仅 SameProcess（子 loop 在子线程运行）；Worktree 为预留项，调用返回明确错误。

### 规则与技能

`rules.md` 一行一条声明式约束，注入 system prompt：

```markdown
# Coding Rules
- 修改代码后必须运行 cargo test
- 禁止使用 unwrap() 和 expect()
```

`.skills/` 目录下带 frontmatter 的技能文件按需加载（命中时读全文），REPL `/skills` 查看列表。

---

## 机制演示

四项确定性演示（mock LLM 驱动、离线运行、约 0.00s 完成），覆盖课程 A 类项目 §A.6 要求的机制演示：

```bash
# 运行全部四项（--nocapture 显示演示横幅）
cargo test --test mechanism_demo -- --nocapture

# 运行单项（逐个展示更清晰）
cargo test --test mechanism_demo demo_guardrail_intercepts_dangerous_action -- --nocapture
cargo test --test mechanism_demo demo_feedback_loop_drives_correction -- --nocapture
cargo test --test mechanism_demo demo_guardrail_pipeline_full_flow -- --nocapture
cargo test --test mechanism_demo demo_subagent_delegation_aggregates_result -- --nocapture
```

| 演示 | 内容 |
|------|------|
| Demo 1 | 护栏拦截危险动作：`rm -rf /` 在工具执行前被 L1 静态规则拒绝 |
| Demo 2 | 反馈闭环驱动自我修正：写 bug 代码 → 反馈失败 → 注入上下文 → 修正 |
| Demo 3 | 护栏管线全流程：四层逐层验证 + `curl \| bash` 走完 L1→L2→L3 超时否决 |
| Demo 4 | 子 agent 委派：父 agent 委派「计算 2+2」→ 子 agent 独立上下文算出结果 → 父汇总 |

---

## 目录结构

```
harnessAgent/
├── src/
│   ├── main.rs              # CLI 入口（子命令 + REPL）
│   ├── lib.rs               # 库根
│   ├── types.rs             # 核心数据类型
│   ├── llm/                 # LLM 抽象层（trait + openai + mock）
│   ├── config/              # 配置（HarnessConfig + rules + skills）
│   ├── loop/                # Agent 主循环（AgentLoop + parser + context）
│   ├── tools/               # 工具系统（9 个内置工具，含 subagent 委派）
│   ├── guardrails/          # 护栏系统（四层管线）
│   ├── feedback/            # 反馈闭环（test/lint/typeck）
│   ├── memory/              # 记忆系统（文件级）
│   ├── credentials/         # 凭据管理（keyring + 加密文件）
│   ├── observability/       # 可观测性（trace log）
│   ├── subagent/            # 子 Agent 派发（深度传播 + AgentLoopRunner + 审批路由）
│   └── tui/                 # 终端 UI（ratatui）
├── tests/mechanism_demo.rs  # 机制演示测试（4 项）
├── docs/                    # 设计文档与计划（SPEC/PLAN/AGENT_LOG/REFLECTION 等）
├── doc/                     # 课程项目要求原文（通用要求 + A 类项目说明）
├── .AuV/config.toml         # 项目局部配置（可选）
├── rules.md                 # 规则文件（可选）
├── Cargo.toml
└── Dockerfile
```

---

## 开发与测试

```bash
# 全部确定性测试（不依赖真实 LLM API）
cargo test --all-targets --locked -- --test-threads=1
cargo test --doc --locked

# 机制演示（带横幅输出）
cargo test --test mechanism_demo -- --nocapture

# 编译 release
cargo build --release
```

---

## CI/CD 与发布

- `.github/workflows/ci.yml` 在每次 push、pull request 和手动触发时运行全目标编译检查、Clippy、全部测试、release 构建与 Docker 构建/烟雾测试。Clippy 当前作为建议项输出既有 lint 债务，不使用 `-D warnings` 阻断交付。
- CI 通过后可下载 `auv-linux-x86_64-gnu` 构建产物，保留 14 天。
- `.github/workflows/publish.yml` 先重跑确定性测试，再在 `master`/`main` 更新时推送 GHCR 分支标签；`v*` 标签会额外推送语义版本与 `latest`，并创建 GitHub Release 二进制。
- `.gitlab-ci.yml` 提供课程交付要求的 `unit-test` job 与 release 构建产物。

首次发布 GHCR 包后，需要在 GitHub package settings 中将 package visibility 设为 Public，公开拉取命令才无需登录。

---

## 已知限制

- **LSP 诊断**：降级为解析 `cargo check` 输出，未实现完整 LSP 协议。
- **Linux 钥匙串**：需 `gnome-keyring` 或 `kwallet`，无桌面环境自动降级到加密文件。
- **加密文件回退**：使用 machine-id 派生密钥且文件权限为 `0600`，主要防止意外明文泄漏，不等价于带独立主密码的凭据库。
- **平台**：主要测试 Linux x86_64；macOS ARM64 可编译但未充分测试。
- **容器平台**：发布流水线当前只构建 `linux/amd64`；运行镜像约 45 MB，不包含 Rust 或其他项目语言工具链。Agent 可执行 shell/git 与基础文件操作，但 `cargo test/check/clippy` 反馈需要在自定义派生镜像中安装 Rust，或使用原生二进制运行。
- **容器凭据**：容器无桌面 Secret Service，推荐只读挂载 secret file；把 `OPENAI_API_KEY` 直接写在 `docker run -e` 后会进入 shell history，应避免。
- **二进制签名**：GitHub Release 的 Linux 二进制当前未签名，只提供 SHA-256 校验文件。
- **自定义 API 传输**：默认远端端点使用 HTTPS；为兼容本地 Ollama/vLLM，当前不会禁止自定义 HTTP URL。非回环地址应只配置 HTTPS。
- **流式输出**：LLM 响应不是流式的，大任务时等待时间较长。
- **子 agent**：REPL 在子 agent 运行期间无法刷新界面（工具执行同步阻塞）；审批超时后子线程无法强制终止（后台跑完、结果丢弃）；Worktree 隔离未实现。
