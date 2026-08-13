# AGENT_LOG（Agent 工作日志）

> AI4SE 课程交付物：AuV harness agent 项目开发全过程的 Agent 工作日志。
> 记录时间：2026-07-08 至 2026-08-14（SPEC 制定 → 核心实现 → REPL/TUI/护栏迭代 → 数据收纳）。
> 对应文档：[SPEC.md](SPEC.md) ｜ [PLAN.md](PLAN.md) ｜ [SPEC_PROCESS.md](SPEC_PROCESS.md) ｜ [REFLECTION.md](REFLECTION.md)

---

## 总览

| 日期 | 阶段 | 关键提交 |
|------|------|---------|
| 2026-07-08/09 | SPEC → PLAN → 核心实现（32 任务落地） | `a289051` … `8f2e55b`、`c55ab48` |
| 2026-08-06 | 评估修复 + REPL 初版 | `35a1cdc`、`d34516d` |
| 2026-08-12 | REPL 对话修复与输入体验 | `fdaec41`、`213f44b`、`19cfc36` |
| 2026-08-13 | REPL/审批/护栏/配置密集迭代（11 轮） | `dc6052a`、`efa9f4d`、`04684ff`、`a30bba0`、`29e5270` |
| 2026-08-14 | 护栏管线修复 + 数据目录统一收纳 | `8c102ec`、`21f50d5`、`227e1d5` |
| 2026-08-14 | subagent 孤岛模块生产接入（10 步计划） | `980b593`、`16db698`、`a2937e8`、`b21f8b5`、`300710b`、`dc3f0da`、`e68e2b2`、`143745d`、`d9e4283` |

测试数量演变：362 → 363 → 368 → 375 → 378 → 384 → 396 → 420 → 425 → 427 → 443 → 469 → 478。

---

## 2026-07-08 至 07-09：SPEC → PLAN → 核心实现

### 条目 1｜2026-07-08 设计文档与实施计划

**任务**：制定设计文档（SPEC）与 32 任务/14 阶段实施计划（PLAN），含决策修订：TUI 提升为核心特性、移除 Web 仪表盘与本地模型 Stretch Goals、主 LLM 供应商切换为 OpenAI 兼容（Anthropic 预留）。

**提交**：`a289051`（设计文档）、`89f6c82`（TUI 核心化）、`15fc2ee`（供应商决策）、`c3097b5`（32 任务实施计划）

### 条目 2｜2026-07-08 冷启动试运行验证

**任务**：执行 PLAN 的冷启动验证（Task 1-2），发现两处步骤级缺陷：模块布局冲突（`src/types/mod.rs` 与 `src/types.rs` 冲突）与 Cargo 目标声明缺失。暂停向用户提问，确认混合模块布局方案后修订 PLAN。

**产出**：`docs/superpowers/SPEC_PROCESS.md` 冷启动验证记录。

### 条目 3｜2026-07-09 阶段 1-14 全部落地

**任务**：按修订后的 PLAN 完成全部 32 个任务：核心类型与错误 → LLM 抽象层（trait + mock）→ 配置 → 工具系统（7 个工具）→ 护栏四层（静态规则/风险评估/审批/沙箱/审计）→ 反馈闭环 → 记忆 → Agent 主循环 → 可观测性 → 子 Agent → 凭据 → TUI → CLI → Docker/CI → 机制演示。

**关键决策落地**：全链路 trait 依赖注入；mock LLM 确定性测试贯穿所有机制。

**验证**：362 个测试全部通过，零编译警告。

**提交**：`3b665ea`、`3939700`、`334300b`、`df8818e`、`41808f6`、`1966818`、`64d55be`、`d336272`、`e0f2f1a`、`81a395b`、`368f72f`、`52493ab`、`4049924`、`e63249e`、`3d45dee`、`5744502`、`7db8c06`、`ff8e33a`、`83eadc5`、`8f2e55b`（警告清零与批量落地）、`36d9c19`、`c55ab48`（合并）、`bb43ed8`（中文使用指南）。任务级映射见 [PLAN.md](PLAN.md) §4。

---

## 2026-08-06：评估修复与 REPL 初版

