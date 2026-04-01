//! Input handling subsystem.
//!
//! Provides an [`InputHandler`] that bridges crossterm key events with the
//! vim state machine and the prompt input widget. The handler routes keys
//! differently depending on the current [`InputMode`] and whether vim mode
//! is enabled globally.

pub mod vim;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

use crate::widgets::prompt_input::PromptInput;
use vim::{
    find_char, process_normal_key, resolve_motion, NormalAction, VimMode, VimState,
};

/// High-level input mode for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputMode {
    /// Normal text entry (emacs-style keybindings, or vim Insert mode).
    Insert,
    /// Vim Normal mode — navigation and commands.
    Normal,
    /// Vim Visual mode — character selection.
    Visual,
    /// Vim Command mode — ex-command line (`:` prefix).
    Command,
}

impl std::fmt::Display for InputMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Insert => write!(f, "INSERT"),
            Self::Normal => write!(f, "NORMAL"),
            Self::Visual => write!(f, "VISUAL"),
            Self::Command => write!(f, "COMMAND"),
        }
    }
}

/// Result of handling a key event through the input handler.
#[derive(Debug)]
pub enum InputResult {
    /// No action needed by the caller.
    Consumed,
    /// The user submitted text (pressed Enter in insert mode).
    Submit(String),
    /// A vim ex-command was submitted (e.g. `:w`, `:q`).
    ExCommand(String),
    /// The key was not consumed — pass it through to the outer handler.
    NotConsumed,
}

/// Manages input routing between vim mode and standard emacs-style editing.
///
/// When vim mode is disabled, all keys go directly to the [`PromptInput`] widget.
/// When vim mode is enabled, keys are routed through the vim state machine first,
/// and the resulting actions are applied to the prompt.
pub struct InputHandler {
    /// Whether vim mode is active.
    vim_enabled: bool,
    /// Vim state machine (always present, but only used when vim_enabled).
    vim_state: VimState,
}

impl InputHandler {
    /// Create a new input handler.
    pub fn new() -> Self {
        Self {
            vim_enabled: false,
            vim_state: VimState::new(),
        }
    }

    /// Whether vim mode is currently enabled.
    pub fn vim_enabled(&self) -> bool {
        self.vim_enabled
    }

    /// Toggle vim mode on/off. When enabling, starts in Insert mode.
    pub fn set_vim_enabled(&mut self, enabled: bool) {
        self.vim_enabled = enabled;
        if enabled {
            self.vim_state = VimState::new(); // starts in Insert
        }
    }

    /// The current input mode (Insert if vim is disabled).
    pub fn mode(&self) -> InputMode {
        if !self.vim_enabled {
            return InputMode::Insert;
        }
        match self.vim_state.mode {
            VimMode::Insert => InputMode::Insert,
            VimMode::Normal => InputMode::Normal,
            VimMode::Visual => InputMode::Visual,
            VimMode::Command => InputMode::Command,
        }
    }

    /// Access the vim state (for status bar display, etc.).
    pub fn vim_state(&self) -> &VimState {
        &self.vim_state
    }

    /// Handle a key event, applying it to the given prompt.
    ///
    /// Returns an [`InputResult`] indicating what the caller should do.
    pub fn handle_key(&mut self, key: KeyEvent, prompt: &mut PromptInput) -> InputResult {
        if !self.vim_enabled {
            return self.handle_emacs_key(key, prompt);
        }

        match self.vim_state.mode {
            VimMode::Insert => self.handle_vim_insert(key, prompt),
            VimMode::Normal => self.handle_vim_normal(key, prompt),
            VimMode::Visual => self.handle_vim_visual(key, prompt),
            VimMode::Command => self.handle_vim_command(key, prompt),
        }
    }

    /// Standard emacs-style key handling — delegates directly to PromptInput.
    fn handle_emacs_key(&mut self, key: KeyEvent, prompt: &mut PromptInput) -> InputResult {
        use crate::widgets::prompt_input::InputAction;
        match prompt.handle_key(key) {
            InputAction::Submit(text) => InputResult::Submit(text),
            InputAction::None => InputResult::Consumed,
        }
    }

