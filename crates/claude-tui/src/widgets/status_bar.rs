//! Status bar widget displayed at the bottom of the TUI.
//!
//! Renders a full-featured status bar with colored segments:
//! Left: product name, model name (colored by model family)
//! Center: token count, cost, context window %
//! Right: mode indicators, session name, rate limit warning

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::input::InputMode;
use crate::theme;

/// A temporary flash message shown in the status bar, auto-dismissed after a timeout.
#[derive(Clone, Debug)]
pub struct FlashMessage {
    pub text: String,
    pub style: FlashStyle,
    pub expires_at: std::time::Instant,
}

/// Style for flash messages.
#[derive(Clone, Copy, Debug)]
pub enum FlashStyle {
    Info,
    Success,
    Warning,
    Error,
}

impl FlashMessage {
    pub fn new(text: impl Into<String>, style: FlashStyle, duration_ms: u64) -> Self {
        Self {
            text: text.into(),
            style,
            expires_at: std::time::Instant::now() + std::time::Duration::from_millis(duration_ms),
        }
    }

    pub fn info(text: impl Into<String>) -> Self {
        Self::new(text, FlashStyle::Info, 3000)
    }

    pub fn success(text: impl Into<String>) -> Self {
        Self::new(text, FlashStyle::Success, 2000)
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self::new(text, FlashStyle::Warning, 4000)
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self::new(text, FlashStyle::Error, 5000)
    }

    pub fn is_expired(&self) -> bool {
        std::time::Instant::now() >= self.expires_at
    }

    fn color(&self) -> Color {
        match self.style {
            FlashStyle::Info => Color::Cyan,
            FlashStyle::Success => Color::Green,
            FlashStyle::Warning => Color::Yellow,
            FlashStyle::Error => Color::Red,
        }
    }
}

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
    /// Context window usage as a percentage (0.0 to 100.0).
    pub context_percent: f64,
    /// Optional session name.
    pub session_name: Option<String>,
    /// Whether a rate limit is currently active.
    pub rate_limited: bool,
    /// Flash message (shown temporarily on the right side).
    pub flash: Option<FlashMessage>,
    /// Active profile display name shown in the status bar, e.g. "user@gmail.com (Pro)".
    pub profile_name: Option<String>,
}

impl Default for StatusBarState {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            total_tokens: 0,
            total_cost: 0.0,
            input_mode: InputMode::Insert,
            vim_enabled: false,
            plan_mode: false,
            context_percent: 0.0,
            session_name: None,
            rate_limited: false,
            flash: None,
            profile_name: None,
        }
    }
}

/// Widget that renders the status bar.
pub struct StatusBarWidget<'a> {
    state: &'a StatusBarState,
}

impl<'a> StatusBarWidget<'a> {
    /// Create a new status bar widget.
    pub fn new(state: &'a StatusBarState) -> Self {
        Self { state }
    }
}