### 条目 4｜2026-08-06 评估发现的关键生产问题修复

**任务**：修复评估暴露的生产装配缺陷（commit `35a1cdc`）：
- `run_cli` 移除嵌套 tokio runtime，改为 async
- `CredentialManager` 的 `key_set`/`key_clear` 异步化，启用 KeyringCredentialBackend
- `LlmProvider::complete` 接受 tools 参数并随请求发送到 OpenAI API
- `ActionParser` 增加文本式工具调用解析（XML 与 LangChain 风格）
- 审计日志创建父目录，错误返回而非静默吞掉
- 生产装配补齐：内置规则、全部 3 个评估器、全部 7 个工具（write_file、glob）、全部 3 个反馈通道、规则/技能/记忆索引
- `ContextBuilder` 增加技能片段字段
- 安全：默认拒绝危险命令（sudo、rm -rf / 等）
- `.gitignore` 增加 `.env`、`.memory/`、`.harness/`、`config.toml`

**验证**：362 个测试通过（327 lib + 28 bin + 3 demo + 4 doctest）。

### 条目 5｜2026-08-06 REPL 初版

**任务**：实现交互式 REPL 模式（commit `d34516d`）：CLI 无子命令进入 REPL；`run_repl` 交互循环；`/exit`、`/quit`、`/help`、Ctrl+D 命令；agent 跨轮复用、保留对话状态。

**验证**：362 个测试全部通过。

---

## 2026-08-12：REPL 对话修复与输入体验

### 条目 6｜2026-08-12 对话历史失效（无上下文记忆）

**现象**：Agent 在多轮对话中无记忆，每次都表现为"第一次对话"。

**根因**：`ContextBuilder::build()` 将当前用户任务放在历史消息**之前**（`[System, User(new), Assistant(old), Tool(old)]`），时间顺序错乱；且 REPL 对话累积逻辑丢弃 `User` 消息，LLM 看不到完整对话流程。

**修复**（commit `fdaec41`）：改为严格时间顺序 `[System, ...history, User(task)]`；REPL 对话累积包含所有非 System 消息（含 User 消息）；顺带修复凭据列表与 tool_call_id 跟踪。

### 条目 7｜2026-08-12 重复助手消息

**现象**：REPL 对话历史中助手消息被重复追加。

**修复**（commit `213f44b`）：追加前去重，历史序列保持唯一。

### 条目 8｜2026-08-12 DeepSeek tool_calls 400 错误

**现象**：`HTTP 400: "An assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'"`。

**根因**：消息时间顺序错乱导致 Assistant(tool_calls) 与 Tool(tool_call_id) 配对校验失败。

**修复**：消息时间排序修正后 API 合法性约束自然满足；`OpenAiProvider` 在非 2xx 响应时记录完整请求体（`tracing::debug` 级别）便于排查。

### 条目 9｜2026-08-12 会话持久化与界面专业化

**实现**：
- 新增 `/save`、`/resume`、`/sessions` 命令（会话序列化到 `.harness/sessions/<名称>.json`，serde_json 完整对话历史）
- 界面专业化：移除全部 emoji，替换为文本标签（`⏳ Running` → `[Running]`、`🔧 Calling` → `[call]`、`✅/❌` → `[ok]/[FAIL]`、角色标签 `🤖/🔧` → `ASSIST/TOOL`）
- 新增中文 REPL 实现方案文档 `PLAN_REPL_CN.md` 与中文架构概述 `ARCHITECTURE_CN.md`

**验证**：362 个测试持续通过，零编译警告。

### 条目 10｜2026-08-12 REPL 输入体验升级

**实现**：
- rustyline 替换裸 `read_line`：`↑`/`↓` 历史导航（持久化 `.harness/repl_history.txt`）、行内光标编辑、`Ctrl+A`/`Ctrl+E` 行首/行尾、Ctrl+C 取消当前输入
- 输入区灰色分隔线（多行提示符一部分）
- 中文界面全量：横幅、帮助、状态、错误、角色标签、工具调用状态
- 自动保存：每轮后存 `.harness/sessions/autosave.json`，启动自动恢复；`/clear` 同时删除自动保存；`/sessions` 标注「（自动保存）」；`/save <名称>` 命名快照
- 移除冗余 `[Running]` 回显行

