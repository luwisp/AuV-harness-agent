# SPEC 过程记录

> 记录 SPEC → PLAN → 实现全过程中的验证、迭代与修订。
> 对应文档：[SPEC.md](SPEC.md) ｜ [PLAN.md](PLAN.md) ｜ [AGENT_LOG.md](AGENT_LOG.md) ｜ [REFLECTION.md](REFLECTION.md)

---

## 1. 冷启动试运行验证

### 背景

选择的任务是：

- Task 1：初始化 Cargo 项目与依赖。
- Task 2：定义核心数据类型。

### 暂停并提问的位置

1. PLAN 中 Rust 模块布局存在冲突。
   - PLAN Task 1 Step 5 要求为所有声明模块创建 `src/<module>/mod.rs`，其中包括 `types` 和 `error`。
   - 但 PLAN Task 2 和 Task 3 又分别要求修改 `src/types.rs` 和 `src/error.rs`。
   - 在 Rust 中，同一个模块不应同时由 `src/types.rs` 和 `src/types/mod.rs` 表示；`error` 也一样。
   - 处理方式：暂停并询问是否采用混合模块布局：
     `src/types.rs` / `src/error.rs` 用于叶子模块，后续需要子模块的部分使用 `src/<module>/mod.rs`。
   - 结果：用户确认采用混合布局，并要求顺便修正文档。

### 暴露出的 SPEC / PLAN 缺陷

1. PLAN 中存在可执行步骤级别的 Rust 模块布局冲突。
   - Task 1 的占位模块创建命令会为 `types` 和 `error` 创建目录模块。
   - Task 2 / Task 3 又要求使用同名文件模块。
   - 如果照单执行，后续会出现模块文件歧义，或者迫使实现者自行选择一个 PLAN 未明确说明的布局。

2. PLAN 的 Cargo.toml 示例与其声明的目标产物不完全一致。
   - PLAN 声明 Task 1 会产出 `harness_agent` library crate 和 `harness` binary crate。
   - 但原始 Cargo 示例只有 `[package]` 和 `[dependencies]`，没有显式 `[lib]` 和 `[[bin]]`。
   - 因此需要补充目标声明，确保产物名称与 PLAN 文本一致。

### 已做修订

本次没有修改 `SPEC.md`。确认的问题集中在 `PLAN.md`，因此只修订了 `docs/superpowers/PLAN.md`。

#### PLAN 中的 Cargo 目标名称

```diff
 [package]
 name = "harnessAgent"
 version = "0.1.0"
 edition = "2024"
 
+[lib]
+name = "harness_agent"
+path = "src/lib.rs"
+
+[[bin]]
+name = "harness"
+path = "src/main.rs"
+
 [dependencies]
 tokio = { version = "1", features = ["full"] }
```

#### PLAN 中的模块占位布局

关键 diff：

```diff
+Use Rust's mixed module layout: leaf modules that are implemented directly by early tasks use
+`src/<module>.rs`, while modules that will contain submodules use `src/<module>/mod.rs`.
+
 Create placeholder files for each module declared in lib.rs:
 ```bash
+touch src/types.rs src/error.rs
-for m in types error llm config tools guardrails feedback memory subagent observability credentials tui; do
+for m in llm config tools guardrails feedback memory subagent observability credentials tui; do
   mkdir -p src/$m 2>/dev/null
   echo "// TODO" > src/$m/mod.rs
 done
