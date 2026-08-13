# 交互式 REPL 模式 — 实现方案

> **目标：** 将 harness 从一次性 `auv run "task"` 转为交互式 REPL，类似 Claude Code（项目已更名 AuV）。无子命令时进入交互会话，用户可连续输入任务，agent 实时显示工具调用过程。

**架构：** REPL 是在现有 `AgentLoop` 之上的薄交互层。Agent 主循环已通过 `AgentLoop::run_with_history()` 支持连续对话——主要改动是将其包装在 stdin/stdout 循环中，维护跨轮次对话历史，并内联渲染 agent 事件（工具调用、进度、护栏审批）。

**技术栈：** Rust (edition 2024), tokio, clap, serde_json

---

## 设计决策

### 1. 对话历史管理

**核心原则：** 消息必须按时间顺序排列，使 LLM 能正确理解多轮对话。

`ContextBuilder::build()` 产出的消息结构：

```
[System(prompt), User(task1), Assistant(response1), Tool(result1), User(task2), ...]
```

System 消息始终在第一位，包含系统提示、工具菜单、规则、技能、记忆索引。历史消息按时间顺序紧随其后。当前用户任务作为最后一条 User 消息追加。

**关键修正（2025-08）：** 原实现将 User(task) 放在历史消息之前（`[System, User(task), ...history]`），导致对话时间顺序错乱，LLM 无法识别前轮对话。修正为 `[System, ...history, User(task)]`，确保 LLM 看到完整的时间线。

### 2. 会话管理

REPL 支持 `/save <name>`、`/resume`、`/sessions`、`/rename <标题>`、`/model` 命令，类似 Claude Code 的多对话管理：

- 会话保存为 `.harness/sessions/<name>.json`
- 使用 serde_json 序列化 `Vec<Message>`
- `/resume <name>` 从文件加载并替换当前对话历史
- `/resume` 不带参数时列出所有会话（编号 + 消息数），输入编号或名称交互选择；空输入 / `q` / `quit` / `取消` / `Ctrl+C` 取消返回原对话（重绘原界面），无效编号提示后重新选择
- `/rename <标题>` 更改当前会话标题：标题经 `sanitize_session_title` 清洗（控制字符剔除、路径字符转空格、引号去除、24 字符截断），同步重命名自动保存文件，空对话时报错
- `/model` 查看当前模型信息（模型、API 端点、上下文窗口、累计 Token）；`/model <名称>` 运行时切换模型（更新配置并重建 agent，沿用事件通道，失败回滚）

### 3. 事件通道

Agent 主循环通过 `tokio::sync::mpsc` 通道发送 `AgentEvent` 给 REPL。REPL 用 `tokio::select!` 在任务 future 与事件流之间并发等待，实时消费事件。**运行结束时通道里可能仍有残留事件（最终助手消息、ProgressUpdate 等）**——`select!` 会先命中 run_fut 完成分支，因此 run 返回后必须用 `handle_agent_event` 把残留事件同样处理完，**直接丢弃会导致最终回答与 Token 统计静默丢失**（历史 bug，见 CHANGELOG「REPL 吞掉最终助手消息」条目）：

- `MessageAdded` → 实时打印 assistant 消息块（「工具调用：」标注附具体命令），带全局序号
- `ToolCallCompleted` → 工具标签块 + 具体命令（事件携带 `detail` 字段，`工具 bash: <命令>`）+ 结果首行（失败时内容红色），带全局序号
- `ToolCallStarted` / `ProgressUpdate` / `Finished` → REPL 中静默（TUI 仍使用；ProgressUpdate 的 tokens_used 被 REPL 累计到状态行 Token）

实时消息的序号按事件发出顺序递增，与任务结束后持久化的消息列表编号一一对应。

### 4. 界面设计

采用简洁专业的设计风格，全中文界面，不使用 emoji。角色标签为彩色背景块（用户蓝底白字、助手绿底黑字、工具紫底白字、系统灰底黑字）：

