# AuV 改名与两级配置系统 — 设计文档

> 日期：2026-08-13 ｜ 状态：已批准（用户确认 4 项关键决策）

## 目标

1. 项目改名为 **AuV harness agent**（简称 **AuV**）：二进制、品牌文案、默认提示词全改
2. 配置改为两级目录：全局 `~/.AuV/`、项目局部 `./.AuV/`，含默认模型、默认审批力度等
3. 用 `AuV.md` 加载角色说明，兼容已有角色说明文件（CLAUDE.md/AGENTS.md 等）

## 已确认的决策

| 问题 | 决策 |
|------|------|
| 配置文件名 | `config.toml`（`~/.AuV/config.toml` 与 `./.AuV/config.toml`） |
| 改名深度 | 品牌全改 + 二进制 `harness`→`auv`；lib/包名、内部类型名、`.harness` 目录、keyring 服务名**不动** |
| AuV.md | 两级（全局 + 项目），兼容已有 CLAUDE.md/AGENTS.md（优先使用，不改文件名） |
| 优先级 | 字段级合并：局部覆盖全局；项目根旧 `config.toml` **不再加载**（仅迁移提示） |

## 架构

### 1. 两级配置目录

```
~/.AuV/config.toml    ← 全局配置（用户级默认值）
./.AuV/config.toml    ← 局部配置（项目级覆盖；cwd == home 时不创建/不加载）
```

**加载逻辑（`load_config` 改造）：**

1. `--config <path>` 显式指定 → 只读该文件（现有行为不变）
2. 否则分层：
   - 全局 `~/.AuV/config.toml`：不存在 → 创建目录并写入默认配置；存在 → 原样读取，**绝不覆盖**
   - 局部 `./.AuV/config.toml`（仅当 cwd ≠ home）：同上
3. 合并：`toml::Value` 递归合并（局部键覆盖全局键，Vec 整体替换不追加）后反序列化为 `HarnessConfig`
4. 旧 `config.toml`：检测到存在时打印一行迁移提示，不自动迁移、不加载

### 2. AuV.md 角色说明（两级 + 兼容已有）

检测顺序（取第一个存在的文件）：

- 全局：`~/.AuV/AuV.md` → `~/CLAUDE.md` → `~/AGENTS.md`
- 项目：`./AuV.md` → `./CLAUDE.md` → `./AGENTS.md`

**合成规则（追加式叠加，低优先级在前）：**

```
默认提示词（AuV 品牌）
  + 全局角色文件（如有）
  + 项目角色文件（如有）
  + [agent] system_prompt 配置（如有，最高优先：取代默认 + 文件）
```

- 项目已有 CLAUDE.md/AGENTS.md 直接兼容，无需改文件名
- 两级都不存在 → 零打扰，不创建文件
- `[guardrails] rules_file` 规则片段机制保持不动

### 3. 命令与错误处理

- `auv init`：创建 `./.AuV/config.toml`（cwd == home 时创建 `~/.AuV/config.toml`），已存在则提示不覆盖
- 配置损坏（TOML 解析失败）→ 中文错误并退出，不静默回退默认
- 目录创建失败 → 中文警告，继续用默认配置运行

## 实现要点

| 文件 | 变更 |
|------|------|
| `Cargo.toml` | `[[bin]]` name `harness` → `auv` |
| `src/config/mod.rs` | 分层路径解析纯函数（home/cwd 参数注入）、`toml::Value` 递归合并、`write_default_config`（建目录 + 写默认 TOML）、分层加载返回 (config, 提示列表) |
| `src/main.rs` | `load_config` 改造、`init` 改造、横幅/CLI 文案、角色文件检测与组装（`default_system_prompt` 改 `pub(crate)`）、旧配置迁移提示 |
| `src/loop/context.rs` | 默认提示词改 AuV 品牌、`default_system_prompt` 可见性 |
| `README.md`、`docs/superpowers/` | 品牌更新、配置章节、CHANGELOG 条目 |

## 测试方案

- 路径解析、合并为纯函数（home/cwd 注入，避免测试并行 env 竞态；CWD 相关测试沿用静态锁串行化教训）
- 用例：
  - 全局不存在 → 创建且内容为默认；二次加载不改内容（幂等）
  - 局部字段级覆盖全局（如仅改 `[llm] model`，其余继承全局）
  - cwd == home → 不创建/不加载局部
  - 角色文件检测顺序（AuV.md > CLAUDE.md > AGENTS.md，全局与项目两级）
  - 旧 config.toml 不加载、产生迁移提示
  - 损坏 TOML → 报错退出
  - `--config` 显式路径优先
- 既有测试更新：`harness`→`auv` 文案断言、`load_config` 旧语义用例
- E2E 冒烟（tmux）：`auv` 启动自动创建 `~/.AuV/` 与 `./.AuV/`、AuV.md 角色生效、审批力度局部覆盖生效