```

### 对实现的影响

当前实现遵循修订后的 PLAN：

- 叶子模块：`src/types.rs`、`src/error.rs`。
- 后续可能包含子模块的模块：`src/llm/mod.rs`、`src/config/mod.rs`、`src/tools/mod.rs`、
  `src/guardrails/mod.rs`、`src/feedback/mod.rs`、`src/memory/mod.rs`、
  `src/subagent/mod.rs`、`src/observability/mod.rs`、`src/credentials/mod.rs`、
  `src/tui/mod.rs`。

这样 Task 1 和 Task 2 可以在不凭空发明未声明布局的情况下继续推进。

---

## 2. 关键迭代记录（节选）

> 完整的时间线见 [AGENT_LOG.md](AGENT_LOG.md)。本节选取最能体现「发现 → 定位 → 修复 → 验证」闭环的四轮迭代。

### 迭代 A：REPL 对话历史时间顺序（2026-08-12）

**现象**：REPL 多轮对话中，LLM 无法识别前一轮的上下文，回答与历史脱节。

**根因**：`ContextBuilder::build()` 将当前任务插在历史消息之前，产出
`[System, User(task), ...history]`——时间顺序错乱，且 system prompt 与任务被历史隔断。

**修复**：改为严格时间顺序 `[System, ...history, User(task)]`（commit `fdaec41`）；
随后修复了同一轮 assistant 消息被重复追加进历史的问题（commit `213f44b`）。

**验证**：集成测试断言上下文消息序列的完整时间顺序；后续每轮对话回归通过。

**教训**：上下文构造是 LLM 应用最容易出错的一环，必须用集成测试固化「消息序列」这一不变量，而不是只测单轮。

### 迭代 B：护栏管线顺序与反馈回路（2026-08-14）

**现象**：用户在真实会话中感叹工作区状态异常，检查会话记录发现若干次工具调用行为可疑——用户批准了一个操作，该操作随后仍被沙箱拦截；审计记录中风险等级与实际评估不符。

**根因**（经真实会话审计日志定位）：
1. 管线原顺序为「审批 → 沙箱」——用户批准后沙箱才校验，产生"假批准"。
2. 部分路径硬编码风险等级字符串，与实际评估值不一致。
3. `GuardResult::Denied` 直接终止 run，护栏拒绝成为会话死路。

**修复**（commit `8c102ec`）：
1. 管线重排为「静态规则 → 风险评估 → **沙箱硬校验 → 审批**」——参数级违规在审批前直接 Blocked。
2. 每条动作恰好一条审计记录，风险等级取真实评估值。
3. 拒绝原因作为 Tool 消息注入对话，LLM 调整后重试，`max_turns` 兜底。
4. 失败路径也保存会话文件，便于事后诊断。

**验证**：新增回归测试 `test_pipeline_sandbox_blocks_before_approval_single_audit_entry`、
`test_pipeline_approval_single_audit_entry`；机制演示测试更新。

**教训**：护栏的不变量（顺序、单记录、反馈回路）必须显式写成回归测试。**真实会话的审计日志是定位护栏问题的关键证据**——mock 单测只能证明各层独立正确，不能证明管线组合正确。

### 迭代 C：审批交互可靠性（2026-08-13）

**现象**（三连问题，逐个暴露）：
1. 审批读取用 `spawn_blocking` + 阻塞读 stdin，审批超时后线程泄漏并继续累积。
2. 审批期间按 Ctrl+C 直接杀死整个进程（cooked 模式下 SIGINT 默认动作）。
3. 行结束判定只接受 `\n`，raw 环境下回车键不生效。

**根因**：stdin 读取与异步运行时、信号处理的交互未被充分考虑。

**修复**（commit `04684ff` + `dc6052a`）：
1. 审批读取改 termios 非规范模式（RAII 恢复原终端属性）+ `libc::poll(2)` 0 超时探测可读性 + 50ms 轮询 + `libc::read(2)` 读取——不再阻塞线程。
2. 审批循环改 `tokio::select!` 三分支竞争：`ctrl_c()` 监听（视为拒绝）/ 超时 / 轮询读 stdin。
3. 行结束同时接受 `\n` 与 `\r`。

**验证**：pty 集成测试覆盖 poll 状态机、终端模式恢复、EOF 即拒绝；tmux E2E 实测 Ctrl+C 不杀进程。

**教训**：交互可靠性问题**无法被 mock 单测捕获**，必须依赖 pty 测试与 tmux E2E（HOME 隔离 + 假 LLM 服务器 127.0.0.1:18999）双层验证。

### 迭代 D：数据目录统一收纳（2026-08-14）

**背景**：项目数据分散在 `.harness/`（会话、审计、历史）与 `.memory/`（记忆）多个隐藏目录；配置则使用项目根 `config.toml`。用户在会话中指出配置位置有误（应为 `~/.AuV` 与 `./.AuV` 两级），并提出统一收纳数据文件。

**修复**：
1. 配置更正为 `~/.AuV/config.toml`（全局）+ `./.AuV/config.toml`（项目），字段级递归合并，启动幂等创建（commit `29e5270`）。
2. 数据统一收纳到 `.AuV/`：`sessions/`、`repl_history.txt`、`audit.jsonl`、`memory/`（commit `227e1d5`）；既有 `.harness` 真实会话文件一次性迁移，**迁移过程保留用户真实数据**。

**验证**：迁移后启动、`--resume`、会话保存、审计写入全部回归通过。

**教训**：目录结构属于「用户可见的接口」，设计时应当由用户确认；AI 单方面选定的布局（`.harness`/`.memory`）在真实使用中被推翻。

---

## 3. AI 建议采纳与推翻记录

### 3.1 被采纳的建议

| 建议 | 提出时机 | 采纳方式 |
|------|---------|---------|
| 混合模块布局（叶子模块 `src/x.rs`，含子模块用 `src/x/mod.rs`） | 冷启动验证发现 PLAN 冲突后提问 | 用户确认 |
| 全链路 trait 依赖注入 + mock LLM 确定性测试（离线测试基石） | 设计阶段 | 进入 SPEC 架构 |
| 两级配置（`~/.AuV` + `./.AuV`）+ AuV.md 角色说明两级检测 | 配置设计阶段 | 经 spec 流程后实现 |
| 护栏管线重排：沙箱硬校验先于审批（消除假批准） | 审计日志定位后提出 | 实现并固化回归测试 |
| 审批力度四档（无/低/中/高），仅工具调用触发审批 | 审批体系完善阶段 | 进入 SPEC §3.4 |
| 数据统一收纳到 `.AuV/` 单一目录 | 用户提出收纳需求后设计 | 用户确认（「确认」） |
| 英文主值 + 中文别名的审批档位参数（兼容与本地化兼顾） | 序列化兼容性考虑 | 实现（CLI/配置/序列化三处统一） |
| 机制演示测试（`tests/mechanism_demo.rs`）三项确定性演示 | 课程交付要求 | 实现并纳入评估 |

### 3.2 被推翻或失败的方案

| 方案 | 推翻者 | 原因与替代 |
|------|--------|-----------|
| 配置目录沿用 `.harness` / 项目根 `config.toml` | **用户** | 用户指出应为 `~/.AuV` 与 `./.AuV` 两级；替代方案见迭代 D |
| 移动/改写 `.harness/sessions/` 中的既有会话文件 | **用户** | 用户明确这些是真实会话数据，不可破坏；迁移改为原样保留 |
| DECSC/DECRC 光标保存/恢复来清除审批提示与状态行 | **实践测试** | tmux 实测滚动与并发输出下不可靠，且与 rustyline 刷新模型互斥；废弃，改为多行提示符布局 + 全屏重绘，并以回归测试禁止该转义序列 |
| 审批「先审批后沙箱」的管线顺序 | **实践验证** | 用户批准后仍被沙箱拦截（假批准）；重排为沙箱先于审批（迭代 B） |
| OS 钥匙串（keyring）作为凭据主方案 | **环境现实** | 无桌面环境可用性差；交付版改为两级配置文件 `[llm] api_key` 为主、环境变量备选，keyring 预留未启用 |
| 二进制名 `harness` | **项目更名** | 项目更名 AuV，二进制改 `auv`，lib/包名与内部类型名保持 |
| 状态行用光标舞步画到输入行下方 | **实践测试** | rustyline 按自身宽度模型重定位光标，回显漂移与输出覆盖；改为多行提示符置于输入行上方 |

### 3.3 观察

- **被用户推翻的方案集中在「用户可见的接口」**：目录结构、数据文件、配置位置。这些决策 AI 依据技术惯性（沿用已有约定）做出，但用户对自身工作流有明确预期。
- **被实践推翻的方案集中在「终端交互」**：光标舞步、管线顺序、阻塞读取。这些只有在真实终端环境（tmux）与真实会话中才会暴露。
- 两者的共同教训：**mock 测试与设计推演无法替代真实环境验证**；越接近用户界面的决策，越应尽早让用户确认。

---

## 4. 过程反思（方法论总结）

> 个人反思正文（课程要求的 REFLECTION.md 主体）由项目作者本人撰写，本节仅记录可供反思引用的过程事实。

1. **冷启动验证价值显著**：PLAN 在进入实现前就暴露出模块布局与 Cargo 目标两处可执行步骤级缺陷，避免了实现中途被迫做未声明的布局选择。SPEC/PLAN 评审不应只查「是否写全」，还应查「步骤之间是否冲突」。

2. **重点维度（护栏）的验证深度与普通模块不同**：护栏不仅需要单元测试（每层独立正确），还需要管线级回归测试（组合顺序、审计单记录、反馈回路三个不变量）。真实会话审计日志是管线缺陷的关键证据来源。

3. **交互可靠性需要双层验证**：pty 集成测试（信号、终端模式、EOF）+ tmux E2E（HOME 隔离 + 假 LLM 服务器）。本项目中 mock 单测全部通过而真实使用仍出问题的案例（审批阻塞、Ctrl+C 杀进程、最终消息丢失）全部属于此类。

4. **文档与实现的同步成本不可忽视**：REPL/TUI/审批/配置经历了大量迭代，SPEC.md 在实现收尾时做了一次全面同步（commit `ed16ad7`）。教训：机制稳定后应尽早回写 SPEC，而不是积累到收尾。

5. **用户指正是高价值信号**：配置目录、真实数据保护两处关键纠正都来自用户对自身工作流的了解。AI 应把「用户可见接口」的设计决策提前暴露给用户确认，而不是实现后等待纠正。
