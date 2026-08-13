# UX 改进日志

> 记录 HarnessAgent 交互体验的重大改进

---

## 2026-08-13：项目更名 AuV + 两级配置系统 + AuV.md 角色说明

### 1. 项目更名 AuV harness agent（简称 AuV）

- 二进制 `harness` → **`auv`**；CLI 说明、REPL 横幅（`AuV harness agent REPL v0.1.0`）、默认系统提示词（"You are AuV harness agent, ..."）、`init` 输出全部品牌化
- `auv init` 交互全程中文化（创建路径提示、覆盖询问、完成指引）
- **保持不动**：lib/包名、内部类型名（`HarnessConfig` 等）、`.harness/` 数据目录、keyring 凭据服务名——已有会话与凭据无缝延续

### 2. 两级配置系统（全局 + 项目）

| 层级 | 路径 | 说明 |
|------|------|------|
| 全局 | `~/AuV/config.toml` | 用户级默认值（默认模型、默认审批力度等） |
| 项目 | `./AuV/config.toml` | 项目级覆盖（cwd 为 home 目录时跳过） |

- **启动自动创建**：配置不存在时创建目录并写入默认配置；**已存在则绝不改动**（幂等，E2E 二次启动验证无提示无改动）
- **字段级合并**：`toml::Value` 递归合并，项目配置写哪个字段覆盖哪个，未写继承全局；`--config` 显式路径仍只读单文件
- **旧版迁移**：项目根 `config.toml` 不再加载，启动时黄色提示「检测到旧版 …，请将配置移至 ./AuV/config.toml」
- 配置损坏（TOML 解析失败）→ 中文错误退出；目录创建失败 → 中文警告继续默认
- 提示信息在 REPL 清屏重绘**之后**打印（初版提示被启动清屏冲掉，E2E 中发现并修复）

### 3. AuV.md 角色说明（两级 + 兼容已有）

- 项目内按 `AuV.md` → `CLAUDE.md` → `AGENTS.md` 取第一个存在的文件；全局对应 `~/AuV/AuV.md` → `~/CLAUDE.md` → `~/AGENTS.md`——已有 CLAUDE.md/AGENTS.md 的项目无需改名
- 合成规则：默认提示词 + 全局角色 + 项目角色（追加式叠加）；配置内联 `[agent] system_prompt` 最高优先；两级都不存在时零打扰、不创建文件
- 加载时黄色提示「已加载全局/项目角色说明：<路径>」

**测试：** 新增 12 个单元测试（路径解析、递归合并、分层创建幂等、字段级覆盖、cwd==home 跳过局部、旧版提示、损坏报错、角色文件优先级与兼容、persona 组装/内联优先/无文件不改）；既有品牌断言与 `load_config` 旧语义用例同步更新（CWD 锁测试改为 home/cwd 注入版，不再修改进程级 CWD）；全量 443 个测试通过（367 lib + 69 main + 3 demo + 4 doctest）。

**E2E（tmux + 假 LLM + HOME 隔离）**：首次启动自动创建两级配置并提示；二次启动幂等；局部配置 `model` 与 `approval_timeout_secs = 3` 覆盖生效（状态行显示 deepseek-v4-flash、审批 3s 超时）；`AuV.md` 角色说明加载提示显示；审批流程端到端正常。

**影响文件：** `Cargo.toml`（bin 改名）、`src/config/mod.rs`（分层加载 + 合并 + 角色文件检测）、`src/main.rs`（load_config 分层、apply_persona、notices、init 中文化、品牌文案）、`src/loop/context.rs`（默认提示词品牌化 + `pub`）、`src/lib.rs`（文档注释）、`README.md`（品牌 + 两级配置章节）、`docs/superpowers/specs/2026-08-13-auv-config-design.md`（新增设计文档）

---

## 2026-08-13：修复审批响应失效（y 按不了）+ 审批竞态残留 + 行结束容错

### 症状（用户报告）

审批提示出现后按 y/回车/Ctrl+C 全部无反应：`是否批准此操作? (y/n): y^M^M^M^C^C 目前批准怎么按不了了`。

### 根因链（strace + tmux 真终端逐层确认，四个根因）

1. **（c）Ctrl+C 杀死进程**：审批期间终端处于 cooked 模式（ISIG 开启），Ctrl+C 产生 SIGINT，进程无处理器，默认动作直接杀死进程（strace 确认「killed by SIGINT」）——审批期间 Ctrl+C 应表示「拒绝」，而不是退出程序。
2. **（a）竞态残留**：agent 端审批门超时与 REPL 端读取超时竞争，门先完成时 `select!` 命中 run_fut 分支、REPL 审批 future 被取消、审批块残留屏幕且无重绘。
3. **（b）超时后输入错位**：超时后补输的 y 落入下一轮任务、Enter 触发新审批，输入与 rustyline 竞争同一 fd（strace 显示 y 被 rustyline 1024 字节缓冲读走、换行被审批读取 128 字节缓冲读走判为拒绝）。
4. **（d）行结束判定只认 `\n`**：raw 环境（ICRNL 关闭）下 Enter 产生 `\r`，永远无法完成行。

### 修复

- `read_yes_no_with_timeout` 重写为 `tokio::select!` 三分支竞争：`tokio::signal::ctrl_c()` 监听（命中 → 清空 stdin → 按「拒绝」返回）/ 超时（清空 stdin → 按「超时」返回）/ 50ms 轮询读 stdin——**审批期间 Ctrl+C 不再杀死进程，转为拒绝决定**。
- 行结束判定 `line_complete` 同时接受 `\n`（cooked 翻译结果）与 `\r`（raw 环境容错）。
- `approval_pending` 标志：审批事件处理中被 run_fut 完成打断时，run 结束后补做全屏重绘（「审批超时，已自动拒绝该操作。」），审批块不再残留。

