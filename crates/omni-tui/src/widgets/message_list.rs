//! Message list widget for the conversation display.
//!
//! Renders a scrolling list of [`MessageEntry`] items with virtual scrolling,
//! sticky-bottom auto-scroll, and optional text search highlighting.
//! Supports 13+ message types with rich rendering including badges, spinners,
//! syntax highlighting, collapsible sections, and diff previews.

use std::cell::Cell;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::markdown::render_markdown;
use crate::syntax::highlight_code_block;

/// Minimum interval between scroll-triggered re-renders (60fps).
const SCROLL_DEBOUNCE_MS: u128 = 16;

/// Number of extra lines to render above and below the visible viewport.
const OVERSCAN_LINES: usize = 10;

/// Spinner frames for animated tool-use indicators.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Default number of visible lines before collapsing output.
const COLLAPSE_THRESHOLD: usize = 10;

/// Default number of thinking lines shown when collapsed.
const THINKING_PREVIEW_LINES: usize = 2;

/// Status of a tool invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolUseStatus {
    Pending,
    Running,
    Complete,
    Error,
}

/// Severity level for system messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemSeverity {
    Info,
    Warning,
    Error,
}

/// A single entry in the message list.
#[derive(Clone, Debug)]
pub enum MessageEntry {
    /// A message from the user.
    User { text: String, images: Vec<String> },
    /// A response from the assistant.
    Assistant { text: String },
    /// A tool invocation (before result).
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        status: ToolUseStatus,
    },
    /// A tool result.
    ToolResult {
        id: String,
        name: String,
        output: String,
        is_error: bool,
        duration_ms: Option<u64>,
    },
    /// Extended thinking content.
    Thinking { text: String, is_collapsed: bool },
    /// System message (info, errors, status).
    System {
        text: String,
        severity: SystemSeverity,
    },
    /// Compact boundary after context compaction.
    CompactBoundary { summary: String },
    /// Raw command output display.
    CommandOutput { command: String, output: String },
    /// Error retry indicator.
    ErrorRetry {
        attempt: u32,
        max_attempts: u32,
        wait_ms: u64,
        error: String,
    },
    /// Rate limit warning.
    RateLimitWarning {
        message: String,
        utilization: f64,
    },
    /// Permission request pending.
    PermissionRequest {
        tool: String,
        input_preview: String,
    },
    /// Agent/teammate status update.
    AgentStatus {
        agent_id: String,
        name: String,
        status: String,
    },
    /// Diff preview for file changes.
    DiffPreview {
        file_path: String,
        additions: usize,
        deletions: usize,
        diff_text: String,
    },
}

/// The message list state, managing messages, scroll position, and search.
pub struct MessageList {
    messages: Vec<MessageEntry>,
    scroll_offset: usize,
    sticky_bottom: bool,
    /// True (default) — follows new messages automatically.
    auto_scroll: bool,
    /// Active search query for highlighting.
    search_query: Option<String>,
    /// Indices of messages that match the current search.
    search_matches: Vec<usize>,
    /// Currently focused search match index.
    search_focus: usize,
    /// Current spinner frame (ticks at 80ms).
    spinner_frame: usize,
    /// Set of message indices with expanded tool output.
    expanded: std::collections::HashSet<usize>,
    /// Whether the assistant is currently streaming.
    streaming: bool,
    /// Compact mode: collapse tool outputs and thinking blocks.
    compact_mode: bool,
    /// Cached viewport height from last render.
    viewport_height: Cell<usize>,
    /// Number of messages added since the user scrolled away from the bottom.
    new_messages_count: usize,
    /// Timestamp of the last scroll event (for debouncing).
    last_scroll_time: Option<Instant>,
}

