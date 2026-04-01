//! Context visualization panel widget.
//!
//! Displays the current conversation context: token usage breakdown,
//! context window utilization bar, active files, and recent tool calls.
//! This provides the user with awareness of how much context has been
//! consumed and what information is in scope.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

/// A file that is part of the current context.
#[derive(Clone, Debug)]
pub struct ContextFile {
    /// File path relative to the project root.
    pub path: String,
    /// Approximate token count for this file.
    pub tokens: u64,
}

/// A recent tool call shown in the context panel.
#[derive(Clone, Debug)]
pub struct ContextToolCall {
    /// Tool name.
    pub name: String,
    /// Brief summary of the call.
    pub summary: String,
    /// Whether the call succeeded.
    pub success: bool,
}

/// State for the context visualization panel.
pub struct ContextPanel {
    /// Maximum context window size in tokens.
    pub max_tokens: u64,
    /// Current input tokens used.
    pub input_tokens: u64,
    /// Current output tokens used.
    pub output_tokens: u64,
    /// Number of messages in conversation.
    pub message_count: usize,
    /// Files currently in context.
    pub files: Vec<ContextFile>,
    /// Recent tool calls.
    pub tool_calls: Vec<ContextToolCall>,
    /// Whether the panel is visible.
    pub visible: bool,
    /// Scroll offset for the file/tool list.
    pub scroll_offset: usize,
}

impl ContextPanel {
    /// Create a new context panel with default values.
    pub fn new(max_tokens: u64) -> Self {
        Self {
            max_tokens,
            input_tokens: 0,
            output_tokens: 0,
            message_count: 0,
            files: Vec::new(),
            tool_calls: Vec::new(),
            visible: false,
            scroll_offset: 0,
        }
    }

    /// Toggle panel visibility.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Update token counts.
    pub fn update_tokens(&mut self, input: u64, output: u64) {
        self.input_tokens = input;
        self.output_tokens = output;
    }

    /// Add a file to the context.
    pub fn add_file(&mut self, path: String, tokens: u64) {
        // Update if already present
        if let Some(existing) = self.files.iter_mut().find(|f| f.path == path) {
            existing.tokens = tokens;
        } else {
            self.files.push(ContextFile { path, tokens });
        }
    }

    /// Record a tool call.
    pub fn add_tool_call(&mut self, name: String, summary: String, success: bool) {
        self.tool_calls.push(ContextToolCall {
            name,
            summary,
            success,
        });
        // Keep only the most recent 20 tool calls
        if self.tool_calls.len() > 20 {
            self.tool_calls.remove(0);
        }
    }

    /// Total tokens used.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Context utilization as a fraction (0.0 to 1.0).
    pub fn utilization(&self) -> f64 {
        if self.max_tokens == 0 {
            return 0.0;
        }
        (self.total_tokens() as f64 / self.max_tokens as f64).min(1.0)
    }

    /// Scroll up.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down.
    pub fn scroll_down(&mut self, n: usize) {
        let max = self.files.len() + self.tool_calls.len();
        self.scroll_offset = (self.scroll_offset + n).min(max.saturating_sub(1));
    }
}

impl Default for ContextPanel {
    fn default() -> Self {
        Self::new(claude_core::utils::context::MODEL_CONTEXT_WINDOW_DEFAULT)
    }
}

impl ContextPanel {
    /// Reconfigure the panel for a specific model's context window.
    pub fn configure_for_model(&mut self, model: &str) {
        self.max_tokens = claude_core::utils::context::get_context_window_for_model(model);
    }
}

/// Widget that renders the context panel.
pub struct ContextPanelWidget<'a> {
    panel: &'a ContextPanel,
}

impl<'a> ContextPanelWidget<'a> {
    /// Create a new context panel widget.
    pub fn new(panel: &'a ContextPanel) -> Self {
        Self { panel }
    }

