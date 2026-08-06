# SPEC 过程记录

## 冷启动试运行验证

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