### 验证

- 新增单元测试：`test_line_complete_accepts_lf_and_cr`（5 断言）、`test_read_yes_no_ctrl_c_returns_no_without_killing_process`（tokio::spawn 内 `libc::raise(SIGINT)` 触发，断言返回 No 且进程存活）
- tmux 真终端 E2E 三场景：审批期间 Ctrl+C → 进程存活 + 「Guardrail blocked: 用户拒绝了该操作」+ 界面正确重绘；窗口内 y → 审批完成、工具输出正常；3s 超时 → 「审批超时，已自动拒绝该操作。」状态行显示 + 审批块清除
- 全量 427 个测试通过（354 lib + 66 main + 3 demo + 4 doctest）

**影响文件：** `src/guardrails/approval.rs`（select! 三分支 + `line_complete` + stdin 清空）、`src/main.rs`（`approval_pending` 补重绘）

---

## 2026-08-13：审批力度参数改英文主值、新增 /skills 技能查看指令

### 1. 审批力度参数改英文主值

`--approval` 与配置 `[guardrails] approval_level` 的主值改为英文 `none`/`low`/`medium`/`high`（原中文「无/低/中/高」保留为兼容别名，旧配置与旧命令行不受影响）；REPL `/approval` 指令中英文均可。序列化（`harness init` 生成的配置、测试转储）输出英文。界面显示仍为中文档位名。

### 2. 新增 `/skills` 指令

REPL 新增 `/skills`：列出技能目录（`[agent] skills_dir`，默认 `.skills`）下已加载的技能文件（名称 + frontmatter `description`），覆盖未配置/目录不存在/目录为空/正常列出四种情况；`/help` 与 README 同步。抽成 `format_skills_list` 纯函数，4 个新单元测试。

**说明：** 项目目前**没有 hook 设计**（源码中无任何 hook 相关机制），因此未加 hook 查看指令。

**测试：** 全量 425 个测试通过（新增配置英文序列化测试、CLI 英文主值 + 中文别名测试、4 个技能列表测试）。

---

## 2026-08-13：审批超时三连 bug 修复、工具调用标注去重、审批力度四档

### 1. 审批超时后状态混乱（bug 修复）

**症状（用户报告）：** 审批请求超时后 (1) 用户补输的 "y" 变成任务发给模型，模型回复「你回复了"y"，但我需要确认…」；(2) 输入文字困难、字符随机丢失；(3) 屏幕没有超时提示，状态混乱。

**根因（tmux 真终端复现确认）：** `read_yes_no_with_timeout` 用 `spawn_blocking` + 阻塞 `read_line` 实现，tokio timeout 到期后**无法取消阻塞线程**——线程永久泄漏在 stdin 读上（复现：线程数 33→34），且泄漏线程与 rustyline 竞争同一 fd 的每个按键（字符随机丢失=「输入文字难输入」，补输的 y 被吞）；另有陈旧决定问题：REPL 超时后 `try_send(Timeout)` 留在 decision 通道，下一次审批未经询问立即消费（打开审批块前就"超时"）。

**修复：** 改为**可取消轮询读取**（`libc::poll` 0 超时探测 + 50ms `tokio::time::sleep` + `libc::read` fd 0——cooked 模式整行到达后 read 不阻塞）；超时与读行后 `drain_stdin()` 清空残留字节；`await_ui_decision` 发送审批事件前清空 decision 通道的陈旧决定；审批超时/拒绝后全屏重绘带状态提示（「审批超时，已自动拒绝该操作。」/「操作已被拒绝。」）。

**验证：** 新增单元测试（陈旧决定 drain、EOF 不阻塞、drain 安全性）+ tmux 真终端 E2E：线程数稳定 33（无泄漏）、超时状态提示显示、第二轮审批块正常显示等待、补输的 y 成为带明确状态的新任务（合理行为）。

### 2. 工具调用标注去重

助手消息块标注 `（工具调用：bash: <命令>）` 不再显示具体命令——工具结果行 `[n] 工具 bash: <命令>` 已有完整命令。标注只保留工具名（多工具用「、」连接、去重保序）。审批块与工具结果行仍显示完整命令。

### 3. 审批力度四档

新增审批力度设置，控制「风险等级**低于等于**对应阈值时自动批准命令」：

| 档位 | 自动批准阈值 | 行为 |
|------|-------------|------|
| 无   | 高          | 仅严重（Critical）需审批 |
| 低   | 中          | 高/严重需审批（默认，与历史行为一致） |
| 中   | 低          | 中及以上需审批 |
| 高   | 无          | 所有风险等级的工具调用都需审批 |

- **三种控制途径**：CLI `--approval <档位>`（global 参数，REPL/TUI/CLI 所有模式；`harness run --approval 无 "任务"` 亦可用，支持 none/low/medium/high 英文别名）；配置 `[guardrails] approval_level = "中"`（中文档位名或英文别名）；REPL `/approval` 查看当前档位与四档说明、`/approval <档位>` 运行时切换（透传管线，无需重建 agent，下轮任务生效）。
- **设计约束**：静态规则 L1 Deny 始终硬拦截、不受力度影响；L1 Escalate 把评估等级抬到高，在「无」档下同样自动批准；审批**只针对工具调用**，`final_answer` 等无执行副作用的动作永不审批（E2E 中发现并修复：力度「高」时若放行所有动作进审批门，agent 的正常回复会被审批超时卡死）。
- **测试**：11 个新单元测试（阈值映射、FromStr 中文/英文解析、四档管线行为、final_answer 豁免、`set_approval_level` 即时生效）+ 4 个 CLI 解析测试 + 2 个配置解析测试；顺带修复既有测试的并行 CWD 竞态（静态锁串行化）；全量 420 个测试通过。
- **E2E（tmux + 假 LLM 服务器）**：`--approval 无` 下高风险命令直接执行（无审批块）；`--approval 高` 下 `ls`（低风险）触发审批超时；对照组 `--approval 低` 下同命令直接执行；`/approval` 显示与切换生效；`run --no-tui --approval 无` 正常执行。

