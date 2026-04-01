//! Welcome screen widget displayed when starting a new session.
//!
//! Uses ratatui's native Block widget for proper border rendering.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, BorderType, Paragraph, Widget};

/// State for the welcome screen.
pub struct WelcomeState {
    /// The active model name.
    pub model_name: String,
}

impl WelcomeState {
    pub fn new(model_name: String) -> Self {
        Self { model_name }
    }
}

/// Widget that renders the welcome screen.
pub struct WelcomeWidget<'a> {
    state: &'a WelcomeState,
}

impl<'a> WelcomeWidget<'a> {
    pub fn new(state: &'a WelcomeState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for WelcomeWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 8 || area.width < 40 {
            // Fallback for very small terminals
            let line = Line::from(Span::styled(
                "Claude Code — Type a message or /help",
                Style::default().fg(Color::Cyan),
            ));
            let p = Paragraph::new(line).alignment(Alignment::Center);
            let y = area.y + area.height / 2;
            p.render(Rect::new(area.x, y, area.width, 1), buf);
            return;
        }

        // Center a box in the area
        let box_width: u16 = 48.min(area.width.saturating_sub(4));
        let box_height: u16 = 12.min(area.height.saturating_sub(2));
        let x0 = area.x + (area.width.saturating_sub(box_width)) / 2;
        let y0 = area.y + (area.height.saturating_sub(box_height)) / 2;
        let box_area = Rect::new(x0, y0, box_width, box_height);

        // Render bordered block with rounded corners
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Claude Code ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .title_alignment(Alignment::Center);

        let inner = block.inner(box_area);
        block.render(box_area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // Build content lines
        let key_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(Color::DarkGray);
        let label_style = Style::default().fg(Color::Gray);
        let value_style = Style::default().fg(Color::White);

        let lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("  Model: ", label_style),
                Span::styled(&self.state.model_name, value_style),
            ]),
            Line::from(vec![
                Span::styled("  Type a message or ", desc_style),
                Span::styled("/help", key_style),
            ]),
            Line::from(""),
            Line::from(Span::styled("  Tips:", Style::default().fg(Color::White))),
            Line::from(vec![
                Span::styled("  • ", Style::default().fg(Color::Cyan)),
                Span::styled("/help", key_style),
                Span::styled("    — show available commands", desc_style),
            ]),
            Line::from(vec![
                Span::styled("  • ", Style::default().fg(Color::Cyan)),
                Span::styled("/model", key_style),
                Span::styled("   — change model", desc_style),
            ]),
            Line::from(vec![
                Span::styled("  • ", Style::default().fg(Color::Cyan)),
                Span::styled("Ctrl+C", key_style),
                Span::styled("   — cancel request", desc_style),
            ]),
            Line::from(vec![
                Span::styled("  • ", Style::default().fg(Color::Cyan)),
                Span::styled("Ctrl+D", key_style),
                Span::styled("   — quit", desc_style),
            ]),
        ];

        let content = Paragraph::new(lines);
        content.render(inner, buf);
    }
}
