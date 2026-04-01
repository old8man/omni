use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use crate::theme;

/// Actions returned by the prompt input key handler.
pub enum InputAction {
    /// Submit the entire text buffer.
    Submit(String),
    /// No action needed by the caller.
    None,
}

/// Cursor position within a multiline buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPos {
    /// Line index (0-based).
    pub row: usize,
    /// Column (byte offset within the line).
    pub col: usize,
}

impl CursorPos {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// A single completion entry.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// Display text shown in the popup.
    pub label: String,
    /// Text inserted on accept.
    pub insert: String,
    /// Optional description shown beside the label.
    pub description: Option<String>,
}

/// Active completion popup state.
#[derive(Debug, Clone)]
pub struct CompletionState {
    /// Available items.
    pub items: Vec<CompletionItem>,
    /// Currently highlighted index.
    pub selected: usize,
    /// Byte offset in the current line where the completable token starts.
    pub token_start: usize,
}

impl CompletionState {
    pub fn new(items: Vec<CompletionItem>, token_start: usize) -> Self {
        Self {
            items,
            selected: 0,
            token_start,
        }
    }

    /// Move selection forward (wrapping).
    pub fn next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    /// Move selection backward (wrapping).
    pub fn prev(&mut self) {
        if !self.items.is_empty() {
            self.selected = if self.selected == 0 {
                self.items.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    /// The currently selected item, if any.
    pub fn current(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }
}

/// History search state for Ctrl+R reverse search.
#[derive(Debug, Clone)]
pub struct HistorySearchState {
    /// Whether the search UI is active.
    pub active: bool,
    /// The current search query.
    pub query: String,
    /// Index into matched history entries (relative to `matches`).
    pub match_index: usize,
    /// Indices into the history Vec that match the current query.
    pub matches: Vec<usize>,
    /// Lines saved when the search was activated, restored on Escape.
    pub saved_lines: Vec<String>,
    /// Cursor saved when the search was activated, restored on Escape.
    pub saved_cursor: CursorPos,
}

impl HistorySearchState {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            match_index: 0,
            matches: Vec::new(),
            saved_lines: vec![String::new()],
            saved_cursor: CursorPos::new(0, 0),
        }
    }
}

impl Default for HistorySearchState {
    fn default() -> Self {
        Self::new()
    }
}

/// Known slash commands with descriptions (sorted alphabetically).
pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/add-dir", "Add a new working directory"),
    ("/advisor", "Configure the advisor model"),
    ("/agents", "Manage agent configurations"),
    ("/branch", "Branch the conversation at this point"),
    ("/brief", "Toggle brief-only mode"),
    ("/btw", "Ask a quick side question without interrupting"),
    ("/chrome", "Claude in Chrome (Beta) settings"),
    ("/clear", "Clear conversation history"),
    ("/color", "Set the prompt bar color for this session"),
    ("/commit", "Create a git commit"),
    ("/commit-push-pr", "Commit, push, and create a pull request"),
    ("/compact", "Compact conversation context"),
    ("/config", "Show current configuration"),
    ("/context", "Show context window usage"),
    ("/copy", "Copy Claude's last response to clipboard"),
    ("/cost", "Show the total cost and duration of the session"),
    ("/ctx-viz", "Visualize context window usage as a colored grid"),
    ("/desktop", "Continue the current session in Claude Desktop"),
    ("/diff", "Show git diff for current project"),
    ("/doctor", "Run diagnostic checks"),
    ("/effort", "Set effort level for model usage"),
    ("/env", "View or set environment variables"),
    ("/exit", "Quit the application"),
    ("/export", "Export conversation to file"),
    ("/extra-usage", "Configure extra usage when limits are hit"),
    ("/fast", "Toggle fast mode"),
    ("/feedback", "Send feedback or report an issue"),
    ("/files", "List all files currently in context"),
    ("/heapdump", "Dump process diagnostics to ~/Desktop"),
    ("/help", "Show available commands"),
    ("/hooks", "View and manage hooks"),
    ("/ide", "Manage IDE integrations and show status"),
    ("/init", "Initialize project configuration"),
    ("/install-github-app", "Set up Claude GitHub Actions for a repo"),
    ("/install-slack-app", "Install the Claude Slack app"),
    ("/keybindings", "View and customize keyboard shortcuts"),
    ("/login", "Log in to your Anthropic account"),
    ("/logout", "Log out and clear stored credentials"),
    ("/mcp", "Manage MCP servers"),
    ("/memory", "List CLAUDE.md memory files"),
    ("/mobile", "Show QR code to download the Claude mobile app"),
    ("/model", "Show or switch model"),
    ("/passes", "Share a free week of Claude Code with friends"),
    ("/permissions", "View and manage permission rules"),
    ("/plan", "Toggle plan mode"),
    ("/plugin", "Manage plugins"),
    ("/pr", "Commit, push, and create a pull request"),
    ("/pr-comments", "Get comments from a GitHub pull request"),
    ("/privacy-settings", "View and update your privacy settings"),
    ("/quit", "Quit the application"),
    ("/rate-limit-options", "Show options when rate limit is reached"),
    ("/release-notes", "View release notes"),
    ("/reload-plugins", "Activate pending plugin changes"),
    ("/remote-control", "Connect terminal for remote-control sessions"),
    ("/remote-env", "Configure default remote environment"),
    ("/rename", "Rename the current conversation"),
    ("/resume", "Resume a previous session"),
    ("/review", "Review a pull request"),
    ("/rewind", "Restore code/conversation to a previous point"),
    ("/sandbox", "Toggle sandbox mode for shell commands"),
    ("/security-review", "Security review of pending changes"),
    ("/session", "Show session info and remote URL"),
    ("/share", "Share conversation transcript"),
    ("/skills", "List available skills"),
    ("/stats", "Show usage statistics and activity"),
    ("/status", "Show session status"),
    ("/stickers", "Order Claude Code stickers"),
    ("/tag", "Toggle a searchable tag on the current session"),
    ("/tasks", "View current task list"),
    ("/terminal-setup", "Install Shift+Enter key binding for newlines"),
    ("/theme", "Switch color theme"),
    ("/think-back", "Your Claude Code Year in Review"),
    ("/thinkback-play", "Play the thinkback animation"),
    ("/upgrade", "Check for and install Claude Code updates"),
    ("/usage", "Show token usage and cost"),
    ("/version", "Show application version"),
    ("/vim", "Toggle vim mode"),
    ("/voice", "Toggle voice input mode"),
    ("/web-setup", "Setup Claude Code on the web"),
];

