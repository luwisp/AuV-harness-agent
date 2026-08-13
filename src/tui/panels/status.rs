use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::tui::app::AppState;

// 真彩色（白字深灰蓝底）：索引色 Color::White/DarkGray 随终端主题变化，
// 浅色主题下白字浅灰底几乎不可读。真彩色两种主题下均清晰。
const COLOR_STATUS_FG: Color = Color::Rgb(255, 255, 255);
const COLOR_STATUS_BG: Color = Color::Rgb(51, 65, 85);

/// 构建状态栏部件（渲染与测试共用）。
///
/// 注意：状态栏区域只有 1 行高，不能使用带边框的 Block
/// （上下边框各占 1 行，内容区高度归零导致文本不可见），
/// 直接渲染单行文本 + 背景色。
fn build_widget(state: &AppState) -> Paragraph<'_> {
    let risk_cn = match state.status.risk_level.to_lowercase().as_str() {
        "high" | "critical" => "高",
        "medium" => "中",
        _ => "低",
    };

    let running_text = if state.running { "运行中" } else { "已完成" };

    let status_text = format!(
        "轮次: {} | Token: {} | 风险: {} | 模型: {} | {}",
        state.status.turn,
        state.status.tokens_used,
        risk_cn,
        state.status.model,
        running_text,
    );

    let guard_text = if !state.guard_requests.is_empty() {
        format!(
            " | 待审批: {}（按 y 批准 / n 拒绝）",
            state.guard_requests.len()
        )
    } else {
        String::new()
    };

    let full_text = format!("{}{}", status_text, guard_text);

    Paragraph::new(Text::from(vec![Line::from(Span::styled(
        full_text,
        Style::default().fg(COLOR_STATUS_FG).bg(COLOR_STATUS_BG),
    ))]))
}

/// Render the status bar: a single line at the bottom showing turn count,
/// token usage, model, risk level, and running state.
pub fn render(f: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let widget = build_widget(state);
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
    use unicode_width::UnicodeWidthStr;

    fn render_to_buffer(state: &AppState, width: u16, height: u16) -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
        let area = Rect::new(0, 0, width, height);
        build_widget(state).render(area, &mut buffer);
        buffer
    }

    fn buffer_to_string(buffer: &Buffer) -> String {
        let mut result = String::new();
        for y in 0..buffer.area.height {
            let mut x = 0u16;
            while x < buffer.area.width {
                let symbol = buffer.cell((x, y)).unwrap().symbol().to_string();
                // 宽字符（如中文）占 2 列，紧随其后的占位格符号为空格，
                // 按显示宽度跳过，否则拼接结果会在每个汉字后多出一个空格。
                x += symbol.width().max(1) as u16;
                result.push_str(&symbol);
            }
            result.push('\n');
        }
        result
    }

    #[test]
    fn test_status_bar_default_state() {
        let state = AppState::new("gpt-4o");
        // 状态栏实际渲染区域为 1 行
        let buffer = render_to_buffer(&state, 80, 1);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("轮次: 0"),
            "Expected turn count, got: {}",
            content
        );
        assert!(
            content.contains("Token: 0"),
            "Expected token count, got: {}",
            content
        );
        assert!(
            content.contains("风险: 低"),
            "Expected risk level, got: {}",
            content
        );
        assert!(
            content.contains("模型: gpt-4o"),
            "Expected model name, got: {}",
            content
        );
        assert!(
            content.contains("运行中"),
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

        let buffer = render_to_buffer(&state, 80, 1);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("已完成"),
            "Expected '已完成', got: {}",
            content
        );
        assert!(
            content.contains("轮次: 5"),
            "Expected turn 5, got: {}",
            content
        );
        assert!(
            content.contains("Token: 1234"),
            "Expected token count, got: {}",
            content
        );
    }

    #[test]
    fn test_status_bar_risk_level_colors() {
        // Test Low risk
        let mut state = AppState::new("gpt-4o");
        state.status.risk_level = "Low".to_string();
        let buffer = render_to_buffer(&state, 80, 1);
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("风险: 低"),
            "Expected Low risk, got: {}",
            content
        );

        // Test Medium risk
        state.status.risk_level = "Medium".to_string();
        let buffer = render_to_buffer(&state, 80, 1);
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("风险: 中"),
            "Expected Medium risk, got: {}",
            content
        );

        // Test High risk
        state.status.risk_level = "High".to_string();
        let buffer = render_to_buffer(&state, 80, 1);
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("风险: 高"),
            "Expected High risk, got: {}",
            content
        );

        // Test Critical risk
        state.status.risk_level = "Critical".to_string();
        let buffer = render_to_buffer(&state, 80, 1);
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("风险: 高"),
            "Expected Critical risk, got: {}",
            content
        );
    }

    #[test]
    fn test_status_bar_uses_truecolor_for_readability() {
        let state = AppState::new("gpt-4o");
        let buffer = render_to_buffer(&state, 80, 1);
        let style = buffer.cell((0, 0)).unwrap().style();
        assert_eq!(style.fg, Some(Color::Rgb(255, 255, 255)));
        assert_eq!(style.bg, Some(Color::Rgb(51, 65, 85)));
    }

    #[test]
    fn test_status_bar_with_pending_approvals() {
        let mut state = AppState::new("gpt-4o");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "test".to_string(),
            "High".to_string(),
            vec![],
            None));
        state.add_guard_request(ApprovalRequest::new(
            "req-2".to_string(),
            "test2".to_string(),
            "Medium".to_string(),
            vec![],
            None));

        let buffer = render_to_buffer(&state, 120, 1);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("待审批: 2"),
            "Expected pending approvals count, got: {}",
            content
        );
        assert!(
            content.contains("按 y 批准 / n 拒绝"),
            "Expected approval hint, got: {}",
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

        let buffer = render_to_buffer(&state, 80, 1);
        let content = buffer_to_string(&buffer);

        assert!(content.contains("轮次: 42"));
        assert!(content.contains("Token: 99999"));
        assert!(content.contains("风险: 中"));
        assert!(content.contains("模型: claude-sonnet-4-20250514"));
    }
}