    fn render_content(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Token usage summary
        let total = self.panel.total_tokens();
        let utilization = self.panel.utilization();
        lines.push(Line::from(vec![
            Span::styled(
                " Tokens: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format_tokens(total),
                Style::default().fg(if utilization > 0.8 {
                    Color::Red
                } else if utilization > 0.6 {
                    Color::Yellow
                } else {
                    Color::Green
                }),
            ),
            Span::styled(
                format!(" / {}", format_tokens(self.panel.max_tokens)),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        // Utilization bar
        let bar_width = 20;
        let filled = (utilization * bar_width as f64) as usize;
        let empty = bar_width - filled;
        let bar_color = if utilization > 0.8 {
            Color::Red
        } else if utilization > 0.6 {
            Color::Yellow
        } else {
            Color::Green
        };
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "\u{2588}".repeat(filled),
                Style::default().fg(bar_color),
            ),
            Span::styled(
                "\u{2591}".repeat(empty),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!(" {:.0}%", utilization * 100.0),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        // Input/output breakdown
        lines.push(Line::from(vec![
            Span::styled(
                format!(" In: {} ", format_tokens(self.panel.input_tokens)),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                format!("Out: {} ", format_tokens(self.panel.output_tokens)),
                Style::default().fg(Color::Magenta),
            ),
            Span::styled(
                format!("Msgs: {}", self.panel.message_count),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        lines.push(Line::from(""));

        // Files in context
        if !self.panel.files.is_empty() {
            lines.push(Line::from(Span::styled(
                " Files in context:",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
            for file in &self.panel.files {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(
                        file.path.clone(),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!(" ({})", format_tokens(file.tokens)),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }

        // Recent tool calls
        if !self.panel.tool_calls.is_empty() {
            lines.push(Line::from(Span::styled(
                " Recent tools:",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
            for call in self.panel.tool_calls.iter().rev().take(10) {
                let indicator = if call.success { "\u{2714}" } else { "\u{2718}" };
                let color = if call.success {
                    Color::Green
                } else {
                    Color::Red
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", indicator), Style::default().fg(color)),
                    Span::styled(
                        call.name.clone(),
                        Style::default().fg(Color::Magenta),
                    ),
                    Span::styled(
                        format!(" {}", truncate(&call.summary, 30)),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
        }

        lines
    }
}

impl<'a> Widget for ContextPanelWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.panel.visible || area.height < 3 || area.width < 10 {
            return;
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Context ")
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        block.render(area, buf);

        let content_lines = self.render_content();
        let visible_height = inner.height as usize;
        let scroll = self
            .panel
            .scroll_offset
            .min(content_lines.len().saturating_sub(visible_height));
        let end = (scroll + visible_height).min(content_lines.len());

        for (i, line) in content_lines[scroll..end].iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }
            buf.set_line(inner.x, y, line, inner.width);
        }
    }
}

/// Format a token count for display (e.g., "1.5k", "1.2M").
///
/// Delegates to [`claude_core::utils::format::format_tokens`].
fn format_tokens(tokens: u64) -> String {
    claude_core::utils::format::format_tokens(tokens)
}

/// Truncate a string in the middle, adding "..." if needed.
///
/// Delegates to [`claude_core::utils::format::truncate_middle`].
fn truncate(s: &str, max_len: usize) -> String {
    claude_core::utils::format::truncate_middle(s, max_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utilization() {
        let mut panel = ContextPanel::new(100_000);
        panel.update_tokens(50_000, 10_000);
        assert!((panel.utilization() - 0.6).abs() < 0.001);
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(1_500_000), "1.5m");
    }

    #[test]
    fn test_add_file() {
        let mut panel = ContextPanel::new(100_000);
        panel.add_file("src/main.rs".to_string(), 500);
        panel.add_file("src/lib.rs".to_string(), 300);
        assert_eq!(panel.files.len(), 2);
        // Update existing file
        panel.add_file("src/main.rs".to_string(), 600);
        assert_eq!(panel.files.len(), 2);
        assert_eq!(panel.files[0].tokens, 600);
    }

    #[test]
    fn test_add_tool_call() {
        let mut panel = ContextPanel::new(100_000);
        panel.add_tool_call("Read".to_string(), "src/main.rs".to_string(), true);
        assert_eq!(panel.tool_calls.len(), 1);
    }

    #[test]
    fn test_toggle() {
        let mut panel = ContextPanel::new(100_000);
        assert!(!panel.visible);
        panel.toggle();
        assert!(panel.visible);
        panel.toggle();
        assert!(!panel.visible);
    }
}