    /// Handle keys in vim Insert mode.
    ///
    /// Escape transitions to Normal mode. All other keys are handled by the
    /// standard prompt input, but we also record inserted text for dot-repeat.
    fn handle_vim_insert(&mut self, key: KeyEvent, prompt: &mut PromptInput) -> InputResult {
        // Escape -> Normal mode
        if key.code == KeyCode::Esc {
            self.vim_state.enter_normal();
            // In vim, cursor backs up one on Esc from insert
            let text = prompt.text().to_string();
            if prompt.cursor() > 0 {
                let new_cursor = vim::prev_char_boundary(&text, prompt.cursor());
                prompt.set_cursor(new_cursor);
            }
            return InputResult::Consumed;
        }

        // Record inserted characters for dot-repeat
        if let KeyCode::Char(c) = key.code {
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                self.vim_state.inserted_text.push(c);
            }
        }

        // Enter submits
        if key.code == KeyCode::Enter && !prompt.is_empty() {
            let text = prompt.submit();
            self.vim_state.enter_insert();
            return InputResult::Submit(text);
        }

        // Delegate to prompt for actual text manipulation
        use crate::widgets::prompt_input::InputAction;
        match prompt.handle_key(key) {
            InputAction::Submit(text) => InputResult::Submit(text),
            InputAction::None => InputResult::Consumed,
        }
    }

    /// Handle keys in vim Normal mode.
    fn handle_vim_normal(&mut self, key: KeyEvent, prompt: &mut PromptInput) -> InputResult {
        // Only handle plain chars and a few specials in normal mode
        let ch = match key.code {
            KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => c,
            KeyCode::Esc => {
                self.vim_state.reset_command();
                return InputResult::Consumed;
            }
            _ => return InputResult::NotConsumed,
        };

        let text = prompt.text().to_string();
        let cursor = prompt.cursor();
        let action = process_normal_key(ch, &text, cursor, &mut self.vim_state);

        self.apply_normal_action(action, prompt)
    }

    /// Handle keys in vim Visual mode.
    ///
    /// Visual mode supports motions to extend selection, plus operators (d/c/y)
    /// to act on the selection, and Escape to cancel.
    fn handle_vim_visual(&mut self, key: KeyEvent, prompt: &mut PromptInput) -> InputResult {
        let ch = match key.code {
            KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => c,
            KeyCode::Esc => {
                self.vim_state.enter_normal();
                return InputResult::Consumed;
            }
            _ => return InputResult::NotConsumed,
        };

        let text = prompt.text().to_string();
        let cursor = prompt.cursor();
        let anchor = self.vim_state.visual_anchor.unwrap_or(cursor);

        // Motions move the cursor end of selection
        if vim::is_simple_motion_pub(ch) {
            let target = resolve_motion(ch, &text, cursor, 1);
            prompt.set_cursor(target);
            return InputResult::Consumed;
        }

        // Operators act on the visual selection
        match ch {
            'd' | 'x' => {
                let (from, to) = visual_range(anchor, cursor);
                let to = vim::next_char_boundary(&text, to);
                self.vim_state.persistent.register = text[from..to].to_string();
                self.vim_state.persistent.register_is_linewise = false;
                prompt.delete_range(from, to);
                self.vim_state.enter_normal();
                InputResult::Consumed
            }
            'c' => {
                let (from, to) = visual_range(anchor, cursor);
                let to = vim::next_char_boundary(&text, to);
                self.vim_state.persistent.register = text[from..to].to_string();
                self.vim_state.persistent.register_is_linewise = false;
                prompt.delete_range(from, to);
                self.vim_state.enter_insert();
                InputResult::Consumed
            }
            'y' => {
                let (from, to) = visual_range(anchor, cursor);
                let to = vim::next_char_boundary(&text, to);
                self.vim_state.persistent.register = text[from..to].to_string();
                self.vim_state.persistent.register_is_linewise = false;
                self.vim_state.enter_normal();
                prompt.set_cursor(from);
                InputResult::Consumed
            }
            _ => InputResult::Consumed,
        }
    }

    /// Handle keys in vim Command mode (`:` ex-command line).
    fn handle_vim_command(&mut self, key: KeyEvent, _prompt: &mut PromptInput) -> InputResult {
        match key.code {
            KeyCode::Esc => {
                self.vim_state.enter_normal();
                InputResult::Consumed
            }
            KeyCode::Enter => {
                let cmd = self.vim_state.command_line.clone();
                self.vim_state.enter_normal();
                if cmd.is_empty() {
                    InputResult::Consumed
                } else {
                    InputResult::ExCommand(cmd)
                }
            }
            KeyCode::Backspace => {
                if self.vim_state.command_line.is_empty() {
                    self.vim_state.enter_normal();
                } else {
                    self.vim_state.command_line.pop();
                }
                InputResult::Consumed
            }
            KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
                self.vim_state.command_line.push(c);
                InputResult::Consumed
            }
            _ => InputResult::Consumed,
        }
    }

    /// Apply a NormalAction from the vim state machine to the prompt.
    fn apply_normal_action(
        &mut self,
        action: NormalAction,
        prompt: &mut PromptInput,
    ) -> InputResult {
        match action {
            NormalAction::None => InputResult::Consumed,
            NormalAction::MoveCursor(pos) => {
                prompt.set_cursor(pos);
                InputResult::Consumed
            }
            NormalAction::EnterInsert(pos) => {
                prompt.set_cursor(pos);
                self.vim_state.enter_insert();
                InputResult::Consumed
            }
            NormalAction::EnterVisual(anchor) => {
                self.vim_state.enter_visual(anchor);
                InputResult::Consumed
            }
            NormalAction::EnterCommand => {
                self.vim_state.enter_command();
                InputResult::Consumed
            }
            NormalAction::Delete { from, to, linewise } => {
                let text = prompt.text().to_string();
                let deleted = text.get(from..to).unwrap_or("").to_string();
                self.vim_state.persistent.register = deleted;
                self.vim_state.persistent.register_is_linewise = linewise;
                prompt.delete_range(from, to);
                InputResult::Consumed
            }
            NormalAction::Change { from, to } => {
                let text = prompt.text().to_string();
                let deleted = text.get(from..to).unwrap_or("").to_string();
                self.vim_state.persistent.register = deleted;
                self.vim_state.persistent.register_is_linewise = false;
                prompt.delete_range(from, to);
                self.vim_state.enter_insert();
                InputResult::Consumed
            }
            NormalAction::Yank { from, to, linewise } => {
                let text = prompt.text().to_string();
                let yanked = text.get(from..to).unwrap_or("").to_string();
                self.vim_state.persistent.register = yanked;
                self.vim_state.persistent.register_is_linewise = linewise;
                InputResult::Consumed
            }
            NormalAction::ReplaceChar { offset, ch } => {
                prompt.replace_char(offset, ch);
                InputResult::Consumed
            }
            NormalAction::Paste { after } => {
                let reg = self.vim_state.persistent.register.clone();
                if !reg.is_empty() {
                    let cursor = prompt.cursor();
                    let text = prompt.text().to_string();
                    let pos = if after {
                        if cursor < text.len() {
                            vim::next_char_boundary(&text, cursor)
                        } else {
                            cursor
                        }
                    } else {
                        cursor
                    };
                    prompt.insert_str_at(pos, &reg);
                    // Position cursor at end of pasted text - 1
                    let end = pos + reg.len();
                    let final_text = prompt.text().to_string();
                    let final_pos = if end > 0 {
                        vim::prev_char_boundary(&final_text, end)
                    } else {
                        0
                    };
                    prompt.set_cursor(final_pos);
                }
                InputResult::Consumed
            }
            NormalAction::Undo => {
                prompt.undo();
                InputResult::Consumed
            }
            NormalAction::DotRepeat => {
                // Replay last recorded change
                if let Some(change) = self.vim_state.persistent.last_change.clone() {
                    self.replay_change(&change, prompt);
                }
                InputResult::Consumed
            }
            NormalAction::ExCommand(cmd) => InputResult::ExCommand(cmd),
        }
    }

    /// Replay a recorded change for dot-repeat.
    fn replay_change(
        &mut self,
        change: &vim::RecordedChange,
        prompt: &mut PromptInput,
    ) {
        let text = prompt.text().to_string();
        let cursor = prompt.cursor();

        match change {
            vim::RecordedChange::Insert { text: insert_text } => {
                prompt.insert_str_at(cursor, insert_text);
                prompt.set_cursor(cursor + insert_text.len());
            }
            vim::RecordedChange::OperatorMotion { op, motion, count } => {
                let motion_ch = motion.chars().next().unwrap_or('l');
                let target = resolve_motion(motion_ch, &text, cursor, *count);
                if target != cursor {
                    let (from, to) = if target < cursor {
                        (target, cursor)
                    } else if vim::is_inclusive_motion(motion_ch) {
                        (cursor, vim::next_char_boundary(&text, target))
                    } else {
                        (cursor, target)
                    };
                    match op {
                        vim::Operator::Delete => {
                            self.vim_state.persistent.register = text[from..to].to_string();
                            prompt.delete_range(from, to);
                        }
                        vim::Operator::Change => {
                            self.vim_state.persistent.register = text[from..to].to_string();
                            prompt.delete_range(from, to);
                            self.vim_state.enter_insert();
                        }
                        vim::Operator::Yank => {
                            self.vim_state.persistent.register = text[from..to].to_string();
                        }
                    }
                }
            }
            vim::RecordedChange::OperatorFind { op, find, ch, count } => {
                if let Some(target) = find_char(&text, cursor, *ch, *find, *count) {
                    let (from, to) = if target < cursor {
                        (target, cursor)
                    } else {
                        (cursor, vim::next_char_boundary(&text, target))
                    };
                    match op {
                        vim::Operator::Delete => {
                            self.vim_state.persistent.register = text[from..to].to_string();
                            prompt.delete_range(from, to);
                        }
                        vim::Operator::Change => {
                            self.vim_state.persistent.register = text[from..to].to_string();
                            prompt.delete_range(from, to);
                            self.vim_state.enter_insert();
                        }
                        vim::Operator::Yank => {
                            self.vim_state.persistent.register = text[from..to].to_string();
                        }
                    }
                }
            }
            vim::RecordedChange::OperatorTextObj { op, obj_type, scope, count: _ } => {
                let inner = *scope == vim::TextObjScope::Inner;
                let obj_ch = obj_type.chars().next().unwrap_or('w');
                if let Some((from, to)) = vim::find_text_object(&text, cursor, obj_ch, inner) {
                    match op {
                        vim::Operator::Delete => {
                            self.vim_state.persistent.register = text[from..to].to_string();
                            prompt.delete_range(from, to);
                        }
                        vim::Operator::Change => {
                            self.vim_state.persistent.register = text[from..to].to_string();
                            prompt.delete_range(from, to);
                            self.vim_state.enter_insert();
                        }
                        vim::Operator::Yank => {
                            self.vim_state.persistent.register = text[from..to].to_string();
                        }
                    }
                }
            }
            vim::RecordedChange::ReplaceChar { ch, count: _ } => {
                if cursor < text.len() {
                    prompt.replace_char(cursor, *ch);
                }
            }
            vim::RecordedChange::DeleteChar { count } => {
                let end = resolve_motion('l', &text, cursor, *count);
                if end > cursor {
                    self.vim_state.persistent.register = text[cursor..end].to_string();
                    prompt.delete_range(cursor, end);
                }
            }
            vim::RecordedChange::ToggleCase { .. }
            | vim::RecordedChange::Join { .. }
            | vim::RecordedChange::Indent { .. }
            | vim::RecordedChange::OpenLine { .. } => {
                // Single-line prompt — these are no-ops
            }
        }
    }
}