/// Color for the context window percentage indicator.
fn context_color(pct: f64) -> Color {
    if pct < 60.0 {
        Color::Green
    } else if pct < 80.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

impl<'a> Widget for StatusBarWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Fill background
        for x in area.x..area.x + area.width {
            for y in area.y..area.y + area.height {
                buf[(x, y)].set_char(' ').set_style(theme::STYLE_STATUS);
            }
        }

        let sep = Span::styled(
            " \u{2502} ", // " │ "
            theme::STYLE_STATUS_DARK_GRAY,
        );
        let bg_style = |fg: Color| -> Style {
            Style::default().fg(fg).bg(theme::STATUS_BG)
        };

        // ── Left section ────────────────────────────────────────────────
        let left_spans: Vec<Span> = Vec::new();

        // ── Center section ──────────────────────────────────────────────
        let mut center_spans: Vec<Span> = Vec::new();

        // Token count
        let token_str = format_tokens(self.state.total_tokens);
        center_spans.push(sep.clone());
        center_spans.push(Span::styled(token_str, bg_style(Color::White)));

        // Cost
        if self.state.total_cost > 0.0 {
            center_spans.push(sep.clone());
            center_spans.push(Span::styled(
                format_cost(self.state.total_cost),
                bg_style(Color::Green),
            ));
        }

        // Context window %
        if self.state.context_percent > 0.0 {
            let ctx_c = context_color(self.state.context_percent);
            center_spans.push(sep.clone());
            center_spans.push(Span::styled("ctx: ", bg_style(Color::DarkGray)));
            center_spans.push(Span::styled(
                format!("{:.0}%", self.state.context_percent),
                bg_style(ctx_c).add_modifier(Modifier::BOLD),
            ));
        }

        // ── Right section ───────────────────────────────────────────────
        let mut right_spans: Vec<Span> = Vec::new();

        // Vim mode indicator
        if self.state.vim_enabled {
            let (mode_str, mode_color) = match self.state.input_mode {
                InputMode::Normal => ("NORMAL", Color::Blue),
                InputMode::Insert => ("INSERT", Color::Green),
                InputMode::Visual => ("VISUAL", Color::Magenta),
                InputMode::Command => ("COMMAND", Color::Yellow),
            };
            right_spans.push(sep.clone());
            right_spans.push(Span::styled(
                format!("\u{25CF} {}", mode_str), // ● MODE
                Style::default()
                    .fg(mode_color)
                    .bg(theme::STATUS_BG)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        // Plan mode
        if self.state.plan_mode {
            right_spans.push(sep.clone());
            right_spans.push(Span::styled(
                "\u{1F4CB} PLAN", // 📋 PLAN
                Style::default()
                    .fg(Color::Yellow)
                    .bg(theme::STATUS_BG)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        // Session name
        if let Some(ref name) = self.state.session_name {
            right_spans.push(sep.clone());
            right_spans.push(Span::styled(
                format!("session: {}", name),
                bg_style(Color::DarkGray),
            ));
        }

        // Rate limit warning
        if self.state.rate_limited {
            right_spans.push(sep.clone());
            right_spans.push(Span::styled(
                "\u{26A0} RATE LIMITED",
                Style::default()
                    .fg(Color::Red)
                    .bg(theme::STATUS_BG)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        // Flash message (replaces right section when active)
        if let Some(ref flash) = self.state.flash {
            if !flash.is_expired() {
                right_spans.clear();
                right_spans.push(sep);
                right_spans.push(Span::styled(
                    &flash.text,
                    Style::default()
                        .fg(flash.color())
                        .bg(theme::STATUS_BG)
                        .add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" ", bg_style(Color::Reset)));
            }
        }

        // Combine all spans and render
        let mut all_spans = left_spans;
        all_spans.extend(center_spans);
        all_spans.extend(right_spans);
        all_spans.push(Span::styled(" ", bg_style(Color::Reset)));

        let line = Line::from(all_spans);
        buf.set_line(area.x, area.y, &line, area.width);
    }
}

/// Format a token count for display.
///
/// Delegates to [`claude_core::utils::format::format_tokens`] and appends " tokens".
fn format_tokens(tokens: u64) -> String {
    format!(
        "{} tokens",
        claude_core::utils::format::format_tokens(tokens)
    )
}

/// Format a cost value for display with smart precision.
fn format_cost(cost: f64) -> String {
    if cost < 0.001 {
        format!("${:.4}", cost)
    } else if cost < 0.01 {
        format!("${:.3}", cost)
    } else if cost < 100.0 {
        format!("${:.2}", cost)
    } else {
        format!("${:.0}", cost)
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
        assert_eq!(format_cost(0.0001), "$0.0001");
        assert_eq!(format_cost(0.005), "$0.005");
        assert_eq!(format_cost(0.05), "$0.05");
        assert_eq!(format_cost(1.5), "$1.50");
        assert_eq!(format_cost(10.0), "$10.00");
        assert_eq!(format_cost(150.0), "$150");
    }

    #[test]
    fn test_context_color() {
        assert_eq!(context_color(30.0), Color::Green);
        assert_eq!(context_color(65.0), Color::Yellow);
        assert_eq!(context_color(85.0), Color::Red);
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
            context_percent: 45.0,
            session_name: Some("my-project".to_string()),
            rate_limited: false,
            flash: None,
        };
        let widget = StatusBarWidget::new(&state);
        let area = Rect::new(0, 0, 120, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        // Just verify it doesn't panic — visual testing would need snapshot tests
    }

    #[test]
    fn test_status_bar_empty_area() {
        let state = StatusBarState::default();
        let widget = StatusBarWidget::new(&state);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
    }
}