**影响文件：** `src/guardrails/mod.rs`（`ApprovalLevel` 枚举 + 管线阈值逻辑 + 仅工具调用审批）、`src/guardrails/approval.rs`（可取消轮询读取 + drain）、`src/config/mod.rs`（`approval_level` 字段）、`src/loop/mod.rs`（`set_approval_level` 透传）、`src/main.rs`（CLI 参数、`/approval` 指令、超时状态提示、CWD 测试锁）、`tests/mechanism_demo.rs`（新参数）、`README.md`（护栏章节新增审批力度小节）

---

## 2026-08-13：修复护栏审批提示——助手消息同行与审批块残留

**症状（用户报告）：** 高风险命令触发护栏审批时，(1) 助手回复与 `是否批准此操作? (y/n): ` 提示挤在**同一行**没有换行；(2) 输入 y/n 确认后**审批块仍残留在屏幕**上。

**根因：** REPL 模式的审批门走 stdin 交互分支（`ApprovalGate::new`），由 AgentLoop 内部直接把审批块打印到 stderr，末尾 `(y/n): ` 提示不带换行；而 `MessageAdded`（助手消息）在护栏检查**之前**发出，REPL 打印 `[8] 助手 …` 时正好落在提示行尾。块清除依赖 DECSC/DECRC 光标舞步（`\x1b7` 保存光标 → `\x1b8\x1b[J` 恢复并清除）：保存的是打印前的绝对屏幕坐标，期间助手消息打印、y 回显、长参数折行与可能的屏幕滚动都会让恢复位置失效，`\x1b[J` 清除错误区域，块便残留下来。这与状态行修复（本日志前一条目）确认的教训相同——光标舞步在并发输出与滚动下不可靠，只是审批路径此前没有被改成重绘方案。

**修复：** REPL 审批改为与 TUI 相同的**事件模式**（`build_agent` 传入 `decision_rx`，审批门走 `with_ui_events`）：AgentLoop 只发 `GuardrailApprovalNeeded` 事件，由 REPL 打印审批块（stdout、每行正常换行、提示行独立）、复用 `read_yes_no_with_timeout` 读取 y/n、决定经 `decision_tx` 发回。审批结束（批准/拒绝/超时）后**全屏重绘**（与 `/resume`、`/cls` 同源的清屏重绘路径）——审批块从屏幕消失，不再使用任何光标舞步。重绘发生在 run 完成前，本轮已打印的助手消息通过 `round_messages` 列表重新印出，不丢失。事件顺序变为「助手消息块 → 审批块 → 重绘 → 工具结果」，各行独立。`--no-tui` 纯文本模式保留 stdin 交互（管道场景审批块留在输出中作为日志记录）。

**验证：**

- 新增单元测试：`test_approval_block_text_has_no_cursor_dance_sequences`（块内容完整 + 回归禁止 `\x1b7`/`\x1b8`）、`test_handle_approval_event_sends_yes_decision`、`test_handle_approval_event_maps_no_and_timeout`（决定映射）、`test_handle_agent_event_skips_approval_input_when_disallowed`（运行结束后不再读 y/n 打扰用户）
- `ApprovalRequest` 新增 `suggested_mitigation` 字段（REPL 审批块与 TUI 护栏面板显示缓解措施）
- 假服务器（返回带管道+重定向+`&&` 的高风险 bash tool_call，组合评分触发审批）+ tmux 真终端端到端：审批块阶段 `[2] 助手 …` 独立行、`(y/n): ` 独立行；`y` 确认后 `需要护栏审批` 整块消失、`[2]` 助手消息经重绘保留、`[3]` 工具结果与 `[4]` 最终回答正常、Token 正常累加

**影响文件：** `src/main.rs`（REPL 审批事件处理 + 重绘 + 决策通道）、`src/events.rs`（`ApprovalRequest` 加字段）、`src/guardrails/approval.rs`（`with_ui_events` 传缓解措施、导出 `read_yes_no_with_timeout`）、`src/tui/panels/guardrails.rs`（面板显示缓解措施）

---

## 2026-08-13：修复 REPL 吞掉最终助手消息与 Token 统计

**症状（用户报告）：** 发送任务后只看到 `[1] 用户 hello` 与标题提示，**看不到 AI 的回复**；`[3] 用户 ?` 的编号跳过 2；状态行 `Token: 0` 始终不增长。

**根因：** agent 循环在返回前会 emit 最终助手消息（MessageAdded）、ProgressUpdate、Finished 等事件，但这些事件还排在 mpsc 通道里时 `run_fut` 已完成，`tokio::select!` 先命中完成分支 `break`，随后 `while rx.try_recv().is_ok() {}` 把残留事件**静默丢弃**——最终回答永不打印（空行即 `println!()`），Token 统计也随 ProgressUpdate 一起丢失。编号跳过 2 是假象：`task_index = conversation.len() + 1` 从返回结果计算，助手消息其实已进入对话并落盘（会话文件里完整存在）。

**为什么此前测试没抓到：** 冒烟测试全部使用假 API key（走错误路径）或手工构造的会话文件，没有覆盖「真实成功运行」路径。

**修复：** 提取 `handle_agent_event()`（处理 MessageAdded / ToolCallCompleted / ProgressUpdate），run 结束后不再丢弃，而是把残留事件同样处理完：

