//! Search overlay widget for Ctrl+F / "/" search within output.
//!
//! Provides an interactive search bar that overlays the bottom of the
//! message area, with match count display and navigation controls.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme;

/// Action returned by the search overlay's key handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchAction {
    /// No action needed.
    None,
    /// Close the search overlay and restore original scroll position (Esc).
    Close,
    /// Close the search overlay but keep the current scroll position (Enter).
    Accept,
    /// Move to the next match.
    NextMatch,
    /// Move to the previous match.
    PreviousMatch,
    /// The query text changed — caller should update highlights.
    QueryChanged(String),
}

/// State for the search overlay.
#[derive(Debug)]
pub struct SearchOverlay {
    /// Whether the search bar is visible/active.
    pub active: bool,
    /// Current search query being typed.
    query: String,
    /// Cursor position within the query string.
    cursor: usize,
    /// Total number of matches found.
    match_count: usize,
    /// Currently focused match (1-indexed for display).
    current_match: usize,
}

impl SearchOverlay {
    /// Create a new inactive search overlay.
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            cursor: 0,
            match_count: 0,
            current_match: 0,
        }
    }

    /// Open the search overlay, optionally pre-filling a query.
    pub fn open(&mut self, initial_query: Option<&str>) {
        self.active = true;
        if let Some(q) = initial_query {
            self.query = q.to_string();
            self.cursor = self.query.len();
        } else {
            self.query.clear();
            self.cursor = 0;
        }
    }

    /// Close the search overlay.
    pub fn close(&mut self) {
        self.active = false;
    }

    /// Get the current search query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Update the match count and current match index from the message list.
    pub fn update_match_info(&mut self, count: usize, current: usize) {
        self.match_count = count;
        self.current_match = current;
    }

    /// Handle a key event while the search bar is focused.
    ///
    /// Returns a [`SearchAction`] indicating what the caller should do.
    pub fn handle_key(&mut self, key: KeyEvent) -> SearchAction {
        match key.code {
            KeyCode::Esc => {
                self.close();
                SearchAction::Close
            }
            KeyCode::Enter => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    SearchAction::PreviousMatch
                } else {
                    // Plain Enter: close search, keep scroll position at current match
                    self.close();
                    SearchAction::Accept
                }
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                SearchAction::NextMatch
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                SearchAction::PreviousMatch
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.query.remove(self.cursor);
                    SearchAction::QueryChanged(self.query.clone())
                } else if self.query.is_empty() {
                    self.close();
                    SearchAction::Close
                } else {
                    SearchAction::None
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.query.len() {
                    self.query.remove(self.cursor);
                    SearchAction::QueryChanged(self.query.clone())
                } else {
                    SearchAction::None
                }
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                SearchAction::None
            }
            KeyCode::Right => {
                if self.cursor < self.query.len() {
                    self.cursor += 1;
                }
                SearchAction::None
            }
            KeyCode::Home | KeyCode::Char('a')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.cursor = 0;
                SearchAction::None
            }
            KeyCode::End | KeyCode::Char('e')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.cursor = self.query.len();
                SearchAction::None
            }
            KeyCode::Char(c) => {
                self.query.insert(self.cursor, c);
                self.cursor += 1;
                SearchAction::QueryChanged(self.query.clone())
            }
            _ => SearchAction::None,
        }
    }
}

impl Default for SearchOverlay {
    fn default() -> Self {
        Self::new()
    }
}

/// Widget that renders the search overlay bar.
pub struct SearchOverlayWidget<'a> {
    overlay: &'a SearchOverlay,
}

impl<'a> SearchOverlayWidget<'a> {
    /// Create a new search overlay widget.
    pub fn new(overlay: &'a SearchOverlay) -> Self {
        Self { overlay }
    }
}

impl<'a> Widget for SearchOverlayWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.overlay.active || area.height == 0 {
            return;
        }

        // Background fill
        let bg_style = Style::default().bg(Color::DarkGray).fg(Color::White);
        for x in area.x..area.x + area.width {
            buf[(x, area.y)]
                .set_char(' ')
                .set_style(bg_style);
        }

        // "Search: " label
        let label = " Search: ";
        let label_style = theme::STYLE_BOLD_YELLOW.bg(Color::DarkGray);
        let label_span = Span::styled(label, label_style);

        // Query text
        let query_span = Span::styled(
            self.overlay.query.clone(),
            Style::default().bg(Color::DarkGray).fg(Color::White),
        );

        // Match count
        let match_info = if self.overlay.match_count > 0 {
            format!(
                " ({}/{})",
                self.overlay.current_match + 1,
                self.overlay.match_count
            )
        } else if !self.overlay.query.is_empty() {
            " (no matches)".to_string()
        } else {
            String::new()
        };
        let match_span = Span::styled(
            match_info,
            Style::default().bg(Color::DarkGray).fg(Color::Gray),
        );

        let line = Line::from(vec![label_span, query_span, match_span]);
        buf.set_line(area.x, area.y, &line, area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_close() {
        let mut overlay = SearchOverlay::new();
        assert!(!overlay.active);
        overlay.open(None);
        assert!(overlay.active);
        assert!(overlay.query().is_empty());
        overlay.close();
        assert!(!overlay.active);
    }

    #[test]
    fn test_open_with_query() {
        let mut overlay = SearchOverlay::new();
        overlay.open(Some("hello"));
        assert_eq!(overlay.query(), "hello");
        assert_eq!(overlay.cursor, 5);
    }

    #[test]
    fn test_typing() {
        let mut overlay = SearchOverlay::new();
        overlay.open(None);
        let action = overlay.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(matches!(action, SearchAction::QueryChanged(_)));
        assert_eq!(overlay.query(), "a");
    }

    #[test]
    fn test_escape_closes() {
        let mut overlay = SearchOverlay::new();
        overlay.open(None);
        let action = overlay.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(action, SearchAction::Close));
        assert!(!overlay.active);
    }

    #[test]
    fn test_enter_accepts() {
        let mut overlay = SearchOverlay::new();
        overlay.open(Some("test"));
        let action = overlay.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, SearchAction::Accept));
        assert!(!overlay.active);
    }

    #[test]
    fn test_shift_enter_prev_match() {
        let mut overlay = SearchOverlay::new();
        overlay.open(Some("test"));
        let action = overlay.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert!(matches!(action, SearchAction::PreviousMatch));
    }
}