**提交**：本日 README 同步重写（commit `19cfc36`）。

---

## 2026-08-13：REPL/审批/护栏/配置密集迭代

### 条目 11｜2026-08-13 工具调用 400 根治 + 护栏中文化

**现象**：工具执行成功后下一次 LLM 调用返回 HTTP 400（insufficient tool messages）。

**根因**：LLM 一次返回**多个** tool_calls 时，`ActionParser` 只解析执行第一个，但 assistant 消息保存了**全部** tool_calls——未被执行的调用没有对应 tool 消息，违反 API 配对约束。

**修复**：assistant 消息只保存**实际被执行**的 tool_call（按解析出的 action id 过滤），配对不变量成立；新增回归测试 `test_agent_loop_multiple_tool_calls_keep_pairing`。同步完成护栏中文化：审批提示、风险等级（低/中/高/严重）、三类评估器原因与缓解措施、沙箱违规、内置规则名称与拦截原因、超时/拒绝错误全部中文。另：Ctrl+C 改为直接退出 REPL；启动恢复会话后展示最近 5 条消息；`/resume` 不带参数时列出会话交互选择。

**验证**：363 个测试通过。

### 条目 12｜2026-08-13 DeepSeek 思考模式 400 + REPL 界面重设计 + TUI 审批打通

**现象**：工具执行后的下一轮调用返回 HTTP 400：`"The 'reasoning_content' in the thinking mode must be passed back to the API."`

**根因**：DeepSeek 思考模式要求 `reasoning_content` 在后续请求的对应 assistant 消息中原样回传，而数据模型无此字段，解析后即丢弃。

**修复**：`Message`/`LlmResponse` 增加 `reasoning_content: Option<String>`（serde 向后兼容，旧会话无缝加载）；`OpenAiProvider` 解析并回传；主循环保存到 assistant 消息；wiremock 线格式测试三断言。同步完成：
- REPL 界面重设计：启动/`/resume`/`/clear` 后清屏重绘；历史以对话消息块渲染（与实时同形式）；彩色角色标签
- TUI 护栏审批打通：`ApprovalGate` 新增 UI 事件模式（`GuardrailApprovalNeeded` 事件 + 决定通道 + 超时），TUI 的 y/n 决定经 mpsc 传回审批门

**验证**：368 个测试通过。

### 条目 13｜2026-08-13 审批提示清除 + 用户消息块 + 历史完整查看

**现象**：审批提示块在确认后残留屏幕；任务没有以「用户」消息形式进入对话流；历史仅显示摘要。

**修复**：
- 审批块用 DECSC/DECRC 光标保存恢复清除（仅 stderr 为终端时执行）——**此方案后续条目 17 被废弃**，此处为过程记录
- 发送任务时把输入行原位替换为蓝底「用户」消息块（按终端宽度折算折行数）
- 历史完整展示（对标 Claude Code 转录模式/Codex resume）：消息编号 `[n]`、用户/助手不截断、工具结果限 12 行/600 字符、`/history N` 尾部、`/view <编号>` 全文

**验证**：375 个测试通过；REPL 冒烟覆盖全量恢复、`/history`、`/view` 越界、`/resume` 取消路径。

### 条目 14｜2026-08-13 滚动重复历史 + 主题无关配色 + /cls + TUI 面板修复

**现象**（四项）：
1. 向上滚动出现重复历史——清屏只用了 `\x1b[2J\x1b[H`，旧内容留在滚动缓冲区
2. 浅色主题下角色标签难以看清——16 色依赖终端主题
3. `/view`、`/help` 输出无法清理
4. TUI 状态栏不可见 + 护栏面板无 y/n 提示

**根因与修复**：
1. 清屏序列追加 `\x1b[3J` 清滚动缓冲区
2. 角色标签改 24 位真彩色（RGB 直指定，任何主题高对比）
3. 新增 `/cls` 清屏重绘命令
4. 状态栏根因：1 行高区域配带边框 Block，内容区高度 1−2=0；修复为去边框单行 Paragraph + 中文内容 + 待审批提示，测试缓冲区改真实 1 行高。护栏面板根因：y/n 提示是列表最后一行，请求多即被裁掉；修复为提示置顶（黄底黑字加粗）+ 面板标签中文化

