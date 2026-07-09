use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::app::AppState;

/// Render the status bar: a single line at the bottom showing turn count,
/// token usage, model, risk level, and running state.
///
/// Colors are determined by risk level:
/// - Low / normal → Green
/// - Medium        → Yellow
/// - High / Critical → Red
pub fn render(f: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let risk_color = match state.status.risk_level.to_lowercase().as_str() {
        "high" | "critical" => Color::Red,
        "medium" => Color::Yellow,
        _ => Color::Green,
    };

    let running_text = if state.running {
        "Running..."
    } else {
        "Finished"
    };

    let status_text = format!(
        "Turn: {} | Tokens: {} | Risk: {} | Model: {} | {}",
        state.status.turn,
        state.status.tokens_used,
        state.status.risk_level,
        state.status.model,
        running_text,
    );

    let guard_text = if !state.guard_requests.is_empty() {
        format!(
            " | Pending approvals: {}",
            state.guard_requests.len()
        )
    } else {
        String::new()
    };

    let full_text = format!("{}{}", status_text, guard_text);

    let lines = vec![Line::from(Span::styled(
        full_text,
        Style::default().fg(Color::White),
    ))];

    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title("Status")
                .borders(Borders::ALL)
                .style(Style::default().fg(risk_color)),
        );

    f.render_widget(widget, area);
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{AppState, ApprovalRequest, StatusInfo};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    fn render_to_buffer(state: &AppState, width: u16, height: u16) -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
        let area = Rect::new(0, 0, width, height);
        build_widget(state).render(area, &mut buffer);
        buffer
    }

    fn build_widget(state: &AppState) -> Paragraph<'_> {
        let risk_color = match state.status.risk_level.to_lowercase().as_str() {
            "high" | "critical" => Color::Red,
            "medium" => Color::Yellow,
            _ => Color::Green,
        };

        let running_text = if state.running {
            "Running..."
        } else {
            "Finished"
        };

        let status_text = format!(
            "Turn: {} | Tokens: {} | Risk: {} | Model: {} | {}",
            state.status.turn,
            state.status.tokens_used,
            state.status.risk_level,
            state.status.model,
            running_text,
        );

        let guard_text = if !state.guard_requests.is_empty() {
            format!(" | Pending approvals: {}", state.guard_requests.len())
        } else {
            String::new()
        };

        let full_text = format!("{}{}", status_text, guard_text);

        let lines = vec![Line::from(Span::styled(
            full_text,
            Style::default().fg(Color::White),
        ))];

        Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .title("Status")
                    .borders(Borders::ALL)
                    .style(Style::default().fg(risk_color)),
            )
    }

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

    #[test]
    fn test_status_bar_default_state() {
        let state = AppState::new("gpt-4o");
        let buffer = render_to_buffer(&state, 80, 3);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("Status"),
            "Expected 'Status' title, got: {}",
            content
        );
        assert!(
            content.contains("Turn: 0"),
            "Expected turn count, got: {}",
            content
        );
        assert!(
            content.contains("Tokens: 0"),
            "Expected token count, got: {}",
            content
        );
        assert!(
            content.contains("Risk: Low"),
            "Expected risk level, got: {}",
            content
        );
        assert!(
            content.contains("Model: gpt-4o"),
            "Expected model name, got: {}",
            content
        );
        assert!(
            content.contains("Running..."),
            "Expected running state, got: {}",
            content
        );
    }

    #[test]
    fn test_status_bar_finished_state() {
        let mut state = AppState::new("gpt-4o");
        state.stop();
        state.status.turn = 5;
        state.status.tokens_used = 1234;

        let buffer = render_to_buffer(&state, 80, 3);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("Finished"),
            "Expected 'Finished', got: {}",
            content
        );
        assert!(
            content.contains("Turn: 5"),
            "Expected turn 5, got: {}",
            content
        );
        assert!(
            content.contains("Tokens: 1234"),
            "Expected token count, got: {}",
            content
        );
    }

    #[test]
    fn test_status_bar_risk_level_colors() {
        // Test Low risk
        let mut state = AppState::new("gpt-4o");
        state.status.risk_level = "Low".to_string();
        let buffer = render_to_buffer(&state, 80, 3);
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("Risk: Low"),
            "Expected Low risk, got: {}",
            content
        );

        // Test Medium risk
        state.status.risk_level = "Medium".to_string();
        let buffer = render_to_buffer(&state, 80, 3);
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("Risk: Medium"),
            "Expected Medium risk, got: {}",
            content
        );

        // Test High risk
        state.status.risk_level = "High".to_string();
        let buffer = render_to_buffer(&state, 80, 3);
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("Risk: High"),
            "Expected High risk, got: {}",
            content
        );

        // Test Critical risk
        state.status.risk_level = "Critical".to_string();
        let buffer = render_to_buffer(&state, 80, 3);
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("Risk: Critical"),
            "Expected Critical risk, got: {}",
            content
        );
    }

    #[test]
    fn test_status_bar_with_pending_approvals() {
        let mut state = AppState::new("gpt-4o");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "test".to_string(),
            "High".to_string(),
            vec![],
        ));
        state.add_guard_request(ApprovalRequest::new(
            "req-2".to_string(),
            "test2".to_string(),
            "Medium".to_string(),
            vec![],
        ));

        let buffer = render_to_buffer(&state, 120, 3);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("Pending approvals: 2"),
            "Expected pending approvals count, got: {}",
            content
        );
    }

    #[test]
    fn test_status_bar_custom_status_info() {
        let mut state = AppState::new("claude-sonnet-4-20250514");
        state.status = StatusInfo {
            turn: 42,
            tokens_used: 99999,
            risk_level: "Medium".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
        };

        let buffer = render_to_buffer(&state, 80, 3);
        let content = buffer_to_string(&buffer);

        assert!(content.contains("Turn: 42"));
        assert!(content.contains("Tokens: 99999"));
        assert!(content.contains("Risk: Medium"));
        assert!(content.contains("Model: claude-sonnet-4-20250514"));
    }
}