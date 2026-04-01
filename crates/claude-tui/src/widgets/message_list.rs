//! Message list widget for the conversation display.
//!
//! Renders a scrolling list of [`MessageEntry`] items with virtual scrolling,
//! sticky-bottom auto-scroll, and optional text search highlighting.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::markdown::render_markdown;

/// A single entry in the message list.
#[derive(Clone, Debug)]
pub enum MessageEntry {
    /// A message from the user.
    User { text: String },
    /// A response from the assistant.
    Assistant { text: String },
    /// A tool invocation (before result).
    ToolUse { name: String, input_summary: String },
    /// A tool result.
    ToolResult {
        name: String,
        output: String,
        is_error: bool,
    },
    /// Extended thinking content.
    Thinking { text: String },
    /// System message (info, errors, status).
    System { text: String },
}

/// The message list state, managing messages, scroll position, and search.
pub struct MessageList {
    messages: Vec<MessageEntry>,
    scroll_offset: usize,
    sticky_bottom: bool,
    /// Active search query for highlighting.
    search_query: Option<String>,
    /// Indices of messages that match the current search.
    search_matches: Vec<usize>,
    /// Currently focused search match index.
    search_focus: usize,
}

impl MessageList {
    /// Create an empty message list.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
            sticky_bottom: true,
            search_query: None,
            search_matches: Vec::new(),
            search_focus: 0,
        }
    }

    /// Append a message to the list.
    pub fn push(&mut self, msg: MessageEntry) {
        self.messages.push(msg);
        if self.search_query.is_some() {
            self.update_search_matches();
        }
    }

    /// Access messages by reference.
    pub fn messages(&self) -> &[MessageEntry] {
        &self.messages
    }

    /// Access messages mutably (for appending to streaming assistant text).
    pub fn messages_mut(&mut self) -> &mut Vec<MessageEntry> {
        &mut self.messages
    }

    /// Number of messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Scroll up by the given number of lines.
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.sticky_bottom = false;
    }

    /// Scroll down by the given number of lines.
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset += lines;
        // sticky_bottom re-enabled in render if we reach the bottom
    }

    /// Jump to the bottom and re-enable auto-scroll.
    pub fn scroll_to_bottom(&mut self) {
        self.sticky_bottom = true;
    }

    /// Clear all messages and reset scroll.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.sticky_bottom = true;
        self.search_query = None;
        self.search_matches.clear();
        self.search_focus = 0;
    }

    /// Set a search query. Pass `None` to clear the search.
    pub fn set_search(&mut self, query: Option<String>) {
        self.search_query = query;
        self.search_focus = 0;
        self.update_search_matches();
    }

    /// The current search query, if any.
    pub fn search_query(&self) -> Option<&str> {
        self.search_query.as_deref()
    }

    /// Number of search matches.
    pub fn search_match_count(&self) -> usize {
        self.search_matches.len()
    }

    /// Jump to the next search match.
    pub fn search_next(&mut self) {
        if !self.search_matches.is_empty() {
            self.search_focus = (self.search_focus + 1) % self.search_matches.len();
        }
    }

    /// Jump to the previous search match.
    pub fn search_prev(&mut self) {
        if !self.search_matches.is_empty() {
            self.search_focus = if self.search_focus == 0 {
                self.search_matches.len() - 1
            } else {
                self.search_focus - 1
            };
        }
    }

    /// Recompute which message indices match the search query.
    fn update_search_matches(&mut self) {
        self.search_matches.clear();
        if let Some(ref query) = self.search_query {
            if query.is_empty() {
                return;
            }
            let query_lower = query.to_lowercase();
            for (idx, msg) in self.messages.iter().enumerate() {
                let text = match msg {
                    MessageEntry::User { text } => text,
                    MessageEntry::Assistant { text } => text,
                    MessageEntry::ToolResult { output, .. } => output,
                    MessageEntry::Thinking { text } => text,
                    MessageEntry::System { text } => text,
                    MessageEntry::ToolUse {
                        name,
                        input_summary,
                    } => {
                        if name.to_lowercase().contains(&query_lower)
                            || input_summary.to_lowercase().contains(&query_lower)
                        {
                            self.search_matches.push(idx);
                        }
                        continue;
                    }
                };
                if text.to_lowercase().contains(&query_lower) {
                    self.search_matches.push(idx);
                }
            }
        }
        if self.search_focus >= self.search_matches.len() {
            self.search_focus = 0;
        }
    }
}

impl Default for MessageList {
    fn default() -> Self {
        Self::new()
    }
}

/// Widget that renders the message list.
pub struct MessageListWidget<'a> {
    list: &'a MessageList,
}

impl<'a> MessageListWidget<'a> {
    /// Create a new message list widget.
    pub fn new(list: &'a MessageList) -> Self {
        Self { list }
    }
}