```rust
while let Ok(event) = rx.try_recv() {
    handle_agent_event(event, &mut next_index, &mut run_tokens, &conversation);
}
```

**验证：**

- 新增单元测试 `test_handle_agent_event_consumes_every_event_without_dropping`（助手消息消耗序号、ProgressUpdate 记录 Token）
- 假 OpenAI 兼容服务器（/tmp/fake_llm.py）+ tmux 真终端端到端：`[2] 助手 你好！你说了：hello` 正常显示，`Token: 110` → `220` 逐轮累加，标题生成正常

**影响文件：** `src/main.rs`（事件处理提取 + 残留事件处理）

---

## 2026-08-13：模型管理 + 标题自动保存 + 输入区状态行 + 工具行直接显示命令

### 1. 工具结果行直接显示具体命令（紧跟工具名）

**背景：** 上一版把具体命令放在 assistant 消息的「工具调用：」标注里，但用户指出：命令应直接跟在工具名后面展示，且此前只有异常工具调用才显示命令。

**改动：**

- `AgentEvent::ToolCallStarted/Completed` 新增 `detail` 字段（`tool_call_detail()`：bash 显示完整命令行，其他工具显示紧凑 JSON，截断 120 字符）
- REPL 实时输出：`[5] 工具 bash: echo "..." && for f in ...` —— 命令紧跟工具名，失败时错误首行跟在后面红色显示
- 历史回显（`/history`、`/cls` 重绘、`--resume`）：通过 `tool_call_id` 反查 assistant 的 tool_calls 找到命令，`[3] 工具 bash: uname -a`；跳过 `Success: true` 包装行，直接显示 `Result: ...`
- `/view <编号>` 查看工具消息时，命令显示在内容上方
- 实时、历史、view 三处渲染一致

### 2. `/model` 命令：查看与切换模型

- `/model`：显示当前模型、API 端点、上下文窗口（128k/64k/32k 按模型家族识别）、累计 Token
- `/model <名称>`：运行时切换模型——更新配置、重建 agent（沿用同一事件通道），下轮任务生效；失败自动回滚并报错
- TUI 状态栏同步显示真实配置模型（此前硬编码 gpt-4o）

### 3. 自动保存：模型起名 + `--resume` 恢复最新会话

- 发送第一条任务、本轮结束后，由模型根据任务内容生成简短标题（≤12 字，失败回退 `autosave`），保存到 `.harness/sessions/<标题>.json`
- 后续每轮按当前标题续存；`harness --resume` 恢复**最近修改**的会话（按 mtime 选择）
- `/rename <标题>` 更改当前会话标题（同步重命名文件）；`/clear` 删除当前会话文件；`/sessions` 当前会话带 `（当前会话）` 标记

### 4. `/resume` 可取消返回原对话

- 交互选择时：空输入 / `q` / `quit` / `取消` / `Ctrl+C` 均可取消，重新绘制原对话
- 无效编号提示「编号无效：N，请重新输入（回车或 q 取消）」并继续选择，不再退出 REPL

### 5. 输入区状态行：模型 / Token / 上下文剩余

- 状态行显示在输入行上方（多行提示符第二行）：`模型: gpt-4o | Token: 1,234 | 上下文剩余: 99%（126,766/128,000）`
- Token 为全会话累计值（ProgressUpdate 每轮增量累加），每轮结束刷新
- 上下文剩余按模型家族窗口估算（gpt-4/5/o 系列与 claude 128k、deepseek 64k、llama/qwen/glm 32k，未知回退 token_budget）

**实现教训：** 最初用 DECSC/DECRC 光标舞步（`\x1b7`/`\x1b8`）把状态行画到输入行**下方**，但 tmux 真终端实测发现：rustyline 刷新时按自身提示符宽度模型重定位光标，导致输入回显漂移到第 63 列、命令输出与状态行互相覆盖（如 `已保存会话：4o | Token: 0 | ...`）。经根因分析确认舞步方案与 rustyline 刷新模型互不兼容，改为可靠的多行提示符布局（状态行在上方），并添加回归测试禁止再用该转义序列。

### 6. TUI 真彩色 + 真实模型

- 护栏面板、状态栏颜色从索引色（随终端明暗主题变化导致黄底黄字看不清）改为 24 位真彩色（Rgb），浅色/深色主题下均可读
- 护栏提示固定黄底白字，状态栏白字深灰底

### 影响范围

| 文件 | 变更 |
|------|------|
| `src/events.rs` | ToolCallStarted/Completed 新增 detail 字段 |
| `src/loop/mod.rs` | 新增 `tool_call_detail()`；detail 共享给三个 emit 点 |
| `src/main.rs` | `/model`、`/rename`、`/resume` 取消、状态行、标题生成、工具行命令反查、`build_repl_prompt()` |
| `src/tui/mod.rs` | run_tui 接收真实模型；状态栏/护栏面板真彩色 |
| `src/tui/panels/guardrails.rs`、`status.rs` | 提取 build_widget，真彩色常量 + 测试 |
| `README.md` | 命令表、自动保存、状态行、工具行命令、测试数 396 |

### 测试

396 个测试全部通过（337 lib + 52 main + 3 demo + 4 doctest），零编译警告；状态行布局经 tmux 真终端渲染验证（回显贴提示符、输出独立成行、状态行完整）。

---

## 2026-08-13：工具命令并入标注 + 实时消息序号 + 默认新会话（--resume 恢复）

### 1. 具体命令展示位置：消息块「工具调用：」标注（不再是单独的 [调用] 行）

**反馈：** 上一轮把具体命令展示在单独的 `[调用] bash: uname -a` 行上，与预期不符；用户要的是命令跟在工具名后面（assistant 消息块的「工具调用：」标注里），且 `/cls` 重绘后同样能看到。

