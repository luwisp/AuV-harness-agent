# Interactive REPL Mode — Implementation Plan

> **Goal:** Convert the harness from one-shot `harness run "task"` to an interactive REPL, like Claude Code. When no subcommand is given, `harness` enters an interactive session where the user types tasks continuously, the agent responds, and tool calls are displayed in real-time.

**Architecture:** The REPL is a thin interaction layer on top of the existing `AgentLoop`. The agent loop already supports continuous conversation through `AgentLoop::run()` — the main change is to wrap it in a stdin/stdout loop that maintains conversation history across turns and renders agent events (tool calls, progress, guardrail approvals) inline.

**Tech Stack:** Rust (edition 2024), tokio, clap, crossterm (for terminal detection)

## Global Constraints

- Rust edition: 2024
- Async runtime: tokio (multi-threaded)
- All existing tests must continue to pass
- No breaking changes to the `AgentLoop` public API
- TUI mode (`run_tui`) remains unchanged — REPL is a separate path
- REPL must work in plain-text mode (no TUI dependency)
- `harness` (no args) → REPL; `harness run "task"` → one-shot (unchanged)
- `/exit`, `/quit`, or Ctrl+D to exit REPL
- Ctrl+C exits gracefully

---

## Task 1: Modify CLI to support no-args REPL mode

**Files:**
- Modify: `src/main.rs`

**Produces:** `harness` (no args) enters REPL instead of showing "requires a subcommand" error

### Changes:

1. Change the `Cli` struct so that `command` is optional:
   ```rust
   #[command(subcommand)]
   command: Option<Commands>,
   ```

2. In `main()`, match on `cli.command`:
   - `None` → enter REPL
   - `Some(Commands::Run { ... })` → existing behavior
   - `Some(Commands::Init)` → existing behavior
   - `Some(Commands::Key { ... })` → existing behavior

3. Update CLI tests:
   - `test_no_subcommand_shows_error` → change to `test_no_subcommand_enters_repl` (verify `command` is `None`)
   - All other existing tests must pass unchanged

---

## Task 2: Implement REPL interaction loop

**Files:**
- Modify: `src/main.rs`

**Produces:** `run_repl()` function that reads stdin, calls agent, displays results

### Implementation:

```rust
async fn run_repl(config: HarnessConfig, workspace: PathBuf) -> Result<()> {
    let api_key = resolve_api_key(&config)?;
    let mut agent = build_agent(&config, &api_key, workspace)?;

    println!("HarnessAgent REPL v0.1.0");
    println!("Type a task for the agent, or /exit to quit.");
    println!("Type /help for available commands.\n");

    let mut conversation: Vec<Message> = Vec::new();

    loop {
        // Read input
        let line = read_input()?;  // Ctrl+D → None → exit

        match line {
            Some(input) => {
                let trimmed = input.trim();
                if trimmed.is_empty() { continue; }
                if trimmed == "/exit" || trimmed == "/quit" { break; }
                if trimmed == "/help" {
                    print_help();
                    continue;
                }
                // Pass to agent and display results
                run_agent_turn(&mut agent, trimmed, &mut conversation).await?;
            }
            None => break, // Ctrl+D
        }
    }

    println!("Goodbye.");
    Ok(())
}
```

### Key design decisions:

1. **Conversation history**: `Vec<Message>` accumulates across turns. Each turn, the user's task is appended as a `User` message, the agent runs, and the agent's messages are appended.

2. **Agent reuse**: The same `AgentLoop` instance is used across turns. The agent's internal state (guardrails, tools, memory) persists.

3. **Input**: Use `std::io::stdin().read_line()` wrapped in a helper that returns `Option<String>` (None on EOF/Ctrl+D).

4. **Output**: Agent events (tool calls, progress) are printed inline using `println!` with clear formatting.

5. **Error handling**: Agent errors are printed but don't terminate the REPL. Only fatal errors (channel disconnect, etc.) exit.

---

## Task 3: Add real-time agent event display in REPL

**Files:**
- Modify: `src/main.rs`

**Produces:** REPL shows tool calls, results, guardrail requests, and progress inline

### Implementation:

Modify `AgentLoop` (or add a wrapper) to emit events during execution. Since the current `AgentLoop::run()` is a black box that returns a final result, we need one of:

1. **Option A**: Add an `AgentLoop::run_streaming()` method that takes a callback/channel for events
2. **Option B**: Parse the trace log after each run and display events
3. **Option C**: Keep it simple — just run the agent and display the final result with tool call info extracted from the trace

**Recommended: Option C** for this iteration. The REPL prints:
- The user's task
- "Agent is thinking..." spinner or status line
- Final result
- Summary of tool calls made (from trace log)
- Any guardrail decisions

This avoids changing the `AgentLoop` API while still providing useful feedback.

---

## Task 4: Run tests and verify

- All 362 existing tests must pass
- New CLI tests: `test_no_subcommand_enters_repl`
- Manual verification: `cargo run` → REPL starts, type `/help`, type `/exit`

---

## Dependency Graph

```
Task 1 (CLI changes) → Task 2 (REPL loop) → Task 3 (Event display) → Task 4 (Tests)
```

Task 2 and 3 are tightly coupled — they can be done in a single task.