```
AuV harness agent REPL v0.1.0
输入任务开始对话，/help 查看命令，/exit 退出
（暂无对话历史）                    ← 默认新会话；auv --resume 时打印恢复横幅
[1]  用户 › <task description>
  ────────────────────────────────
[2]  助手 › ...
         （工具调用：bash: uname -a）
  ────────────────────────────────
[3]  工具 › bash: uname -a          ← 命令紧跟工具名，跳过 "Success: true" 包装行
         Result: Linux 主机
  ────────────────────────────────
[4]  助手 › FINAL ANSWER: ...

────────────────────────────────────────────────
模型: gpt-4o | Token: 1,234 | 上下文剩余: 99%    ← 状态行常驻输入行上方
> 
```

- 灰色分隔线 + 状态行 + `> ` 构成 rustyline 多行提示符，输入区域清晰可辨
- **状态行**：显示当前模型、累计 Token、上下文剩余（百分比 + 剩余/窗口）；模型与窗口按配置实时取值，Token 每轮结束累加刷新；状态行置于输入行**上方**——曾尝试 DECSC/DECRC 光标舞步画到输入行下方，但 rustyline 刷新按自身宽度模型重定位光标，导致回显漂移与输出覆盖（tmux 实测），故改用多行提示符布局，并以回归测试禁止舞步转义
- **用户消息块**：发送任务后把输入行的 `> 任务` 回显原位替换为彩色用户消息块（光标按终端宽度折算视觉行数），任务以对话消息形式留在屏幕与历史中
- **实时消息带序号**：发送任务、实时 assistant 消息块、工具结果行全部带灰色序号 `[n]`，按事件发出顺序递增，与历史/`/cls` 重绘编号一致
- **清屏重绘**：启动、`/resume`、`/clear` 后清屏（`\x1b[2J\x1b[H\x1b[3J`，`3J` 同时清空滚动缓冲区）并重绘横幅与历史；向上滚动不会出现重复的历史记录；`/resume` 交互选择无论成功、失败、取消，结束后都清理选择列表
- **`/cls` 命令**：清屏并重绘当前界面（清掉 `/view`、`/help` 等命令输出），等同重新进入会话时的状态
- **历史与实时同一形式**：历史消息以对话消息块渲染（彩色角色标签 + 内容 + 灰色分隔线），与实时输出完全一致；带工具调用的 assistant 消息标注 `（工具调用：bash: uname -a）`（具体命令附在工具名后）
- **工具结果行直接显示命令**：`[3] 工具 bash: uname -a`——命令紧跟工具名（bash 显示命令行，其他工具紧凑 JSON，超 120 字符截断），失败时错误首行红色跟在后面；跳过 `Success: true` 包装行直接显示 `Result: ...`；历史回显经 `tool_call_id` 反查 assistant tool_calls 得到命令，实时（事件 detail 字段）、`/history`、`/cls`、`--resume`、`/view` 五处渲染一致
- **完整历史（对标 Claude Code 转录模式 / Codex resume）**：恢复会话时完整打印全部消息，每条消息前有灰色序号 `[n]`；用户/助手消息不截断，工具结果限 12 行 / 600 字符，超出标注 `…（共 N 行，已省略，/view n 查看全文）`，靠终端滚动条回看
- **`/history` 与 `/view`**：`/history` 显示全部消息（`/history N` 只看最近 N 条）；`/view <编号>` 打印单条消息全文（不限行、不限字符），编号越界给出中文提示
- **护栏审批提示清除**：REPL 审批走 UI 事件模式（与 TUI 同机制，`build_agent` 传 `decision_rx`）——AgentLoop 只发 `GuardrailApprovalNeeded` 事件，由 REPL 打印审批块（stdout、每行正常换行、`(y/n): ` 提示独立行）、读取 y/n、决定经 `decision_tx` 发回；审批结束（批准/拒绝/超时）后**全屏重绘**（与 `/resume`、`/cls` 同源）整块从屏幕清除。不使用 DECSC/DECRC 光标舞步（滚动/并发输出下不可靠，与状态行同源教训）；重绘发生在 run 完成前，本轮已打印的助手消息经 `round_messages` 重新印出；`--no-tui` 保留 stdin 交互（审批块留在输出中作为日志）
- **审批期间 Ctrl+C 视为拒绝**：cooked 模式下 SIGINT 默认动作会直接杀死进程——审批读取循环改为 `tokio::select!` 三分支竞争（`tokio::signal::ctrl_c()` 监听 → 清空 stdin 按「拒绝」返回 / 超时 → 清空 stdin 按「超时」返回 / 50ms 轮询读 stdin），进程继续运行；行结束判定 `line_complete` 同时接受 `\n`（cooked 翻译结果）与 `\r`（raw 环境容错）；若 agent 端审批门先于 REPL 读取完成（超时竞态），`approval_pending` 标志保证 run 结束后补做全屏重绘（「审批超时，已自动拒绝该操作。」），审批块不残留