**修复：** assistant 消息块的标注从 `（工具调用：bash）` 改为 `（工具调用：bash: uname -a）`——bash 显示命令行本身，其他工具显示紧凑参数 JSON，超 120 字符截断；多条工具调用以 `; ` 分隔。摘要逻辑复用 `loop::tool_call_detail`（改为公开）。`print_message_block`、`view_message` 同路径生效，因此 `/cls`、`/history`、`/view`、启动恢复全部显示具体命令。单独的 `[调用]` 实时行已移除。

### 2. 实时新消息显示序号

**症状：** 历史信息有序号 `[n]`，但新发送的任务、实时消息没有序号，两种展示不一致。

**修复：** 实时消息全部带全局序号——发送任务时用户块带 `conversation.len() + 1`，运行期间 assistant 消息块与工具结果行按事件发出顺序递增（与任务结束后持久化的消息列表编号一一对应）。事件消费方式从「独立后台任务」改为 `tokio::select!` 在任务 future 与事件流之间并发等待，任务结束后丢弃残留未读事件，避免污染下一轮序号；同时 assistant 消息块改为实时打印（此前实时只显示工具结果行）。

### 3. 默认进入新会话，`--resume` 参数才恢复上次会话

**症状：** 每次启动都自动恢复上次自动保存的会话，无法直接开启新会话。

**修复：** REPL 默认以空会话启动（横幅下显示「（暂无对话历史）」）；新增 CLI 参数 `--resume`，`harness --resume` 启动时恢复上次自动保存的会话并打印恢复横幅。REPL 内 `/resume` 命令保持不变。

### 影响范围

| 文件 | 变更 |
|------|------|
| `src/loop/mod.rs` | `tool_call_detail` 改为公开，供 REPL 标注复用 |
| `src/main.rs` | 标注附具体命令（新增 `tool_call_label` + 4 测试）、移除 `[调用]` 行、实时消息带序号（`tokio::select!` 内联事件消费）、`--resume` 参数（+2 测试）、`print_task_as_user_block` 带序号 |
| `README.md` | REPL 示例更新（新会话、序号、命令标注）、`harness --resume` 用法、实时进度说明、测试数更新 |
| `docs/superpowers/PLAN_REPL_CN.md` | 事件通道与界面设计节更新、自动保存节更新、清单补项 |

### 测试

全部 384 个测试通过（335 lib + 42 main + 3 demo + 4 文档测试），零编译警告；管道冒烟验证：默认启动为新会话（无恢复横幅）、`--resume` 恢复历史且标注含具体命令（如 `（工具调用：bash: uname -a）`）。

---

## 2026-08-13：滚动重复历史修复 + 主题无关配色 + /cls 清屏 + TUI 面板修复 + 工具调用命令展示

### 1. 向上滚动出现重复历史（根因修复）

**症状：** 终端里一直往上滚动会看到「上一段会话的尾巴 + 横幅 + [已恢复上次会话，共 18 条消息] + [1] 用户 …」的重复内容，仿佛历史被打印了两遍。

**根因：** 清屏只用了 `\x1b[2J\x1b[H`（清屏 + 光标归位），旧内容仍留在终端的滚动缓冲区（scrollback）里。REPL 每次重绘界面（启动、/resume、/clear）后向上滚动，就会先看到缓冲区里的旧画面再接上新画面。

**修复：** 清屏序列追加 `\x1b[3J`（清除滚动缓冲区），重绘后向上滚动只剩当前界面内容，不再出现重复历史。

### 2. 角色标签在部分背景下难以看清（主题相关配色 → 24 位真彩色）

**症状：** 浅色主题（如 KDE 默认配色）下「用户」「助手」等角色标签的文字与终端调色板冲突，难以看清。

**根因：** 角色标签使用标准 16 色背景（蓝/绿/紫），实际颜色由终端主题决定，与前景文字的组合对比度不可控。

**修复：** 标签改为 24 位真彩色（RGB 直接指定，如蓝底 `48;2;37;99;235` + 白字 `38;2;255;255;255`），不依赖终端主题；四种标签统一白字，任何背景下均高对比可读。

### 3. `/cls` 清屏重绘命令

**症状：** `/view`、`/help` 的输出留在屏幕上无法清理，界面越来越乱。

**修复：** 新增 `/cls` 命令：清屏并重绘当前界面（当前对话历史），等同重新进入会话时的状态；已加入 `/help` 命令表。

### 4. TUI 状态栏不可见 + 护栏面板无 y/n 提示（根因修复）

**症状 A：** `run` 的 TUI 模式下底部状态栏（state）完全看不见。

**根因：** 布局给状态栏分配高度 1 行，但渲染使用带边框的 Block（上下边框各占 1 行），内容区高度 1−2=0，文本永远画不出来。单元测试使用 3 行高的缓冲区，掩盖了真实布局下的问题。

**修复：** 状态栏去掉边框，单行 Paragraph 白字深灰底直接渲染；内容中文化：`轮次: N | Token: N | 风险: 低/中/高 | 模型: … | 运行中/已完成`，有待审批请求时追加 `待审批: N（按 y 批准 / n 拒绝）`。测试缓冲区改为真实高度 1 行。

**症状 B：** 护栏面板出现待审批请求时没有提示要按 y/n。

**根因：** y/n 操作提示是内容列表的最后一行，护栏面板高度只有右栏一半，请求条目一多提示行就被裁掉。

**修复：** 操作提示移到面板顶部（标题下第一行，黄底黑字加粗）：`按 y 批准 / n 拒绝，Esc 或 q 退出`，始终可见；面板标签中文化：标题「护栏」、字段「风险/操作/原因/编号」（风险等级显示为 低/中/高/严重）、空状态「暂无待审批请求。」。