/// Match quality for fuzzy command matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchQuality {
    /// Command starts with the query (best).
    Prefix = 0,
    /// Command contains the query as a substring.
    Substring = 1,
    /// All query characters appear in order in the command.
    Fuzzy = 2,
}

/// Check if `haystack` contains all characters of `needle` in order (fuzzy match).
fn fuzzy_matches(needle: &str, haystack: &str) -> bool {
    let mut hay_chars = haystack.chars();
    for nc in needle.chars() {
        let nc_lower = nc.to_ascii_lowercase();
        loop {
            match hay_chars.next() {
                Some(hc) if hc.to_ascii_lowercase() == nc_lower => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// Return matching slash commands for the given query, sorted by match quality.
fn match_slash_commands(query: &str) -> Vec<CompletionItem> {
    let query_lower = query.to_lowercase();
    let mut matches: Vec<(MatchQuality, &str, &str)> = Vec::new();

    for &(cmd, desc) in SLASH_COMMANDS {
        let cmd_lower = cmd.to_lowercase();
        let quality = if cmd_lower.starts_with(&query_lower) {
            Some(MatchQuality::Prefix)
        } else if cmd_lower.contains(&query_lower) {
            Some(MatchQuality::Substring)
        } else if fuzzy_matches(&query_lower, &cmd_lower) {
            Some(MatchQuality::Fuzzy)
        } else {
            None
        };
        if let Some(q) = quality {
            matches.push((q, cmd, desc));
        }
    }

    matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));

    matches
        .into_iter()
        .map(|(_, cmd, desc)| CompletionItem {
            label: cmd.to_string(),
            insert: cmd.to_string(),
            description: Some(desc.to_string()),
        })
        .collect()
}

/// Maximum number of history entries persisted to disk.
const MAX_HISTORY_ENTRIES: usize = 1000;

/// Multiline prompt input with history, completion, and history search.
pub struct PromptInput {
    /// Lines of text (always at least one empty string).
    lines: Vec<String>,
    /// Cursor position within the line buffer.
    cursor: CursorPos,
    /// Submitted input history.
    history: Vec<String>,
    /// `None` = editing current buffer; `Some(i)` = browsing history[i].
    history_index: Option<usize>,
    /// Current input saved when the user starts browsing history.
    saved_current: Vec<String>,
    /// Active completion popup, if any.
    pub completion: Option<CompletionState>,
    /// History search state (Ctrl+R).
    pub history_search: HistorySearchState,
}

/// Return the path to the history file: `~/.claude-omni/history.jsonl`
fn history_file_path() -> Option<std::path::PathBuf> {
    dirs_home().map(|h| std::path::PathBuf::from(h).join(".claude-omni").join("history.jsonl"))
}

/// Load history from `~/.claude/history.jsonl` (one JSON object per line).
fn load_history_from_disk() -> Vec<String> {
    let Some(path) = history_file_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Each line is a JSON object: {"text":"...","timestamp":...,"cwd":"..."}
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    entries.push(text.to_string());
                }
            }
        } else {
            // Fallback: treat as plain text
            entries.push(line.to_string());
        }
    }
    // Keep only the last MAX_HISTORY_ENTRIES
    if entries.len() > MAX_HISTORY_ENTRIES {
        entries.drain(..entries.len() - MAX_HISTORY_ENTRIES);
    }
    entries
}

/// Append a single entry to `~/.claude/history.jsonl`.
fn save_history_entry(text: &str) {
    let Some(path) = history_file_path() else {
        return;
    };
    // Ensure directory exists
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let entry = serde_json::json!({
        "text": text,
        "timestamp": timestamp,
        "cwd": cwd,
    });
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", entry);
    }
}