### 5. 输入编辑器（rustyline）

使用 rustyline 15 提供完整行编辑能力：

- `↑`/`↓` 输入历史导航（持久化到 `.harness/repl_history.txt`）
- `←`/`→` 行内光标编辑
- `Ctrl+C` 退出 REPL（ReadlineError::Interrupted → break）
- `Ctrl+D` 退出（ReadlineError::Eof）

### 6. 自动保存（模型起名）

- 发送第一条任务、本轮运行结束后，由模型根据任务内容生成简短标题（≤12 字，`generate_conversation_title`：OpenAiProvider 单轮调用 + 标题 system prompt），保存到 `.harness/sessions/<标题>.json`；标题生成失败（网络/API 错误）回退 `autosave`，并给出黄色警告
- 后续每轮对话自动按当前标题续存
- 默认启动开启新会话；`auv --resume` 启动时恢复**最近修改**的会话（按文件 mtime 选择，跳过损坏文件）
- REPL 内 `/resume` 命令仍可随时恢复任意会话
- `/clear` 同时删除当前会话文件（含 `autosave` 兼容清理）
- `/rename <标题>` 更改当前会话标题并重命名文件
- `/save <名称>` 保留为命名快照功能

---

## 实现清单

- [x] CLI 无子命令时进入 REPL
- [x] 多轮对话历史维护（时间顺序正确）
- [x] 实时工具调用显示
- [x] 会话保存/恢复（/save, /resume, /sessions）
- [x] 自动保存/自动恢复（autosave.json）
- [x] 对话历史查看（/history）
- [x] 对话历史清除（/clear）
- [x] 中文界面（横幅、帮助、状态、角色标签）
- [x] 护栏审批信息中文化（审批提示、评估原因、风险等级、沙箱违规）
- [x] rustyline 输入编辑器（历史导航 + 行内编辑）
- [x] 输入区域分隔线
- [x] 移除冗余 [Running] 回显
- [x] 日志默认 warn 级别，输出到 stderr
- [x] Ctrl+C 退出 REPL
- [x] 启动时展示恢复的最近 5 条消息 → **已改为完整展示全部历史（带序号）**
- [x] /resume 不带参数时列出会话交互选择
- [x] 护栏审批提示在确认后从屏幕清除（REPL 事件模式 + 全屏重绘；废弃 DECSC/DECRC 光标保存恢复）
- [x] 发送任务以用户消息块展示（原位替换 `>` 前缀回显）
- [x] 恢复会话完整展示全部历史（消息编号 `[n]`，工具结果限 12 行/600 字符）
- [x] /history 全量展示（`/history N` 显示尾部）
- [x] /view <编号> 查看单条消息全文
- [x] 多 tool_calls 响应保持 tool_call_id 配对（修复 DeepSeek HTTP 400）
- [x] DeepSeek 思考模式 `reasoning_content` 原样回传（修复 HTTP 400）
- [x] REPL 清屏重绘（启动、/resume、/clear；选择界面用后清理）
- [x] 历史以对话消息块形式展示（与初次发信息同形式）
- [x] ANSI 彩色输出（背景色角色标签、状态色、错误块）
- [x] TUI 模式护栏审批可确认（审批门 UI 事件模式 + 决定通道）
- [x] 清屏同时清空滚动缓冲区（`\x1b[3J`），向上滚动不再出现重复历史
- [x] 角色标签改为 24 位真彩色（主题无关，任何背景下清晰可读）
- [x] `/cls` 命令：清屏重绘，清理命令输出（等同重新进入会话）
- [x] TUI 状态栏修复：1 行高区域去掉边框直接渲染（原边框导致内容高度归零不可见），中文单行 + 待审批 y/n 提示
- [x] TUI 护栏面板：y/n 操作提示置顶（黄底黑字加粗），请求多时不再被裁掉；标签中文化（护栏/风险/操作/原因/编号/暂无待审批请求）
- [x] 工具调用展示具体命令：`[调用] 工具名: 命令`（bash 显示命令行，其他工具显示紧凑 JSON，超 120 字符截断）；TUI 工具面板同步显示
- [x] 工具调用具体命令移到消息块「工具调用：」标注中（如 `（工具调用：bash: uname -a）`），移除单独的 `[调用]` 行；`/cls`、`/history`、`/view`、启动恢复同路径生效
- [x] 实时新消息同样显示序号：发送任务、实时 assistant 消息块、工具结果行按全局编号递增（与历史编号一致）
- [x] 工具调用标注去重：`（工具调用：bash）` 只留工具名，具体命令见工具结果行
- [x] 审批超时修复：可取消轮询读取（无阻塞线程泄漏）+ 陈旧决定清空 + 超时/拒绝状态提示
- [x] 审批力度四档：配置 `[guardrails] approval_level` + CLI `--approval`（全局参数）+ REPL `/approval` 查看/切换；仅工具调用触发审批，L1 Deny 始终硬拦截
- [x] 审批力度参数英文主值（none/low/medium/high），中文档位名为兼容别名（CLI、配置、序列化；REPL 中英文均可）
- [x] `/skills` 指令：列出技能目录下已加载的技能（名称 + 描述），覆盖未配置/目录不存在/目录为空/正常列出
- [x] 默认进入新会话（不自动恢复 autosave），`auv --resume` 启动参数恢复上次自动保存的会话
- [x] 工具结果行直接显示具体命令（`工具 bash: <命令>`，事件 detail 字段 + 历史 tool_call_id 反查，实时/历史/view 一致；跳过 `Success: true` 包装行）
- [x] `/model` 查看当前模型信息（模型、API 端点、上下文窗口、累计 Token）；`/model <名称>` 运行时切换（重建 agent，失败回滚）
- [x] TUI 状态栏显示真实配置模型（不再硬编码）
- [x] TUI 护栏面板与状态栏改 24 位真彩色（主题无关可读）
- [x] 自动保存改为模型起名：首条任务后生成 ≤12 字标题保存到 `<标题>.json`，失败回退 autosave；`--resume` 恢复最近修改会话（按 mtime）
- [x] `/rename <标题>` 更改当前会话标题（清洗 + 重命名文件）；`/clear` 删除当前会话文件；`/sessions` 标记 `（当前会话）`
- [x] `/resume` 交互选择可取消（空输入/q/quit/取消/Ctrl+C 返回原对话并重绘），无效编号循环重试不再退出
- [x] 输入区状态行（模型/累计 Token/上下文剩余），多行提示符置于输入行上方；回归测试禁止 DECSC/DECRC 光标舞步
- [x] 上下文窗口按模型家族识别（gpt-4/5/o 与 claude 128k、deepseek 64k、llama/qwen/glm 32k，未知回退 token_budget）
- [x] 护栏审批改 REPL 事件模式：审批块独立行打印、y/n 经决定通道发回、审批结束全屏重绘清除（含本轮助手消息重印），回归测试禁止 DECSC/DECRC
- [x] 审批期间 Ctrl+C 视为拒绝（`tokio::signal::ctrl_c()` 监听，不再杀死进程）；行结束 `\r` 容错；审批竞态残留经 `approval_pending` 补重绘
- [x] 项目更名 AuV：二进制 `auv`、横幅/CLI/默认提示词品牌化；`.harness` 数据目录与内部类型名保持
- [x] 两级配置：`~/.AuV/config.toml`（全局）与 `./.AuV/config.toml`（项目，cwd 为 home 时跳过），启动自动创建（存在不改）、字段级合并（局部覆盖全局）、旧版 config.toml 迁移提示
- [x] AuV.md 角色说明：全局（`~/.AuV/AuV.md`）与项目（`./AuV.md`）两级检测，兼容已有 CLAUDE.md/AGENTS.md，叠加到默认提示词，内联 `[agent] system_prompt` 最高优先