**另有**：`ToolCallStarted` 增加 `detail` 字段展示具体命令（`bash: uname -a`）。

**验证**：378 个测试通过；REPL 管道冒烟：横幅与恢复历史仅出现一次、`/cls` 不带横幅。

### 条目 15｜2026-08-13 工具命令并入标注 + 实时消息序号 + 默认新会话

**实现**：
- 具体命令展示位置从独立 `[调用]` 行改为 assistant 消息块「工具调用：」标注（`（工具调用：bash: uname -a）`，超 120 字符截断）；`/cls`、`/history`、`/view`、启动恢复同路径生效
- 实时消息带全局序号（发送任务、实时助手块、工具结果行按事件顺序递增，与持久化列表编号一致）；事件消费改为 `tokio::select!` 任务与事件流并发等待
- 默认进入新会话；新增 `--resume` 参数才恢复上次会话

**验证**：384 个测试通过；管道冒烟确认默认新会话、`--resume` 恢复且标注含命令。

### 条目 16｜2026-08-13 模型管理 + 自动保存模型起名 + 状态行 + 工具行直接显示命令

**实现**：
- 工具结果行直接显示具体命令（用户反馈：命令应紧跟工具名）——`detail` 字段 + 历史 `tool_call_id` 反查，实时/历史/view 三处一致，跳过 `Success: true` 包装行
- `/model` 查看（模型/端点/上下文窗口/累计 Token）与切换（重建 agent、失败回滚）；TUI 状态栏显示真实模型（不再硬编码）
- 自动保存改**模型起名**：首条任务后生成 ≤12 字标题存 `<标题>.json`，失败回退 autosave；`--resume` 按 mtime 恢复最近会话；`/rename`、`/clear` 同步文件
- `/resume` 交互选择可取消（空输入/q/quit/取消/Ctrl+C），无效编号循环重试
- 输入区状态行（模型/Token/上下文剩余）置于输入行上方

**实现教训（重要）**：状态行最初用 DECSC/DECRC 光标舞步画到输入行**下方**，tmux 实测发现 rustyline 按自身宽度模型重定位光标，回显漂移到第 63 列、输出互相覆盖。改为多行提示符布局，并加回归测试禁止舞步转义。

**验证**：396 个测试通过；状态行经 tmux 真终端渲染验证。

### 条目 17｜2026-08-13 REPL 吞掉最终助手消息与 Token 统计

**现象（用户报告）**：任务后看不到 AI 回复；消息编号跳过 2；`Token: 0` 不增长。

**根因**：agent 返回前 emit 的最终事件（MessageAdded、ProgressUpdate、Finished）还排在 mpsc 通道中时 `run_fut` 已完成，`select!` 命中完成分支，随后 `try_recv` 循环把残留事件**静默丢弃**——最终回答永不打印，Token 统计随 ProgressUpdate 丢失。编号跳过是假象（助手消息已在会话文件中）。此前测试未抓到：冒烟测试全走假 API key 错误路径。

**修复**：提取 `handle_agent_event()`；run 结束后残留事件同样处理完。

**验证**：新增单测 `test_handle_agent_event_consumes_every_event_without_dropping`；假 LLM 服务器 + tmux E2E：助手消息显示、Token 110→220 逐轮累加。

### 条目 18｜2026-08-13 审批提示同行与审批块残留

**现象（用户报告）**：助手回复与 `是否批准此操作? (y/n): ` 挤在同一行；确认后审批块残留。

**根因**：REPL 审批走 stdin 交互分支由 AgentLoop 直接打印到 stderr（提示不带换行），助手消息事件在护栏检查**之前**发出，恰好落在提示行尾；块清除依赖 DECSC/DECRC 光标舞步，保存的是打印前坐标，期间助手消息、y 回显、折行、滚动都使恢复位置失效。