impl PromptInput {
    pub fn new() -> Self {
        let history = load_history_from_disk();
        Self {
            lines: vec![String::new()],
            cursor: CursorPos::new(0, 0),
            history,
            history_index: None,
            saved_current: vec![String::new()],
            completion: None,
            history_search: HistorySearchState::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Public accessors
    // -----------------------------------------------------------------------

    /// Return the full text joined with newlines.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Direct access to lines.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Number of lines.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Current cursor position as (row, col).
    pub fn cursor_pos(&self) -> CursorPos {
        self.cursor
    }

    /// Flat byte offset compatible with the old single-line API.
    /// The vim layer still works with flat offsets into the joined text.
    pub fn cursor(&self) -> usize {
        let mut offset = 0;
        for i in 0..self.cursor.row {
            offset += self.lines[i].len() + 1; // +1 for '\n'
        }
        offset + self.cursor.col
    }

    /// Whether the buffer is completely empty.
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Clear the input buffer and reset cursor.
    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor = CursorPos::new(0, 0);
        self.history_index = None;
        self.completion = None;
    }

    /// Access to history entries.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    // -----------------------------------------------------------------------
    // Flat-offset API (used by the vim/input handler layer)
    // -----------------------------------------------------------------------

    /// Set cursor from a flat byte offset (clamped to text length).
    pub fn set_cursor(&mut self, flat: usize) {
        let (row, col) = self.flat_to_pos(flat);
        self.cursor = CursorPos::new(row, col);
    }

    /// Delete a byte range [from, to) in the flat text and reposition cursor.
    pub fn delete_range(&mut self, from: usize, to: usize) {
        let mut joined = self.text();
        let from = from.min(joined.len());
        let to = to.min(joined.len());
        if from < to {
            joined.replace_range(from..to, "");
            self.set_lines_from_string(&joined);
            self.set_cursor(from);
        }
    }

    /// Replace the character at a flat byte offset with another character.
    pub fn replace_char(&mut self, offset: usize, ch: char) {
        let mut joined = self.text();
        if offset < joined.len() {
            if let Some(old) = joined[offset..].chars().next() {
                let end = offset + old.len_utf8();
                let mut buf = [0u8; 4];
                let replacement = ch.encode_utf8(&mut buf);
                joined.replace_range(offset..end, replacement);
                self.set_lines_from_string(&joined);
            }
        }
    }

    /// Insert a string at a flat byte position.
    pub fn insert_str_at(&mut self, pos: usize, s: &str) {
        let mut joined = self.text();
        let pos = pos.min(joined.len());
        joined.insert_str(pos, s);
        self.set_lines_from_string(&joined);
        self.set_cursor(pos + s.len());
    }

    /// Submit: add to history, clear buffer, return the submitted text.
    pub fn submit(&mut self) -> String {
        let submitted = self.text();
        if !submitted.is_empty() {
            self.history.push(submitted.clone());
            // Persist to ~/.claude/history.jsonl
            save_history_entry(&submitted);
            // Trim in-memory history to limit
            if self.history.len() > MAX_HISTORY_ENTRIES {
                self.history.drain(..self.history.len() - MAX_HISTORY_ENTRIES);
            }
        }
        self.lines = vec![String::new()];
        self.cursor = CursorPos::new(0, 0);
        self.history_index = None;
        self.completion = None;
        submitted
    }

    /// Undo placeholder.
    pub fn undo(&mut self) {}

    /// Select all text: move cursor to end of last line.
    pub fn select_all(&mut self) {
        let last_row = self.lines.len().saturating_sub(1);
        self.cursor = CursorPos::new(last_row, self.lines[last_row].len());
    }

    /// Paste text from the system clipboard.
    /// Returns true if paste was successful.
    pub fn paste_clipboard(&mut self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            if let Ok(mut ctx) = copypasta::ClipboardContext::new() {
                use copypasta::ClipboardProvider;
                if let Ok(content) = ctx.get_contents() {
                    self.insert_paste(&content);
                    return true;
                }
            }
        }
        let _ = self; // suppress unused warning on other platforms
        false
    }

    /// Clear all text in the buffer.
    pub fn clear_buffer(&mut self) {
        self.lines = vec![String::new()];
        self.cursor = CursorPos::new(0, 0);
        self.completion = None;
    }

    // -----------------------------------------------------------------------
    // Multiline-aware key handler
    // -----------------------------------------------------------------------

