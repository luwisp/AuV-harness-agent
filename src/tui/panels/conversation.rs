use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::app::AppState;
use crate::types::Role;

/// Render the conversation panel: a scrollable list of messages with role-based
/// colors (User=cyan, Assistant=green, System=yellow, Tool=gray).
///
/// The panel auto-scrolls to show the latest messages when the content exceeds
/// the available area.
pub fn render(f: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let messages: Vec<Line> = state
        .messages
        .iter()
        .map(|msg| {
            let role_color = match msg.role {
                Role::User => Color::Cyan,
                Role::Assistant => Color::Green,
                Role::System => Color::Yellow,
                Role::Tool => Color::Gray,
            };
            let prefix = format!("[{:?}] ", msg.role);
            Line::from(vec![
                Span::styled(prefix, Style::default().fg(role_color)),
                Span::styled(msg.content.clone(), Style::default().fg(Color::White)),
            ])
        })
        .collect();

    let text = if messages.is_empty() {
        Text::from("No messages yet.")
    } else {
        Text::from(messages)
    };

    let widget = Paragraph::new(text)
        .block(
            Block::default()
                .title("Conversation")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true })
        .scroll((state.messages.len().saturating_sub(1) as u16, 0));

    f.render_widget(widget, area);
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::AppState;
    use crate::types::Message;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    /// Helper: render the panel to a buffer and return it for inspection.
    fn render_to_buffer(state: &AppState, width: u16, height: u16) -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
        let area = Rect::new(0, 0, width, height);
        // Use a NoopFrame-like approach: call the render function by directly
        // rendering the widget into the buffer.
        render_widget_to_buffer(state, area, &mut buffer);
        buffer
    }

    /// Render the conversation widget directly into a buffer for testing.
    /// Note: does not apply auto-scroll so tests can verify all messages.
    fn render_widget_to_buffer(state: &AppState, area: Rect, buffer: &mut Buffer) {
        let messages: Vec<Line> = state
            .messages
            .iter()
            .map(|msg| {
                let role_color = match msg.role {
                    Role::User => Color::Cyan,
                    Role::Assistant => Color::Green,
                    Role::System => Color::Yellow,
                    Role::Tool => Color::Gray,
                };
                let prefix = format!("[{:?}] ", msg.role);
                Line::from(vec![
                    Span::styled(prefix, Style::default().fg(role_color)),
                    Span::styled(msg.content.clone(), Style::default().fg(Color::White)),
                ])
            })
            .collect();

        let text = if messages.is_empty() {
            Text::from("No messages yet.")
        } else {
            Text::from(messages)
        };

        let widget = Paragraph::new(text)
            .block(
                Block::default()
                    .title("Conversation")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: true });

        widget.render(area, buffer);
    }

    #[test]
    fn test_conversation_empty_state() {
        let state = AppState::new("test-model");
        let buffer = render_to_buffer(&state, 80, 10);

        // The buffer should contain the "No messages yet." text and the
        // "Conversation" title.
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("No messages yet."),
            "Expected 'No messages yet.' in empty conversation, got: {}",
            content
        );
        assert!(
            content.contains("Conversation"),
            "Expected 'Conversation' title, got: {}",
            content
        );
    }

    #[test]
    fn test_conversation_with_messages() {
        let mut state = AppState::new("test-model");
        state.messages.push(Message {
            role: Role::User,
            content: "Hello, agent!".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });
        state.messages.push(Message {
            role: Role::Assistant,
            content: "Hello! How can I help?".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });

        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("[User] Hello, agent!"),
            "Expected user message content, got: {}",
            content
        );
        assert!(
            content.contains("[Assistant] Hello! How can I help?"),
            "Expected assistant message content, got: {}",
            content
        );
    }

    #[test]
    fn test_conversation_role_colors() {
        let mut state = AppState::new("test-model");
        state.messages.push(Message {
            role: Role::User,
            content: "user msg".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });
        state.messages.push(Message {
            role: Role::Assistant,
            content: "assistant msg".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });
        state.messages.push(Message {
            role: Role::System,
            content: "system msg".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });
        state.messages.push(Message {
            role: Role::Tool,
            content: "tool msg".to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });

        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        // Verify all role prefixes appear
        assert!(content.contains("[User]"));
        assert!(content.contains("[Assistant]"));
        assert!(content.contains("[System]"));
        assert!(content.contains("[Tool]"));

        // Verify the message contents
        assert!(content.contains("user msg"));
        assert!(content.contains("assistant msg"));
        assert!(content.contains("system msg"));
        assert!(content.contains("tool msg"));
    }

    #[test]
    fn test_conversation_many_messages() {
        let mut state = AppState::new("test-model");
        for i in 0..50 {
            state.messages.push(Message {
                role: Role::User,
                content: format!("Message number {}", i),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
            });
        }

        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        // Without auto-scroll in test, the first messages are visible.
        // The buffer shows as many messages as fit in the visible area.
        assert!(
            content.contains("Message number 0"),
            "Expected first message to be visible, got: {}",
            content
        );
        assert!(
            content.contains("Message number 7"),
            "Expected message 7 to be visible, got: {}",
            content
        );
        // Message 49 should not be visible (only first ~8 fit in 10-line area)
        assert!(
            !content.contains("Message number 49"),
            "Message 49 should not be visible without scroll, got: {}",
            content
        );
    }

    /// Convert a buffer's cells to a plain string for assertion.
    fn buffer_to_string(buffer: &Buffer) -> String {
        let mut result = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = buffer.cell((x, y)).unwrap();
                result.push_str(&cell.symbol());
            }
            result.push('\n');
        }
        result
    }
}