### 5. 工具调用展示具体命令

**症状：** 工具调用只显示工具名（`[调用] bash`），不知道 agent 到底执行了什么命令。

**修复：** `AgentEvent::ToolCallStarted` 增加 `detail` 字段（具体命令/参数摘要）：bash 直接显示命令行本身（如 `bash: uname -a`），其他工具显示紧凑参数 JSON，超 120 字符截断加省略号。REPL 显示 `[调用] bash: uname -a`，TUI 工具面板同步显示。

### 影响范围

| 文件 | 变更 |
|------|------|
| `src/main.rs` | 清屏追加 `\x1b[3J`、角色标签 24 位真彩色、`/cls` 命令与 `/help` 条目、工具调用打印 detail |
| `src/events.rs` | `ToolCallStarted` 增加 `detail` 字段 |
| `src/loop/mod.rs` | 新增 `tool_call_detail()` 提取命令/参数摘要（+3 测试） |
| `src/tui/mod.rs` | 工具调用事件带 detail 更新工具面板 |
| `src/tui/panels/status.rs` | 去边框单行中文状态栏 + 待审批提示（测试改 1 行高） |
| `src/tui/panels/guardrails.rs` | y/n 提示置顶、中文化标签 |
| `Cargo.toml` | dev-dependencies 增加 `unicode-width`（测试缓冲区宽字符处理） |
| `README.md` | REPL 命令表（/cls）、TUI 面板说明、彩色输出说明、测试数更新 |
| `docs/superpowers/PLAN_REPL_CN.md` | 界面设计节更新、清单补项 |

### 测试

全部 378 个测试通过（335 lib + 36 main + 3 demo + 4 文档测试），零编译警告；REPL 管道冒烟验证：启动横幅与恢复历史仅出现一次、`/cls` 重绘不带横幅、`/help` 含 `/cls`、`/history` 正常输出。

---

## 2026-08-13：护栏审批提示清除 + 用户消息块 + 历史完整查看

### 1. 护栏审批提示在确认后从屏幕清除

**症状：** `=== 需要护栏审批 ===` 提示块在用户 y/n 确认后仍留在屏幕上，与后续工具调用输出混在一起；连续多次审批时界面越来越乱。

**修复：** 审批门打印提示块前用 ANSI 光标保存（DECSC `\x1b7`）记录位置；用户回答（批准/拒绝/超时）后恢复光标并清除到屏幕底（DECRC `\x1b8` + `\x1b[J`），整块提示连同 y/n 回显一起消失，屏幕回到对话流。光标操作仅在 stderr 为终端时执行，重定向/CI 场景不写转义序列。

### 2. 发送任务后显示为用户消息块

**症状：** 发送任务后输入行仍是 `> 任务内容` 前缀回显，任务没有以「用户」消息形式进入对话流，与恢复历史后的展示形式不一致。

**修复：** 提交任务时把输入行原位替换为用户消息块（蓝底「用户」标签 + 完整任务文字）：光标上移回提示行（按终端宽度折算折行数）、清除回显、打印用户块。非终端（管道/重定向）直接打印用户块。任务从此以对话消息形式留在屏幕与历史中，对标 Claude Code 的用户气泡。

### 3. 历史完整展示与全文查看（对标 Claude Code 转录模式 / Codex resume）

**调研结论：** Claude Code 与 Codex 的共同模式是「完整渲染 + 滚动查看 + 超长内容折叠后按需展开」，均无「尾部摘要」设计。本方案（用户确认的「完整输出全部历史」选项）：

- **消息编号**：每条消息块前显示灰色序号 `[n]`（从 1 起），作为 `/view` 的定位依据
- **完整历史**：启动与 `/resume` 恢复会话后完整打印全部消息；用户/助手消息不再截断；工具结果限 12 行 / 600 字符，超出标注 `…（共 N 行，已省略，/view n 查看全文）`
- **`/history`**：默认显示全部消息（完整内容）；`/history N` 只看最近 N 条
- **`/view <编号>`**：打印单条消息全文（不限行、不限字符）；编号越界提示「消息编号无效，当前共 N 条消息（1–N）」

### 影响范围

| 文件 | 变更 |
|------|------|
| `src/guardrails/approval.rs` | 审批提示光标保存/恢复清除（DECSC/DECRC，终端判定） |
| `src/main.rs` | 用户消息块替换、消息编号、工具结果 12 行/600 字符截断提示、`/history N`、`/view`、全量恢复渲染 |
| `README.md` | REPL 命令表（/history、/view）、特性列表、示例更新 |
| `docs/superpowers/PLAN_REPL_CN.md` | 界面设计节更新、清单补项 |
| `docs/superpowers/specs/2026-08-13-history-viewing-design.md` | 新增设计文档 |

### 测试

所有 375 个测试通过（332 lib + 36 main + 3 mechanism_demo + 4 doctests），零编译警告；release 构建干净；REPL smoke 覆盖全量恢复、`/history`、`/history N`、`/view` 有效/越界/非法参数、`/resume` 选择器取消路径。

---

## 2026-08-13：思考模式回传修复 + REPL 界面重设计 + TUI 护栏审批打通

### 1. DeepSeek 思考模式 400 错误（reasoning_content 回传）

**症状：** 工具执行后的下一轮 LLM 调用返回 HTTP 400：
`"The 'reasoning_content' in the thinking mode must be passed back to the API."`

**根因：** DeepSeek 思考模式要求 assistant 响应中的 `reasoning_content` 字段必须在**后续请求的对应 assistant 消息中原样回传**，而我们的数据模型（`Message`、`LlmResponse`、API 线格式）完全没有这个字段，响应解析后即被丢弃。