    /// Handle a key event for the prompt input.
    ///
    /// This handles normal typing, emacs-style shortcuts, multiline editing,
    /// and completion/history-search interactions.
    pub fn handle_key(&mut self, key: KeyEvent) -> InputAction {
        // ----- History search mode -----
        if self.history_search.active {
            return self.handle_history_search_key(key);
        }

        // ----- Completion popup -----
        if self.completion.is_some() {
            if let Some(action) = self.handle_completion_key(&key) {
                return action;
            }
        }

        match (key.modifiers, key.code) {
            // ---- Submit shortcuts ----
            (KeyModifiers::CONTROL, KeyCode::Enter) if !self.is_empty() => {
                InputAction::Submit(self.submit())
            }
            (KeyModifiers::SHIFT, KeyCode::Enter) if !self.is_empty() => {
                InputAction::Submit(self.submit())
            }
            (KeyModifiers::ALT, KeyCode::Enter) if !self.is_empty() => {
                InputAction::Submit(self.submit())
            }
            (_, KeyCode::Enter) if key.modifiers.is_empty() => {
                if self.lines.len() == 1 && !self.is_empty() {
                    InputAction::Submit(self.submit())
                } else if self.is_empty() {
                    InputAction::None
                } else {
                    self.insert_newline();
                    InputAction::None
                }
            }

            // ---- Tab completion ----
            (_, KeyCode::Tab) if key.modifiers.is_empty() => {
                self.trigger_or_cycle_completion(false);
                InputAction::None
            }
            (_, KeyCode::BackTab) | (KeyModifiers::SHIFT, KeyCode::Tab) => {
                self.trigger_or_cycle_completion(true);
                InputAction::None
            }

            // ---- Character input ----
            (_, KeyCode::Char(c))
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                if self.history_index.is_some() {
                    self.history_index = None;
                }
                self.insert_char(c);
                // Auto-trigger slash command completion when typing at start of line
                self.refresh_slash_completion();
                InputAction::None
            }

            // ---- Backspace ----
            (_, KeyCode::Backspace) => {
                if self.history_index.is_some() { self.history_index = None; }
                self.backspace();
                // Re-trigger completion after backspace if still in a slash command
                self.refresh_slash_completion();
                InputAction::None
            }

            // ---- Delete ----
            (_, KeyCode::Delete) => {
                if self.history_index.is_some() { self.history_index = None; }
                self.delete_forward();
                self.completion = None;
                InputAction::None
            }

            // ---- Arrow keys ----
            (_, KeyCode::Left) if key.modifiers.is_empty() => {
                self.move_left();
                InputAction::None
            }
            (_, KeyCode::Right) if key.modifiers.is_empty() => {
                self.move_right();
                InputAction::None
            }
            (_, KeyCode::Up) if key.modifiers.is_empty() => {
                if self.cursor.row > 0 {
                    self.cursor.row -= 1;
                    self.cursor.col = self.cursor.col.min(self.lines[self.cursor.row].len());
                } else {
                    self.history_prev();
                }
                InputAction::None
            }
            (_, KeyCode::Down) if key.modifiers.is_empty() => {
                if self.cursor.row + 1 < self.lines.len() {
                    self.cursor.row += 1;
                    self.cursor.col = self.cursor.col.min(self.lines[self.cursor.row].len());
                } else {
                    self.history_next();
                }
                InputAction::None
            }

            // ---- Word navigation ----
            (KeyModifiers::ALT, KeyCode::Left)
            | (KeyModifiers::CONTROL, KeyCode::Left) => {
                self.move_word_left();
                InputAction::None
            }
            (KeyModifiers::ALT, KeyCode::Right)
            | (KeyModifiers::CONTROL, KeyCode::Right) => {
                self.move_word_right();
                InputAction::None
            }

            (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                self.cursor.col = 0;
                InputAction::None
            }
            (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                self.cursor.col = self.lines[self.cursor.row].len();
                InputAction::None
            }
            (KeyModifiers::CONTROL, KeyCode::Char('k')) => {
                self.lines[self.cursor.row].truncate(self.cursor.col);
                InputAction::None
            }
            (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                let rest = self.lines[self.cursor.row][self.cursor.col..].to_string();
                self.lines[self.cursor.row] = rest;
                self.cursor.col = 0;
                InputAction::None
            }
            (KeyModifiers::CONTROL, KeyCode::Char('w')) => {
                self.delete_word_backward();
                InputAction::None
            }

            (_, KeyCode::Home) => {
                self.cursor.col = 0;
                InputAction::None
            }
            (_, KeyCode::End) => {
                self.cursor.col = self.lines[self.cursor.row].len();
                InputAction::None
            }

            _ => InputAction::None,
        }
    }

    // -----------------------------------------------------------------------
    // Multiline editing operations
    // -----------------------------------------------------------------------

    fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.cursor.row];
        line.insert(self.cursor.col, c);
        self.cursor.col += c.len_utf8();
    }

    fn insert_newline(&mut self) {
        let rest = self.lines[self.cursor.row][self.cursor.col..].to_string();
        self.lines[self.cursor.row].truncate(self.cursor.col);
        self.cursor.row += 1;
        self.lines.insert(self.cursor.row, rest);
        self.cursor.col = 0;
    }

    fn backspace(&mut self) {
        if self.cursor.col > 0 {
            let prev_len = self.lines[self.cursor.row][..self.cursor.col]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor.col -= prev_len;
            let end = self.cursor.col + prev_len;
            self.lines[self.cursor.row].replace_range(self.cursor.col..end, "");
        } else if self.cursor.row > 0 {
            let current = self.lines.remove(self.cursor.row);
            self.cursor.row -= 1;
            self.cursor.col = self.lines[self.cursor.row].len();
            self.lines[self.cursor.row].push_str(&current);
        }
    }

    fn delete_forward(&mut self) {
        let line_len = self.lines[self.cursor.row].len();
        if self.cursor.col < line_len {
            let char_len = self.lines[self.cursor.row][self.cursor.col..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            let end = (self.cursor.col + char_len).min(line_len);
            self.lines[self.cursor.row].replace_range(self.cursor.col..end, "");
        } else if self.cursor.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor.row + 1);
            self.lines[self.cursor.row].push_str(&next);
        }
    }

    fn move_left(&mut self) {
        if self.cursor.col > 0 {
            let prev_len = self.lines[self.cursor.row][..self.cursor.col]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor.col -= prev_len;
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.cursor.col = self.lines[self.cursor.row].len();
        }
    }

    fn move_right(&mut self) {
        let line_len = self.lines[self.cursor.row].len();
        if self.cursor.col < line_len {
            let next_len = self.lines[self.cursor.row][self.cursor.col..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor.col += next_len;
        } else if self.cursor.row + 1 < self.lines.len() {
            self.cursor.row += 1;
            self.cursor.col = 0;
        }
    }

    fn move_word_left(&mut self) {
        if self.cursor.col == 0 {
            if self.cursor.row > 0 {
                self.cursor.row -= 1;
                self.cursor.col = self.lines[self.cursor.row].len();
            }
            return;
        }

        let line = &self.lines[self.cursor.row];
        let before = &line[..self.cursor.col];
        let trimmed = before.trim_end();
        if trimmed.is_empty() {
            self.cursor.col = 0;
            return;
        }
        let last_word_end = trimmed.len();
        let word_start = trimmed
            .rfind(|c: char| c.is_whitespace() || !c.is_alphanumeric() && c != '_')
            .map(|i| {
                let nc = i + trimmed[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                if nc >= last_word_end { i } else { nc }
            })
            .unwrap_or(0);
        self.cursor.col = word_start;
    }

    fn move_word_right(&mut self) {
        let line = &self.lines[self.cursor.row];
        if self.cursor.col >= line.len() {
            if self.cursor.row + 1 < self.lines.len() {
                self.cursor.row += 1;
                self.cursor.col = 0;
            }
            return;
        }

        let after = &line[self.cursor.col..];
        let mut pos = 0;
        let mut chars = after.chars();
        for c in chars.by_ref() {
            if c.is_whitespace() {
                break;
            }
            pos += c.len_utf8();
        }
        for c in chars {
            if !c.is_whitespace() {
                break;
            }
            pos += c.len_utf8();
        }
        if pos == 0 {
            let mut chars2 = after.chars();
            for c in chars2.by_ref() {
                if !c.is_whitespace() {
                    break;
                }
                pos += c.len_utf8();
            }
        }
        self.cursor.col = (self.cursor.col + pos).min(line.len());
    }

    fn delete_word_backward(&mut self) {
        if self.cursor.col == 0 {
            return;
        }
        let line = &self.lines[self.cursor.row];
        let before = &line[..self.cursor.col];
        let trimmed = before.trim_end();
        let word_boundary = if trimmed.is_empty() {
            0
        } else {
            trimmed
                .rfind(|c: char| c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(0)
        };
        let old_col = self.cursor.col;
        self.cursor.col = word_boundary;
        self.lines[self.cursor.row].replace_range(word_boundary..old_col, "");
    }

    // -----------------------------------------------------------------------
    // History browsing
    // -----------------------------------------------------------------------

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.saved_current = self.lines.clone();
                self.history_index = Some(self.history.len() - 1);
            }
            Some(0) => return,
            Some(i) => {
                self.history_index = Some(i - 1);
            }
        }
        if let Some(i) = self.history_index {
            self.set_lines_from_string(&self.history[i].clone());
            let last_row = self.lines.len() - 1;
            self.cursor = CursorPos::new(last_row, self.lines[last_row].len());
        }
    }

    fn history_next(&mut self) {
        match self.history_index {
            None => (),
            Some(i) if i >= self.history.len() - 1 => {
                self.history_index = None;
                self.lines = self.saved_current.clone();
                let last_row = self.lines.len() - 1;
                self.cursor = CursorPos::new(last_row, self.lines[last_row].len());
            }
            Some(i) => {
                self.history_index = Some(i + 1);
                self.set_lines_from_string(&self.history[i + 1].clone());
                let last_row = self.lines.len() - 1;
                self.cursor = CursorPos::new(last_row, self.lines[last_row].len());
            }
        }
    }

    // -----------------------------------------------------------------------
    // Reverse history search (Ctrl+R)
    // -----------------------------------------------------------------------

    pub fn start_history_search(&mut self) {
        self.history_search.saved_lines = self.lines.clone();
        self.history_search.saved_cursor = self.cursor;
        self.history_search.active = true;
        self.history_search.query.clear();
        self.history_search.match_index = 0;
        self.update_history_search_matches();
        self.preview_history_search_match();
    }

    fn update_history_search_matches(&mut self) {
        let query = self.history_search.query.to_lowercase();
        self.history_search.matches = self
            .history
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, entry)| {
                if query.is_empty() {
                    true
                } else {
                    entry.to_lowercase().contains(&query)
                }
            })
            .map(|(i, _)| i)
            .collect();
        if self.history_search.match_index >= self.history_search.matches.len() {
            self.history_search.match_index = 0;
        }
    }

    fn handle_history_search_key(&mut self, key: KeyEvent) -> InputAction {
        match key.code {
            KeyCode::Esc => {
                self.lines = self.history_search.saved_lines.clone();
                self.cursor = self.history_search.saved_cursor;
                self.history_search.active = false;
                InputAction::None
            }
            KeyCode::Enter => {
                self.history_search.active = false;
                InputAction::None
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.history_search.matches.is_empty() {
                    self.history_search.match_index =
                        (self.history_search.match_index + 1) % self.history_search.matches.len();
                    self.preview_history_search_match();
                }
                InputAction::None
            }
            KeyCode::Down => {
                if !self.history_search.matches.is_empty() {
                    self.history_search.match_index =
                        (self.history_search.match_index + 1) % self.history_search.matches.len();
                    self.preview_history_search_match();
                }
                InputAction::None
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.history_search.matches.is_empty() {
                    self.history_search.match_index =
                        if self.history_search.match_index == 0 {
                            self.history_search.matches.len() - 1
                        } else {
                            self.history_search.match_index - 1
                        };
                    self.preview_history_search_match();
                }
                InputAction::None
            }
            KeyCode::Up => {
                if !self.history_search.matches.is_empty() {
                    self.history_search.match_index =
                        if self.history_search.match_index == 0 {
                            self.history_search.matches.len() - 1
                        } else {
                            self.history_search.match_index - 1
                        };
                    self.preview_history_search_match();
                }
                InputAction::None
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.lines = self.history_search.saved_lines.clone();
                self.cursor = self.history_search.saved_cursor;
                self.history_search.active = false;
                InputAction::None
            }
            KeyCode::Backspace => {
                self.history_search.query.pop();
                self.history_search.match_index = 0;
                self.update_history_search_matches();
                self.preview_history_search_match();
                InputAction::None
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.history_search.query.push(c);
                self.history_search.match_index = 0;
                self.update_history_search_matches();
                self.preview_history_search_match();
                InputAction::None
            }
            _ => InputAction::None,
        }
    }

    fn preview_history_search_match(&mut self) {
        if let Some(&hist_idx) = self
            .history_search
            .matches
            .get(self.history_search.match_index)
        {
            let entry = self.history[hist_idx].clone();
            self.set_lines_from_string(&entry);
            let last_row = self.lines.len() - 1;
            self.cursor = CursorPos::new(last_row, self.lines[last_row].len());
        } else {
            self.lines = self.history_search.saved_lines.clone();
            self.cursor = self.history_search.saved_cursor;
        }
    }

    /// Return the currently matched history entry for display.
    pub fn history_search_current_match(&self) -> Option<&str> {
        if !self.history_search.active {
            return None;
        }
        self.history_search
            .matches
            .get(self.history_search.match_index)
            .and_then(|&idx| self.history.get(idx))
            .map(|s| s.as_str())
    }

    // -----------------------------------------------------------------------
    // Tab completion
    // -----------------------------------------------------------------------

    /// Refresh slash command completion based on current input.
    /// Called after character input and backspace to keep the popup in sync.
    fn refresh_slash_completion(&mut self) {
        let current_line = &self.lines[self.cursor.row];
        let before_cursor = &current_line[..self.cursor.col];
        if before_cursor.starts_with('/') && self.cursor.row == 0 {
            let items = match_slash_commands(before_cursor);
            if !items.is_empty() {
                self.completion = Some(CompletionState::new(items, 0));
            } else {
                self.completion = None;
            }
        } else {
            self.completion = None;
        }
    }

    /// Handle a key while the completion popup is visible.
    /// Returns `Some(action)` if consumed, `None` to pass through.
    fn handle_completion_key(&mut self, key: &KeyEvent) -> Option<InputAction> {
        match key.code {
            KeyCode::Tab => {
                // Accept the selected completion and dismiss popup
                self.accept_completion();
                Some(InputAction::None)
            }
            KeyCode::BackTab => {
                if let Some(ref mut state) = self.completion {
                    state.prev();
                }
                Some(InputAction::None)
            }
            KeyCode::Enter => {
                self.accept_completion();
                let text = self.text();
                if text.starts_with('/') && SLASH_COMMANDS.iter().any(|(cmd, _)| *cmd == text) {
                    Some(InputAction::Submit(self.submit()))
                } else {
                    Some(InputAction::None)
                }
            }
            KeyCode::Esc => {
                self.completion = None;
                Some(InputAction::None)
            }
            KeyCode::Up => {
                if let Some(ref mut state) = self.completion {
                    state.prev();
                }
                Some(InputAction::None)
            }
            KeyCode::Down => {
                if let Some(ref mut state) = self.completion {
                    state.next();
                }
                Some(InputAction::None)
            }
            _ => None, // pass through to normal handling
        }
    }

    fn trigger_or_cycle_completion(&mut self, backward: bool) {
        if let Some(ref mut state) = self.completion {
            // Already showing completions -- cycle
            if backward {
                state.prev();
            } else {
                state.next();
            }
            return;
        }

        // Build completion items based on context
        let line = &self.lines[self.cursor.row];
        let before = &line[..self.cursor.col];

        let items = if before.starts_with('/') {
            match_slash_commands(before)
        } else {
            let token_start = before
                .rfind(|c: char| c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(0);
            let token = &before[token_start..];
            if token.contains('/') || token.starts_with('.') || token.starts_with('~') {
                self.complete_file_path(token, token_start)
            } else {
                Vec::new()
            }
        };

        if !items.is_empty() {
            let token_start = if before.starts_with('/') {
                0
            } else {
                before
                    .rfind(|c: char| c.is_whitespace())
                    .map(|i| i + 1)
                    .unwrap_or(0)
            };
            self.completion = Some(CompletionState::new(items, token_start));
        }
    }

    /// Accept the currently selected completion item.
    pub fn accept_completion(&mut self) {
        if let Some(state) = self.completion.take() {
            if let Some(item) = state.items.get(state.selected) {
                let insert = item.insert.clone();
                let line = &mut self.lines[self.cursor.row];
                line.replace_range(state.token_start..self.cursor.col, &insert);
                self.cursor.col = state.token_start + insert.len();
            }
        }
    }

    /// Attempt file path completion for a given token.
    fn complete_file_path(&self, token: &str, _token_start: usize) -> Vec<CompletionItem> {
        use std::path::Path;

        let expanded = if token.starts_with('~') {
            if let Some(home) = dirs_home() {
                token.replacen('~', &home, 1)
            } else {
                token.to_string()
            }
        } else {
            token.to_string()
        };

        let path = Path::new(&expanded);
        let (dir, prefix) = if path.is_dir() {
            (path.to_path_buf(), "")
        } else {
            (
                path.parent().unwrap_or(Path::new(".")).to_path_buf(),
                path.file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or(""),
            )
        };

        let mut items = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if prefix.is_empty() || name_str.starts_with(prefix) {
                    let mut full = if token.ends_with('/') || path.is_dir() {
                        format!("{}{}", token, name_str)
                    } else {
                        let parent_token = token
                            .rfind('/')
                            .map(|i| &token[..=i])
                            .unwrap_or("");
                        format!("{}{}", parent_token, name_str)
                    };
                    if entry.path().is_dir() && !full.ends_with('/') {
                        full.push('/');
                    }
                    items.push(CompletionItem {
                        label: name_str.to_string(),
                        insert: full,
                        description: if entry.path().is_dir() {
                            Some("dir".to_string())
                        } else {
                            None
                        },
                    });
                }
            }
        }
        items.sort_by(|a, b| a.label.cmp(&b.label));
        items.truncate(20);
        items
    }

    // -----------------------------------------------------------------------
    // Paste support
    // -----------------------------------------------------------------------

    /// Insert pasted text, which may contain newlines.
    pub fn insert_paste(&mut self, pasted: &str) {
        if pasted.is_empty() {
            return;
        }
        let paste_lines: Vec<&str> = pasted.split('\n').collect();
        if paste_lines.len() == 1 {
            let line = &mut self.lines[self.cursor.row];
            line.insert_str(self.cursor.col, paste_lines[0]);
            self.cursor.col += paste_lines[0].len();
        } else {
            let rest = self.lines[self.cursor.row][self.cursor.col..].to_string();
            self.lines[self.cursor.row].truncate(self.cursor.col);
            self.lines[self.cursor.row].push_str(paste_lines[0]);

            for (i, paste_line) in paste_lines.iter().enumerate().skip(1) {
                if i == paste_lines.len() - 1 {
                    let new_line = format!("{}{}", paste_line, rest);
                    self.cursor.row += 1;
                    self.cursor.col = paste_line.len();
                    self.lines.insert(self.cursor.row, new_line);
                } else {
                    self.cursor.row += 1;
                    self.lines.insert(self.cursor.row, paste_line.to_string());
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn set_lines_from_string(&mut self, s: &str) {
        self.lines = s.split('\n').map(String::from).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
    }

    fn flat_to_pos(&self, flat: usize) -> (usize, usize) {
        let mut remaining = flat;
        for (i, line) in self.lines.iter().enumerate() {
            if remaining <= line.len() || i == self.lines.len() - 1 {
                return (i, remaining.min(line.len()));
            }
            remaining -= line.len() + 1;
        }
        let last = self.lines.len() - 1;
        (last, self.lines[last].len())
    }
}

impl Default for PromptInput {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the home directory as a String, if available.
fn dirs_home() -> Option<String> {
    std::env::var("HOME").ok()
}

// ===========================================================================
// Widget: PromptInputWidget
// ===========================================================================

pub struct PromptInputWidget<'a> {
    input: &'a PromptInput,
    style: Style,
    vim_mode: Option<&'a str>,
}

impl<'a> PromptInputWidget<'a> {
    pub fn new(input: &'a PromptInput) -> Self {
        Self {
            input,
            style: Style::default(),
            vim_mode: None,
        }
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// Show a vim mode indicator (e.g. "-- INSERT --") in the input area.
    pub fn vim_mode(mut self, mode: &'a str) -> Self {
        self.vim_mode = Some(mode);
        self
    }
}

impl<'a> Widget for PromptInputWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let block = Block::default().borders(Borders::TOP);
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // ----- History search overlay -----
        if self.input.history_search.active {
            let prompt_text = format!(
                "reverse-i-search: {}",
                self.input.history_search.query
            );
            let matched = self
                .input
                .history_search_current_match()
                .unwrap_or("");
            let line = Line::from(vec![
                Span::styled(&prompt_text, theme::STYLE_BOLD_YELLOW),
                Span::styled(" | ", theme::STYLE_DARK_GRAY),
                Span::styled(matched, theme::STYLE_WHITE),
            ]);
            buf.set_line(inner.x, inner.y, &line, inner.width);
            return;
        }

        // ----- Vim mode indicator -----
        let _mode_width = if let Some(mode) = self.vim_mode {
            let mode_str = format!("-- {} -- ", mode);
            let mode_span = Span::styled(
                &mode_str,
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            );
            let mode_line = Line::from(vec![mode_span]);
            let mode_y = inner.y + inner.height.saturating_sub(1);
            let mode_x = inner.x + inner.width.saturating_sub(mode_str.len() as u16);
            buf.set_line(mode_x, mode_y, &mode_line, mode_str.len() as u16);
            mode_str.len() as u16
        } else {
            0
        };

        // ----- Render input lines -----
        let prompt_str = "> ";
        let prompt_width = prompt_str.len() as u16;
        let content_width = inner.width.saturating_sub(prompt_width) as usize;

        if content_width == 0 {
            return;
        }

        let max_visible_lines = inner.height as usize;
        let total_lines = self.input.lines.len();
        let cursor_row = self.input.cursor.row;

        let scroll_offset = if cursor_row >= max_visible_lines {
            cursor_row - max_visible_lines + 1
        } else {
            0
        };

        for (display_row, line_idx) in (scroll_offset..total_lines)
            .enumerate()
            .take(max_visible_lines)
        {
            let y = inner.y + display_row as u16;
            if y >= inner.y + inner.height {
                break;
            }

            let line_text = &self.input.lines[line_idx];

            let prefix = if line_idx == 0 && scroll_offset == 0 {
                prompt_str
            } else if line_idx == 0 {
                "> "
            } else {
                "  "
            };

            let spans = vec![
                Span::styled(prefix, theme::STYLE_CYAN),
                Span::raw(line_text.as_str()),
            ];
            let line = Line::from(spans);
            buf.set_line(inner.x, y, &line, inner.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn test_basic_typing_and_submit() {
        let mut p = PromptInput::new();
        p.handle_key(key(KeyCode::Char('h')));
        p.handle_key(key(KeyCode::Char('i')));
        assert_eq!(p.text(), "hi");
        let result = p.handle_key(key(KeyCode::Enter));
        assert!(matches!(result, InputAction::Submit(ref s) if s == "hi"));
        assert!(p.is_empty());
    }

    #[test]
    fn test_multiline_enter_inserts_newline() {
        let mut p = PromptInput::new();
        p.handle_key(key(KeyCode::Char('a')));
        p.insert_newline();
        p.handle_key(key(KeyCode::Char('b')));
        assert_eq!(p.text(), "a\nb");
        assert_eq!(p.line_count(), 2);

        let result = p.handle_key(key_mod(KeyCode::Enter, KeyModifiers::CONTROL));
        assert!(matches!(result, InputAction::Submit(ref s) if s == "a\nb"));
    }

    #[test]
    fn test_cursor_flat_offset() {
        let mut p = PromptInput::new();
        p.lines = vec!["hello".to_string(), "world".to_string()];
        p.cursor = CursorPos::new(1, 3);
        assert_eq!(p.cursor(), 9);
    }

    #[test]
    fn test_set_cursor_flat() {
        let mut p = PromptInput::new();
        p.lines = vec!["abc".to_string(), "de".to_string(), "f".to_string()];
        p.set_cursor(5);
        assert_eq!(p.cursor_pos(), CursorPos::new(1, 1));
    }

    #[test]
    fn test_backspace_joins_lines() {
        let mut p = PromptInput::new();
        p.lines = vec!["abc".to_string(), "def".to_string()];
        p.cursor = CursorPos::new(1, 0);
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(p.lines(), &["abcdef"]);
        assert_eq!(p.cursor_pos(), CursorPos::new(0, 3));
    }

    #[test]
    fn test_delete_forward_joins_lines() {
        let mut p = PromptInput::new();
        p.lines = vec!["abc".to_string(), "def".to_string()];
        p.cursor = CursorPos::new(0, 3);
        p.handle_key(key(KeyCode::Delete));
        assert_eq!(p.lines(), &["abcdef"]);
    }

    #[test]
    fn test_history_search() {
        let mut p = PromptInput::new();
        p.history = vec![
            "first command".to_string(),
            "second thing".to_string(),
            "first again".to_string(),
        ];
        p.start_history_search();
        assert!(p.history_search.active);

        for c in "first".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.history_search.matches.len(), 2);
        let current = p.history_search_current_match().unwrap();
        assert_eq!(current, "first again");
    }

    #[test]
    fn test_paste_multiline() {
        let mut p = PromptInput::new();
        p.handle_key(key(KeyCode::Char('>')));
        p.handle_key(key(KeyCode::Char(' ')));
        p.insert_paste("line1\nline2\nline3");
        assert_eq!(p.line_count(), 3);
        assert_eq!(p.lines()[0], "> line1");
        assert_eq!(p.lines()[1], "line2");
        assert_eq!(p.lines()[2], "line3");
    }

    #[test]
    fn test_slash_command_completion() {
        let mut p = PromptInput::new();
        p.handle_key(key(KeyCode::Char('/')));
        p.handle_key(key(KeyCode::Char('m')));
        p.trigger_or_cycle_completion(false);
        assert!(p.completion.is_some());
        let comp = p.completion.as_ref().unwrap();
        assert!(comp.items.iter().any(|i| i.label == "/model"));
    }

    #[test]
    fn test_fuzzy_matching() {
        // "/cmt" should fuzzy-match "/commit"
        let items = match_slash_commands("/cmt");
        assert!(items.iter().any(|i| i.label == "/commit"));

        // "/com" should prefix-match "/commit", "/compact", etc.
        let items = match_slash_commands("/com");
        assert!(items.iter().any(|i| i.label == "/commit"));
        assert!(items.iter().any(|i| i.label == "/compact"));

        // "/rv" should fuzzy-match "/review" and "/rewind"
        let items = match_slash_commands("/rv");
        assert!(items.iter().any(|i| i.label == "/review"));

        // "/" should match everything
        let items = match_slash_commands("/");
        assert_eq!(items.len(), SLASH_COMMANDS.len());
    }

    #[test]
    fn test_fuzzy_match_quality_ordering() {
        // "/mo" should have prefix matches first
        let items = match_slash_commands("/mo");
        assert!(!items.is_empty());
        assert_eq!(items[0].label, "/mobile");
        assert!(items.iter().any(|i| i.label == "/model"));
    }

    #[test]
    fn test_tab_accepts_completion() {
        let mut p = PromptInput::new();
        p.handle_key(key(KeyCode::Char('/')));
        p.handle_key(key(KeyCode::Char('q')));
        // Completion should be auto-triggered
        assert!(p.completion.is_some());
        // Tab should accept the selected completion
        p.handle_key(key(KeyCode::Tab));
        // Completion should be dismissed after accepting
        assert!(p.completion.is_none());
        // Text should be the accepted command
        assert_eq!(p.text(), "/quit");
    }
}