impl<'a> Widget for MessageListWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let mut all_lines: Vec<Line> = Vec::new();

        for (msg_idx, msg) in self.list.messages.iter().enumerate() {
            let is_search_focus = self
                .list
                .search_matches
                .get(self.list.search_focus)
                .copied()
                == Some(msg_idx);

            match msg {
                MessageEntry::User { text } => {
                    all_lines.push(Line::from(""));
                    all_lines.push(Line::from(vec![Span::styled(
                        " You ",
                        Style::default()
                            .fg(Color::White)
                            .bg(Color::Blue)
                            .add_modifier(Modifier::BOLD),
                    )]));
                    for line in text.lines() {
                        let style = if is_search_focus {
                            Style::default().bg(Color::DarkGray)
                        } else {
                            Style::default()
                        };
                        all_lines.push(Line::from(vec![
                            Span::raw(" "),
                            Span::styled(line.to_string(), style),
                        ]));
                    }
                }
                MessageEntry::Assistant { text } => {
                    all_lines.push(Line::from(""));
                    all_lines.push(Line::from(vec![Span::styled(
                        " Claude ",
                        Style::default()
                            .fg(Color::White)
                            .bg(Color::Rgb(180, 100, 60))
                            .add_modifier(Modifier::BOLD),
                    )]));
                    let md_lines = render_markdown(text);
                    for md_line in md_lines {
                        let mut spans = vec![Span::raw(" ")];
                        spans.extend(md_line.spans.into_iter().map(|s| {
                            if is_search_focus {
                                Span::styled(s.content.to_string(), s.style.bg(Color::DarkGray))
                            } else {
                                Span::styled(s.content.to_string(), s.style)
                            }
                        }));
                        all_lines.push(Line::from(spans));
                    }
                }
                MessageEntry::ToolUse {
                    name,
                    input_summary,
                } => {
                    all_lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!(" {} ", name),
                            Style::default().fg(Color::White).bg(Color::Magenta),
                        ),
                        Span::raw(" "),
                        Span::styled(
                            input_summary.clone(),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                MessageEntry::ToolResult {
                    name: _,
                    output,
                    is_error,
                } => {
                    let indicator = if *is_error { "\u{2718}" } else { "\u{2714}" };
                    let color = if *is_error { Color::Red } else { Color::Green };
                    all_lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!("{} ", indicator),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            if *is_error { "Error" } else { "Done" },
                            Style::default().fg(color),
                        ),
                    ]));
                    let max_preview_lines = 6;
                    for line in output.lines().take(max_preview_lines) {
                        all_lines.push(Line::from(Span::styled(
                            format!("    {}", line),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                    let output_line_count = output.lines().count();
                    if output_line_count > max_preview_lines {
                        all_lines.push(Line::from(Span::styled(
                            format!(
                                "    ... ({} more lines)",
                                output_line_count - max_preview_lines
                            ),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                MessageEntry::Thinking { text } => {
                    let preview = if text.len() > 100 {
                        format!("{}...", &text[..97])
                    } else {
                        text.clone()
                    };
                    all_lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            "thinking",
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        ),
                        Span::styled(
                            format!(" {}", preview),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                MessageEntry::System { text } => {
                    all_lines.push(Line::from(vec![
                        Span::raw(" "),
                        Span::styled(text.clone(), Style::default().fg(Color::Yellow)),
                    ]));
                }
            }
        }

        // Virtual scrolling
        let total_lines = all_lines.len();
        let visible_height = area.height as usize;

        let scroll = if self.list.sticky_bottom {
            total_lines.saturating_sub(visible_height)
        } else {
            self.list
                .scroll_offset
                .min(total_lines.saturating_sub(visible_height))
        };

        let visible = &all_lines[scroll..total_lines.min(scroll + visible_height)];
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
    fn test_push_and_len() {
        let mut list = MessageList::new();
        assert!(list.is_empty());
        list.push(MessageEntry::User {
            text: "hello".to_string(),
        });
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_clear() {
        let mut list = MessageList::new();
        list.push(MessageEntry::System {
            text: "test".to_string(),
        });
        list.clear();
        assert!(list.is_empty());
    }

    #[test]
    fn test_scroll() {
        let mut list = MessageList::new();
        list.scroll_down(5);
        assert_eq!(list.scroll_offset, 5);
        list.scroll_up(3);
        assert_eq!(list.scroll_offset, 2);
        list.scroll_to_bottom();
        assert!(list.sticky_bottom);
    }

    #[test]
    fn test_search() {
        let mut list = MessageList::new();
        list.push(MessageEntry::User {
            text: "hello world".to_string(),
        });
        list.push(MessageEntry::Assistant {
            text: "goodbye".to_string(),
        });
        list.push(MessageEntry::User {
            text: "hello again".to_string(),
        });

        list.set_search(Some("hello".to_string()));
        assert_eq!(list.search_match_count(), 2);

        list.search_next();
        list.search_prev();

        list.set_search(None);
        assert_eq!(list.search_match_count(), 0);
    }

    #[test]
    fn test_search_empty_query() {
        let mut list = MessageList::new();
        list.push(MessageEntry::User {
            text: "hello".to_string(),
        });
        list.set_search(Some(String::new()));
        assert_eq!(list.search_match_count(), 0);
    }
}
