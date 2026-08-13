use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::app::AppState;

// 全部使用 24 位真彩色：索引色（Color::Yellow/Black 等）随终端
// 浅色/深色主题变化，黑字黄底在浅色主题下几乎不可读。
// 真彩色不依赖终端调色板，两种主题下均清晰。
const COLOR_PROMPT_FG: Color = Color::Rgb(255, 255, 255);
const COLOR_PROMPT_BG: Color = Color::Rgb(180, 83, 9); // 深琥珀，白字对比度高
const COLOR_HEADER: Color = Color::Rgb(250, 204, 21);
const COLOR_RISK_HIGH: Color = Color::Rgb(248, 113, 113);
const COLOR_RISK_MEDIUM: Color = Color::Rgb(250, 204, 21);
const COLOR_RISK_LOW: Color = Color::Rgb(74, 222, 128);
const COLOR_LABEL: Color = Color::Rgb(229, 231, 235);
const COLOR_ACTION: Color = Color::Rgb(103, 232, 249);
const COLOR_REASON: Color = Color::Rgb(156, 163, 175);
const COLOR_ID: Color = Color::Rgb(148, 163, 184);

/// 构建护栏面板部件（渲染与测试共用）。
///
/// 高/严重风险红色、中风险黄色。y/n 操作提示固定在面板顶部：
/// 面板高度有限（右栏一半），请求条目多时提示行必须优先可见，
/// 不能放在内容列表末尾被裁掉。
fn build_widget(state: &AppState) -> Paragraph<'_> {
    let mut lines: Vec<Line> = Vec::new();

    if state.guard_requests.is_empty() {
        lines.push(Line::from(Span::styled(
            "暂无待审批请求。",
            Style::default().fg(COLOR_ID),
        )));
    } else {
        let header = format!("待审批请求（{} 个）：", state.guard_requests.len());
        lines.push(Line::from(Span::styled(
            header,
            Style::default().fg(COLOR_HEADER),
        )));

        // 操作提示置顶（白字深琥珀底加粗），保证始终可见
        lines.push(Line::from(Span::styled(
            "按 y 批准 / n 拒绝，Esc 或 q 退出",
            Style::default()
                .fg(COLOR_PROMPT_FG)
                .bg(COLOR_PROMPT_BG)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        for entry in &state.guard_requests {
            let request = &entry.request;

            // 来源标签（子 agent 审批）：面板顶部标注审批来源
            if let Some(label) = &entry.label {
                lines.push(Line::from(vec![
                    Span::styled("  来源: ", Style::default().fg(COLOR_LABEL)),
                    Span::styled(
                        label.clone(),
                        Style::default().fg(COLOR_HEADER).add_modifier(
                            ratatui::style::Modifier::BOLD,
                        ),
                    ),
                ]));
            }

            // Risk level with color
            let (risk_cn, risk_color) = match request.risk_level.to_lowercase().as_str() {
                "critical" => ("严重", COLOR_RISK_HIGH),
                "high" => ("高", COLOR_RISK_HIGH),
                "medium" => ("中", COLOR_RISK_MEDIUM),
                _ => ("低", COLOR_RISK_LOW),
            };
            lines.push(Line::from(vec![
                Span::styled("  风险: ", Style::default().fg(COLOR_LABEL)),
                Span::styled(
                    risk_cn.to_string(),
                    Style::default().fg(risk_color).add_modifier(
                        ratatui::style::Modifier::BOLD,
                    ),
                ),
            ]));

            // Action summary
            lines.push(Line::from(vec![
                Span::styled("  操作: ", Style::default().fg(COLOR_LABEL)),
                Span::styled(
                    request.action_summary.clone(),
                    Style::default().fg(COLOR_ACTION),
                ),
            ]));

            // Reasons
            if !request.reasons.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  原因:",
                    Style::default().fg(COLOR_LABEL),
                )));
                for reason in &request.reasons {
                    lines.push(Line::from(Span::styled(
                        format!("    - {}", reason),
                        Style::default().fg(COLOR_REASON),
                    )));
                }
            }

            // Suggested mitigation
            if let Some(ref mitigation) = request.suggested_mitigation {
                lines.push(Line::from(vec![
                    Span::styled("  缓解: ", Style::default().fg(COLOR_LABEL)),
                    Span::styled(
                        mitigation.clone(),
                        Style::default().fg(COLOR_REASON),
                    ),
                ]));
            }

            // ID
            lines.push(Line::from(Span::styled(
                format!("  编号: {}", request.id),
                Style::default().fg(COLOR_ID),
            )));

            lines.push(Line::from(""));
        }
    }

    let block_style = if state.guard_requests.is_empty() {
        Style::default()
    } else {
        Style::default().fg(COLOR_HEADER)
    };

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title("护栏")
                .borders(Borders::ALL)
                .style(block_style),
        )
        .wrap(Wrap { trim: true })
}

