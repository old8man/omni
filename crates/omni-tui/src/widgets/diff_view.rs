//! Diff display widget with colored +/- lines for file edits.
//!
//! Renders unified diff output with syntax-highlighted context lines,
//! green additions, red deletions, and cyan hunk headers.  Supports
//! both unified diff format and simple before/after file previews.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// A single line in a diff view.
#[derive(Clone, Debug)]
pub enum DiffLine {
    /// File header (e.g., `--- a/file.rs`, `+++ b/file.rs`).
    Header(String),
    /// Hunk header (e.g., `@@ -1,5 +1,7 @@`).
    HunkHeader(String),
    /// Added line.
    Added(String),
    /// Removed line.
    Removed(String),
    /// Context (unchanged) line.
    Context(String),
}

/// Parse unified diff text into structured diff lines.
pub fn parse_unified_diff(diff_text: &str) -> Vec<DiffLine> {
    let mut result = Vec::new();

    for line in diff_text.lines() {
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            result.push(DiffLine::Header(line.to_string()));
        } else if line.starts_with("@@") {
            result.push(DiffLine::HunkHeader(line.to_string()));
        } else if let Some(rest) = line.strip_prefix('+') {
            result.push(DiffLine::Added(rest.to_string()));
        } else if let Some(rest) = line.strip_prefix('-') {
            result.push(DiffLine::Removed(rest.to_string()));
        } else if let Some(rest) = line.strip_prefix(' ') {
            result.push(DiffLine::Context(rest.to_string()));
        } else {
            result.push(DiffLine::Context(line.to_string()));
        }
    }

    result
}

/// State for a diff view, holding parsed lines and scroll position.
pub struct DiffView {
    /// The file path being diffed.
    pub file_path: String,
    /// Parsed diff lines.
    pub lines: Vec<DiffLine>,
    /// Current scroll offset.
    pub scroll_offset: usize,
}

impl DiffView {
    /// Create a new diff view from unified diff text.
    pub fn new(file_path: String, diff_text: &str) -> Self {
        Self {
            file_path,
            lines: parse_unified_diff(diff_text),
            scroll_offset: 0,
        }
    }

    /// Create a diff view from pre-parsed lines.
    pub fn from_lines(file_path: String, lines: Vec<DiffLine>) -> Self {
        Self {
            file_path,
            lines,
            scroll_offset: 0,
        }
    }

    /// Scroll up by the given number of lines.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down by the given number of lines.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = (self.scroll_offset + n).min(self.lines.len().saturating_sub(1));
    }

    /// Count of added lines.
    pub fn additions(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| matches!(l, DiffLine::Added(_)))
            .count()
    }

    /// Count of removed lines.
    pub fn deletions(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| matches!(l, DiffLine::Removed(_)))
            .count()
    }
}

/// Widget that renders a diff view.
pub struct DiffViewWidget<'a> {
    view: &'a DiffView,
}

impl<'a> DiffViewWidget<'a> {
    /// Create a new diff view widget.
    pub fn new(view: &'a DiffView) -> Self {
        Self { view }
    }

    /// Render the diff lines to styled ratatui lines.
    fn render_lines(&self) -> Vec<Line<'static>> {
        let mut result = Vec::new();

        // File header
        let add_count = self.view.additions();
        let del_count = self.view.deletions();
        result.push(Line::from(vec![
            Span::styled(
                format!(" {} ", self.view.file_path),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("+{}", add_count),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" "),
            Span::styled(
                format!("-{}", del_count),
                Style::default().fg(Color::Red),
            ),
        ]));

        // Diff lines
        for diff_line in &self.view.lines {
            let line = match diff_line {
                DiffLine::Header(text) => Line::from(Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                DiffLine::HunkHeader(text) => Line::from(Span::styled(
                    text.clone(),
                    Style::default().fg(Color::Cyan),
                )),
                DiffLine::Added(text) => Line::from(Span::styled(
                    format!("+ {}", text),
                    Style::default().fg(Color::Green).bg(Color::Rgb(0, 40, 0)),
                )),
                DiffLine::Removed(text) => Line::from(Span::styled(
                    format!("- {}", text),
                    Style::default().fg(Color::Red).bg(Color::Rgb(40, 0, 0)),
                )),
                DiffLine::Context(text) => Line::from(Span::styled(
                    format!("  {}", text),
                    Style::default().fg(Color::DarkGray),
                )),
            };
            result.push(line);
        }

        result
    }
}

impl<'a> Widget for DiffViewWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let all_lines = self.render_lines();
        let visible_height = area.height as usize;
        let scroll = self
            .view
            .scroll_offset
            .min(all_lines.len().saturating_sub(visible_height));
        let end = (scroll + visible_height).min(all_lines.len());
        let visible = &all_lines[scroll..end];

        for (i, line) in visible.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }
            buf.set_line(area.x, y, line, area.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_unified_diff() {
        let diff = "--- a/file.rs\n+++ b/file.rs\n@@ -1,3 +1,4 @@\n context\n-removed\n+added\n+also added";
        let lines = parse_unified_diff(diff);
        assert_eq!(lines.len(), 7);
        assert!(matches!(lines[0], DiffLine::Header(_)));
        assert!(matches!(lines[2], DiffLine::HunkHeader(_)));
        assert!(matches!(lines[4], DiffLine::Removed(_)));
        assert!(matches!(lines[5], DiffLine::Added(_)));
    }

    #[test]
    fn test_diff_view_counts() {
        let diff = "+added1\n+added2\n-removed1\n context";
        let view = DiffView::new("test.rs".to_string(), diff);
        assert_eq!(view.additions(), 2);
        assert_eq!(view.deletions(), 1);
    }

    #[test]
    fn test_diff_view_scroll() {
        let diff = "+a\n+b\n+c\n+d\n+e";
        let mut view = DiffView::new("test.rs".to_string(), diff);
        assert_eq!(view.scroll_offset, 0);
        view.scroll_down(3);
        assert_eq!(view.scroll_offset, 3);
        view.scroll_up(2);
        assert_eq!(view.scroll_offset, 1);
    }
}
