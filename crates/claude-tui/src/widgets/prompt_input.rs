use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

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
}

impl HistorySearchState {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            match_index: 0,
            matches: Vec::new(),
        }
    }
}

impl Default for HistorySearchState {
    fn default() -> Self {
        Self::new()
    }
}

/// Known slash commands with descriptions.
pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/model", "Switch AI model"),
    ("/clear", "Clear conversation"),
    ("/compact", "Compact conversation context"),
    ("/resume", "Resume a previous session"),
    ("/help", "Show help information"),
    ("/config", "Open configuration"),
    ("/vim", "Toggle vim mode"),
    ("/theme", "Switch color theme"),
    ("/plan", "Toggle plan mode"),
    ("/commit", "Generate a commit message"),
    ("/diff", "Show git diff"),
    ("/review", "Review code changes"),
    ("/cost", "Show session cost breakdown"),
    ("/stats", "Show session statistics"),
    ("/usage", "Show token usage"),
    ("/quit", "Exit the application"),
];

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

impl PromptInput {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor: CursorPos::new(0, 0),
            history: Vec::new(),
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
        }
        self.lines = vec![String::new()];
        self.cursor = CursorPos::new(0, 0);
        self.history_index = None;
        self.completion = None;
        submitted
    }

    /// Undo placeholder — full undo stack to be added later.
    pub fn undo(&mut self) {}

    /// Select all text: move cursor to end of last line.
    /// For Cmd+A on macOS, this selects the full buffer.
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
            // Ctrl+Enter always submits
            (KeyModifiers::CONTROL, KeyCode::Enter) if !self.is_empty() => {
                InputAction::Submit(self.submit())
            }
            // Shift+Enter submits (Option+Enter maps the same on many terminals)
            (KeyModifiers::SHIFT, KeyCode::Enter) if !self.is_empty() => {
                InputAction::Submit(self.submit())
            }
            // Alt+Enter submits
            (KeyModifiers::ALT, KeyCode::Enter) if !self.is_empty() => {
                InputAction::Submit(self.submit())
            }
            // Plain Enter: if single-line and non-empty, submit; otherwise insert newline
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
                // Any editing resets history navigation — user is now editing
                // a new/modified entry, not browsing history
                if self.history_index.is_some() {
                    self.history_index = None;
                }
                self.insert_char(c);
                // Auto-trigger slash command completion when typing '/' at start of line
                let current_line = &self.lines[self.cursor.row];
                let before_cursor = &current_line[..self.cursor.col];
                if before_cursor.starts_with('/') && self.cursor.row == 0 {
                    // Refresh slash command suggestions as user types
                    let query = before_cursor;
                    let items: Vec<CompletionItem> = SLASH_COMMANDS
                        .iter()
                        .filter(|(cmd, _)| cmd.starts_with(query))
                        .map(|(cmd, desc)| CompletionItem {
                            label: cmd.to_string(),
                            insert: cmd.to_string(),
                            description: Some(desc.to_string()),
                        })
                        .collect();
                    if !items.is_empty() {
                        self.completion = Some(CompletionState::new(items, 0));
                    } else {
                        self.completion = None;
                    }
                } else {
                    self.completion = None;
                }
                InputAction::None
            }

            // ---- Backspace ----
            (_, KeyCode::Backspace) => {
                if self.history_index.is_some() { self.history_index = None; }
                self.backspace();
                self.completion = None;
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
                    // Move up within multiline buffer
                    self.cursor.row -= 1;
                    self.cursor.col = self.cursor.col.min(self.lines[self.cursor.row].len());
                } else {
                    // At top line — browse history
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

            // ---- Word navigation: Alt+Left/Right and Ctrl+Left/Right ----
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

            // ---- Ctrl+A — beginning of line ----
            (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                self.cursor.col = 0;
                InputAction::None
            }
            // ---- Ctrl+E — end of line ----
            (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                self.cursor.col = self.lines[self.cursor.row].len();
                InputAction::None
            }

            // ---- Ctrl+K — kill to end of line ----
            (KeyModifiers::CONTROL, KeyCode::Char('k')) => {
                self.lines[self.cursor.row].truncate(self.cursor.col);
                InputAction::None
            }
            // ---- Ctrl+U — kill to start of line ----
            (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                let rest = self.lines[self.cursor.row][self.cursor.col..].to_string();
                self.lines[self.cursor.row] = rest;
                self.cursor.col = 0;
                InputAction::None
            }
            // ---- Ctrl+W — delete word backward ----
            (KeyModifiers::CONTROL, KeyCode::Char('w')) => {
                self.delete_word_backward();
                InputAction::None
            }

            // ---- Home/End ----
            (_, KeyCode::Home) => {
                self.cursor.col = 0;
                InputAction::None
            }
            (_, KeyCode::End) => {
                self.cursor.col = self.lines[self.cursor.row].len();
                InputAction::None
            }

            // ---- Ctrl+R — reverse history search ----
            (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                self.start_history_search();
                InputAction::None
            }

            _ => InputAction::None,
        }
    }

    // -----------------------------------------------------------------------
    // Multiline editing operations
    // -----------------------------------------------------------------------

    /// Insert a character at the current cursor position.
    fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.cursor.row];
        line.insert(self.cursor.col, c);
        self.cursor.col += c.len_utf8();
    }

    /// Insert a newline at the current cursor position (splitting the line).
    fn insert_newline(&mut self) {
        let rest = self.lines[self.cursor.row][self.cursor.col..].to_string();
        self.lines[self.cursor.row].truncate(self.cursor.col);
        self.cursor.row += 1;
        self.lines.insert(self.cursor.row, rest);
        self.cursor.col = 0;
    }

    /// Delete the character before the cursor (backspace).
    fn backspace(&mut self) {
        if self.cursor.col > 0 {
            let prev_len = self.lines[self.cursor.row][..self.cursor.col]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor.col -= prev_len;
            // Remove the full character (may be multi-byte for UTF-8)
            let end = self.cursor.col + prev_len;
            self.lines[self.cursor.row].replace_range(self.cursor.col..end, "");
        } else if self.cursor.row > 0 {
            // Join with previous line
            let current = self.lines.remove(self.cursor.row);
            self.cursor.row -= 1;
            self.cursor.col = self.lines[self.cursor.row].len();
            self.lines[self.cursor.row].push_str(&current);
        }
    }

    /// Delete the character at the cursor (Delete key).
    fn delete_forward(&mut self) {
        let line_len = self.lines[self.cursor.row].len();
        if self.cursor.col < line_len {
            // Find the length of the character at cursor (may be multi-byte)
            let char_len = self.lines[self.cursor.row][self.cursor.col..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            let end = (self.cursor.col + char_len).min(line_len);
            self.lines[self.cursor.row].replace_range(self.cursor.col..end, "");
        } else if self.cursor.row + 1 < self.lines.len() {
            // Join next line into current
            let next = self.lines.remove(self.cursor.row + 1);
            self.lines[self.cursor.row].push_str(&next);
        }
    }

    /// Move cursor one character left.
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

    /// Move cursor one character right.
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

    /// Move cursor one word left.
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
        // Skip whitespace, then skip word chars
        let trimmed = before.trim_end();
        if trimmed.is_empty() {
            self.cursor.col = 0;
            return;
        }
        // Find start of previous word
        let last_word_end = trimmed.len();
        let word_start = trimmed
            .rfind(|c: char| c.is_whitespace() || !c.is_alphanumeric() && c != '_')
            .map(|i| {
                // i is the index of the non-word char, we want the char after it
                let nc = i + trimmed[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                if nc >= last_word_end { i } else { nc }
            })
            .unwrap_or(0);
        self.cursor.col = word_start;
    }

    /// Move cursor one word right.
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
        // Skip current word chars, then skip whitespace
        let mut pos = 0;
        let mut chars = after.chars();
        // Skip non-whitespace
        for c in chars.by_ref() {
            if c.is_whitespace() {
                break;
            }
            pos += c.len_utf8();
        }
        // Skip whitespace
        for c in chars {
            if !c.is_whitespace() {
                break;
            }
            pos += c.len_utf8();
        }
        if pos == 0 {
            // We were already at whitespace, skip it
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

    /// Delete one word backward (Ctrl+W).
    fn delete_word_backward(&mut self) {
        if self.cursor.col == 0 {
            return;
        }
        let line = &self.lines[self.cursor.row];
        let before = &line[..self.cursor.col];
        // Skip trailing whitespace
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

    fn start_history_search(&mut self) {
        self.history_search.active = true;
        self.history_search.query.clear();
        self.history_search.match_index = 0;
        self.update_history_search_matches();
    }

    fn update_history_search_matches(&mut self) {
        let query = self.history_search.query.to_lowercase();
        self.history_search.matches = self
            .history
            .iter()
            .enumerate()
            .rev() // Most recent first
            .filter(|(_, entry)| {
                if query.is_empty() {
                    true
                } else {
                    entry.to_lowercase().contains(&query)
                }
            })
            .map(|(i, _)| i)
            .collect();
        // Clamp match_index
        if self.history_search.match_index >= self.history_search.matches.len() {
            self.history_search.match_index = 0;
        }
    }

    fn handle_history_search_key(&mut self, key: KeyEvent) -> InputAction {
        match key.code {
            KeyCode::Esc => {
                // Cancel search, restore original input
                self.history_search.active = false;
                InputAction::None
            }
            KeyCode::Enter => {
                // Accept the matched entry and place it in the input
                if let Some(&hist_idx) = self
                    .history_search
                    .matches
                    .get(self.history_search.match_index)
                {
                    let entry = self.history[hist_idx].clone();
                    self.set_lines_from_string(&entry);
                    let last_row = self.lines.len() - 1;
                    self.cursor = CursorPos::new(last_row, self.lines[last_row].len());
                }
                self.history_search.active = false;
                InputAction::None
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Cycle to next match
                if !self.history_search.matches.is_empty() {
                    self.history_search.match_index =
                        (self.history_search.match_index + 1) % self.history_search.matches.len();
                }
                InputAction::None
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.history_search.active = false;
                InputAction::None
            }
            KeyCode::Backspace => {
                self.history_search.query.pop();
                self.history_search.match_index = 0;
                self.update_history_search_matches();
                InputAction::None
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.history_search.query.push(c);
                self.history_search.match_index = 0;
                self.update_history_search_matches();
                InputAction::None
            }
            _ => InputAction::None,
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

    /// Trigger tab completion or cycle through existing completions.
    /// Handle a key while the completion popup is visible.
    /// Returns `Some(action)` if consumed, `None` to pass through.
    fn handle_completion_key(&mut self, key: &KeyEvent) -> Option<InputAction> {
        match key.code {
            KeyCode::Tab => {
                if let Some(ref mut state) = self.completion {
                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                        state.prev();
                    } else {
                        state.next();
                    }
                }
                Some(InputAction::None)
            }
            KeyCode::Enter => {
                self.accept_completion();
                // If the accepted text is a complete slash command, submit it
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
            // Already showing completions — cycle
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
            // Slash command completion
            let query = before;
            SLASH_COMMANDS
                .iter()
                .filter(|(cmd, _)| cmd.starts_with(query))
                .map(|(cmd, desc)| CompletionItem {
                    label: cmd.to_string(),
                    insert: cmd.to_string(),
                    description: Some(desc.to_string()),
                })
                .collect::<Vec<_>>()
        } else {
            // File path completion: look for a path-like token
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

    // NOTE: handle_completion_key is defined above (near trigger_or_cycle_completion)

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
                    // Append '/' for directories
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
        // Limit to avoid huge popups
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
            // Single line paste
            let line = &mut self.lines[self.cursor.row];
            line.insert_str(self.cursor.col, paste_lines[0]);
            self.cursor.col += paste_lines[0].len();
        } else {
            // Multiline paste
            let rest = self.lines[self.cursor.row][self.cursor.col..].to_string();
            self.lines[self.cursor.row].truncate(self.cursor.col);
            self.lines[self.cursor.row].push_str(paste_lines[0]);

            for (i, paste_line) in paste_lines.iter().enumerate().skip(1) {
                if i == paste_lines.len() - 1 {
                    // Last line: append the rest of the original line
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

    /// Set the lines buffer from a single string (splitting on newlines).
    fn set_lines_from_string(&mut self, s: &str) {
        self.lines = s.split('\n').map(String::from).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
    }

    /// Convert a flat byte offset to (row, col).
    fn flat_to_pos(&self, flat: usize) -> (usize, usize) {
        let mut remaining = flat;
        for (i, line) in self.lines.iter().enumerate() {
            if remaining <= line.len() || i == self.lines.len() - 1 {
                return (i, remaining.min(line.len()));
            }
            remaining -= line.len() + 1; // +1 for the '\n'
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
// Widget: PromptInputWidget — renders the multiline input area
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
                Span::styled(
                    &prompt_text,
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(matched, Style::default().fg(Color::White)),
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
            // Render at right side of last line, or on the border line
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

        // We show as many lines as fit in the area, scrolled so the cursor is visible
        let max_visible_lines = inner.height as usize;
        let total_lines = self.input.lines.len();
        let cursor_row = self.input.cursor.row;

        // Compute scroll offset so cursor row is visible
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

            // Only the first visible line gets the "> " prompt
            let prefix = if line_idx == 0 && scroll_offset == 0 {
                prompt_str
            } else if line_idx == 0 {
                "> " // still show prompt for the first line even if scrolled
            } else {
                "  " // continuation indent
            };

            let spans = vec![
                Span::styled(prefix, Style::default().fg(Color::Cyan)),
                Span::raw(line_text.as_str()),
            ];
            let line = Line::from(spans);
            buf.set_line(inner.x, y, &line, inner.width);
        }

        // NOTE: Completion popup is rendered in app.rs above the input area
        // so it doesn't get clipped by the small input widget bounds.
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
        // Type something, then press Enter to submit (single line)
        p.handle_key(key(KeyCode::Char('a')));
        // To get multiline, we first need at least 2 lines.
        // Let's use insert_newline directly.
        p.insert_newline();
        p.handle_key(key(KeyCode::Char('b')));
        assert_eq!(p.text(), "a\nb");
        assert_eq!(p.line_count(), 2);

        // Ctrl+Enter submits multiline
        let result = p.handle_key(key_mod(KeyCode::Enter, KeyModifiers::CONTROL));
        assert!(matches!(result, InputAction::Submit(ref s) if s == "a\nb"));
    }

    #[test]
    fn test_cursor_flat_offset() {
        let mut p = PromptInput::new();
        p.lines = vec!["hello".to_string(), "world".to_string()];
        p.cursor = CursorPos::new(1, 3); // "wor|ld"
        // Flat offset: "hello\n" = 6, + 3 = 9
        assert_eq!(p.cursor(), 9);
    }

    #[test]
    fn test_set_cursor_flat() {
        let mut p = PromptInput::new();
        p.lines = vec!["abc".to_string(), "de".to_string(), "f".to_string()];
        p.set_cursor(5); // "abc\nd|e" -> row=1, col=1
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

        // Type "first" to filter
        for c in "first".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.history_search.matches.len(), 2);
        // First match should be most recent "first again"
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
        // Now trigger completion
        p.trigger_or_cycle_completion(false);
        assert!(p.completion.is_some());
        let comp = p.completion.as_ref().unwrap();
        assert!(comp.items.iter().any(|i| i.label == "/model"));
    }
}