**修复：**
- `Message` 与 `LlmResponse` 增加 `reasoning_content: Option<String>`（serde 带 `default` 与 `skip_serializing_if`，旧会话 JSON 文件可无缝加载）
- `OpenAiProvider` 解析响应中的 `reasoning_content`，并在请求序列化时按消息原样回传
- Agent 主循环把本轮响应的 `reasoning_content` 保存到 assistant 消息中，下一轮请求自动携带
- 新增 wiremock 线格式测试：验证响应解析 + 请求回传 + 非 assistant 消息不携带该字段

### 2. REPL 界面重设计（对标 Claude Code）

- **及时清空过期信息**：启动、`/resume`、`/clear` 后清屏并重绘界面；`/resume` 交互选择无论成功、失败还是取消，结束后都清理掉选择列表
- **历史展示与初次发信息同形式**：历史消息以对话消息块渲染（彩色角色标签 + 内容 + 灰色分隔线），与实时输出完全一致；带工具调用的 assistant 消息标注 `（工具调用：bash）`
- **彩色输出**：角色标签使用背景色块（用户蓝底白字、助手绿底黑字、工具紫底白字、系统灰底黑字）；`[调用]`、分隔线等辅助信息灰色；失败内容红色；错误信息红底白字块；`/help` 命令名青色
- 工具结果块限制行数与字数（4 行 / 200 字符），防止长输出刷屏

### 3. TUI 模式护栏审批无法确认（根因修复）

**症状：** TUI 模式下遇到护栏审批时无法确认——审批门直接读 stdin，与 crossterm raw mode 冲突；且 y/n 按键只更新界面状态，决定从未传回审批管线。

**修复：**
- `ApprovalGate` 新增 UI 事件模式（`with_ui_events`）：审批请求以 `GuardrailApprovalNeeded` 事件发给 UI，通过决策通道等待决定（带超时），批准后照常写入会话白名单
- TUI 的 y/n 按键把 `Approved`/`Denied` 决定经 mpsc 通道传回审批门，审批管线随即继续或拦截
- REPL 与纯文本模式保持 stdin 交互审批不变
- 新增测试：事件模式的批准/拒绝/超时/白名单、TUI 按键决定传回通道

### 影响范围

| 文件 | 变更 |
|------|------|
| `src/types.rs` | `Message`/`LlmResponse` 增加 `reasoning_content`（serde 向后兼容） |
| `src/llm/openai.rs` | 解析并回传 `reasoning_content`；新增线格式测试 |
| `src/loop/mod.rs` | assistant 消息保存本轮 `reasoning_content` |
| `src/guardrails/approval.rs` | UI 事件模式审批门（事件发出 + 决定通道 + 超时） |
| `src/tui/mod.rs` | y/n 决定经通道传回审批门；测试更新 |
| `src/main.rs` | TUI 决策通道装配；REPL 清屏重绘、消息块渲染、彩色输出 |

### 测试

所有 368 个测试通过（332 lib + 29 main + 3 mechanism_demo + 4 doctests），零编译警告。

---

## 2026-08-13：工具调用 400 错误根治 + 护栏中文化 + REPL 交互完善

### 1. 工具调用 API 400 错误（根因修复）

**症状：** 工具执行成功后，下一次 LLM 调用返回 HTTP 400：
`"An assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'. (insufficient tool messages following tool_calls message)"`

**根因：** 当 LLM 一次返回**多个** tool_calls 时，`ActionParser` 只解析并执行第一个工具调用，但 assistant 消息却保存了**全部** tool_calls。DeepSeek 等严格 API 要求 assistant 消息中的每个 tool_call_id 都必须被随后的 tool 消息逐一应答——未被执行的工具调用没有对应的 tool 消息，下一轮请求直接被拒。

**修复：** `src/loop/mod.rs` 中 assistant 消息只保存**实际被执行**的那个工具调用（按解析出的 action id 过滤），保证「assistant 消息中的每个 tool_call_id → 紧随其后的 tool 消息」这一 API 配对约束永远成立。新增回归测试 `test_agent_loop_multiple_tool_calls_keep_pairing`，对多工具调用响应验证配对不变量。

### 2. 护栏审批信息中文化

审批提示（原先的 `=== Guardrail Approval Required ===`）全部本地化：

```
=== 需要护栏审批 ===
操作: 工具调用: bash
参数: {"command":"uname -a && ..."}
风险等级: 高
原因:
  - 命令使用了链式操作符 &&
  - 命令使用了输出重定向
缓解措施: 执行前请仔细检查该命令及其影响
===================================
是否批准此操作? (y/n):
```

- 风险等级显示为中文：低 / 中 / 高 / 严重
- 三类评估器（命令/文件/网络）的原因与缓解措施全部中文化
- 沙箱违规消息（路径越界、命令黑名单、网络禁用等）中文化
- 内置静态规则名称与拦截原因中文化（如 `规则 'escalate-curl-pipe-bash' 触发：升级审批 curl | bash`）
- 审批超时、用户拒绝等错误信息中文化

### 3. Ctrl+C 退出 REPL

`Ctrl+C` 从「取消当前输入」改为**直接退出 REPL**（原先必须用 `/exit` 或 `Ctrl+D`，与终端使用习惯不符）。

### 4. 启动时展示恢复的历史

启动自动恢复会话后，不再只显示「共 N 条消息」，而是直接展示最近 5 条消息（角色 + 内容），用户无需翻 `.harness` 文件即可回忆上下文。`/history` 与启动展示共用同一渲染函数。

### 5. 交互式 /resume

`/resume` 不带参数时列出所有会话（带编号、消息数、autosave 标注），通过输入编号或名称选择恢复：

```
> /resume
请选择要恢复的会话：
  [1] autosave（自动保存）— 12 条消息
  [2] bugfix-login — 28 条消息
> 2
会话 'bugfix-login' 已恢复（28 条消息）。
```

