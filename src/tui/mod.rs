pub mod app;

use std::time::Duration;

use crate::error::HarnessError;
use crate::error::Result;
use crate::r#loop::AgentLoop;
use crate::types::{Message, Role};

use app::AppState;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tokio::sync::mpsc;

// ============================================================================
// Agent events sent from the background agent task to the TUI event loop.
// ============================================================================

/// Messages sent from the background agent task to the TUI.
enum AgentEvent {
    /// The agent loop has finished executing.
    Finished {
        result: std::result::Result<String, HarnessError>,
    },
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
/// 4. On exit (q/ESC key or agent completion), restores the terminal.
///
/// Communication between the TUI and the agent loop uses a
/// `tokio::sync::mpsc` channel.
pub async fn run_tui(mut agent: AgentLoop, task: String) -> Result<()> {
    // Channel for agent-to-TUI communication
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(32);

    // Spawn the agent loop in a background task
    let agent_handle = tokio::spawn(async move {
        let result = agent.run(&task).await;
        // Ignore send error — the TUI may have already exited
        let _ = tx.send(AgentEvent::Finished { result }).await;
    });

    // ---------- Terminal setup ----------
    let mut stdout = std::io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut app_state = AppState::new("gpt-4o");

    // ---------- Main event loop ----------
    let exit_code = run_event_loop(&mut terminal, &mut rx, &mut app_state).await;

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
async fn run_event_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    rx: &mut mpsc::Receiver<AgentEvent>,
    state: &mut AppState,
) -> Result<()> {
    loop {
        // Check for agent events (non-blocking)
        match rx.try_recv() {
            Ok(AgentEvent::Finished { result }) => {
                state.running = false;
                match result {
                    Ok(summary) => {
                        state.messages.push(Message {
                            role: Role::Assistant,
                            content: summary,
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                    Err(e) => {
                        state.messages.push(Message {
                            role: Role::System,
                            content: format!("Error: {}", e),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }
                // Draw final state
                terminal.draw(|f| draw_ui(f, state))?;
                return Ok(());
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                // Channel closed unexpectedly — agent task must have panicked
                state.running = false;
                state.messages.push(Message {
                    role: Role::System,
                    content: "Agent task terminated unexpectedly".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                });
                terminal.draw(|f| draw_ui(f, state))?;
                return Ok(());
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                // No event yet — continue
            }
        }

        // Check for input (poll with 100ms timeout so we don't busy-wait)
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        state.running = false;
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }

        // Draw UI
        terminal.draw(|f| draw_ui(f, state))?;
    }
}

// ============================================================================
// run_cli — Plain-text fallback for non-TTY environments
// ============================================================================

/// Run the agent loop in plain-text CLI mode.
///
/// This is a fallback for non-TTY environments (e.g., CI, redirected output).
/// It runs the agent loop directly and prints the result to stdout.
pub fn run_cli(mut agent: AgentLoop, task: String) -> Result<()> {
    println!("Starting agent loop for task: {}", task);
    println!("---");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| HarnessError::Config(format!("Failed to create tokio runtime: {}", e)))?;

    let result = rt.block_on(agent.run(&task));

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

/// Draw the TUI panels.
fn draw_ui(f: &mut ratatui::Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),     // Messages panel
            Constraint::Length(3),  // Status bar
        ])
        .split(f.area());

    // ---- Messages panel ----
    let messages_text = if state.messages.is_empty() {
        if state.running {
            "Running agent...".to_string()
        } else {
            "No messages".to_string()
        }
    } else {
        state
            .messages
            .iter()
            .map(|m| format!("[{:?}] {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let messages_widget = Paragraph::new(messages_text)
        .block(
            Block::default()
                .title("Messages")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(messages_widget, chunks[0]);

    // ---- Status bar ----
    let status_text = format!(
        "Turn: {} | Tokens: {} | Risk: {} | Model: {} | {}",
        state.status.turn,
        state.status.tokens_used,
        state.status.risk_level,
        state.status.model,
        if state.running { "Running..." } else { "Finished" }
    );

    if !state.guard_requests.is_empty() {
        let guard_text = format!(
            " | Pending approvals: {}",
            state.guard_requests.len()
        );
        let status_with_guards = format!("{}{}", status_text, guard_text);
        let status_widget = Paragraph::new(status_with_guards)
            .block(
                Block::default()
                    .title("Status")
                    .borders(Borders::ALL)
                    .style(Style::default().fg(Color::Yellow)),
            );
        f.render_widget(status_widget, chunks[1]);
    } else {
        let status_widget = Paragraph::new(status_text)
            .block(
                Block::default()
                    .title("Status")
                    .borders(Borders::ALL)
                    .style(Style::default().fg(Color::White)),
            );
        f.render_widget(status_widget, chunks[1]);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // app::AppState and app::StatusInfo tests are in app.rs; these tests
    // cover the module-level constructs.
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
            tool_calls: None,
            tool_call_id: None,
        });
        state.messages.push(Message {
            role: Role::Assistant,
            content: "done".to_string(),
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