impl Default for InputHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the ordered (from, to) range for a visual selection.
fn visual_range(anchor: usize, cursor: usize) -> (usize, usize) {
    if anchor <= cursor {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn test_emacs_mode_submit() {
        let mut handler = InputHandler::new();
        let mut prompt = PromptInput::new();
        handler.handle_key(key(KeyCode::Char('h')), &mut prompt);
        handler.handle_key(key(KeyCode::Char('i')), &mut prompt);
        let result = handler.handle_key(key(KeyCode::Enter), &mut prompt);
        assert!(matches!(result, InputResult::Submit(ref s) if s == "hi"));
    }

    #[test]
    fn test_vim_mode_escape_to_normal() {
        let mut handler = InputHandler::new();
        handler.set_vim_enabled(true);
        let mut prompt = PromptInput::new();

        assert_eq!(handler.mode(), InputMode::Insert);

        handler.handle_key(key(KeyCode::Char('a')), &mut prompt);
        handler.handle_key(key(KeyCode::Char('b')), &mut prompt);
        assert_eq!(prompt.text(), "ab");

        handler.handle_key(key(KeyCode::Esc), &mut prompt);
        assert_eq!(handler.mode(), InputMode::Normal);
    }

    #[test]
    fn test_vim_normal_motion() {
        let mut handler = InputHandler::new();
        handler.set_vim_enabled(true);
        let mut prompt = PromptInput::new();

        for c in "hello".chars() {
            handler.handle_key(key(KeyCode::Char(c)), &mut prompt);
        }
        handler.handle_key(key(KeyCode::Esc), &mut prompt);
        assert_eq!(prompt.cursor(), 4);

        handler.handle_key(key(KeyCode::Char('0')), &mut prompt);
        assert_eq!(prompt.cursor(), 0);

        handler.handle_key(key(KeyCode::Char('l')), &mut prompt);
        assert_eq!(prompt.cursor(), 1);
    }

    #[test]
    fn test_vim_delete_word() {
        let mut handler = InputHandler::new();
        handler.set_vim_enabled(true);
        let mut prompt = PromptInput::new();

        for c in "hello world".chars() {
            handler.handle_key(key(KeyCode::Char(c)), &mut prompt);
        }
        handler.handle_key(key(KeyCode::Esc), &mut prompt);

        handler.handle_key(key(KeyCode::Char('0')), &mut prompt);
        handler.handle_key(key(KeyCode::Char('d')), &mut prompt);
        handler.handle_key(key(KeyCode::Char('w')), &mut prompt);
        assert_eq!(prompt.text(), "world");
    }

    #[test]
    fn test_vim_command_mode() {
        let mut handler = InputHandler::new();
        handler.set_vim_enabled(true);
        let mut prompt = PromptInput::new();

        handler.handle_key(key(KeyCode::Esc), &mut prompt);
        handler.handle_key(key(KeyCode::Char(':')), &mut prompt);
        assert_eq!(handler.mode(), InputMode::Command);

        handler.handle_key(key(KeyCode::Char('q')), &mut prompt);
        let result = handler.handle_key(key(KeyCode::Enter), &mut prompt);
        assert!(matches!(result, InputResult::ExCommand(ref s) if s == "q"));
        assert_eq!(handler.mode(), InputMode::Normal);
    }

    #[test]
    fn test_input_mode_display() {
        assert_eq!(InputMode::Insert.to_string(), "INSERT");
        assert_eq!(InputMode::Normal.to_string(), "NORMAL");
        assert_eq!(InputMode::Visual.to_string(), "VISUAL");
        assert_eq!(InputMode::Command.to_string(), "COMMAND");
    }
}
