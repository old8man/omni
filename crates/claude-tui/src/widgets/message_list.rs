//! Message list widget for the conversation display.
//!
//! Renders a scrolling list of [`MessageEntry`] items with virtual scrolling,
//! sticky-bottom auto-scroll, and optional text search highlighting.
//! Supports 13+ message types with rich rendering including badges, spinners,
//! syntax highlighting, collapsible sections, and diff previews.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::markdown::render_markdown;
use crate::syntax::highlight_code_block;

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
            spinner_frame: 0,
            expanded: std::collections::HashSet::new(),
            streaming: false,
            compact_mode: false,
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
        self.expanded.clear();
        self.streaming = false;
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

        let mut all_lines: Vec<Line> = Vec::new();

        for (msg_idx, msg) in self.list.messages.iter().enumerate() {
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
                    // Render user text with markdown
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
                    // Show image indicators
                    for img in images {
                        all_lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(
                                format!("📎 {}", img),
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
                    // Show streaming cursor if this is the last message and we're streaming
                    let is_last = msg_idx + 1 == self.list.messages.len();
                    if is_last && self.list.streaming {
                        // Blinking cursor effect: use spinner frame to alternate
                        let cursor_char = if self.list.spinner_frame.is_multiple_of(2) {
                            "█"
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
                        ToolUseStatus::Pending => ("◦", Color::DarkGray),
                        ToolUseStatus::Running => {
                            let frame = SPINNER_FRAMES[self.list.spinner_frame];
                            // We can't return a reference to a local, so we handle
                            // this case inline below
                            (frame, Color::Yellow)
                        }
                        ToolUseStatus::Complete => ("✔", Color::Green),
                        ToolUseStatus::Error => ("✘", Color::Red),
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

                    // Show elapsed time for running tools
                    if *status == ToolUseStatus::Running {
                        tool_spans.push(Span::styled(
                            " running...",
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::ITALIC),
                        ));
                    }

                    all_lines.push(Line::from(tool_spans));

                    // Show input summary (compact by default, expanded on toggle)
                    let input_str =
                        serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                    if is_expanded {
                        // Full JSON input with syntax highlighting
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
                        // Compact one-line summary
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
                    let indicator = if *is_error { "✘" } else { "✔" };
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

                    // Show duration if available
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

                    // Output lines with collapsing
                    let output_lines: Vec<&str> = output.lines().collect();
                    let show_all = is_expanded || output_lines.len() <= COLLAPSE_THRESHOLD;
                    let visible_count = if show_all {
                        output_lines.len()
                    } else {
                        COLLAPSE_THRESHOLD
                    };

                    // Attempt syntax highlighting for structured output
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
                            format!("    ▸ {} more lines", remaining),
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
                            "💭 thinking",
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC | Modifier::BOLD),
                        ),
                        if collapsed && thinking_lines.len() > THINKING_PREVIEW_LINES {
                            Span::styled(
                                format!(
                                    "  ▸ {} lines",
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
                            " ℹ ",
                            Style::default()
                                .fg(Color::White)
                                .bg(Color::Blue),
                        ),
                        SystemSeverity::Warning => (
                            " ⚠ ",
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        SystemSeverity::Error => (
                            " ✘ ",
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
                            "─── Context compacted ─── {}",
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
                    // Render output with bash syntax highlighting
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
                            " ↻ ",
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Rgb(255, 165, 0)) // Orange
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
                    // Show utilization bar
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
                            " ⚠ Rate Limit ",
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
                            "█".repeat(filled),
                            Style::default().fg(bar_color),
                        ),
                        Span::styled(
                            "░".repeat(empty),
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
                            " 🔒 Permission ",
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
                        "running" | "active" => ("●", Color::Yellow),
                        "completed" | "done" => ("●", Color::Green),
                        "error" | "failed" => ("●", Color::Red),
                        _ => ("○", Color::DarkGray),
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
                    // File header with +/- counts
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

                    // Render diff lines with coloring
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
                            format!("    ▸ {} more lines", remaining),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
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