impl MessageList {
    /// Create an empty message list.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
            sticky_bottom: true,
            auto_scroll: true,
            search_query: None,
            search_matches: Vec::new(),
            search_focus: 0,
            spinner_frame: 0,
            expanded: std::collections::HashSet::new(),
            streaming: false,
            compact_mode: false,
            viewport_height: Cell::new(0),
            new_messages_count: 0,
            last_scroll_time: None,
        }
    }

    /// Toggle compact transcript view mode.
    pub fn toggle_compact_mode(&mut self) {
        self.compact_mode = !self.compact_mode;
    }

    /// Whether compact mode is active.
    pub fn compact_mode(&self) -> bool {
        self.compact_mode
    }

    /// Append a message to the list.
    pub fn push(&mut self, msg: MessageEntry) {
        self.messages.push(msg);
        if self.auto_scroll {
            self.scroll_to_bottom();
        } else {
            self.new_messages_count += 1;
        }
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

    /// Access all message entries.
    pub fn entries(&self) -> &[MessageEntry] {
        &self.messages
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Whether enough time has elapsed since the last scroll event to re-render.
    pub fn should_debounce_scroll(&self) -> bool {
        if let Some(last) = self.last_scroll_time {
            last.elapsed().as_millis() < SCROLL_DEBOUNCE_MS
        } else {
            false
        }
    }

    /// Scroll up by the given number of lines.
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.sticky_bottom = false;
        self.auto_scroll = false;
        self.last_scroll_time = Some(Instant::now());
    }

    /// Scroll down by the given number of lines.
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset += lines;
        self.last_scroll_time = Some(Instant::now());
        // sticky_bottom and auto_scroll re-enabled in render if we reach the bottom
    }

    /// Jump to the bottom and re-enable auto-scroll.
    pub fn scroll_to_bottom(&mut self) {
        self.sticky_bottom = true;
        self.auto_scroll = true;
        self.new_messages_count = 0;
    }

    /// Jump to the top.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.sticky_bottom = false;
        self.auto_scroll = false;
    }

    /// Clear all messages and reset scroll.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.sticky_bottom = true;
        self.auto_scroll = true;
        self.search_query = None;
        self.search_matches.clear();
        self.search_focus = 0;
        self.expanded.clear();
        self.streaming = false;
        self.new_messages_count = 0;
        self.last_scroll_time = None;
    }

    /// Advance the spinner frame. Call this on each SpinnerTick (~80ms).
    pub fn tick_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
    }

    /// Toggle expanded state for a message at the given index.
    pub fn toggle_expanded(&mut self, msg_idx: usize) {
        if self.expanded.contains(&msg_idx) {
            self.expanded.remove(&msg_idx);
        } else {
            self.expanded.insert(msg_idx);
        }
    }

    /// Set whether the assistant is currently streaming.
    pub fn set_streaming(&mut self, streaming: bool) {
        self.streaming = streaming;
    }

    /// Whether the assistant is currently streaming.
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// Number of new messages since the user scrolled away from the bottom.
    pub fn new_messages_count(&self) -> usize {
        self.new_messages_count
    }

    /// Cached viewport height (set during render).
    pub fn viewport_height(&self) -> usize {
        self.viewport_height.get()
    }

    /// Find a ToolUse entry by tool use ID and update it in-place.
    /// Returns true if found and updated.
    pub fn update_tool_status(&mut self, id: &str, status: ToolUseStatus) -> bool {
        for msg in self.messages.iter_mut().rev() {
            if let MessageEntry::ToolUse {
                id: ref tool_id,
                status: ref mut s,
                ..
            } = msg
            {
                if tool_id == id {
                    *s = status;
                    return true;
                }
            }
        }
        false
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

    /// The current search focus index (for overlay display).
    pub fn search_focus_index(&self) -> usize {
        self.search_focus
    }

    /// Scroll so the message at the given index is roughly visible.
    /// Uses a heuristic: estimate ~3 lines per message.
    pub fn scroll_to_message(&mut self, msg_idx: usize) {
        let estimated_line = msg_idx.saturating_mul(3);
        self.scroll_offset = estimated_line;
        self.sticky_bottom = false;
    }

    /// Save current scroll position (for restoring after search cancel).
    pub fn save_scroll_position(&self) -> usize {
        self.scroll_offset
    }

    /// Restore a previously saved scroll position.
    pub fn restore_scroll_position(&mut self, offset: usize) {
        self.scroll_offset = offset;
        self.sticky_bottom = false;
    }

    /// Return the message index of the current focused search match, if any.
    pub fn current_search_match_message(&self) -> Option<usize> {
        self.search_matches.get(self.search_focus).copied()
    }

    /// Return the text of the last assistant message, if any.
    pub fn last_assistant_text(&self) -> Option<String> {
        for msg in self.messages.iter().rev() {
            if let MessageEntry::Assistant { text } = msg {
                return Some(text.clone());
            }
        }
        None
    }

    /// Return the index of the last assistant message, if any.
    pub fn last_assistant_index(&self) -> Option<usize> {
        for (i, msg) in self.messages.iter().enumerate().rev() {
            if let MessageEntry::Assistant { .. } = msg {
                return Some(i);
            }
        }
        None
    }

    /// Truncate messages to keep only up to (and including) the given index.
    pub fn truncate(&mut self, up_to_inclusive: usize) {
        if up_to_inclusive + 1 < self.messages.len() {
            self.messages.truncate(up_to_inclusive + 1);
        }
        if self.search_query.is_some() {
            self.update_search_matches();
        }
    }

    /// Estimate the rendered height of a single message (in terminal lines).
    pub fn estimate_message_height(&self, msg: &MessageEntry, width: u16) -> usize {
        let w = width.max(1) as usize;
        match msg {
            MessageEntry::User { text, images } => {
                // blank + badge + text lines + images
                2 + text.lines().count().max(1).saturating_add(
                    text.lines().map(|l| l.len() / w).sum::<usize>()
                ) + images.len()
            }
            MessageEntry::Assistant { text } => {
                // blank + badge + text lines
                2 + text.lines().count().max(1).saturating_add(
                    text.lines().map(|l| l.len() / w).sum::<usize>()
                )
            }
            MessageEntry::ToolUse { input, .. } => {
                // tool header + compact summary
                2 + if self.expanded.contains(&0) {
                    serde_json::to_string_pretty(input)
                        .unwrap_or_default()
                        .lines()
                        .count()
                } else {
                    1
                }
            }
            MessageEntry::ToolResult { output, .. } => {
                2 + output.lines().count().min(COLLAPSE_THRESHOLD + 1)
            }
            MessageEntry::Thinking { text, is_collapsed } => {
                if *is_collapsed {
                    1 + THINKING_PREVIEW_LINES
                } else {
                    1 + text.lines().count()
                }
            }
            MessageEntry::System { .. } => 1,
            MessageEntry::CompactBoundary { .. } => 3,
            MessageEntry::CommandOutput { output, .. } => 1 + output.lines().count(),
            MessageEntry::ErrorRetry { error, .. } => if error.is_empty() { 1 } else { 2 },
            MessageEntry::RateLimitWarning { .. } => 2,
            MessageEntry::PermissionRequest { input_preview, .. } => {
                if input_preview.is_empty() { 1 } else { 2 }
            }
            MessageEntry::AgentStatus { .. } => 1,
            MessageEntry::DiffPreview { diff_text, .. } => {
                1 + diff_text.lines().count().min(COLLAPSE_THRESHOLD + 1)
            }
        }
    }

    /// Estimate the rendered height of a message at a given index, accounting for expand state.
    /// This is used by the virtual scrolling renderer to compute cumulative offsets.
    fn estimate_message_height_for_render(&self, msg: &MessageEntry, width: u16, msg_idx: usize) -> usize {
        let w = width.max(1) as usize;
        let is_expanded = self.expanded.contains(&msg_idx);
        match msg {
            MessageEntry::User { text, images } => {
                2 + text.lines().count().max(1).saturating_add(
                    text.lines().map(|l| l.len() / w).sum::<usize>()
                ) + images.len()
            }
            MessageEntry::Assistant { text } => {
                let extra = if msg_idx + 1 == self.messages.len() && self.streaming { 1 } else { 0 };
                2 + text.lines().count().max(1).saturating_add(
                    text.lines().map(|l| l.len() / w).sum::<usize>()
                ) + extra
            }
            MessageEntry::ToolUse { input, .. } => {
                2 + if is_expanded {
                    serde_json::to_string_pretty(input)
                        .unwrap_or_default()
                        .lines()
                        .count()
                } else {
                    1
                }
            }
            MessageEntry::ToolResult { output, .. } => {
                let output_lines = output.lines().count();
                let visible = if is_expanded || output_lines <= COLLAPSE_THRESHOLD {
                    output_lines
                } else {
                    COLLAPSE_THRESHOLD + 1
                };
                1 + visible
            }
            MessageEntry::Thinking { text, is_collapsed } => {
                let collapsed = *is_collapsed && !is_expanded;
                if collapsed {
                    1 + THINKING_PREVIEW_LINES.min(text.lines().count())
                } else {
                    1 + text.lines().count()
                }
            }
            MessageEntry::System { .. } => 1,
            MessageEntry::CompactBoundary { .. } => 3,
            MessageEntry::CommandOutput { output, .. } => 1 + output.lines().count(),
            MessageEntry::ErrorRetry { error, .. } => if error.is_empty() { 1 } else { 2 },
            MessageEntry::RateLimitWarning { .. } => 2,
            MessageEntry::PermissionRequest { input_preview, .. } => {
                if input_preview.is_empty() { 1 } else { 2 }
            }
            MessageEntry::AgentStatus { .. } => 1,
            MessageEntry::DiffPreview { diff_text, .. } => {
                let diff_lines = diff_text.lines().count();
                let visible = if is_expanded || diff_lines <= COLLAPSE_THRESHOLD {
                    diff_lines
                } else {
                    COLLAPSE_THRESHOLD + 1
                };
                1 + visible
            }
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
                let matches = match msg {
                    MessageEntry::User { text, .. } => {
                        text.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::Assistant { text } => {
                        text.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::ToolUse { name, input, .. } => {
                        name.to_lowercase().contains(&query_lower)
                            || input.to_string().to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::ToolResult { output, name, .. } => {
                        name.to_lowercase().contains(&query_lower)
                            || output.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::Thinking { text, .. } => {
                        text.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::System { text, .. } => {
                        text.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::CompactBoundary { summary } => {
                        summary.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::CommandOutput {
                        command, output, ..
                    } => {
                        command.to_lowercase().contains(&query_lower)
                            || output.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::ErrorRetry { error, .. } => {
                        error.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::RateLimitWarning { message, .. } => {
                        message.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::PermissionRequest { tool, .. } => {
                        tool.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::AgentStatus { name, status, .. } => {
                        name.to_lowercase().contains(&query_lower)
                            || status.to_lowercase().contains(&query_lower)
                    }
                    MessageEntry::DiffPreview {
                        file_path,
                        diff_text,
                        ..
                    } => {
                        file_path.to_lowercase().contains(&query_lower)
                            || diff_text.to_lowercase().contains(&query_lower)
                    }
                };
                if matches {
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

        let visible_height = area.height as usize;
        self.list.viewport_height.set(visible_height);

        // Phase 1: Compute cumulative line offsets per message using height estimates.
        // cumulative_heights[i] = total lines for messages 0..=i
        let msg_count = self.list.messages.len();
        if msg_count == 0 {
            return;
        }

        let mut cumulative_heights: Vec<usize> = Vec::with_capacity(msg_count);
        let mut running_total: usize = 0;
        for (idx, msg) in self.list.messages.iter().enumerate() {
            let h = self.list.estimate_message_height_for_render(msg, area.width, idx);
            running_total += h;
            cumulative_heights.push(running_total);
        }
        let total_lines = running_total;

        // Phase 2: Determine the scroll line offset.
        let scroll = if self.list.sticky_bottom {
            total_lines.saturating_sub(visible_height)
        } else {
            self.list
                .scroll_offset
                .min(total_lines.saturating_sub(visible_height))
        };

        let viewport_start = scroll.saturating_sub(OVERSCAN_LINES);
        let viewport_end = (scroll + visible_height + OVERSCAN_LINES).min(total_lines);

        // Phase 3: Find the range of messages that overlap [viewport_start..viewport_end].
        let first_msg = match cumulative_heights.binary_search(&(viewport_start + 1)) {
            Ok(idx) => idx,
            Err(idx) => idx,
        };
        let last_msg = match cumulative_heights.binary_search(&viewport_end) {
            Ok(idx) => idx,
            Err(idx) => idx.min(msg_count - 1),
        };

        // The line offset where first_msg starts.
        let first_msg_line_start = if first_msg == 0 {
            0
        } else {
            cumulative_heights[first_msg - 1]
        };

        // Phase 4: Render only the visible messages into lines.
        let mut all_lines: Vec<Line> = Vec::new();

        for msg_idx in first_msg..=last_msg {
            let msg = &self.list.messages[msg_idx];
            let is_search_focus = self
                .list
                .search_matches
                .get(self.list.search_focus)
                .copied()
                == Some(msg_idx);
            let is_expanded = self.list.expanded.contains(&msg_idx);

            let search_highlight = if is_search_focus {
                Some(Style::default().bg(Color::DarkGray))
            } else {
                None
            };

            Self::render_message(
                self.list,
                msg,
                msg_idx,
                is_expanded,
                search_highlight,
                &mut all_lines,
            );
        }

        // Phase 5: Slice the rendered lines to the visible viewport.
        // `all_lines` starts at line `first_msg_line_start`.
        // We need lines from `scroll` to `scroll + visible_height`.
        let local_scroll_start = scroll.saturating_sub(first_msg_line_start);
        let local_scroll_end = (local_scroll_start + visible_height).min(all_lines.len());

        let visible = &all_lines[local_scroll_start..local_scroll_end];
        for (i, line) in visible.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }
            buf.set_line(area.x, y, line, area.width);
        }

        // Phase 6: Overlay indicators.
        let lines_above = scroll;
        let visible_end_line = (scroll + visible_height).min(total_lines);
        let lines_below = total_lines.saturating_sub(visible_end_line);

        if lines_above > 0 && area.height > 0 {
            let new_msg_hint = if self.list.new_messages_count > 0 {
                format!(
                    " \u{2191} {} new message{} ",
                    self.list.new_messages_count,
                    if self.list.new_messages_count == 1 { "" } else { "s" }
                )
            } else {
                format!(" \u{25b2} {} more above ", lines_above)
            };
            let indicator_line = Line::from(Span::styled(
                new_msg_hint,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ));
            buf.set_line(area.x, area.y, &indicator_line, area.width);
        }
        if lines_below > 0 && area.height > 1 {
            let indicator = format!(" \u{25bc} {} more below ", lines_below);
            let indicator_line = Line::from(Span::styled(
                indicator,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ));
            let bottom_y = area.y + area.height - 1;
            buf.set_line(area.x, bottom_y, &indicator_line, area.width);
        }
    }
}

impl<'a> MessageListWidget<'a> {
    /// Render a single message entry into the given line buffer.
    fn render_message(
        list: &MessageList,
        msg: &MessageEntry,
        msg_idx: usize,
        is_expanded: bool,
        search_highlight: Option<Style>,
        all_lines: &mut Vec<Line<'static>>,
    ) {
        match msg {
            MessageEntry::User { text, images } => {
                all_lines.push(Line::from(""));
                all_lines.push(Line::from(vec![Span::styled(
                    " You ",
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                )]));
                let md_lines = render_markdown(text);
                for md_line in md_lines {
                    let mut spans = vec![Span::raw(" ")];
                    spans.extend(md_line.spans.into_iter().map(|s| {
                        if let Some(hl) = search_highlight {
                            Span::styled(s.content.to_string(), s.style.patch(hl))
                        } else {
                            Span::styled(s.content.to_string(), s.style)
                        }
                    }));
                    all_lines.push(Line::from(spans));
                }
                for img in images {
                    all_lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!("\u{1f4ce} {}", img),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        ),
                    ]));
                }
            }
            MessageEntry::Assistant { text } => {
                all_lines.push(Line::from(""));
                all_lines.push(Line::from(vec![Span::styled(
                    " Claude ",
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )]));
                let md_lines = render_markdown(text);
                for md_line in md_lines {
                    let mut spans = vec![Span::raw(" ")];
                    spans.extend(md_line.spans.into_iter().map(|s| {
                        if let Some(hl) = search_highlight {
                            Span::styled(s.content.to_string(), s.style.patch(hl))
                        } else {
                            Span::styled(s.content.to_string(), s.style)
                        }
                    }));
                    all_lines.push(Line::from(spans));
                }
                let is_last = msg_idx + 1 == list.messages.len();
                if is_last && list.streaming {
                    let cursor_char = if list.spinner_frame % 2 == 0 {
                        "\u{2588}"
                    } else {
                        " "
                    };
                    all_lines.push(Line::from(vec![
                        Span::raw(" "),
                        Span::styled(
                            cursor_char,
                            Style::default().fg(Color::Green),
                        ),
                    ]));
                }
            }
            MessageEntry::ToolUse {
                id: _,
                name,
                input,
                status,
            } => {
                let (status_indicator, status_color) = match status {
                    ToolUseStatus::Pending => ("\u{25e6}", Color::DarkGray),
                    ToolUseStatus::Running => {
                        let frame = SPINNER_FRAMES[list.spinner_frame];
                        (frame, Color::Yellow)
                    }
                    ToolUseStatus::Complete => ("\u{2714}", Color::Green),
                    ToolUseStatus::Error => ("\u{2718}", Color::Red),
                };

                let mut tool_spans = vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} ", status_indicator),
                        Style::default()
                            .fg(status_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {} ", name),
                        Style::default().fg(Color::White).bg(Color::Magenta),
                    ),
                ];

                if *status == ToolUseStatus::Running {
                    tool_spans.push(Span::styled(
                        " running...",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::ITALIC),
                    ));
                }

                all_lines.push(Line::from(tool_spans));

                let input_str =
                    serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                if is_expanded {
                    let highlighted = highlight_code_block("json", &input_str);
                    for hl_line in highlighted {
                        let mut spans = vec![Span::raw("    ")];
                        spans.extend(
                            hl_line
                                .spans
                                .into_iter()
                                .map(|s| Span::styled(s.content.to_string(), s.style)),
                        );
                        all_lines.push(Line::from(spans));
                    }
                } else {
                    let compact = serde_json::to_string(input)
                        .unwrap_or_else(|_| input.to_string());
                    let summary = if compact.len() > 120 {
                        format!("{}...", &compact[..117])
                    } else {
                        compact
                    };
                    all_lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(summary, Style::default().fg(Color::DarkGray)),
                    ]));
                }
            }
            MessageEntry::ToolResult {
                id: _,
                name,
                output,
                is_error,
                duration_ms,
            } => {
                let indicator = if *is_error { "\u{2718}" } else { "\u{2714}" };
                let color = if *is_error { Color::Red } else { Color::Green };
                let label = if *is_error { "Error" } else { "Done" };

                let mut result_spans = vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} ", indicator),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{} ", label),
                        Style::default().fg(color),
                    ),
                    Span::styled(
                        name.clone(),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ];

                if let Some(ms) = duration_ms {
                    let duration_text = if *ms >= 1000 {
                        format!(" ({:.1}s)", *ms as f64 / 1000.0)
                    } else {
                        format!(" ({}ms)", ms)
                    };
                    result_spans.push(Span::styled(
                        duration_text,
                        Style::default().fg(Color::DarkGray),
                    ));
                }

                all_lines.push(Line::from(result_spans));

                let output_lines: Vec<&str> = output.lines().collect();
                let show_all = is_expanded || output_lines.len() <= COLLAPSE_THRESHOLD;
                let visible_count = if show_all {
                    output_lines.len()
                } else {
                    COLLAPSE_THRESHOLD
                };

                let looks_like_json = output.trim_start().starts_with('{')
                    || output.trim_start().starts_with('[');

                if looks_like_json && !*is_error {
                    let highlighted = highlight_code_block("json", output);
                    for (i, hl_line) in highlighted.iter().enumerate() {
                        if i >= visible_count {
                            break;
                        }
                        let mut spans = vec![Span::raw("    ")];
                        spans.extend(
                            hl_line
                                .spans
                                .iter()
                                .map(|s| Span::styled(s.content.to_string(), s.style)),
                        );
                        all_lines.push(Line::from(spans));
                    }
                } else {
                    for line in output_lines.iter().take(visible_count) {
                        let style = if *is_error {
                            Style::default().fg(Color::Red)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        all_lines.push(Line::from(Span::styled(
                            format!("    {}", line),
                            style,
                        )));
                    }
                }

                if !show_all {
                    let remaining = output_lines.len() - COLLAPSE_THRESHOLD;
                    all_lines.push(Line::from(Span::styled(
                        format!("    \u{25b8} {} more lines", remaining),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
            MessageEntry::Thinking {
                text,
                is_collapsed,
            } => {
                let collapsed = *is_collapsed && !is_expanded;
                let thinking_lines: Vec<&str> = text.lines().collect();

                all_lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "\u{1f4ad} thinking",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC | Modifier::BOLD),
                    ),
                    if collapsed && thinking_lines.len() > THINKING_PREVIEW_LINES {
                        Span::styled(
                            format!(
                                "  \u{25b8} {} lines",
                                thinking_lines.len()
                            ),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        )
                    } else {
                        Span::raw("")
                    },
                ]));

                let visible_lines = if collapsed {
                    thinking_lines
                        .iter()
                        .take(THINKING_PREVIEW_LINES)
                        .copied()
                        .collect::<Vec<_>>()
                } else {
                    thinking_lines
                };

                for line in &visible_lines {
                    all_lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(
                            line.to_string(),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        ),
                    ]));
                }
            }
            MessageEntry::System { text, severity } => {
                let (badge, badge_style) = match severity {
                    SystemSeverity::Info => (
                        " \u{2139} ",
                        Style::default()
                            .fg(Color::White)
                            .bg(Color::Blue),
                    ),
                    SystemSeverity::Warning => (
                        " \u{26a0} ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    SystemSeverity::Error => (
                        " \u{2718} ",
                        Style::default()
                            .fg(Color::White)
                            .bg(Color::Red)
                            .add_modifier(Modifier::BOLD),
                    ),
                };
                let text_color = match severity {
                    SystemSeverity::Info => Color::Blue,
                    SystemSeverity::Warning => Color::Yellow,
                    SystemSeverity::Error => Color::Red,
                };
                all_lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(badge, badge_style),
                    Span::raw(" "),
                    Span::styled(text.clone(), Style::default().fg(text_color)),
                ]));
            }
            MessageEntry::CompactBoundary { summary } => {
                all_lines.push(Line::from(""));
                all_lines.push(Line::from(Span::styled(
                    format!(
                        "\u{2500}\u{2500}\u{2500} Context compacted \u{2500}\u{2500}\u{2500} {}",
                        if summary.is_empty() {
                            String::new()
                        } else {
                            format!("({})", summary)
                        }
                    ),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
                all_lines.push(Line::from(""));
            }
            MessageEntry::CommandOutput { command, output } => {
                all_lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        " $ ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        command.clone(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                let highlighted = highlight_code_block("bash", output);
                for hl_line in highlighted {
                    let mut spans = vec![Span::raw("    ")];
                    spans.extend(
                        hl_line
                            .spans
                            .into_iter()
                            .map(|s| Span::styled(s.content.to_string(), s.style)),
                    );
                    all_lines.push(Line::from(spans));
                }
            }
            MessageEntry::ErrorRetry {
                attempt,
                max_attempts,
                wait_ms,
                error,
            } => {
                let wait_str = if *wait_ms >= 1000 {
                    format!("{:.1}s", *wait_ms as f64 / 1000.0)
                } else {
                    format!("{}ms", wait_ms)
                };
                all_lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        " \u{21bb} ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Rgb(255, 165, 0))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("Retry {}/{} in {}", attempt, max_attempts, wait_str),
                        Style::default().fg(Color::Rgb(255, 165, 0)),
                    ),
                ]));
                if !error.is_empty() {
                    all_lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(
                            error.clone(),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        ),
                    ]));
                }
            }
            MessageEntry::RateLimitWarning {
                message,
                utilization,
            } => {
                let bar_width = 20;
                let filled = ((utilization * bar_width as f64) as usize).min(bar_width);
                let empty = bar_width - filled;
                let bar_color = if *utilization > 0.8 {
                    Color::Red
                } else if *utilization > 0.5 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                all_lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        " \u{26a0} Rate Limit ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(message.clone(), Style::default().fg(Color::Yellow)),
                ]));
                all_lines.push(Line::from(vec![
                    Span::raw("    "),
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
                        Style::default().fg(bar_color),
                    ),
                ]));
            }
            MessageEntry::PermissionRequest {
                tool,
                input_preview,
            } => {
                all_lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        " \u{1f512} Permission ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        tool.clone(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                if !input_preview.is_empty() {
                    all_lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(
                            input_preview.clone(),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
            }
            MessageEntry::AgentStatus {
                agent_id: _,
                name,
                status,
            } => {
                let (icon, color) = match status.as_str() {
                    "running" | "active" => ("\u{25cf}", Color::Yellow),
                    "completed" | "done" => ("\u{25cf}", Color::Green),
                    "error" | "failed" => ("\u{25cf}", Color::Red),
                    _ => ("\u{25cb}", Color::DarkGray),
                };
                all_lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} ", icon),
                        Style::default().fg(color),
                    ),
                    Span::styled(
                        name.clone(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        status.clone(),
                        Style::default().fg(color),
                    ),
                ]));
            }
            MessageEntry::DiffPreview {
                file_path,
                additions,
                deletions,
                diff_text,
            } => {
                all_lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        file_path.clone(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("+{}", additions),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("-{}", deletions),
                        Style::default().fg(Color::Red),
                    ),
                ]));

                let diff_lines: Vec<&str> = diff_text.lines().collect();
                let visible = if is_expanded || diff_lines.len() <= COLLAPSE_THRESHOLD {
                    &diff_lines[..]
                } else {
                    &diff_lines[..COLLAPSE_THRESHOLD]
                };

                for line in visible {
                    let style = if line.starts_with('+') {
                        Style::default().fg(Color::Green)
                    } else if line.starts_with('-') {
                        Style::default().fg(Color::Red)
                    } else if line.starts_with("@@") {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    all_lines.push(Line::from(Span::styled(
                        format!("    {}", line),
                        style,
                    )));
                }

                if !is_expanded && diff_lines.len() > COLLAPSE_THRESHOLD {
                    let remaining = diff_lines.len() - COLLAPSE_THRESHOLD;
                    all_lines.push(Line::from(Span::styled(
                        format!("    \u{25b8} {} more lines", remaining),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
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
            images: vec![],
        });
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_clear() {
        let mut list = MessageList::new();
        list.push(MessageEntry::System {
            text: "test".to_string(),
            severity: SystemSeverity::Info,
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
            images: vec![],
        });
        list.push(MessageEntry::Assistant {
            text: "goodbye".to_string(),
        });
        list.push(MessageEntry::User {
            text: "hello again".to_string(),
            images: vec![],
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
            images: vec![],
        });
        list.set_search(Some(String::new()));
        assert_eq!(list.search_match_count(), 0);
    }

    #[test]
    fn test_spinner_tick() {
        let mut list = MessageList::new();
        assert_eq!(list.spinner_frame, 0);
        list.tick_spinner();
        assert_eq!(list.spinner_frame, 1);
        for _ in 0..SPINNER_FRAMES.len() {
            list.tick_spinner();
        }
        // Should wrap around
        assert_eq!(list.spinner_frame, 1);
    }

    #[test]
    fn test_toggle_expanded() {
        let mut list = MessageList::new();
        assert!(!list.expanded.contains(&0));
        list.toggle_expanded(0);
        assert!(list.expanded.contains(&0));
        list.toggle_expanded(0);
        assert!(!list.expanded.contains(&0));
    }

    #[test]
    fn test_update_tool_status() {
        let mut list = MessageList::new();
        list.push(MessageEntry::ToolUse {
            id: "tool-1".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"path": "/tmp/test"}),
            status: ToolUseStatus::Pending,
        });
        assert!(list.update_tool_status("tool-1", ToolUseStatus::Running));
        if let MessageEntry::ToolUse { status, .. } = &list.messages()[0] {
            assert_eq!(*status, ToolUseStatus::Running);
        } else {
            panic!("expected ToolUse");
        }
        assert!(!list.update_tool_status("nonexistent", ToolUseStatus::Complete));
    }

    #[test]
    fn test_streaming() {
        let mut list = MessageList::new();
        assert!(!list.is_streaming());
        list.set_streaming(true);
        assert!(list.is_streaming());
        list.set_streaming(false);
        assert!(!list.is_streaming());
    }

    #[test]
    fn test_search_tool_use() {
        let mut list = MessageList::new();
        list.push(MessageEntry::ToolUse {
            id: "t1".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"file_path": "/tmp/test.rs"}),
            status: ToolUseStatus::Complete,
        });
        list.set_search(Some("test.rs".to_string()));
        assert_eq!(list.search_match_count(), 1);
    }

    #[test]
    fn test_search_diff_preview() {
        let mut list = MessageList::new();
        list.push(MessageEntry::DiffPreview {
            file_path: "src/main.rs".to_string(),
            additions: 5,
            deletions: 2,
            diff_text: "+fn new_function() {}\n-fn old_function() {}".to_string(),
        });
        list.set_search(Some("main.rs".to_string()));
        assert_eq!(list.search_match_count(), 1);
    }
}