**修复**：REPL 审批改为与 TUI 相同的**事件模式**——AgentLoop 只发 `GuardrailApprovalNeeded` 事件，REPL 打印审批块（每行正常换行、提示独立行）、读 y/n、决定经 `decision_tx` 发回；审批结束后**全屏重绘**清除（与 `/resume`、`/cls` 同源），不再使用任何光标舞步。`--no-tui` 保留 stdin 交互（审批块留作日志）。`ApprovalRequest` 增加 `suggested_mitigation` 字段。

**验证**：新增回归测试禁止 `\x1b7`/`\x1b8`；tmux E2E 确认审批块阶段各行独立、确认后整块消失、助手消息经重绘保留。

### 条目 19｜2026-08-13 审批超时三连 bug + 标注去重 + 审批力度四档

**现象（用户报告）**：审批超时后 (1) 补输的 "y" 变成任务发给模型；(2) 输入文字困难、字符随机丢失；(3) 屏幕无超时提示。

**根因（tmux 复现确认）**：`read_yes_no_with_timeout` 用 `spawn_blocking` + 阻塞 `read_line`，tokio timeout 无法取消阻塞线程——线程永久泄漏（33→34），泄漏线程与 rustyline 竞争同一 fd 的每个按键（字符丢失、y 被吞）；另有陈旧决定问题：REPL 超时后 `try_send(Timeout)` 留在 decision 通道，下一次审批未经询问立即消费。

**修复**：可取消轮询读取（`libc::poll` 0 超时探测 + 50ms sleep + `libc::read`）；超时与读行后 `drain_stdin()` 清空残留；发送审批事件前清空陈旧决定；超时/拒绝后全屏重绘带状态提示。同步完成：
- 工具调用标注去重：标注只留工具名（具体命令已在工具结果行）
- **审批力度四档**：无/低/中/高（风险等级低于等于阈值自动批准；默认「低」）；三途径控制（CLI `--approval`、配置 `approval_level`、REPL `/approval` 运行时切换）；L1 Deny 始终硬拦截；**审批只针对工具调用**——E2E 发现「高」档若放行所有动作，agent 正常回复会被审批超时卡死，`final_answer` 等无副作用动作永不审批

**验证**：11 个新单测 + 4 CLI + 2 配置；E2E 四场景（无档直接执行/高档 ls 触发超时/低档对照/`/approval` 切换）；420 个测试通过。

### 条目 20｜2026-08-13 审批力度英文主值 + /skills

**实现**：`--approval` 与配置主值改英文 `none`/`low`/`medium`/`high`（中文「无/低/中/高」保留为兼容别名，旧配置不受影响）；序列化输出英文；界面显示中文。新增 `/skills` 指令（列出技能目录下技能名称 + frontmatter 描述，覆盖四种情况）。

**说明**：项目无 hook 设计（源码无任何 hook 机制），故未加 hook 查看指令。

**验证**：425 个测试通过。

### 条目 21｜2026-08-13 审批响应失效（y 按不了）+ 竞态残留 + 行结束容错

**现象（用户报告）**：审批提示出现后按 y/回车/Ctrl+C 全部无反应：`目前批准怎么按不了了`。

**根因链（strace + tmux 真终端逐层确认，四个根因）**：
1. Ctrl+C 杀死进程：cooked 模式 ISIG 开启，SIGINT 默认动作直接杀进程（strace 确认）——审批期间 Ctrl+C 应表示「拒绝」
2. 竞态残留：agent 端审批门超时与 REPL 端读取超时竞争，门先完成时 REPL 审批 future 被取消、审批块残留且无重绘
3. 超时后输入错位：补输的 y 落入下一轮任务、Enter 触发新审批（strace 显示 y 被 rustyline 1024 字节缓冲读走、换行被审批读取判为拒绝）
4. 行结束判定只认 `\n`：raw 环境 Enter 产生 `\r`，永远无法完成行

**修复**：`read_yes_no_with_timeout` 重写为 `tokio::select!` 三分支竞争（`ctrl_c()` 监听→清空 stdin→「拒绝」/ 超时→清空→「超时」/ 50ms 轮询读 stdin）；`line_complete` 同时接受 `\n` 与 `\r`；`approval_pending` 标志保证 run 结束后补做全屏重绘。配套提交 `04684ff`（termios 非规范模式 + RAII 恢复终端属性）与 `efa9f4d`（事件系统、会话管理、审批重绘与输入体验整体完善）。