### 影响范围

| 文件 | 变更 |
|------|------|
| `src/loop/mod.rs` | assistant 消息只保存实际执行的 tool_call；新增配对回归测试 |
| `src/guardrails/approval.rs` | 审批提示中文化、风险等级中文名、操作友好展示 |
| `src/guardrails/assessor.rs` | 评估原因与缓解措施中文化 |
| `src/guardrails/mod.rs` | 管线原因信息中文化 |
| `src/guardrails/rules.rs` | 内置规则名称与拦截原因中文化 |
| `src/guardrails/sandbox.rs` | 沙箱违规消息中文化 |
| `src/main.rs` | Ctrl+C 退出、启动历史展示、交互式 /resume、共享渲染函数 |
| `README.md` | REPL 章节更新 |

### 测试

所有 363 个测试通过（328 lib + 28 main + 3 mechanism_demo + 4 doctests），零编译警告。

---

## 2026-08-12：REPL 输入体验升级（rustyline + 自动保存 + 中文界面）

### 改进内容

#### 1. rustyline 输入编辑器

用 rustyline 替换了裸 `stdin().read_line()`：

- `↑`/`↓` 浏览输入历史（跨会话持久化到 `.harness/repl_history.txt`）
- `←`/`→` 行内光标移动编辑
- `Ctrl+A`/`Ctrl+E` 跳到行首/行尾
- `Ctrl+C` 取消当前输入（不再误退出）

#### 2. 输入区域分隔线

提示符前显示灰色分隔线（`─` × 48），使输入区域与 agent 输出清晰可辨。分隔线作为 rustyline 多行提示符的一部分，随输入框一起重绘。

#### 3. 中文界面

所有 REPL 系统提示本地化为中文：横幅、帮助、状态消息、错误信息、角色标签（系统/用户/助手/工具）、工具调用状态（调用/完成/失败）。

#### 4. 自动保存会话

- 每轮对话结束后自动保存到 `.harness/sessions/autosave.json`
- REPL 启动时自动恢复上次会话（`[已恢复上次会话，共 N 条消息]`）
- `/clear` 同时删除自动保存
- `/sessions` 中标注 autosave 为"（自动保存）"
- `/save <名称>` 仍可用于命名快照

#### 5. 移除冗余输出

删除 `[Running] <task>` 回显行——任务已在输入行显示，无需重复。

### 影响范围

| 文件 | 变更 |
|------|------|
| `Cargo.toml` | 新增 rustyline 15 依赖 |
| `src/main.rs` | run_repl 重写（rustyline、中文、自动保存）、会话管理函数中文化 |
| `README.md` | REPL 章节更新（新界面示例、快捷键、自动保存说明） |

---

## 2026-08-12：REPL 对话修复与界面专业化

### 修复的关键问题

#### 1. 对话历史失效（无上下文记忆）

**症状：** Agent 在多轮对话中无记忆，每次都表现为"第一次对话"。

**根因：** `ContextBuilder::build()` 将当前用户任务放在历史消息**之前**，导致消息时间顺序错乱：

```
旧顺序: [System, User(new_task), Assistant(old), Tool(old), Assistant(old2)]
新顺序: [System, User(old_task), Assistant(old), Tool(old), User(new_task)]
```

此外，REPL 的对话累积逻辑只保留 `Assistant` 和 `Tool` 消息，丢弃了 `User` 消息，使 LLM 看不到完整的对话流程。

**修复：**
- `ContextBuilder::build()` 改为按时间顺序排列：`[System, ...history, User(task)]`
- REPL 对话累积包含所有非 System 消息（含 User 消息）
- 影响范围：`src/loop/context.rs`, `src/main.rs`

#### 2. DeepSeek API tool_calls 400 错误

**症状：** `HTTP 400: "An assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'"`

**疑似原因：** 消息顺序错乱导致 Assistant(tool_calls) 与 Tool(tool_call_id) 的配对校验失败。

**修复：** 消息时间排序修正后，API 合法性约束自然满足。同时增加了 `tracing::debug!` 请求日志以便排查。

#### 3. 会话持久化

新增 `/save`、`/resume`、`/sessions` 命令，实现类似 Claude Code 的多对话管理：

- 会话保存为 `.harness/sessions/<name>.json`
- 使用 serde_json 序列化完整对话历史

#### 4. 界面专业化

移除所有 emoji 标记，替换为简洁的文本标签：

| 旧（emoji） | 新（文本） |
|-----------|----------|
| ⏳ Running agent for: "..." | [Running] ... |
| 🔧 Calling ... | [call] ... |
| ✅ ... | [ok] ... |
| ❌ ... | [FAIL] ... |
| ✅ Result: ... | [Result] ... |
| ❌ Error: ... | [Error] ... |
| 🤖 / 🔧 角色标签 | ASSIST / TOOL |

#### 5. API 调试日志

`OpenAiProvider` 现在在非 2xx 响应时记录完整请求体（`tracing::debug` 级别），方便排查 API 兼容性问题。

### 影响范围

| 文件 | 变更 |
|------|------|
| `src/loop/context.rs` | `build()` 改为 `[System, ...history, User(task)]` 顺序 |
| `src/main.rs` | REPL 对话累积、会话管理、界面文本化 |
| `src/llm/openai.rs` | 增加 debug 请求日志 |
| `README.md` | 更新示例、新增会话管理命令 |
| `docs/superpowers/PLAN_REPL_CN.md` | 新增中文 REPL 实现方案 |
| `docs/superpowers/ARCHITECTURE_CN.md` | 新增中文架构概述 |

### 测试

所有 362 个测试持续通过，零编译警告。

---

*维护者：luorong*
*最后更新：2026-08-13*
