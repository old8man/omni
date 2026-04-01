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

/// Return a display color for the model based on its family.
fn model_color(model: &str) -> Color {
    let lower = model.to_lowercase();
    if lower.contains("opus") {
        Color::Magenta
    } else if lower.contains("sonnet") {
        Color::Cyan
    } else if lower.contains("haiku") {
        Color::Green
    } else {
        Color::White
    }
}

/// Derive a short display name from a full model identifier.
///
/// Examples:
///   "claude-opus-4-6"   -> "Opus 4.6"
///   "claude-sonnet-4-6" -> "Sonnet 4.6"
///   "claude-haiku-3-5"  -> "Haiku 3.5"
///   anything else       -> passed through as-is
fn short_model_name(model: &str) -> String {
    let lower = model.to_lowercase();
    // Try to extract family and version from patterns like "claude-opus-4-6"
    for family in &["opus", "sonnet", "haiku"] {
        if let Some(pos) = lower.find(family) {
            let rest = &model[pos + family.len()..];
            // rest might be "-4-6" or "-4-20250514" etc.
            let digits: String = rest
                .trim_start_matches('-')
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-')
                .collect();
            let version = digits.replace('-', ".");
            // Remove trailing dots
            let version = version.trim_end_matches('.').to_string();
            let cap = format!(
                "{}{}",
                family.chars().next().unwrap().to_uppercase(),
                &family[1..]
            );
            if version.is_empty() {
                return cap;
            }
            return format!("{} {}", cap, version);
        }
    }
    model.to_string()
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
        let bg = Style::default().bg(Color::Rgb(30, 30, 40)).fg(Color::White);
        for x in area.x..area.x + area.width {
            for y in area.y..area.y + area.height {
                buf[(x, y)].set_char(' ').set_style(bg);
            }
        }

        let sep = Span::styled(
            " \u{2502} ", // " │ "
            Style::default()
                .fg(Color::DarkGray)
                .bg(Color::Rgb(30, 30, 40)),
        );
        let bg_style = |fg: Color| -> Style {
            Style::default().fg(fg).bg(Color::Rgb(30, 30, 40))
        };

        // ── Left section ────────────────────────────────────────────────
        let mut left_spans: Vec<Span> = Vec::new();

        // Product name
        left_spans.push(Span::styled(
            " Claude Code",
            bg_style(Color::Cyan).add_modifier(Modifier::BOLD),
        ));

        left_spans.push(sep.clone());

        // Model name (colored by family)
        let display_model = short_model_name(&self.state.model_name);
        let m_color = model_color(&self.state.model_name);
        left_spans.push(Span::styled(display_model, bg_style(m_color)));

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
                    .bg(Color::Rgb(30, 30, 40))
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
                    .bg(Color::Rgb(30, 30, 40))
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
            right_spans.push(sep);
            right_spans.push(Span::styled(
                "\u{26A0} RATE LIMITED",
                Style::default()
                    .fg(Color::Red)
                    .bg(Color::Rgb(30, 30, 40))
                    .add_modifier(Modifier::BOLD),
            ));
        }

        // Combine all spans and render
        let mut all_spans = left_spans;
        all_spans.extend(center_spans);
        all_spans.extend(right_spans);
        // Pad to fill remaining width
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
    fn test_short_model_name() {
        assert_eq!(short_model_name("claude-opus-4-6"), "Opus 4.6");
        assert_eq!(short_model_name("claude-sonnet-4-6"), "Sonnet 4.6");
        assert_eq!(short_model_name("claude-haiku-3-5"), "Haiku 3.5");
        assert_eq!(short_model_name("gpt-4"), "gpt-4");
    }

    #[test]
    fn test_model_color() {
        assert_eq!(model_color("claude-opus-4-6"), Color::Magenta);
        assert_eq!(model_color("claude-sonnet-4-6"), Color::Cyan);
        assert_eq!(model_color("claude-haiku-3-5"), Color::Green);
        assert_eq!(model_color("unknown-model"), Color::White);
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
