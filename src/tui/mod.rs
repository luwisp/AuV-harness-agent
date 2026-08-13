pub mod app;
pub mod panels;

use std::time::Duration;

use crate::error::Result;
use crate::events::AgentEvent;
use crate::guardrails::approval::ApprovalDecision;
use crate::r#loop::AgentLoop;
use crate::types::{Message, Role};

use app::AppState;
#[cfg(test)]
use app::Focus;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use tokio::sync::mpsc;

// ============================================================================
// Key actions — extracted for testability
// ============================================================================

/// The logical action produced by a key press, given the current app state.
#[derive(Debug, PartialEq, Eq)]
enum KeyAction {
    /// Exit the TUI.
    Quit,
    /// Approve the first pending guardrail request.
    ApproveGuardrail,
    /// Deny the first pending guardrail request.
    DenyGuardrail,
    /// Confirm the current action.
    Confirm,
    /// Switch keyboard focus to the next panel.
    SwitchFocus,
    /// No action to take.
    None,
}

/// Map a key event to a logical action based on the current app state.
fn handle_key(key: &KeyEvent, state: &AppState) -> KeyAction {
    match key.code {
        KeyCode::Char('q') => KeyAction::Quit,
        KeyCode::Esc => KeyAction::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyAction::Quit,
        KeyCode::Char('y') if !state.guard_requests.is_empty() => KeyAction::ApproveGuardrail,
        KeyCode::Char('n') if !state.guard_requests.is_empty() => KeyAction::DenyGuardrail,
        KeyCode::Enter => KeyAction::Confirm,
        KeyCode::Tab => KeyAction::SwitchFocus,
        _ => KeyAction::None,
    }
}

/// Apply a key action to the application state, returning whether the TUI
/// should exit.
///
/// `decision_tx` 把父审批决定传回 `ApprovalGate`（事件模式），使
/// y/n 按键真正驱动审批流程；子审批请求自带 reply_tx，决定优先经
/// 该通道直接回发给子 loop 的审批门。测试传 `None` 时仅更新界面状态。
fn apply_key_action(
    action: KeyAction,
    state: &mut AppState,
    decision_tx: Option<&mpsc::Sender<ApprovalDecision>>,
) -> bool {
    match action {
        KeyAction::Quit => {
            state.stop();
            true
        }
        KeyAction::ApproveGuardrail => {
            // Remove the first pending guardrail request and notify the gate
            if let Some(req) = state.guard_requests.first() {
                let id = req.request.id.clone();
                // 子审批：决定发回请求自带的 reply_tx；父审批：走全局决策通道
                let reply_tx = req.reply_tx.clone();
                state.remove_guard_request(&id);
                let decision = ApprovalDecision::Approved {
                    by: "user".to_string(),
                    reason: Some("用户批准了该操作".to_string()),
                };
                if let Some(tx) = reply_tx {
                    let _ = tx.try_send(decision);
                } else if let Some(tx) = decision_tx {
                    let _ = tx.try_send(decision);
                }
            }
            false
        }
        KeyAction::DenyGuardrail => {
            // Remove the first pending guardrail request and notify the gate
            if let Some(req) = state.guard_requests.first() {
                let id = req.request.id.clone();
                // 子审批：决定发回请求自带的 reply_tx；父审批：走全局决策通道
                let reply_tx = req.reply_tx.clone();
                state.remove_guard_request(&id);
                let decision = ApprovalDecision::Denied {
                    reason: "用户拒绝了该操作".to_string(),
                };
                if let Some(tx) = reply_tx {
                    let _ = tx.try_send(decision);
                } else if let Some(tx) = decision_tx {
                    let _ = tx.try_send(decision);
                }
            }
            false
        }
        KeyAction::Confirm => {
            // Confirm is a no-op for now; future iterations may use it to
            // submit input or advance the agent loop.
            false
        }
        KeyAction::SwitchFocus => {
            state.focus = state.focus.next();
            false
        }
        KeyAction::None => false,
    }
}

