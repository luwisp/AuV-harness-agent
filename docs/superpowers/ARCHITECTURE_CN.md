# HarnessAgent 架构概述

> 基于 `doc/AI4SE_Final_Project_A_Coding_Agent_Harness.md` 与 `doc/common.md` 的项目要求

---

## 核心等式：Agent = LLM + Harness

LLM 相当于 CPU，只负责"决定下一步做什么"这一步决策。Harness 是其余的全部工程：

- **决策封装**：组织上下文、调用 LLM、解析动作
- **动作/工具**：agent 操作外部世界（读写文件、执行命令、运行测试）
- **上下文与记忆**：决定向模型提供哪些信息，跨会话组织检索
- **治理护栏**：危险动作拦截、人工审批（HITL）、沙箱边界
- **反馈闭环**：客观信号（测试/lint/类型检查）驱动自我修正
- **配置**：声明式规则约束 agent 行为

---

## 模块架构

```
src/
├── main.rs              # CLI 入口（harness / harness run / harness init / harness key）
├── lib.rs               # 库根
├── types.rs             # 核心数据类型（Message, Action, LlmResponse, Role...）
├── error.rs             # 统一错误类型 HarnessError
├── events.rs            # AgentEvent 枚举（agent 到 UI 的通信通道）
├── config/              # 配置系统（HarnessConfig + rules.md + .skills/）
├── llm/                 # LLM 抽象层
│   ├── mod.rs           # LlmProvider trait
│   ├── openai.rs        # OpenAI 兼容实现（DeepSeek, Groq, Ollama...）
│   └── mock.rs          # Mock 实现（离线测试）
├── loop/                # Agent 主循环
│   ├── mod.rs           # AgentLoop 核心
│   ├── parser.rs        # ActionParser（LLM 响应 → Action）
│   └── context.rs       # ContextBuilder（构建每次 LLM 调用的消息上下文）
├── tools/               # 工具系统（6 个内置工具）
│   ├── mod.rs           # Tool trait + ToolRegistry
│   ├── bash.rs          # bash 命令执行
│   ├── file.rs          # read_file / write_file
│   ├── search.rs        # grep / glob
│   ├── git.rs           # git_diff
│   └── test_runner.rs   # run_test
├── guardrails/          # 四层护栏管线
│   ├── mod.rs           # GuardrailPipeline
│   ├── rules.rs         # L1: 静态规则引擎
│   ├── assessor.rs      # L2: 风险评估（命令/文件/网络）
│   ├── approval.rs      # L3: 人工审批门
│   ├── sandbox.rs       # L4: 沙箱边界
│   └── audit.rs         # 审计日志
├── feedback/            # 反馈闭环
│   ├── mod.rs           # FeedbackRunner
│   ├── test_runner.rs   # cargo test
│   ├── type_check.rs    # cargo check
│   └── lint.rs          # cargo clippy
├── memory/              # 记忆系统
│   └── mod.rs           # MemoryStore（文件级 CRUD + 索引）
├── credentials/         # 凭据管理
│   ├── mod.rs           # CredentialManager
│   └── keyring.rs       # OS 钥匙串 + AES-256-GCM 加密文件降级
├── observability/       # 可观测性
│   └── mod.rs           # 追踪日志
└── tui/                 # 终端 UI（ratatui）
    ├── mod.rs           # run_tui / run_cli / run_event_loop
    └── app.rs           # TuiState + 渲染逻辑
```

---

## Agent 主循环

```
run_with_history(task, history)
  │
  ├─ 1. 加载记忆 (MemoryStore)
  ├─ 2. 构建上下文 (ContextBuilder::build)
  │     → [System, ...history, User(task)]
  │
  └─ 3. 主循环 (for turn in 0..max_turns)
       │
       ├─ a. 调用 LLM (llm.complete)
       ├─ b. 解析响应 (ActionParser::parse)
       │     → ToolCall | FinalAnswer | AskUser | NoOp
       ├─ c. 记录 Assistant 消息（保留 tool_calls 用于 API 追踪）
       ├─ d. 护栏检查 (GuardrailPipeline)
       │     → Allowed → 继续
       │     → Denied → 返回错误
       │     → NeedsApproval → 返回错误（TUI 模式走审批流程）
       ├─ e. 检查 FinalAnswer → 返回结果
       ├─ f. 执行工具调用 (ToolRegistry::execute)
       ├─ g. 运行反馈 (FeedbackRunner)
       ├─ h. 注入工具结果（带 tool_call_id）
       └─ i. 停机判断（max_turns / token_budget）
```

---

## 护栏管线

| 层级 | 名称 | 机制 | 示例 |
|------|------|------|------|
| L1 | 静态规则 | glob 模式匹配 → Allow/Deny/Escalate | `rm -rf /` → Deny |
| L2 | 风险评估 | 命令/文件/网络三维度打分 | `curl \| bash` → High |
| L3 | 人工审批 | High 风险暂停，等待 y/n | 审批超时自动拒绝 |
| L4 | 沙箱边界 | 工作目录限制、命令黑白名单 | 写 `/etc/` → 拒绝 |

---

## 关键设计决策

### 1. 四类机制的代码实现（非提示词）

根据 AI4SE 项目要求，以下机制必须是确定性代码，不能是提示词：

- **动作/工具**：`Tool` trait + `ToolRegistry`，每个工具是独立 struct
- **客观反馈信号**：`FeedbackChannel` trait，test/lint/typeck 各自实现
- **危险动作**：`GuardrailPipeline`，命令/文件/网络评估器
- **记忆**：`MemoryStore`，文件级存储 + 索引注入 context

### 2. Mock LLM 可注入

`LlmProvider` trait 允许替换实现：
- `OpenAiProvider` — 真实 API 调用（兼容 OpenAI/DeepSeek/Groq/Ollama）
- `MockLlmProvider` — 预设响应序列，用于 362 个离线测试

### 3. 平台适应

- Linux x86_64（主要测试平台）
- macOS ARM64（可编译，未充分测试）
- Linux 钥匙串需要 `gnome-keyring` 或 `kwallet`，无桌面环境自动降级到 AES-256-GCM 加密文件

---

## 已知限制

- **LSP 诊断**：降级为解析 `cargo check` 输出，未实现完整 LSP 协议
- **流式输出**：LLM 响应非流式，大任务时等待时间较长
- **平台**：主要测试 Linux x86_64
- **DeepSeek 兼容**：对 tool_calls 格式有更严格校验，消息必须保持正确的 tool_call_id 配对且时间顺序排列

---

*最后更新：2025-08-12*
