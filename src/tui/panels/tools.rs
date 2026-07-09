use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::app::AppState;

/// Render the tools panel: shows the current tool call (if any) and recent
/// tool execution results.
///
/// The panel displays:
/// - The name of the currently executing tool, or "None" if idle.
/// - The most recent tool results with success/failure status and content.
pub fn render(f: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let mut lines: Vec<Line> = Vec::new();

    // Current tool
    let current_tool = match &state.current_tool {
        Some(name) => format!("Current tool: {}", name),
        None => "Current tool: None".to_string(),
    };
    lines.push(Line::from(Span::styled(
        current_tool,
        Style::default().fg(Color::Cyan),
    )));

    // Separator
    lines.push(Line::from(""));

    // Recent tool results (last 5)
    let results_header = format!("Recent tool results ({} total):", state.tool_results.len());
    lines.push(Line::from(Span::styled(
        results_header,
        Style::default().fg(Color::White),
    )));

    let start = state.tool_results.len().saturating_sub(5);
    for result in state.tool_results.iter().skip(start) {
        let status_icon = if result.success { "OK" } else { "FAIL" };
        let status_color = if result.success {
            Color::Green
        } else {
            Color::Red
        };

        // Truncate content for display
        let content_preview = if result.content.len() > 60 {
            format!("{}...", &result.content[..57])
        } else {
            result.content.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("  [{}] ", status_icon),
                Style::default().fg(status_color),
            ),
            Span::styled(content_preview, Style::default().fg(Color::Gray)),
        ]));
    }

    if state.tool_results.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no tool results yet)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let widget = Paragraph::new(Text::from(lines))
        .block(Block::default().title("Tools").borders(Borders::ALL))
        .wrap(Wrap { trim: true });

    f.render_widget(widget, area);
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::AppState;
    use crate::types::{Artifact, ToolResult};
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

        let current_tool = match &state.current_tool {
            Some(name) => format!("Current tool: {}", name),
            None => "Current tool: None".to_string(),
        };
        lines.push(Line::from(Span::styled(
            current_tool,
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::from(""));

        let results_header =
            format!("Recent tool results ({} total):", state.tool_results.len());
        lines.push(Line::from(Span::styled(
            results_header,
            Style::default().fg(Color::White),
        )));

        let start = state.tool_results.len().saturating_sub(5);
        for result in state.tool_results.iter().skip(start) {
            let status_icon = if result.success { "OK" } else { "FAIL" };
            let status_color = if result.success {
                Color::Green
            } else {
                Color::Red
            };
            let content_preview = if result.content.len() > 60 {
                format!("{}...", &result.content[..57])
            } else {
                result.content.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  [{}] ", status_icon),
                    Style::default().fg(status_color),
                ),
                Span::styled(content_preview, Style::default().fg(Color::Gray)),
            ]));
        }

        if state.tool_results.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no tool results yet)",
                Style::default().fg(Color::DarkGray),
            )));
        }

        Paragraph::new(Text::from(lines))
            .block(Block::default().title("Tools").borders(Borders::ALL))
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

    fn make_tool_result(success: bool, content: &str) -> ToolResult {
        ToolResult {
            success,
            content: content.to_string(),
            structured: None,
            artifacts: vec![Artifact {
                path: std::path::PathBuf::from("output.txt"),
                content_type: "text/plain".to_string(),
                size_bytes: 42,
            }],
        }
    }

    #[test]
    fn test_tools_panel_no_tool() {
        let state = AppState::new("test-model");
        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("Current tool: None"),
            "Expected 'Current tool: None', got: {}",
            content
        );
        assert!(
            content.contains("(no tool results yet)"),
            "Expected 'no tool results yet', got: {}",
            content
        );
        assert!(
            content.contains("Tools"),
            "Expected 'Tools' title, got: {}",
            content
        );
    }

    #[test]
    fn test_tools_panel_with_current_tool() {
        let mut state = AppState::new("test-model");
        state.set_current_tool(Some("bash".to_string()));

        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("Current tool: bash"),
            "Expected 'Current tool: bash', got: {}",
            content
        );
    }

    #[test]
    fn test_tools_panel_with_results() {
        let mut state = AppState::new("test-model");
        state.add_tool_result(make_tool_result(true, "echo: hello world"));
        state.add_tool_result(make_tool_result(false, "command not found: foobar"));

        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("Recent tool results (2 total):"),
            "Expected results header, got: {}",
            content
        );
        assert!(
            content.contains("[OK]"),
            "Expected success indicator, got: {}",
            content
        );
        assert!(
            content.contains("[FAIL]"),
            "Expected failure indicator, got: {}",
            content
        );
        assert!(
            content.contains("echo: hello world"),
            "Expected result content, got: {}",
            content
        );
        assert!(
            content.contains("command not found: foobar"),
            "Expected error content, got: {}",
            content
        );
    }

    #[test]
    fn test_tools_panel_truncates_long_content() {
        let mut state = AppState::new("test-model");
        let long_content = "a".repeat(100);
        state.add_tool_result(make_tool_result(true, &long_content));

        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        // Should be truncated to 60 chars with "..."
        assert!(
            content.contains("..."),
            "Expected truncated content with '...', got: {}",
            content
        );
        // The full 100-char string should not appear
        assert!(
            !content.contains(&long_content),
            "Full long content should not appear, got: {}",
            content
        );
    }

    #[test]
    fn test_tools_panel_only_shows_last_5_results() {
        let mut state = AppState::new("test-model");
        for i in 0..10 {
            state.add_tool_result(make_tool_result(true, &format!("result {}", i)));
        }

        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("Recent tool results (10 total):"),
            "Expected header with total count, got: {}",
            content
        );

        // Results 0-4 should not be visible (only last 5)
        assert!(
            !content.contains("result 0"),
            "Result 0 should not be visible, got: {}",
            content
        );
        assert!(
            !content.contains("result 4"),
            "Result 4 should not be visible, got: {}",
            content
        );

        // Results 5-9 should be visible
        assert!(
            content.contains("result 9"),
            "Latest result should be visible, got: {}",
            content
        );
    }
}