// ============================================================================
// run_tui — Full TUI mode
// ============================================================================

/// Run the agent loop with a full ratatui TUI.
///
/// This function:
/// 1. Initializes the terminal (alternate screen, raw mode).
/// 2. Spawns the agent loop in a background tokio task.
/// 3. Runs the main event loop: draws panels, handles input.
/// 4. On exit (q/ESC/Ctrl+C key), restores the terminal.
///
/// The TUI stays open after the agent finishes so the user can review
/// results — press `q` to exit.
pub async fn run_tui(
    mut agent: AgentLoop,
    task: String,
    mut rx: mpsc::Receiver<AgentEvent>,
    decision_tx: mpsc::Sender<ApprovalDecision>,
    model: String,
) -> Result<()> {
    // Spawn the agent loop in a background task.
    // Events are emitted through the channel inside AgentLoop (event_tx),
    // so the spawned task just awaits the agent — no manual sends needed.
    let agent_handle = tokio::spawn(async move {
        let _ = agent.run(&task).await;
    });

    // ---------- Terminal setup ----------
    let mut stdout = std::io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    // 状态栏显示实际配置的模型（不再硬编码）
    let mut app_state = AppState::new(&model);

    // ---------- Main event loop ----------
    let exit_code = run_event_loop(&mut terminal, &mut rx, &mut app_state, &decision_tx).await;

    // ---------- Terminal cleanup ----------
    terminal::disable_raw_mode()?;
    terminal
        .backend_mut()
        .execute(LeaveAlternateScreen)?;

    // Cancel the agent task if the user quit early
    if !agent_handle.is_finished() {
        agent_handle.abort();
    }

    exit_code
}