/// Render the guardrails panel: shows pending approval requests with risk
/// details and prompts the user for y/n input.
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
    use crate::tui::app::{AppState, ApprovalRequest};
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
    fn test_guardrails_empty_state() {
        let state = AppState::new("test-model");
        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("暂无待审批请求。"),
            "Expected '暂无待审批请求。', got: {}",
            content
        );
        assert!(
            content.contains("护栏"),
            "Expected '护栏' title, got: {}",
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
            None));

        let buffer = render_to_buffer(&state, 80, 15);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("待审批请求（1 个）："),
            "Expected header, got: {}",
            content
        );
        assert!(
            content.contains("按 y 批准 / n 拒绝"),
            "Expected prompt, got: {}",
            content
        );
        assert!(
            content.contains("风险:"),
            "Expected risk label, got: {}",
            content
        );
        assert!(
            content.contains("高"),
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
    }

    #[test]
    fn test_guardrails_prompt_uses_truecolor_for_readability() {
        let mut state = AppState::new("test-model");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "bash: rm -rf /tmp/test".to_string(),
            "High".to_string(),
            vec![],
            None));

        let buffer = render_to_buffer(&state, 80, 6);
        // 定位包含 y/n 提示的行（拼接整行文本，宽字符按显示宽度跳过占位格），
        // 校验真彩色样式（索引色在浅色主题下不可读）
        let prompt_row = (0..buffer.area.height)
            .find(|&y| {
                let mut row_text = String::new();
                let mut x = 0u16;
                while x < buffer.area.width {
                    let symbol = buffer.cell((x, y)).unwrap().symbol().to_string();
                    x += symbol.width().max(1) as u16;
                    row_text.push_str(&symbol);
                }
                row_text.contains("按 y 批准")
            })
            .expect("prompt line should be visible");

        let style = buffer.cell((1, prompt_row)).unwrap().style();
        assert_eq!(style.fg, Some(Color::Rgb(255, 255, 255)));
        assert_eq!(style.bg, Some(Color::Rgb(180, 83, 9)));
    }

    #[test]
    fn test_guardrails_multiple_requests() {
        let mut state = AppState::new("test-model");
        state.add_guard_request(ApprovalRequest::new(
            "req-1".to_string(),
            "action one".to_string(),
            "Low".to_string(),
            vec!["minor".to_string()],
            None));
        state.add_guard_request(ApprovalRequest::new(
            "req-2".to_string(),
            "action two".to_string(),
            "Critical".to_string(),
            vec!["major".to_string()],
            None));

        let buffer = render_to_buffer(&state, 80, 20);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("待审批请求（2 个）："),
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
            content.contains("风险: 低"),
            "Expected Low risk, got: {}",
            content
        );
        assert!(
            content.contains("风险: 严重"),
            "Expected Critical risk, got: {}",
            content
        );
    }

    #[test]
    fn test_guardrails_subagent_request_shows_source_label() {
        let mut state = AppState::new("test-model");
        let (reply_tx, _reply_rx) = tokio::sync::mpsc::channel(4);
        state.add_guard_request_with_reply(
            ApprovalRequest::new(
                "req-sub-1".to_string(),
                "subagent: 计算 2+2".to_string(),
                "High".to_string(),
                vec![],
                None),
            "子 agent".to_string(),
            reply_tx,
        );

        let buffer = render_to_buffer(&state, 80, 12);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("来源: 子 agent"),
            "子审批条目应标注来源标签，got: {}",
            content
        );
        assert!(
            content.contains("subagent: 计算 2+2"),
            "Expected action summary, got: {}",
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
            None));

        let buffer = render_to_buffer(&state, 80, 10);
        let content = buffer_to_string(&buffer);

        assert!(
            content.contains("simple action"),
            "Expected action summary, got: {}",
            content
        );
        // "原因:" label should not appear when there are no reasons
        assert!(
            !content.contains("原因:"),
            "Reasons label should not appear when empty, got: {}",
            content
        );
    }
}