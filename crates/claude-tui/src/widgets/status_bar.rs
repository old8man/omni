//! Status bar widget displayed at the bottom of the TUI.
//!
//! Shows the current model name, token count, estimated cost, vim mode
//! indicator (when enabled), and plan mode status.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::input::InputMode;

/// Configuration for the status bar display.
pub struct StatusBarState {
    /// The model name to display.
    pub model_name: String,
    /// Total tokens used this session.
    pub total_tokens: u64,
    /// Estimated total cost in USD.
    pub total_cost: f64,
    /// Current input mode (INSERT/NORMAL/etc).
    pub input_mode: InputMode,
    /// Whether vim mode is enabled.
    pub vim_enabled: bool,
    /// Whether plan mode is enabled.
    pub plan_mode: bool,
}

/// Widget that renders the status bar.
pub struct StatusBarWidget<'a> {
    state: &'a StatusBarState,
    accent_color: Color,
    muted_color: Color,
    border_color: Color,
}

impl<'a> StatusBarWidget<'a> {
    /// Create a new status bar widget.
    pub fn new(state: &'a StatusBarState) -> Self {
        Self {
            state,
            accent_color: Color::Cyan,
            muted_color: Color::DarkGray,
            border_color: Color::DarkGray,
        }
    }

    /// Set the accent color.
    pub fn accent_color(mut self, color: Color) -> Self {
        self.accent_color = color;
        self
    }

    /// Set the muted text color.
    pub fn muted_color(mut self, color: Color) -> Self {
        self.muted_color = color;
        self
    }

    /// Set the border/separator color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }
}

impl<'a> Widget for StatusBarWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let sep = Span::styled(" | ", Style::default().fg(self.border_color));

        let mut spans = Vec::new();

        // Vim mode indicator
        if self.state.vim_enabled {
            let (mode_str, mode_color) = match self.state.input_mode {
                InputMode::Normal => ("NORMAL", Color::Blue),
                InputMode::Insert => ("INSERT", Color::Green),
                InputMode::Visual => ("VISUAL", Color::Magenta),
                InputMode::Command => ("COMMAND", Color::Yellow),
            };
            spans.push(Span::styled(
                format!(" {} ", mode_str),
                Style::default()
                    .fg(Color::Black)
                    .bg(mode_color)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
        }

        // Model name
        spans.push(Span::styled(
            &self.state.model_name,
            Style::default().fg(self.muted_color),
        ));

        spans.push(sep.clone());

        // Token count
        let token_str = format_tokens(self.state.total_tokens);
        spans.push(Span::styled(
            token_str,
            Style::default().fg(self.muted_color),
        ));

        // Cost
        if self.state.total_cost > 0.0 {
            spans.push(sep.clone());
            spans.push(Span::styled(
                format_cost(self.state.total_cost),
                Style::default().fg(self.muted_color),
            ));
        }

        // Plan mode
        if self.state.plan_mode {
            spans.push(sep);
            spans.push(Span::styled(
                "PLAN",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);
    }
}

/// Format a token count for display.
///
/// Delegates to [`claude_core::utils::format::format_tokens`] and appends " tokens".
fn format_tokens(tokens: u64) -> String {
    format!("{} tokens", claude_core::utils::format::format_tokens(tokens))
}

/// Format a cost value for display.
fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${:.4}", cost)
    } else if cost < 1.0 {
        format!("${:.3}", cost)
    } else {
        format!("${:.2}", cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(0), "0 tokens");
        assert_eq!(format_tokens(500), "500 tokens");
        assert_eq!(format_tokens(1500), "1.5k tokens");
        assert_eq!(format_tokens(1_500_000), "1.5m tokens");
    }

    #[test]
    fn test_format_cost() {
        assert_eq!(format_cost(0.001), "$0.0010");
        assert_eq!(format_cost(0.05), "$0.050");
        assert_eq!(format_cost(1.5), "$1.50");
        assert_eq!(format_cost(10.0), "$10.00");
    }

    #[test]
    fn test_status_bar_renders() {
        let state = StatusBarState {
            model_name: "claude-sonnet-4-6".to_string(),
            total_tokens: 1234,
            total_cost: 0.05,
            input_mode: InputMode::Insert,
            vim_enabled: true,
            plan_mode: false,
        };
        let widget = StatusBarWidget::new(&state);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        // Just verify it doesn't panic — visual testing would need snapshot tests
    }
}