**验证**：新增单测（`line_complete` 5 断言、`raise(SIGINT)` 触发断言进程存活）；tmux E2E 三场景（Ctrl+C 存活且正确拒绝/窗口内 y 正常/3s 超时提示且审批块清除）；427 个测试通过。

### 条目 22｜2026-08-13 项目更名 AuV + 两级配置 + AuV.md 角色说明

**实现**（commit `a30bba0`）：
- 二进制 `harness` → `auv`，CLI/横幅/默认提示词/init 输出品牌化；lib/包名、内部类型名、数据目录、keyring 服务名保持（已有会话与凭据无缝延续）
- **两级配置**：全局 `~/.AuV/config.toml` + 项目 `./.AuV/config.toml`（cwd 为 home 跳过）；启动自动创建（已存在绝不改动，幂等）；`toml::Value` 字段级递归合并；`--config` 仍只读单文件；旧版项目根 `config.toml` 黄色迁移提示；损坏→中文错误退出
- **AuV.md 角色说明**：项目按 `AuV.md` → `CLAUDE.md` → `AGENTS.md` 取第一个，全局对应 `~/.AuV/AuV.md` → `~/CLAUDE.md` → `~/AGENTS.md`；默认提示词 + 全局角色 + 项目角色追加式叠加；内联 `[agent] system_prompt` 最高优先；不存在时零打扰
- 提示信息在 REPL 清屏重绘**之后**打印（初版被启动清屏冲掉，E2E 发现并修复）

**验证**：12 个新单测；E2E（tmux + 假 LLM + HOME 隔离）：首次启动自动创建、二次启动幂等、局部覆盖生效（状态行显示 deepseek-v4-flash、审批 3s 超时）、角色说明加载提示、审批端到端正常；443 个测试通过。

### 条目 23｜2026-08-13 配置目录更正为隐藏目录

**背景**：用户在会话中指出配置位置有误（应为 `~/.AuV` 与 `./.AuV` 两级隐藏目录）。

**修复**（commit `29e5270`）：配置目录更正为 `~/.AuV/config.toml` 与 `./.AuV/config.toml`。

---

## 2026-08-14：护栏管线修复与数据目录统一收纳

### 条目 24｜2026-08-14 护栏审批管线修复（真实会话审计日志定位）

**背景**：用户在真实会话中感叹工作区状态异常，检查会话记录发现若干次工具调用行为可疑。

**根因（经真实会话审计日志定位，三项）**：
1. 管线原顺序「审批 → 沙箱」产生**假批准**——用户批准的操作随后仍被沙箱拦截
2. 部分路径硬编码风险等级字符串，与真实评估值矛盾（`High/Approved + Low/Blocked` 双记录）
3. `GuardResult::Denied` 直接终止 run，护栏拒绝成为会话死路

**修复**（commit `8c102ec`）：
1. 管线重排为「静态规则 → 风险评估 → **沙箱硬校验 → 审批**」
2. 每条动作恰好一条审计记录，`Blocked` 记录使用真实评估风险等级
3. **护栏拒绝注入反馈回路**：拒绝原因作为 Tool 消息注入对话，LLM 调整操作重试（`max_turns` 兜底）
4. 失败路径也保存会话（清除无配对 tool_calls），修复护栏拦截后整段对话丢失

**验证**：新增回归测试 `test_pipeline_sandbox_blocks_before_approval_single_audit_entry`、`test_pipeline_approval_single_audit_entry`；机制演示测试更新。

### 条目 25｜2026-08-14 默认系统提示词补充运行环境说明

**实现**（commit `21f50d5`）：默认系统提示词补充运行环境说明，帮助模型感知运行上下文。

### 条目 26｜2026-08-14 数据目录统一收纳到 .AuV + 记忆现状检查

