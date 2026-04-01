//! Generic scrollable information dialog overlay.
//!
//! Used by commands that want to display multi-line text inside a ratatui
//! dialog rather than dumping output into the chat. The dialog supports
//! keyboard navigation (arrows, page up/down, home/end, Esc/q to close).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{StatefulWidget, Widget};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::theme;

/// Actions the info dialog can produce in response to a keypress.
pub enum InfoDialogAction {
    /// The keypress was consumed — nothing more to do.
    Consumed,
    /// Close the dialog.
    Close,
}

/// A generic scrollable information dialog overlay.
pub struct InfoDialog {
    /// Dialog title (shown in the border).
    title: String,
    /// All content lines.
    lines: Vec<Line<'static>>,
    /// Current scroll offset.
    scroll: u16,
}

impl InfoDialog {
    /// Create a new dialog with the given title and plain-text content.
    ///
    /// The content string is split on newlines. Each line is checked for a
    /// simple prefix convention so that structural elements get colour:
    /// - Lines that look like `=====` or `─────` section dividers → dim
    /// - Lines starting with `  [ok]` → green
    /// - Lines starting with `  [!!]` → yellow
    /// - Lines starting with `  [--]` → red
    /// - Lines starting with `  ` (indented) → lighter colour
    /// - Lines that end with `:` and have no leading space (section headings) → cyan bold
    /// - Everything else → white
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        let title = title.into();
        let content = content.into();
        let lines = parse_content_lines(content);
        Self { title, lines, scroll: 0 }
    }

    /// Handle a keypress. Returns what the caller should do.
    pub fn handle_key(&mut self, code: crossterm::event::KeyCode) -> InfoDialogAction {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Esc | KeyCode::Char('q') => InfoDialogAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                InfoDialogAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                InfoDialogAction::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(20);
                InfoDialogAction::Consumed
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(20);
                InfoDialogAction::Consumed
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.scroll = 0;
                InfoDialogAction::Consumed
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.scroll = self.lines.len().saturating_sub(1) as u16;
                InfoDialogAction::Consumed
            }
            _ => InfoDialogAction::Consumed,
        }
    }

    fn total_lines(&self) -> usize {
        self.lines.len()
    }
}

impl Widget for &InfoDialog {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Dialog size: 90% width, up to 85% tall
        let width = (area.width * 90 / 100).max(60).min(area.width);
        let height = (area.height * 85 / 100).max(16).min(area.height);
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let dialog_area = Rect::new(x, y, width, height);

        Clear.render(dialog_area, buf);

        let title_text = format!("  {}  [↑/↓ scroll · Esc close] ", self.title);
        let block = Block::default()
            .title(Span::styled(
                title_text,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(theme::STATUS_BG));

        let inner = block.inner(dialog_area);
        block.render(dialog_area, buf);

        let visible_rows = inner.height as usize;
        let total = self.total_lines();
        let max_scroll = total.saturating_sub(visible_rows) as u16;
        let scroll = self.scroll.min(max_scroll);
        let start = scroll as usize;
        let end = (start + visible_rows).min(total);

        for (i, line) in self.lines[start..end].iter().enumerate() {
            let row = inner.y + i as u16;
            if row >= inner.y + inner.height {
                break;
            }
            buf.set_line(inner.x, row, line, inner.width);
        }

        if total > visible_rows {
            let mut scrollbar_state = ScrollbarState::new(total)
                .position(start)
                .viewport_content_length(visible_rows);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .end_symbol(Some("▼"))
                .track_symbol(Some("│"))
                .thumb_symbol("█");
            let scrollbar_area = Rect::new(
                dialog_area.x + dialog_area.width - 1,
                dialog_area.y + 1,
                1,
                dialog_area.height.saturating_sub(2),
            );
            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
        }
    }
}

// ── Line parser ──────────────────────────────────────────────────────────────

fn parse_content_lines(content: String) -> Vec<Line<'static>> {
    content
        .lines()
        .map(|raw| line_from_str(raw.to_string()))
        .collect()
}

fn line_from_str(raw: String) -> Line<'static> {
    // Section dividers (all dashes, equals, box-drawing chars)
    if raw.chars().all(|c| matches!(c, '=' | '-' | '─' | '═' | ' ')) && !raw.trim().is_empty() {
        return Line::from(Span::styled(
            raw,
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Status check lines
    if raw.trim_start().starts_with("[ok]") {
        return Line::from(Span::styled(raw, Style::default().fg(Color::Green)));
    }
    if raw.trim_start().starts_with("[!!]") || raw.trim_start().starts_with("[warn]") {
        return Line::from(Span::styled(raw, Style::default().fg(Color::Yellow)));
    }
    if raw.trim_start().starts_with("[--]") || raw.trim_start().starts_with("[err]") {
        return Line::from(Span::styled(raw, Style::default().fg(Color::Red)));
    }

    // Section headings: no leading space, ends with `:`
    let trimmed = raw.trim_end();
    if !trimmed.starts_with(' ')
        && trimmed.ends_with(':')
        && !trimmed.is_empty()
        && !trimmed.contains('/') // avoid "http://..." lines
    {
        return Line::from(Span::styled(
            raw,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Title / header lines (all caps, or has === prefix)
    if raw.starts_with("===") {
        return Line::from(Span::styled(
            raw,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Command reference lines: start with "  /" (from /help)
    if raw.starts_with("  /") {
        let mut spans: Vec<Span> = Vec::new();
        // Try to split at first double-space gap between name and description
        if let Some(sep_pos) = raw[3..].find("  ") {
            let name_part = &raw[..3 + sep_pos];
            let rest = &raw[3 + sep_pos..];
            spans.push(Span::styled(
                name_part.to_string(),
                Style::default().fg(Color::Yellow),
            ));
            spans.push(Span::styled(
                rest.to_string(),
                Style::default().fg(Color::Gray),
            ));
        } else {
            spans.push(Span::styled(raw, Style::default().fg(Color::Yellow)));
        }
        return Line::from(spans);
    }

    // Indented detail lines
    if raw.starts_with("  ") || raw.starts_with('\t') {
        return Line::from(Span::styled(raw, Style::default().fg(Color::Gray)));
    }

    // Default: white
    Line::from(Span::styled(raw, Style::default().fg(Color::White)))
}
