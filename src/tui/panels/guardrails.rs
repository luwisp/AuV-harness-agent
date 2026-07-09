use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::app::AppState;

/// Render the guardrails panel: shows pending approval requests with risk
/// details and prompts the user for y/n input.
///
/// High and Critical risk levels are highlighted in red. Medium uses yellow.
/// The panel displays a "Press y to approve, n to deny" prompt when there are
/// pending requests.
pub fn render(f: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let mut lines: Vec<Line> = Vec::new();

    if state.guard_requests.is_empty() {
        lines.push(Line::from(Span::styled(
            "No pending approval requests.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let header = format!(
            "Pending Approvals ({} request{}):",
            state.guard_requests.len(),
            if state.guard_requests.len() == 1 { "" } else { "s" }
        );
        lines.push(Line::from(Span::styled(
            header,
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));

        for request in &state.guard_requests {
            // Risk level with color
            let risk_color = match request.risk_level.to_lowercase().as_str() {
                "high" | "critical" => Color::Red,
                "medium" => Color::Yellow,
                _ => Color::Green,
            };
            lines.push(Line::from(vec![
                Span::styled("  Risk: ", Style::default().fg(Color::White)),
                Span::styled(
                    request.risk_level.clone(),
                    Style::default().fg(risk_color).add_modifier(
                        ratatui::style::Modifier::BOLD,
                    ),
                ),
            ]));

            // Action summary
            lines.push(Line::from(vec![
                Span::styled("  Action: ", Style::default().fg(Color::White)),
                Span::styled(
                    request.action_summary.clone(),
                    Style::default().fg(Color::Cyan),
                ),
            ]));

            // Reasons
            if !request.reasons.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  Reasons:",
                    Style::default().fg(Color::White),
                )));
                for reason in &request.reasons {
                    lines.push(Line::from(Span::styled(
                        format!("    - {}", reason),
                        Style::default().fg(Color::Gray),
                    )));
                }
            }

            // ID
            lines.push(Line::from(Span::styled(
                format!("  ID: {}", request.id),
                Style::default().fg(Color::DarkGray),
            )));

            lines.push(Line::from(""));
        }

        // Prompt
        lines.push(Line::from(Span::styled(
            "Press y to approve, n to deny, or Esc to cancel",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )));
    }

    let block_style = if state.guard_requests.is_empty() {
        Style::default()
    } else {
        Style::default().fg(Color::Yellow)
    };

    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title("Guardrails")
                .borders(Borders::ALL)
                .style(block_style),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(widget, area);
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{AppState, ApprovalRequest};
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
        let mut lines: Vec<Line> = Vec::new();

        if state.guard_requests.is_empty() {
            lines.push(Line::from(Span::styled(
                "No pending approval requests.",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let header = format!(
                "Pending Approvals ({} request{}):",
                state.guard_requests.len(),
                if state.guard_requests.len() == 1 {
                    ""
                } else {
                    "s"
                }
            );
            lines.push(Line::from(Span::styled(
                header,
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(""));

            for request in &state.guard_requests {
                let risk_color = match request.risk_level.to_lowercase().as_str() {
                    "high" | "critical" => Color::Red,
                    "medium" => Color::Yellow,
                    _ => Color::Green,
                };
                lines.push(Line::from(vec![
                    Span::styled("  Risk: ", Style::default().fg(Color::White)),
                    Span::styled(
                        request.risk_level.clone(),
                        Style::default()
                            .fg(risk_color)
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Action: ", Style::default().fg(Color::White)),
                    Span::styled(
                        request.action_summary.clone(),
                        Style::default().fg(Color::Cyan),
                    ),
                ]));
                if !request.reasons.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  Reasons:",
                        Style::default().fg(Color::White),
                    )));
                    for reason in &request.reasons {
                        lines.push(Line::from(Span::styled(
                            format!("    - {}", reason),
                            Style::default().fg(Color::Gray),
                        )));
                    }
                }
                lines.push(Line::from(Span::styled(
                    format!("  ID: {}", request.id),
                    Style::default().fg(Color::DarkGray),
                )));
                lines.push(Line::from(""));
            }

            lines.push(Line::from(Span::styled(
                "Press y to approve, n to deny, or Esc to cancel",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            )));
        }

        let block_style = if state.guard_requests.is_empty() {
            Style::default()
        } else {
            Style::default().fg(Color::Yellow)
        };

        Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .title("Guardrails")
                    .borders(Borders::ALL)
                    .style(block_style),
            )
            .wrap(Wrap { trim: true })
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
    fn test_guardrails_empty_state() {
        let state = AppState::new("test-model");
        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("No pending approval requests."),
            "Expected 'No pending approval requests.', got: {}",
            content
        );
        assert!(
            content.contains("Guardrails"),
            "Expected 'Guardrails' title, got: {}",
            content
        );
    }

    #[test]
    fn test_guardrails_with_pending_request() {
        let mut state = AppState::new("test-model");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "bash: rm -rf /tmp/test".to_string(),
            "High".to_string(),
            vec!["Destructive command".to_string()],
        ));

        let buffer = render_to_buffer(&state, 80, 15);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("Pending Approvals (1 request):"),
            "Expected header, got: {}",
            content
        );
        assert!(
            content.contains("Risk:"),
            "Expected risk label, got: {}",
            content
        );
        assert!(
            content.contains("High"),
            "Expected risk level, got: {}",
            content
        );
        assert!(
            content.contains("bash: rm -rf /tmp/test"),
            "Expected action summary, got: {}",
            content
        );
        assert!(
            content.contains("Destructive command"),
            "Expected reason, got: {}",
            content
        );
        assert!(
            content.contains("req-1"),
            "Expected request ID, got: {}",
            content
        );
        assert!(
            content.contains("Press y to approve"),
            "Expected prompt, got: {}",
            content
        );
    }

    #[test]
    fn test_guardrails_multiple_requests() {
        let mut state = AppState::new("test-model");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "action one".to_string(),
            "Low".to_string(),
            vec!["minor".to_string()],
        ));
        state.add_guard_request(ApprovalRequest::new(
            "req-2".to_string(),
            "action two".to_string(),
            "Critical".to_string(),
            vec!["major".to_string()],
        ));

        let buffer = render_to_buffer(&state, 80, 20);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("Pending Approvals (2 requests):"),
            "Expected plural header, got: {}",
            content
        );
        assert!(
            content.contains("action one"),
            "Expected first action, got: {}",
            content
        );
        assert!(
            content.contains("action two"),
            "Expected second action, got: {}",
            content
        );
        assert!(
            content.contains("Low"),
            "Expected Low risk, got: {}",
            content
        );
        assert!(
            content.contains("Critical"),
            "Expected Critical risk, got: {}",
            content
        );
    }

    #[test]
    fn test_guardrails_no_reasons() {
        let mut state = AppState::new("test-model");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "simple action".to_string(),
            "Medium".to_string(),
            vec![],
        ));

        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("simple action"),
            "Expected action summary, got: {}",
            content
        );
        // "Reasons:" label should not appear when there are no reasons
        assert!(
            !content.contains("Reasons:"),
            "Reasons label should not appear when empty, got: {}",
            content
        );
    }
}