**实现**（commit `227e1d5`）：
- 项目级运行时数据统一收纳：`.harness/sessions/` → `.AuV/sessions/`、`.harness/repl_history.txt` → `.AuV/repl_history.txt`、`.harness/audit.jsonl` → `.AuV/audit.jsonl`、`.memory/` → `.AuV/memory/`
- 一次性手动迁移（用户选择不做运行时迁移逻辑）：既有会话、历史、审计、记忆数据搬入 `.AuV/`（**用户真实会话数据原样保留**），旧目录删除
- 配置默认值同步（`audit_log_path`、`storage_path`）；`auv init` 创建 `.AuV/memory/`；`.gitignore` 保留旧条目防误提交

**记忆现状检查**：读侧已启用（`MEMORY.md` 注入系统提示词、每轮加载、默认 enabled）；**写侧缺失**——`MemoryStore::write()` 无调用点，agent 无法保存新记忆，列为后续可选任务（见 [SPEC.md](SPEC.md) §11 风险）。

### 条目 27｜2026-08-14 Docker、CI/CD 与发布收尾

**任务**：按 `doc/common.md` 的最终交付清单完善容器、GitHub Actions、GitLab CI、公开 Registry 与 Release 说明。

**关键发现与修复**（commit `7079391`）：
- 原 Dockerfile 仍复制旧二进制 `harness`；改为 Rust 1.88 多阶段构建、`auv` ENTRYPOINT、非 root 用户与约 45 MB Alpine 运行层
- 锁文件依赖最低需要 Rust 1.88，原 `rust:1.85-alpine` 无法构建；Alpine 静态 PIE 还需要 `openssl-libs-static`
- `auv key set` 已写入钥匙串/加密文件，但 Agent 启动只读取配置与环境变量；接通安全存储读取，并增加容器用 `OPENAI_API_KEY_FILE`
- 修正无桌面环境的钥匙串误判；加密回退文件强制 `0600`，SPEC 明确 machine-id 派生密钥的威胁边界
- GitHub CI 增加 check、Clippy、449 项测试、release 产物与 Docker 烟雾测试；Publish 工作流在测试后推送 GHCR，并为 `v*` 标签创建带 SHA-256 的 Release；补齐课程要求的 `.gitlab-ci.yml` `unit-test` job
- 首次 GitHub Actions 在 Rust 1.88 runner 上暴露 PTY 测试不稳定：`tcgetattr` 写入未清零的 `termios` 后，测试会比较未定义的 `c_cc` 保留槽；将生产路径与测试 helper 的接收缓冲区改为零初始化，并在 Rust 1.88/musl 中复现失败后验证修复
- GitHub 未登录状态不公开完整 Actions 日志；为两条流水线加入保留原退出码的测试包装脚本，失败时提取 panic、FAILED、失败汇总与编译错误上下文写入 Checks annotation，使公开 API 可诊断具体失败测试
- 第四轮 annotation 定位 `test_type_check_parses_errors`：workflow 的 `CARGO_TERM_COLOR=always` 传入测试内层 `cargo check`，ANSI 序列使错误正则失配，而通道又忽略非零退出码，形成假阳性；内层检查固定 `--color never`，并规定非零退出即失败且生成兜底诊断

**人工干预**：用户提供 GitHub 仓库地址；PLAN 中“REPL 扩展不再标为计划外”和维护者账号改名由用户并行修改，本条记录保留这些修改。远端初始化提交仅含 GPLv3 `LICENSE`，用户确认将其原样纳入本地历史，并明确授权以 force push 替换远端初始化历史。

**验证**：371 lib + 71 bin + 3 mechanism demo + 4 doctest 全部通过；Rust 1.88 `cargo check`/Clippy 成功；最终镜像构建和非 root 启动成功；Git 当前文件与全部历史未发现非占位符 key 模式。仓库仍有 2 条既有 `unused_mut` 编译警告和约 300 条 Clippy 风格建议，CI 采用 check 阻断、Clippy 建议模式。

---

## 2026-08-14（续）：subagent 孤岛模块生产接入

### 条目 28｜2026-08-14 subagent 孤岛模块生产接入（10 步计划）

**背景**：用户通过真实运行 `auv` 发现 `src/subagent/mod.rs` 是孤岛死代码——`SubagentSpawner`/`AgentRunner` 编译通过、自测全绿，但生产零引用（工具注册表无 subagent 工具；递归调用 agent_loop 的生产 Runner 从未实现），而 PLAN 任务 24 标「已完成」、SPEC §3.8 声称支持子 Agent——文档与现实脱节。用户要求接入（「目前subagent没有用啊」）。