/// Drive the main event loop: poll for input, check for agent events, draw.
///
/// The loop continues even after the agent finishes (channel disconnects),
/// so the user can review results. Only user input (`q`/`Esc`/`Ctrl+C`)
/// causes the loop to exit.
async fn run_event_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    rx: &mut mpsc::Receiver<AgentEvent>,
    state: &mut AppState,
    decision_tx: &mpsc::Sender<ApprovalDecision>,
) -> Result<()> {
    let mut agent_done = false;

    loop {
        // Check for agent events (non-blocking, skip if agent is done)
        if !agent_done {
            match rx.try_recv() {
                Ok(event) => {
                    let is_finished = matches!(event, AgentEvent::Finished { .. });
                    apply_agent_event(event, state);
                    if is_finished {
                        agent_done = true;
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // Agent task ended. If we never saw Finished, it crashed.
                    if !agent_done {
                        state.messages.push(Message {
                            role: Role::System,
                            content: "Agent task terminated unexpectedly".to_string(),
                            reasoning_content: None,
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                    agent_done = true; // Stop polling the channel
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    // No event yet — continue
                }
            }
        }

        // Check for input (poll with 100ms timeout so we don't busy-wait)
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                let action = handle_key(&key, state);
                if apply_key_action(action, state, Some(decision_tx)) {
                    // User requested quit
                    return Ok(());
                }
            }
        }

        // Draw UI
        terminal.draw(|f| draw_ui(f, state))?;
    }
}

/// Apply an agent event to the application state.
fn apply_agent_event(event: AgentEvent, state: &mut AppState) {
    match event {
        AgentEvent::MessageAdded { message } => {
            state.messages.push(message);
        }
        AgentEvent::ToolCallStarted { name, detail } => {
            // 工具面板展示具体命令：bash 显示命令行，其余工具显示参数 JSON
            let label = if detail.is_empty() {
                name
            } else {
                format!("{}: {}", name, detail)
            };
            state.set_current_tool(Some(label));
        }
        AgentEvent::ToolCallCompleted {
            name: _name,
            detail: _detail,
            result_content,
            success,
        } => {
            state.set_current_tool(None);
            state.add_tool_result(crate::types::ToolResult {
                success,
                content: result_content,
                structured: None,
                artifacts: vec![],
            });
        }
        AgentEvent::GuardrailApprovalNeeded { request } => {
            state.add_guard_request(request);
        }
        AgentEvent::SubagentApprovalNeeded {
            request,
            label,
            decision_tx,
        } => {
            // 子审批：请求入列并携带回发通道与来源标签，
            // y/n 决定经该通道直接回发给子 loop 的审批门
            state.add_guard_request_with_reply(request, label, decision_tx);
        }
        AgentEvent::ProgressUpdate {
            turn,
            tokens_used,
            risk_level,
        } => {
            state.status.turn = turn;
            state.status.tokens_used = tokens_used;
            state.status.risk_level = risk_level;
        }
        AgentEvent::Finished { result } => {
            state.running = false;
            match result {
                Ok(summary) => {
                    state.messages.push(Message {
                        role: Role::Assistant,
                        content: summary,
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                Err(e) => {
                    state.messages.push(Message {
                        role: Role::System,
                        content: format!("Error: {}", e),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }
        }
    }
}

// ============================================================================
// run_cli — Plain-text fallback for non-TTY environments
// ============================================================================

/// Run the agent loop in plain-text CLI mode.
///
/// This is a fallback for non-TTY environments (e.g., CI, redirected output).
/// It runs the agent loop directly and prints the result to stdout.
pub async fn run_cli(mut agent: AgentLoop, task: String) -> Result<()> {
    println!("Starting agent loop for task: {}", task);
    println!("---");

    let result = agent.run(&task).await;

    match result {
        Ok(summary) => {
            println!("Result: {}", summary);
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            Err(e)
        }
    }
}

// ============================================================================
// UI drawing
// ============================================================================

/// Draw the TUI panels using the horizontal-split layout:
///
/// ```text
/// ┌──────────────────────────────┬───────────────────────┐
/// │                              │       Tools            │
/// │      Conversation (70%)      ├───────────────────────┤
/// │                              │     Guardrails         │
/// │                              │                       │
/// ├──────────────────────────────┴───────────────────────┤
/// │                  Status Bar (1 line)                  │
/// └──────────────────────────────────────────────────────┘
/// ```
fn draw_ui(f: &mut ratatui::Frame, state: &AppState) {
    let area = f.area();

    // Split into main area and status bar (1 line at bottom)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),     // Main area
            Constraint::Length(1),  // Status bar
        ])
        .split(area);

    // Split main area into conversation (70%) and right panel (30%)
    let main_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(70),  // Conversation
            Constraint::Percentage(30),  // Right panel
        ])
        .split(main_chunks[0]);

    // Split right panel into tools (top) and guardrails (bottom)
    let right_col = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),  // Tools
            Constraint::Percentage(50),  // Guardrails
        ])
        .split(main_row[1]);

    panels::conversation::render(f, main_row[0], state);
    panels::tools::render(f, right_col[0], state);
    panels::guardrails::render(f, right_col[1], state);
    panels::status::render(f, main_chunks[1], state);
}

// ============================================================================
// Layout helpers (public for testing)
// ============================================================================

/// Compute the layout chunks for a given terminal area.
///
/// Returns `(conversation, tools, guardrails, status)` areas.
#[allow(dead_code)]
pub(crate) fn compute_layout(area: Rect) -> (Rect, Rect, Rect, Rect) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let main_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(main_chunks[0]);

    let right_col = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(main_row[1]);

    (main_row[0], right_col[0], right_col[1], main_chunks[1])
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ApprovalRequest;

    // -----------------------------------------------------------------------
    // Layout tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_layout_sections_exist() {
        let area = Rect::new(0, 0, 120, 30);
        let (conv, tools, guardrails, status) = compute_layout(area);

        // Status bar must be exactly 1 line
        assert_eq!(
            status.height, 1,
            "Status bar should be 1 line, got {}",
            status.height
        );

        // Status bar spans the full width
        assert_eq!(
            status.width, area.width,
            "Status bar should span full width, got {}",
            status.width
        );

        // Conversation is approximately 70% of width
        let expected_conv_width = (area.width as f32 * 0.70) as u16;
        let width_diff = if conv.width > expected_conv_width {
            conv.width - expected_conv_width
        } else {
            expected_conv_width - conv.width
        };
        assert!(
            width_diff <= 1,
            "Conversation width {} should be approximately 70% of {} (expected ~{})",
            conv.width,
            area.width,
            expected_conv_width
        );

        // Right panel is approximately 30% of width
        let right_width = tools.width;
        let expected_right_width = (area.width as f32 * 0.30) as u16;
        let right_diff = if right_width > expected_right_width {
            right_width - expected_right_width
        } else {
            expected_right_width - right_width
        };
        assert!(
            right_diff <= 1,
            "Right panel width {} should be approximately 30% of {} (expected ~{})",
            right_width,
            area.width,
            expected_right_width
        );

        // Tools and guardrails have the same width (they share the right column)
        assert_eq!(
            tools.width, guardrails.width,
            "Tools and guardrails should have the same width"
        );

        // Tools and guardrails are in the right column
        assert!(
            tools.x > conv.x + conv.width - 1,
            "Tools should be to the right of conversation"
        );
        assert!(
            guardrails.x > conv.x + conv.width - 1,
            "Guardrails should be to the right of conversation"
        );

        // Guardrails is below tools
        assert!(
            guardrails.y > tools.y,
            "Guardrails should be below tools"
        );

        // Status bar is at the bottom
        assert!(
            status.y > guardrails.y,
            "Status bar should be below guardrails"
        );

        // All sections have non-zero area
        assert!(conv.width > 0 && conv.height > 0);
        assert!(tools.width > 0 && tools.height > 0);
        assert!(guardrails.width > 0 && guardrails.height > 0);
        assert!(status.width > 0 && status.height > 0);
    }

    #[test]
    fn test_layout_small_terminal() {
        // Even with a very small terminal, the layout should still produce
        // valid areas.
        let area = Rect::new(0, 0, 40, 10);
        let (conv, tools, guardrails, status) = compute_layout(area);

        // Status bar is still 1 line
        assert_eq!(status.height, 1);

        // All areas have non-zero dimensions
        assert!(conv.width > 0);
        assert!(tools.width > 0);
        assert!(guardrails.width > 0);

        // Conversation is wider than the right panel
        assert!(conv.width > tools.width);
    }

    // -----------------------------------------------------------------------
    // Keybinding tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_keybindings_quit() {
        let state = AppState::new("test-model");

        // 'q' quits
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE), &state),
            KeyAction::Quit
        );

        // Esc quits
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &state),
            KeyAction::Quit
        );

        // Ctrl+C quits
        assert_eq!(
            handle_key(
                &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &state
            ),
            KeyAction::Quit
        );
    }

    #[test]
    fn test_keybindings_tab_switches_focus() {
        let state = AppState::new("test-model");
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &state),
            KeyAction::SwitchFocus
        );
    }

    #[test]
    fn test_keybindings_enter_confirm() {
        let state = AppState::new("test-model");
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &state),
            KeyAction::Confirm
        );
    }

    #[test]
    fn test_guardrail_approval_input() {
        let mut state = AppState::new("test-model");

        // Without guardrail requests, y/n should not trigger approval
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE), &state),
            KeyAction::None
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE), &state),
            KeyAction::None
        );

        // Add a guardrail request
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "bash: rm -rf /tmp/test".to_string(),
            "High".to_string(),
            vec!["Destructive command".to_string()],
            None));

        // With guardrail requests, 'y' should approve
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE), &state),
            KeyAction::ApproveGuardrail
        );

        // 'n' should deny
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE), &state),
            KeyAction::DenyGuardrail
        );
    }

    #[test]
    fn test_apply_guardrail_approval_removes_request_and_notifies_gate() {
        let mut state = AppState::new("test-model");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "test action".to_string(),
            "High".to_string(),
            vec!["reason".to_string()],
            None));
        assert_eq!(state.guard_requests.len(), 1);

        // Apply approval should remove the request and send the decision
        // back to the approval gate through the decision channel.
        let (tx, mut rx) = mpsc::channel(1);
        let should_quit = apply_key_action(KeyAction::ApproveGuardrail, &mut state, Some(&tx));
        assert!(!should_quit);
        assert!(
            state.guard_requests.is_empty(),
            "Guardrail request should be removed after approval"
        );
        let decision = rx.try_recv().expect("approval decision sent to gate");
        assert_eq!(
            decision,
            ApprovalDecision::Approved {
                by: "user".to_string(),
                reason: Some("用户批准了该操作".to_string()),
            }
        );
    }

    #[test]
    fn test_apply_guardrail_deny_removes_request_and_notifies_gate() {
        let mut state = AppState::new("test-model");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "test action".to_string(),
            "High".to_string(),
            vec!["reason".to_string()],
            None));
        assert_eq!(state.guard_requests.len(), 1);

        // Apply denial should remove the request and send the decision
        // back to the approval gate through the decision channel.
        let (tx, mut rx) = mpsc::channel(1);
        let should_quit = apply_key_action(KeyAction::DenyGuardrail, &mut state, Some(&tx));
        assert!(!should_quit);
        assert!(
            state.guard_requests.is_empty(),
            "Guardrail request should be removed after denial"
        );
        let decision = rx.try_recv().expect("denial decision sent to gate");
        assert_eq!(
            decision,
            ApprovalDecision::Denied {
                reason: "用户拒绝了该操作".to_string(),
            }
        );
    }

    #[test]
    fn test_apply_quit_stops_state() {
        let mut state = AppState::new("test-model");
        assert!(state.running);

        let should_quit = apply_key_action(KeyAction::Quit, &mut state, None);
        assert!(should_quit);
        assert!(!state.running);
    }

    #[test]
    fn test_apply_switch_focus_cycles() {
        let mut state = AppState::new("test-model");
        assert_eq!(state.focus, Focus::Conversation);

        apply_key_action(KeyAction::SwitchFocus, &mut state, None);
        assert_eq!(state.focus, Focus::Tools);

        apply_key_action(KeyAction::SwitchFocus, &mut state, None);
        assert_eq!(state.focus, Focus::Guardrails);

        apply_key_action(KeyAction::SwitchFocus, &mut state, None);
        assert_eq!(state.focus, Focus::Status);

        apply_key_action(KeyAction::SwitchFocus, &mut state, None);
        assert_eq!(state.focus, Focus::Conversation);
    }

    #[test]
    fn test_apply_key_action_confirm_noop() {
        let mut state = AppState::new("test-model");
        let should_quit = apply_key_action(KeyAction::Confirm, &mut state, None);
        assert!(!should_quit);
        assert!(state.running);
    }

    #[test]
    fn test_apply_key_action_none_noop() {
        let mut state = AppState::new("test-model");
        let should_quit = apply_key_action(KeyAction::None, &mut state, None);
        assert!(!should_quit);
        assert!(state.running);
    }

    // -----------------------------------------------------------------------
    // Agent event tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_apply_agent_event_message_added() {
        let mut state = AppState::new("test-model");
        let msg = Message {
            role: Role::Assistant,
            content: "Hello".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        };
        apply_agent_event(AgentEvent::MessageAdded {
            message: msg.clone(),
        }, &mut state);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].content, "Hello");
    }

    #[test]
    fn test_apply_agent_event_tool_call_started() {
        let mut state = AppState::new("test-model");
        apply_agent_event(
            AgentEvent::ToolCallStarted {
                name: "bash".to_string(),
                detail: "uname -a".to_string(),
            },
            &mut state,
        );
        assert_eq!(state.current_tool, Some("bash: uname -a".to_string()));
    }

    #[test]
    fn test_apply_agent_event_tool_call_completed() {
        let mut state = AppState::new("test-model");
        state.set_current_tool(Some("bash".to_string()));

        apply_agent_event(
            AgentEvent::ToolCallCompleted {
                name: "bash".to_string(),
                detail: "uname -a".to_string(),
                result_content: "echo: hello".to_string(),
                success: true,
            },
            &mut state,
        );

        assert_eq!(state.current_tool, None);
        assert_eq!(state.tool_results.len(), 1);
        assert!(state.tool_results[0].success);
        assert_eq!(state.tool_results[0].content, "echo: hello");
    }

    #[test]
    fn test_apply_agent_event_guardrail_approval() {
        let mut state = AppState::new("test-model");
        let req = ApprovalRequest::new(
            "req-1".to_string(),
            "test action".to_string(),
            "High".to_string(),
            vec!["reason".to_string()],
        None);

        apply_agent_event(
            AgentEvent::GuardrailApprovalNeeded {
                request: req.clone(),
            },
            &mut state,
        );

        assert_eq!(state.guard_requests.len(), 1);
        assert_eq!(state.guard_requests[0].request.id, "req-1");
        assert!(state.guard_requests[0].reply_tx.is_none());
        assert!(state.guard_requests[0].label.is_none());
    }

    #[test]
    fn test_apply_agent_event_subagent_approval() {
        let mut state = AppState::new("test-model");
        let req = ApprovalRequest::new(
            "req-sub-1".to_string(),
            "subagent: 计算 2+2".to_string(),
            "High".to_string(),
            vec!["subagent action".to_string()],
            None);
        let (decision_tx, _decision_rx) = mpsc::channel(4);

        apply_agent_event(
            AgentEvent::SubagentApprovalNeeded {
                request: req.clone(),
                label: "子 agent".to_string(),
                decision_tx: decision_tx.clone(),
            },
            &mut state,
        );

        assert_eq!(state.guard_requests.len(), 1);
        assert_eq!(state.guard_requests[0].request.id, "req-sub-1");
        assert!(state.guard_requests[0].reply_tx.is_some(), "子审批携带回发通道");
        assert_eq!(
            state.guard_requests[0].label.as_deref(),
            Some("子 agent"),
            "子审批携带来源标签"
        );
    }

    #[test]
    fn test_subagent_approval_routes_decision_through_reply_tx() {
        let mut state = AppState::new("test-model");
        let (reply_tx, mut reply_rx) = mpsc::channel(4);
        state.add_guard_request_with_reply(
            ApprovalRequest::new(
                "req-sub-1".to_string(),
                "subagent action".to_string(),
                "High".to_string(),
                vec!["reason".to_string()],
                None),
            "子 agent".to_string(),
            reply_tx,
        );
        // 全局决策通道存在时，子审批决定也不得走全局通道（避免与父审批串扰）
        let (global_tx, mut global_rx) = mpsc::channel(4);

        let should_quit =
            apply_key_action(KeyAction::ApproveGuardrail, &mut state, Some(&global_tx));
        assert!(!should_quit);
        assert!(state.guard_requests.is_empty(), "审批后请求应移除");

        let decision = reply_rx
            .try_recv()
            .expect("子审批决定应经 reply_tx 回发");
        assert_eq!(
            decision,
            ApprovalDecision::Approved {
                by: "user".to_string(),
                reason: Some("用户批准了该操作".to_string()),
            }
        );
        assert!(
            global_rx.try_recv().is_err(),
            "子审批决定不得走全局决策通道"
        );
    }

    #[test]
    fn test_subagent_denial_routes_through_reply_tx() {
        let mut state = AppState::new("test-model");
        let (reply_tx, mut reply_rx) = mpsc::channel(4);
        state.add_guard_request_with_reply(
            ApprovalRequest::new(
                "req-sub-1".to_string(),
                "subagent action".to_string(),
                "High".to_string(),
                vec!["reason".to_string()],
                None),
            "子 agent".to_string(),
            reply_tx,
        );

        let should_quit = apply_key_action(KeyAction::DenyGuardrail, &mut state, None);
        assert!(!should_quit);
        assert!(state.guard_requests.is_empty());

        let decision = reply_rx
            .try_recv()
            .expect("子审批拒绝决定应经 reply_tx 回发");
        assert_eq!(
            decision,
            ApprovalDecision::Denied {
                reason: "用户拒绝了该操作".to_string(),
            }
        );
    }

    #[test]
    fn test_apply_agent_event_progress_update() {
        let mut state = AppState::new("test-model");

        apply_agent_event(
            AgentEvent::ProgressUpdate {
                turn: 5,
                tokens_used: 1234,
                risk_level: "Medium".to_string(),
            },
            &mut state,
        );

        assert_eq!(state.status.turn, 5);
        assert_eq!(state.status.tokens_used, 1234);
        assert_eq!(state.status.risk_level, "Medium");
    }

    #[test]
    fn test_apply_agent_event_finished_ok() {
        let mut state = AppState::new("test-model");
        assert!(state.running);

        apply_agent_event(
            AgentEvent::Finished {
                result: Ok("All done!".to_string()),
            },
            &mut state,
        );

        assert!(!state.running);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].content, "All done!");
        assert_eq!(state.messages[0].role, Role::Assistant);
    }

    #[test]
    fn test_apply_agent_event_finished_err() {
        let mut state = AppState::new("test-model");
        assert!(state.running);

        apply_agent_event(
            AgentEvent::Finished {
                result: Err("MaxTurnsReached".to_string()),
            },
            &mut state,
        );

        assert!(!state.running);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].role, Role::System);
        assert!(state.messages[0].content.contains("Error"));
    }

    // -----------------------------------------------------------------------
    // Legacy tests (ported from the original test module)
    // -----------------------------------------------------------------------

    #[test]
    fn test_app_state_new_defaults() {
        let state = AppState::new("gpt-4o");
        assert!(state.running);
        assert!(state.messages.is_empty());
        assert!(state.tool_results.is_empty());
        assert!(state.guard_requests.is_empty());
        assert_eq!(state.current_tool, None);
        assert_eq!(state.status.turn, 0);
        assert_eq!(state.status.tokens_used, 0);
        assert_eq!(state.status.risk_level, "Low");
        assert_eq!(state.status.model, "gpt-4o");
    }

    #[test]
    fn test_app_state_stop() {
        let mut state = AppState::new("gpt-4o");
        assert!(state.running);
        state.stop();
        assert!(!state.running);
    }

    #[test]
    fn test_app_state_messages_accumulate() {
        let mut state = AppState::new("gpt-4o");
        state.messages.push(Message {
            role: Role::User,
            content: "task".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });
        state.messages.push(Message {
            role: Role::Assistant,
            content: "done".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].content, "task");
        assert_eq!(state.messages[1].content, "done");
    }

    #[test]
    fn test_app_state_tool_tracking() {
        let mut state = AppState::new("gpt-4o");
        assert_eq!(state.current_tool, None);

        state.set_current_tool(Some("bash".to_string()));
        assert_eq!(state.current_tool, Some("bash".to_string()));

        state.set_current_tool(None);
        assert_eq!(state.current_tool, None);
    }

    #[test]
    fn test_status_info_clone() {
        let a = app::StatusInfo::new("gpt-4o");
        let b = a.clone();
        assert_eq!(a, b);
    }
}