**用户确认的决策**：子 agent 审批路由到父界面（REPL 状态区提示 + 查看子上下文）；隔离模式仅实现 SameProcess（Worktree 预留）；新增第四项机制演示；新增 `[subagent]` 配置段。

**实施**（每步独立编译 + 测试 + 提交，共 9 个提交）：
1. `[subagent]` 配置段（max_depth=3 / max_total_agents=10，serde default）— `980b593`
2. Spawner 深度传播重构：`depth` 字段 + `for_child()`（深度 +1、共享 `Arc<AtomicUsize>` 活跃计数）、`AgentRunner::run` 签名改收 `Arc<SubagentSpawner>`（工厂闭包需拥有子层 spawner）— `16db698`
3. `SubagentTool` 委派工具：同步 execute 内用 BashTool 先例（子线程 + current_thread runtime + `recv_timeout` 超时），结果含耗时/深度结构化字段 — `a2937e8`
4. `AgentLoopRunner` 生产 Runner：工厂闭包为每次委派构建独立子 loop；工厂错误降级为失败的 SubagentResult — `b21f8b5`
5. main.rs 生产装配：`SubagentWiring` + `build_subagent_wiring`（捕获 config/API key/工作区/审批模式）；build_agent 第 6 参注册 SubagentTool；REPL/CLI 启用（InBandStdin 审批）；修复并行 env var 测试竞态（静态 Mutex 串行化）— `300710b`、`5a49d0a`、`82450f4`
6. 审批上下文预览：`ApprovalGate` 加 preview 字段，agent loop 每次护栏检查前注入最近 5 条消息（每条 60 字符截断），stdin 审批块尾部追加「上下文（最近消息）」段 — `e68e2b2`
7. TUI 子审批路由：`AgentEvent::SubagentApprovalNeeded { request, label, decision_tx }`（子专属回发通道，与父审批全局通道分离）；`AppState.guard_requests` 改 `Vec<GuardRequest>`；护栏面板标注「来源: 子 agent」；TUI 装配启用子 agent — `143745d`
8. 第四项机制演示：父 mock LLM 委派「计算 2+2」→ 工厂构建子 loop（子 mock FinalAnswer「结果为 4」）→ 父汇总；验证子 loop 注册递归 subagent 工具 — `d9e4283`
9. 文档同步：SPEC §3.3/§3.7/§3.8/§5.4、PLAN 任务 24 生产接入记录、本日志

**设计决策**：
- 父子审批不可能并发（父在工具执行期间同步阻塞），因此 REPL/CLI 子审批走 in-band stdin 路径（子线程内打印 + 读 y/n），TUI 走事件路由；子 loop 不设事件通道（转发只会堆积错序）
- 已知限制（如实文档化）：REPL 冻结期间无实时子 agent 状态行（前置：Tool::execute 异步化）；`recv_timeout` 超时后子线程无法强制终止（孤儿线程跑完、结果丢弃，受 max_total_agents 约束）；Worktree 调用返回明确错误

**验证**：全量 478 项测试通过（469 → 478）；机制演示 4/4；步骤 10 清除 2 条既有 `unused_mut` 警告，全量回归零编译警告（`b2bab5a`）。

---

## 统计汇总

- 提交总数：69（2026-07-08 至 2026-08-14，含 subagent 生产接入 13 个提交：步骤实现 11 + 文档同步 1 + 警告清除 1）
- 测试数量：362 → 478（+116）；当前测试构建零编译警告，Clippy 有约 300 条风格建议
- 用户报告 bug 数：5（审批提示同行/残留、审批超时三连、y 按不了、吞掉最终消息、滚动重复历史）——全部经根因定位修复并加回归测试
- 关键方法论：mock 确定性测试（全部机制离线可测）+ pty 集成测试 + tmux E2E（HOME 隔离 + 假 LLM 服务器 127.0.0.1:18999）+ 真实会话审计日志定位

---

*维护者：luwisp*
*最后更新：2026-08-14*
