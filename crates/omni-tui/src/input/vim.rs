//! Vim mode state machine and input handler.
//!
//! Implements a vim-like editing model with Normal, Insert, Visual, and Command
//! modes. The state machine follows the same architecture as the original
//! TypeScript implementation: a `CommandState` enum drives Normal mode parsing,
//! with typed transitions for operators, counts, finds, and text objects.

use serde::{Deserialize, Serialize};

/// Top-level vim modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VimMode {
    /// Standard text entry mode.
    Insert,
    /// Navigation and command mode.
    Normal,
    /// Character/line selection mode.
    Visual,
    /// Ex-command line (`:` prefix).
    Command,
}

impl std::fmt::Display for VimMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Insert => write!(f, "INSERT"),
            Self::Normal => write!(f, "NORMAL"),
            Self::Visual => write!(f, "VISUAL"),
            Self::Command => write!(f, "COMMAND"),
        }
    }
}

/// Vim operator (verb).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

/// Find direction for f/F/t/T motions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindType {
    /// `f` — find forward, land on character.
    FindForward,
    /// `F` — find backward, land on character.
    FindBackward,
    /// `t` — find forward, land before character.
    TilForward,
    /// `T` — find backward, land after character.
    TilBackward,
}

/// Text object scope: inner vs. around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObjScope {
    Inner,
    Around,
}

/// Normal mode command parsing state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandState {
    /// Waiting for first input.
    Idle,
    /// Accumulating a repeat count (e.g. `3`).
    Count { digits: String },
    /// Operator entered, awaiting motion/text-object (e.g. `d`).
    Operator { op: Operator, count: u32 },
    /// Operator + count digits (e.g. `d3`).
    OperatorCount {
        op: Operator,
        count: u32,
        digits: String,
    },
    /// Operator awaiting find character (e.g. `df`).
    OperatorFind {
        op: Operator,
        count: u32,
        find: FindType,
    },
    /// Operator awaiting text object type (e.g. `di`).
    OperatorTextObj {
        op: Operator,
        count: u32,
        scope: TextObjScope,
    },
    /// Standalone find awaiting character (e.g. `f`).
    Find { find: FindType, count: u32 },
    /// After `g`, awaiting second key (`g`, `j`, `k`).
    G { count: u32 },
    /// Operator then `g`, awaiting `g`/`j`/`k`.
    OperatorG { op: Operator, count: u32 },
    /// Replace mode, awaiting character (`r`).
    Replace { count: u32 },
    /// Indent, awaiting repeated direction (`>`/`<`).
    Indent { dir: IndentDir, count: u32 },
}

/// Indent direction for `>>` / `<<`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentDir {
    Right,
    Left,
}

/// Recorded change for dot-repeat.
#[derive(Debug, Clone)]
pub enum RecordedChange {
    Insert { text: String },
    OperatorMotion { op: Operator, motion: String, count: u32 },
    OperatorTextObj { op: Operator, obj_type: String, scope: TextObjScope, count: u32 },
    OperatorFind { op: Operator, find: FindType, ch: char, count: u32 },
    ReplaceChar { ch: char, count: u32 },
    DeleteChar { count: u32 },
    ToggleCase { count: u32 },
    Indent { dir: IndentDir, count: u32 },
    OpenLine { below: bool },
    Join { count: u32 },
}

/// Persistent state across commands (register, last find, dot-repeat).
#[derive(Debug, Clone, Default)]
pub struct PersistentState {
    /// Last recorded change for `.` repeat.
    pub last_change: Option<RecordedChange>,
    /// Last `f`/`F`/`t`/`T` find for `;`/`,` repeat.
    pub last_find: Option<(FindType, char)>,
    /// Yank/delete register content.
    pub register: String,
    /// Whether the register content is linewise (for `p`/`P`).
    pub register_is_linewise: bool,
}

/// Complete vim state.
#[derive(Debug, Clone)]
pub struct VimState {
    /// Current mode.
    pub mode: VimMode,
    /// Normal-mode command state machine.
    pub command: CommandState,
    /// Text inserted during current Insert session (for dot-repeat).
    pub inserted_text: String,
    /// Visual mode anchor (byte offset).
    pub visual_anchor: Option<usize>,
    /// Command-line buffer (for `:` commands).
    pub command_line: String,
    /// Persistent state (survives mode changes).
    pub persistent: PersistentState,
}

impl VimState {
    /// Create a new vim state starting in Insert mode (Claude Code default).
    pub fn new() -> Self {
        Self {
            mode: VimMode::Insert,
            command: CommandState::Idle,
            inserted_text: String::new(),
            visual_anchor: None,
            command_line: String::new(),
            persistent: PersistentState::default(),
        }
    }

    /// Transition to Normal mode.
    pub fn enter_normal(&mut self) {
        if self.mode == VimMode::Insert && !self.inserted_text.is_empty() {
            self.persistent.last_change = Some(RecordedChange::Insert {
                text: self.inserted_text.clone(),
            });
        }
        self.mode = VimMode::Normal;
        self.command = CommandState::Idle;
        self.inserted_text.clear();
        self.visual_anchor = None;
        self.command_line.clear();
    }

    /// Transition to Insert mode.
    pub fn enter_insert(&mut self) {
        self.mode = VimMode::Insert;
        self.command = CommandState::Idle;
        self.inserted_text.clear();
    }

    /// Transition to Visual mode, anchoring at the current cursor position.
    pub fn enter_visual(&mut self, anchor: usize) {
        self.mode = VimMode::Visual;
        self.visual_anchor = Some(anchor);
        self.command = CommandState::Idle;
    }

    /// Transition to Command mode.
    pub fn enter_command(&mut self) {
        self.mode = VimMode::Command;
        self.command_line.clear();
    }

    /// Reset command state to idle (e.g. after an unrecognized key).
    pub fn reset_command(&mut self) {
        self.command = CommandState::Idle;
    }
}

impl Default for VimState {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of processing a key in Normal mode.
#[derive(Debug)]
pub enum NormalAction {
    /// No-op (key consumed, state updated).
    None,
    /// Move cursor to the given byte offset.
    MoveCursor(usize),
    /// Enter insert mode at the given byte offset.
    EnterInsert(usize),
    /// Enter visual mode at the given anchor.
    EnterVisual(usize),
    /// Enter command mode.
    EnterCommand,
    /// Delete text in range [from, to), place in register.
    Delete { from: usize, to: usize, linewise: bool },
    /// Change text in range [from, to), enter insert at `from`.
    Change { from: usize, to: usize },
    /// Yank text in range [from, to).
    Yank { from: usize, to: usize, linewise: bool },
    /// Open a new line above or below the current line and enter insert mode.
    OpenLine { below: bool },
    /// Replace character at offset.
    ReplaceChar { offset: usize, ch: char },
    /// Paste register content.
    Paste { after: bool },
    /// Undo.
    Undo,
    /// Repeat last change.
    DotRepeat,
    /// Execute a command-line string (from `:` mode).
    ExCommand(String),
}

/// Maximum count value to prevent runaway multipliers.
const MAX_VIM_COUNT: u32 = 10_000;

/// Check if a key is a simple motion (public for use in Visual mode handling).
pub fn is_simple_motion_pub(key: char) -> bool {
    is_simple_motion(key)
}

/// Set of keys that are simple motions (resolved by cursor position).
fn is_simple_motion(key: char) -> bool {
    matches!(
        key,
        'h' | 'l' | 'j' | 'k' | 'w' | 'b' | 'e' | 'W' | 'B' | 'E' | '0' | '^' | '$'
    )
}

/// Check if a key starts a find motion.
fn is_find_key(key: char) -> Option<FindType> {
    match key {
        'f' => Some(FindType::FindForward),
        'F' => Some(FindType::FindBackward),
        't' => Some(FindType::TilForward),
        'T' => Some(FindType::TilBackward),
        _ => None,
    }
}

/// Check if a key is an operator.
fn is_operator_key(key: char) -> Option<Operator> {
    match key {
        'd' => Some(Operator::Delete),
        'c' => Some(Operator::Change),
        'y' => Some(Operator::Yank),
        _ => None,
    }
}

/// Check if a key is a text-object scope prefix.
fn is_text_obj_scope(key: char) -> Option<TextObjScope> {
    match key {
        'i' => Some(TextObjScope::Inner),
        'a' => Some(TextObjScope::Around),
        _ => None,
    }
}

/// Valid text object type characters.
fn is_text_obj_type(key: char) -> bool {
    matches!(
        key,
        'w' | 'W' | '"' | '\'' | '`' | '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<'
            | '>'
    )
}

/// Resolve a simple motion to a target byte offset in the text.
///
/// This is a pure function — it reads the text and cursor position,
/// returning where the cursor should move to.
pub fn resolve_motion(key: char, text: &str, cursor: usize, count: u32) -> usize {
    let mut pos = cursor;
    for _ in 0..count {
        let next = apply_single_motion(key, text, pos);
        if next == pos {
            break;
        }
        pos = next;
    }
    pos
}

fn apply_single_motion(key: char, text: &str, cursor: usize) -> usize {
    match key {
        'h' => prev_char_boundary(text, cursor),
        'l' => next_char_boundary(text, cursor),
        'j' => move_down_line(text, cursor),
        'k' => move_up_line(text, cursor),
        'w' => next_word_start(text, cursor),
        'b' => prev_word_start(text, cursor),
        'e' => end_of_word(text, cursor),
        'W' => next_word_start_big(text, cursor),
        'B' => prev_word_start_big(text, cursor),
        'E' => end_of_word_big(text, cursor),
        '0' => start_of_line(text, cursor),
        '^' => first_non_blank(text, cursor),
        '$' => end_of_line(text, cursor),
        _ => cursor,
    }
}

/// Whether a motion is inclusive (includes destination character for operators).
pub fn is_inclusive_motion(key: char) -> bool {
    matches!(key, 'e' | 'E' | '$')
}

/// Whether a motion is linewise (operates on full lines with operators).
pub fn is_linewise_motion(key: char) -> bool {
    matches!(key, 'j' | 'k' | 'G')
}

// ---------------------------------------------------------------------------
// Cursor movement helpers
// ---------------------------------------------------------------------------

/// Move to the previous character boundary.
pub fn prev_char_boundary(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut p = pos - 1;
    while p > 0 && !text.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Move to the next character boundary.
pub fn next_char_boundary(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    let mut p = pos + 1;
    while p < text.len() && !text.is_char_boundary(p) {
        p += 1;
    }
    p.min(text.len())
}

fn start_of_line(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

fn end_of_line(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map_or(text.len(), |i| pos + i)
}

/// Public accessor for `start_of_line` (used by `InputHandler` for `o`/`O`).
pub fn start_of_line_offset(text: &str, pos: usize) -> usize {
    start_of_line(text, pos)
}

/// Public accessor for `end_of_line` (used by `InputHandler` for `o`/`O`).
pub fn end_of_line_offset(text: &str, pos: usize) -> usize {
    end_of_line(text, pos)
}

fn first_non_blank(text: &str, pos: usize) -> usize {
    let sol = start_of_line(text, pos);
    let eol = end_of_line(text, pos);
    let line = &text[sol..eol];
    sol + line.len() - line.trim_start().len()
}

fn move_down_line(text: &str, pos: usize) -> usize {
    let sol = start_of_line(text, pos);
    let col = pos - sol;
    let eol = end_of_line(text, pos);
    if eol >= text.len() {
        return pos; // Already on last line
    }
    let next_sol = eol + 1;
    let next_eol = end_of_line(text, next_sol);
    let next_line_len = next_eol - next_sol;
    next_sol + col.min(next_line_len)
}

fn move_up_line(text: &str, pos: usize) -> usize {
    let sol = start_of_line(text, pos);
    if sol == 0 {
        return pos; // Already on first line
    }
    let col = pos - sol;
    let prev_eol = sol - 1; // The '\n' of the previous line
    let prev_sol = start_of_line(text, prev_eol);
    let prev_line_len = prev_eol - prev_sol;
    prev_sol + col.min(prev_line_len)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn next_word_start(text: &str, pos: usize) -> usize {
    let len = text.len();
    if pos >= len {
        return len;
    }
    let mut p = pos;

    // Skip current word/punctuation
    let first = text[p..].chars().next().unwrap_or(' ');
    if is_word_char(first) {
        while p < len && text[p..].chars().next().is_some_and(is_word_char) {
            p = next_char_boundary(text, p);
        }
    } else if !first.is_whitespace() {
        // Punctuation
        while p < len
            && text[p..]
                .chars()
                .next()
                .is_some_and(|c| !is_word_char(c) && !c.is_whitespace())
        {
            p = next_char_boundary(text, p);
        }
    }

    // Skip whitespace
    while p < len && text.as_bytes().get(p).is_some_and(|b| b.is_ascii_whitespace()) {
        p = next_char_boundary(text, p);
    }

    p.min(len)
}

fn prev_word_start(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut p = prev_char_boundary(text, pos);

    // Skip whitespace backward
    while p > 0 && text[p..].chars().next().is_some_and(|c| c.is_whitespace()) {
        p = prev_char_boundary(text, p);
    }

    if p == 0 {
        return 0;
    }

    // Find start of word/punctuation group
    let ch = text[p..].chars().next().unwrap_or(' ');
    if is_word_char(ch) {
        while p > 0 {
            let prev = prev_char_boundary(text, p);
            if text[prev..].chars().next().is_some_and(is_word_char) {
                p = prev;
            } else {
                break;
            }
        }
    } else {
        while p > 0 {
            let prev = prev_char_boundary(text, p);
            if text[prev..]
                .chars()
                .next()
                .is_some_and(|c| !is_word_char(c) && !c.is_whitespace())
            {
                p = prev;
            } else {
                break;
            }
        }
    }

    p
}

fn end_of_word(text: &str, pos: usize) -> usize {
    let len = text.len();
    if pos >= len {
        return len;
    }
    let mut p = next_char_boundary(text, pos);

    // Skip whitespace
    while p < len && text[p..].chars().next().is_some_and(|c| c.is_whitespace()) {
        p = next_char_boundary(text, p);
    }

    if p >= len {
        return len.saturating_sub(1).max(pos);
    }

    // Move to end of word/punctuation group
    let ch = text[p..].chars().next().unwrap_or(' ');
    if is_word_char(ch) {
        while p < len {
            let next = next_char_boundary(text, p);
            if next < len && text[next..].chars().next().is_some_and(is_word_char) {
                p = next;
            } else {
                break;
            }
        }
    } else if !ch.is_whitespace() {
        while p < len {
            let next = next_char_boundary(text, p);
            if next < len
                && text[next..]
                    .chars()
                    .next()
                    .is_some_and(|c| !is_word_char(c) && !c.is_whitespace())
            {
                p = next;
            } else {
                break;
            }
        }
    }

    p
}

fn next_word_start_big(text: &str, pos: usize) -> usize {
    let len = text.len();
    let mut p = pos;
    // Skip non-whitespace
    while p < len && !text[p..].starts_with(char::is_whitespace) {
        p = next_char_boundary(text, p);
    }
    // Skip whitespace
    while p < len && text[p..].starts_with(char::is_whitespace) {
        p = next_char_boundary(text, p);
    }
    p.min(len)
}

fn prev_word_start_big(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut p = prev_char_boundary(text, pos);
    // Skip whitespace backward
    while p > 0 && text[p..].chars().next().is_some_and(|c| c.is_whitespace()) {
        p = prev_char_boundary(text, p);
    }
    // Find start of WORD
    while p > 0 {
        let prev = prev_char_boundary(text, p);
        if text[prev..].chars().next().is_some_and(|c| !c.is_whitespace()) {
            p = prev;
        } else {
            break;
        }
    }
    p
}

fn end_of_word_big(text: &str, pos: usize) -> usize {
    let len = text.len();
    let mut p = next_char_boundary(text, pos);
    // Skip whitespace
    while p < len && text[p..].starts_with(char::is_whitespace) {
        p = next_char_boundary(text, p);
    }
    // Move to end of WORD
    while p < len {
        let next = next_char_boundary(text, p);
        if next < len && !text[next..].starts_with(char::is_whitespace) {
            p = next;
        } else {
            break;
        }
    }
    p.min(len.saturating_sub(1).max(pos))
}

// ---------------------------------------------------------------------------
// Find character
// ---------------------------------------------------------------------------

/// Find a character on the current line.
pub fn find_char(text: &str, cursor: usize, ch: char, find_type: FindType, count: u32) -> Option<usize> {
    let sol = start_of_line(text, cursor);
    let eol = end_of_line(text, cursor);
    let line = &text[sol..eol];

    let col = cursor - sol;
    let mut found = 0u32;

    match find_type {
        FindType::FindForward | FindType::TilForward => {
            for (i, c) in line[col + 1..].char_indices() {
                if c == ch {
                    found += 1;
                    if found == count {
                        let target = sol + col + 1 + i;
                        return Some(if matches!(find_type, FindType::TilForward) {
                            prev_char_boundary(text, target)
                        } else {
                            target
                        });
                    }
                }
            }
        }
        FindType::FindBackward | FindType::TilBackward => {
            for (i, c) in line[..col].char_indices().rev() {
                if c == ch {
                    found += 1;
                    if found == count {
                        let target = sol + i;
                        return Some(if matches!(find_type, FindType::TilBackward) {
                            next_char_boundary(text, target)
                        } else {
                            target
                        });
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Text objects
// ---------------------------------------------------------------------------

/// Find a text object range.
pub fn find_text_object(
    text: &str,
    offset: usize,
    obj_type: char,
    inner: bool,
) -> Option<(usize, usize)> {
    match obj_type {
        'w' => find_word_object(text, offset, inner, is_word_char),
        'W' => find_word_object(text, offset, inner, |c| !c.is_whitespace()),
        '"' | '\'' | '`' => find_quote_object(text, offset, obj_type, inner),
        '(' | ')' | 'b' => find_bracket_object(text, offset, '(', ')', inner),
        '[' | ']' => find_bracket_object(text, offset, '[', ']', inner),
        '{' | '}' | 'B' => find_bracket_object(text, offset, '{', '}', inner),
        '<' | '>' => find_bracket_object(text, offset, '<', '>', inner),
        _ => None,
    }
}

fn find_word_object(
    text: &str,
    offset: usize,
    inner: bool,
    classify: fn(char) -> bool,
) -> Option<(usize, usize)> {
    if offset >= text.len() {
        return None;
    }

    let ch = text[offset..].chars().next()?;
    let mut start = offset;
    let mut end = offset;

    if classify(ch) {
        // Expand word
        while start > 0 && text[..start].ends_with(|c: char| classify(c)) {
            start = prev_char_boundary(text, start);
        }
        if start > 0 && !classify(text[start..].chars().next().unwrap_or(' ')) {
            start = next_char_boundary(text, start);
        }
        let nc = next_char_boundary(text, end);
        end = nc;
        while end < text.len() && text[end..].chars().next().is_some_and(&classify) {
            end = next_char_boundary(text, end);
        }
    } else if ch.is_whitespace() {
        while start > 0 && text[..start].ends_with(char::is_whitespace) {
            start = prev_char_boundary(text, start);
        }
        if start > 0 && !text[start..].starts_with(char::is_whitespace) {
            start = next_char_boundary(text, start);
        }
        let nc = next_char_boundary(text, end);
        end = nc;
        while end < text.len() && text[end..].starts_with(char::is_whitespace) {
            end = next_char_boundary(text, end);
        }
        return Some((start, end));
    } else {
        // Punctuation
        while start > 0
            && text[..start].ends_with(|c: char| !classify(c) && !c.is_whitespace())
        {
            start = prev_char_boundary(text, start);
        }
        if start > 0 {
            let sc = text[start..].chars().next().unwrap_or(' ');
            if classify(sc) || sc.is_whitespace() {
                start = next_char_boundary(text, start);
            }
        }
        let nc = next_char_boundary(text, end);
        end = nc;
        while end < text.len()
            && text[end..]
                .chars()
                .next()
                .is_some_and(|c| !classify(c) && !c.is_whitespace())
        {
            end = next_char_boundary(text, end);
        }
    }

    if !inner {
        // Include surrounding whitespace
        if end < text.len() && text[end..].starts_with(char::is_whitespace) {
            while end < text.len() && text[end..].starts_with(char::is_whitespace) {
                end = next_char_boundary(text, end);
            }
        } else {
            while start > 0 && text[..start].ends_with(char::is_whitespace) {
                start = prev_char_boundary(text, start);
            }
            if start > 0 && !text[start..].starts_with(char::is_whitespace) {
                // Don't adjust — we went one too far
            } else if start == 0 && text[start..].starts_with(char::is_whitespace) {
                // Start is whitespace, keep it
            }
        }
    }

    Some((start, end))
}

fn find_quote_object(
    text: &str,
    offset: usize,
    quote: char,
    inner: bool,
) -> Option<(usize, usize)> {
    let sol = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let eol = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    let line = &text[sol..eol];
    let col = offset - sol;

    let positions: Vec<usize> = line
        .char_indices()
        .filter(|&(_, c)| c == quote)
        .map(|(i, _)| i)
        .collect();

    // Pair quotes: 0-1, 2-3, 4-5, etc.
    let mut i = 0;
    while i + 1 < positions.len() {
        let qs = positions[i];
        let qe = positions[i + 1];
        if qs <= col && col <= qe {
            return if inner {
                Some((sol + qs + 1, sol + qe))
            } else {
                Some((sol + qs, sol + qe + 1))
            };
        }
        i += 2;
    }
    None
}

fn find_bracket_object(
    text: &str,
    offset: usize,
    open: char,
    close: char,
    inner: bool,
) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let mut start = None;

    // Search backward for opening bracket
    for i in (0..=offset).rev() {
        if !text.is_char_boundary(i) {
            continue;
        }
        let ch = text[i..].chars().next()?;
        if ch == close && i != offset {
            depth += 1;
        } else if ch == open {
            if depth == 0 {
                start = Some(i);
                break;
            }
            depth -= 1;
        }
    }

    let start = start?;
    depth = 0;
    let mut end = None;

    for i in (start + 1)..text.len() {
        if !text.is_char_boundary(i) {
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or('\0');
        if ch == open {
            depth += 1;
        } else if ch == close {
            if depth == 0 {
                end = Some(i);
                break;
            }
            depth -= 1;
        }
    }

    let end = end?;
    if inner {
        Some((start + 1, end))
    } else {
        Some((start, end + 1))
    }
}

// ---------------------------------------------------------------------------
// Normal mode transition logic
// ---------------------------------------------------------------------------

/// Process a key press in Normal mode, returning the action to take.
///
/// This modifies the vim state's command state machine and returns an action
/// for the caller (PromptInput) to execute.
pub fn process_normal_key(
    key: char,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    let cmd = state.command.clone();
    match cmd {
        CommandState::Idle => from_idle(key, text, cursor, state),
        CommandState::Count { ref digits } => {
            let digits = digits.clone();
            from_count(key, &digits, text, cursor, state)
        }
        CommandState::Operator { op, count } => from_operator(key, op, count, text, cursor, state),
        CommandState::OperatorCount { op, count, ref digits } => {
            let digits = digits.clone();
            from_operator_count(key, op, count, &digits, text, cursor, state)
        }
        CommandState::OperatorFind { op, count, find } => {
            from_operator_find(key, op, count, find, text, cursor, state)
        }
        CommandState::OperatorTextObj { op, count, scope } => {
            from_operator_text_obj(key, op, count, scope, text, cursor, state)
        }
        CommandState::Find { find, count } => from_find(key, find, count, text, cursor, state),
        CommandState::G { count } => from_g(key, count, text, cursor, state),
        CommandState::OperatorG { op, count } => {
            from_operator_g(key, op, count, text, cursor, state)
        }
        CommandState::Replace { count } => from_replace(key, count, text, cursor, state),
        CommandState::Indent { dir, count } => from_indent(key, dir, count, state),
    }
}

fn handle_normal_input(
    key: char,
    count: u32,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> Option<NormalAction> {
    if let Some(op) = is_operator_key(key) {
        state.command = CommandState::Operator { op, count };
        return Some(NormalAction::None);
    }

    if is_simple_motion(key) {
        let target = resolve_motion(key, text, cursor, count);
        state.command = CommandState::Idle;
        return Some(NormalAction::MoveCursor(target));
    }

    if let Some(find) = is_find_key(key) {
        state.command = CommandState::Find { find, count };
        return Some(NormalAction::None);
    }

    if key == 'g' {
        state.command = CommandState::G { count };
        return Some(NormalAction::None);
    }
    if key == 'r' {
        state.command = CommandState::Replace { count };
        return Some(NormalAction::None);
    }
    if key == '>' || key == '<' {
        let dir = if key == '>' { IndentDir::Right } else { IndentDir::Left };
        state.command = CommandState::Indent { dir, count };
        return Some(NormalAction::None);
    }

    // Simple actions
    match key {
        'x' => {
            let end = resolve_motion('l', text, cursor, count);
            if end > cursor {
                state.persistent.last_change = Some(RecordedChange::DeleteChar { count });
                state.command = CommandState::Idle;
                return Some(NormalAction::Delete {
                    from: cursor,
                    to: end,
                    linewise: false,
                });
            }
            state.command = CommandState::Idle;
            Some(NormalAction::None)
        }
        'p' | 'P' => {
            state.command = CommandState::Idle;
            Some(NormalAction::Paste { after: key == 'p' })
        }
        'D' => {
            let eol = end_of_line(text, cursor);
            state.command = CommandState::Idle;
            if eol > cursor {
                Some(NormalAction::Delete {
                    from: cursor,
                    to: eol,
                    linewise: false,
                })
            } else {
                Some(NormalAction::None)
            }
        }
        'C' => {
            let eol = end_of_line(text, cursor);
            state.command = CommandState::Idle;
            Some(NormalAction::Change {
                from: cursor,
                to: eol,
            })
        }
        'G' => {
            let target = if count > 1 {
                go_to_line(text, count as usize)
            } else {
                start_of_last_line(text)
            };
            state.command = CommandState::Idle;
            Some(NormalAction::MoveCursor(target))
        }
        'i' => {
            state.command = CommandState::Idle;
            Some(NormalAction::EnterInsert(cursor))
        }
        'I' => {
            let target = first_non_blank(text, cursor);
            state.command = CommandState::Idle;
            Some(NormalAction::EnterInsert(target))
        }
        'a' => {
            let target = if cursor < text.len() {
                next_char_boundary(text, cursor)
            } else {
                cursor
            };
            state.command = CommandState::Idle;
            Some(NormalAction::EnterInsert(target))
        }
        'A' => {
            let target = end_of_line(text, cursor);
            state.command = CommandState::Idle;
            Some(NormalAction::EnterInsert(target))
        }
        'o' => {
            state.persistent.last_change = Some(RecordedChange::OpenLine { below: true });
            state.command = CommandState::Idle;
            Some(NormalAction::OpenLine { below: true })
        }
        'O' => {
            state.persistent.last_change = Some(RecordedChange::OpenLine { below: false });
            state.command = CommandState::Idle;
            Some(NormalAction::OpenLine { below: false })
        }
        'v' => {
            state.command = CommandState::Idle;
            Some(NormalAction::EnterVisual(cursor))
        }
        ':' => {
            state.command = CommandState::Idle;
            Some(NormalAction::EnterCommand)
        }
        'u' => {
            state.command = CommandState::Idle;
            Some(NormalAction::Undo)
        }
        '.' => {
            state.command = CommandState::Idle;
            Some(NormalAction::DotRepeat)
        }
        ';' | ',' => {
            if let Some((find_type, ch)) = state.persistent.last_find {
                let effective = if key == ',' {
                    match find_type {
                        FindType::FindForward => FindType::FindBackward,
                        FindType::FindBackward => FindType::FindForward,
                        FindType::TilForward => FindType::TilBackward,
                        FindType::TilBackward => FindType::TilForward,
                    }
                } else {
                    find_type
                };
                if let Some(target) = find_char(text, cursor, ch, effective, count) {
                    state.command = CommandState::Idle;
                    return Some(NormalAction::MoveCursor(target));
                }
            }
            state.command = CommandState::Idle;
            Some(NormalAction::None)
        }
        _ => None,
    }
}

fn from_idle(
    key: char,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    if key.is_ascii_digit() && key != '0' {
        state.command = CommandState::Count {
            digits: key.to_string(),
        };
        return NormalAction::None;
    }
    if key == '0' {
        let target = start_of_line(text, cursor);
        state.command = CommandState::Idle;
        return NormalAction::MoveCursor(target);
    }
    handle_normal_input(key, 1, text, cursor, state).unwrap_or_else(|| {
        state.command = CommandState::Idle;
        NormalAction::None
    })
}

fn from_count(
    key: char,
    digits: &str,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    if key.is_ascii_digit() {
        let new_digits = format!("{}{}", digits, key);
        let val = new_digits.parse::<u32>().unwrap_or(MAX_VIM_COUNT).min(MAX_VIM_COUNT);
        state.command = CommandState::Count {
            digits: val.to_string(),
        };
        return NormalAction::None;
    }
    let count = digits.parse::<u32>().unwrap_or(1);
    handle_normal_input(key, count, text, cursor, state).unwrap_or_else(|| {
        state.command = CommandState::Idle;
        NormalAction::None
    })
}

fn from_operator(
    key: char,
    op: Operator,
    count: u32,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    // dd, cc, yy — line operation
    let op_char = match op {
        Operator::Delete => 'd',
        Operator::Change => 'c',
        Operator::Yank => 'y',
    };
    if key == op_char {
        let sol = start_of_line(text, cursor);
        let mut eol = end_of_line(text, cursor);
        if eol < text.len() {
            eol += 1;
        }
        state.command = CommandState::Idle;
        return match op {
            Operator::Delete => NormalAction::Delete {
                from: sol,
                to: eol,
                linewise: true,
            },
            Operator::Change => NormalAction::Change { from: sol, to: eol },
            Operator::Yank => NormalAction::Yank {
                from: sol,
                to: eol,
                linewise: true,
            },
        };
    }

    if key.is_ascii_digit() {
        state.command = CommandState::OperatorCount {
            op,
            count,
            digits: key.to_string(),
        };
        return NormalAction::None;
    }

    if let Some(result) = handle_operator_input(key, op, count, text, cursor, state) {
        return result;
    }

    state.command = CommandState::Idle;
    NormalAction::None
}

fn from_operator_count(
    key: char,
    op: Operator,
    count: u32,
    digits: &str,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    if key.is_ascii_digit() {
        let new_digits = format!("{}{}", digits, key);
        let val = new_digits.parse::<u32>().unwrap_or(MAX_VIM_COUNT).min(MAX_VIM_COUNT);
        state.command = CommandState::OperatorCount {
            op,
            count,
            digits: val.to_string(),
        };
        return NormalAction::None;
    }
    let motion_count = digits.parse::<u32>().unwrap_or(1);
    let effective = count.saturating_mul(motion_count);
    handle_operator_input(key, op, effective, text, cursor, state).unwrap_or_else(|| {
        state.command = CommandState::Idle;
        NormalAction::None
    })
}

fn handle_operator_input(
    key: char,
    op: Operator,
    count: u32,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> Option<NormalAction> {
    if let Some(scope) = is_text_obj_scope(key) {
        state.command = CommandState::OperatorTextObj { op, count, scope };
        return Some(NormalAction::None);
    }

    if let Some(find) = is_find_key(key) {
        state.command = CommandState::OperatorFind { op, count, find };
        return Some(NormalAction::None);
    }

    if is_simple_motion(key) {
        let target = resolve_motion(key, text, cursor, count);
        if target == cursor {
            state.command = CommandState::Idle;
            return Some(NormalAction::None);
        }
        let (from, to) = if target < cursor {
            (target, cursor)
        } else if is_inclusive_motion(key) {
            (cursor, next_char_boundary(text, target))
        } else {
            (cursor, target)
        };
        state.persistent.last_change = Some(RecordedChange::OperatorMotion {
            op,
            motion: key.to_string(),
            count,
        });
        state.command = CommandState::Idle;
        return Some(match op {
            Operator::Delete => NormalAction::Delete {
                from,
                to,
                linewise: is_linewise_motion(key),
            },
            Operator::Change => NormalAction::Change { from, to },
            Operator::Yank => NormalAction::Yank {
                from,
                to,
                linewise: is_linewise_motion(key),
            },
        });
    }

    if key == 'G' {
        let target = if count > 1 {
            go_to_line(text, count as usize)
        } else {
            start_of_last_line(text)
        };
        let (from, to) = if target < cursor {
            (start_of_line(text, target), end_of_line(text, cursor))
        } else {
            (start_of_line(text, cursor), end_of_line(text, target))
        };
        let to = if to < text.len() { to + 1 } else { to };
        state.command = CommandState::Idle;
        return Some(match op {
            Operator::Delete => NormalAction::Delete {
                from,
                to,
                linewise: true,
            },
            Operator::Change => NormalAction::Change { from, to },
            Operator::Yank => NormalAction::Yank {
                from,
                to,
                linewise: true,
            },
        });
    }

    if key == 'g' {
        state.command = CommandState::OperatorG { op, count };
        return Some(NormalAction::None);
    }

    None
}

fn from_operator_find(
    key: char,
    op: Operator,
    count: u32,
    find: FindType,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    state.command = CommandState::Idle;
    if let Some(target) = find_char(text, cursor, key, find, count) {
        let (from, to) = if target < cursor {
            (target, cursor)
        } else {
            (cursor, next_char_boundary(text, target))
        };
        state.persistent.last_find = Some((find, key));
        state.persistent.last_change = Some(RecordedChange::OperatorFind {
            op,
            find,
            ch: key,
            count,
        });
        match op {
            Operator::Delete => NormalAction::Delete {
                from,
                to,
                linewise: false,
            },
            Operator::Change => NormalAction::Change { from, to },
            Operator::Yank => NormalAction::Yank {
                from,
                to,
                linewise: false,
            },
        }
    } else {
        NormalAction::None
    }
}

fn from_operator_text_obj(
    key: char,
    op: Operator,
    count: u32,
    scope: TextObjScope,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    state.command = CommandState::Idle;
    if !is_text_obj_type(key) {
        return NormalAction::None;
    }
    if let Some((from, to)) = find_text_object(text, cursor, key, scope == TextObjScope::Inner) {
        state.persistent.last_change = Some(RecordedChange::OperatorTextObj {
            op,
            obj_type: key.to_string(),
            scope,
            count,
        });
        match op {
            Operator::Delete => NormalAction::Delete {
                from,
                to,
                linewise: false,
            },
            Operator::Change => NormalAction::Change { from, to },
            Operator::Yank => NormalAction::Yank {
                from,
                to,
                linewise: false,
            },
        }
    } else {
        NormalAction::None
    }
}

fn from_find(
    key: char,
    find: FindType,
    count: u32,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    state.command = CommandState::Idle;
    if let Some(target) = find_char(text, cursor, key, find, count) {
        state.persistent.last_find = Some((find, key));
        NormalAction::MoveCursor(target)
    } else {
        NormalAction::None
    }
}

fn from_g(
    key: char,
    count: u32,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    state.command = CommandState::Idle;
    match key {
        'g' => {
            let target = if count > 1 {
                go_to_line(text, count as usize)
            } else {
                0
            };
            NormalAction::MoveCursor(target)
        }
        'j' => NormalAction::MoveCursor(resolve_motion('j', text, cursor, count)),
        'k' => NormalAction::MoveCursor(resolve_motion('k', text, cursor, count)),
        _ => NormalAction::None,
    }
}

fn from_operator_g(
    key: char,
    op: Operator,
    count: u32,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    state.command = CommandState::Idle;
    match key {
        'g' => {
            let target = if count > 1 {
                go_to_line(text, count as usize)
            } else {
                0
            };
            let (from, to) = if target < cursor {
                (target, end_of_line(text, cursor))
            } else {
                (start_of_line(text, cursor), end_of_line(text, target))
            };
            let to = if to < text.len() { to + 1 } else { to };
            match op {
                Operator::Delete => NormalAction::Delete {
                    from,
                    to,
                    linewise: true,
                },
                Operator::Change => NormalAction::Change { from, to },
                Operator::Yank => NormalAction::Yank {
                    from,
                    to,
                    linewise: true,
                },
            }
        }
        _ => NormalAction::None,
    }
}

fn from_replace(
    key: char,
    _count: u32,
    text: &str,
    cursor: usize,
    state: &mut VimState,
) -> NormalAction {
    state.command = CommandState::Idle;
    if cursor < text.len() {
        state.persistent.last_change = Some(RecordedChange::ReplaceChar { ch: key, count: 1 });
        NormalAction::ReplaceChar { offset: cursor, ch: key }
    } else {
        NormalAction::None
    }
}

fn from_indent(
    key: char,
    dir: IndentDir,
    count: u32,
    state: &mut VimState,
) -> NormalAction {
    state.command = CommandState::Idle;
    let expected = if dir == IndentDir::Right { '>' } else { '<' };
    if key == expected {
        state.persistent.last_change = Some(RecordedChange::Indent { dir, count });
    }
    NormalAction::None
}

// ---------------------------------------------------------------------------
// Line navigation helpers
// ---------------------------------------------------------------------------

fn go_to_line(text: &str, line_num: usize) -> usize {
    let target = line_num.saturating_sub(1);
    let mut offset = 0;
    for (i, line) in text.split('\n').enumerate() {
        if i == target {
            return offset;
        }
        offset += line.len() + 1;
    }
    start_of_last_line(text)
}

fn start_of_last_line(text: &str) -> usize {
    text.rfind('\n').map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_motion_h_l() {
        let text = "hello";
        assert_eq!(resolve_motion('l', text, 0, 1), 1);
        assert_eq!(resolve_motion('l', text, 0, 3), 3);
        assert_eq!(resolve_motion('h', text, 3, 1), 2);
        assert_eq!(resolve_motion('h', text, 0, 1), 0);
    }

    #[test]
    fn test_resolve_motion_0_dollar() {
        let text = "hello\nworld";
        assert_eq!(resolve_motion('0', text, 3, 1), 0);
        assert_eq!(resolve_motion('$', text, 0, 1), 5);
        assert_eq!(resolve_motion('0', text, 8, 1), 6);
        assert_eq!(resolve_motion('$', text, 8, 1), 11);
    }

    #[test]
    fn test_resolve_motion_j_k() {
        let text = "abc\ndef\nghi";
        assert_eq!(resolve_motion('j', text, 1, 1), 5); // a[b]c -> d[e]f
        assert_eq!(resolve_motion('k', text, 5, 1), 1); // d[e]f -> a[b]c
    }

    #[test]
    fn test_resolve_motion_w_b() {
        let text = "hello world";
        let w = resolve_motion('w', text, 0, 1);
        assert_eq!(w, 6);
        let b = resolve_motion('b', text, 6, 1);
        assert_eq!(b, 0);
    }

    #[test]
    fn test_find_char_forward() {
        let text = "hello world";
        assert_eq!(find_char(text, 0, 'o', FindType::FindForward, 1), Some(4));
        assert_eq!(find_char(text, 0, 'o', FindType::TilForward, 1), Some(3));
        assert_eq!(find_char(text, 0, 'z', FindType::FindForward, 1), None);
    }

    #[test]
    fn test_find_text_object_word() {
        let text = "hello world";
        let range = find_text_object(text, 2, 'w', true);
        assert_eq!(range, Some((0, 5)));
        let range = find_text_object(text, 2, 'w', false);
        assert_eq!(range, Some((0, 6))); // includes trailing space
    }

    #[test]
    fn test_find_text_object_brackets() {
        let text = "foo(bar)baz";
        let range = find_text_object(text, 5, '(', true);
        assert_eq!(range, Some((4, 7)));
        let range = find_text_object(text, 5, '(', false);
        assert_eq!(range, Some((3, 8)));
    }

    #[test]
    fn test_find_text_object_quotes() {
        let text = "say \"hello\" end";
        let range = find_text_object(text, 6, '"', true);
        assert_eq!(range, Some((5, 10)));
    }

    #[test]
    fn test_process_normal_key_basic_motions() {
        let text = "hello world";
        let mut state = VimState::new();
        state.enter_normal();

        let action = process_normal_key('l', text, 0, &mut state);
        assert!(matches!(action, NormalAction::MoveCursor(1)));

        let action = process_normal_key('w', text, 0, &mut state);
        assert!(matches!(action, NormalAction::MoveCursor(6)));
    }

    #[test]
    fn test_process_normal_key_insert_transitions() {
        let text = "hello";
        let mut state = VimState::new();
        state.enter_normal();

        let action = process_normal_key('i', text, 2, &mut state);
        assert!(matches!(action, NormalAction::EnterInsert(2)));

        state.enter_normal();
        let action = process_normal_key('a', text, 2, &mut state);
        assert!(matches!(action, NormalAction::EnterInsert(3)));

        state.enter_normal();
        let action = process_normal_key('A', text, 2, &mut state);
        assert!(matches!(action, NormalAction::EnterInsert(5)));
    }

    #[test]
    fn test_process_normal_key_delete_word() {
        let text = "hello world";
        let mut state = VimState::new();
        state.enter_normal();

        let action = process_normal_key('d', text, 0, &mut state);
        assert!(matches!(action, NormalAction::None));
        assert!(matches!(state.command, CommandState::Operator { .. }));

        let action = process_normal_key('w', text, 0, &mut state);
        assert!(matches!(action, NormalAction::Delete { from: 0, to: 6, .. }));
    }

    #[test]
    fn test_process_normal_key_dd() {
        let text = "hello\nworld";
        let mut state = VimState::new();
        state.enter_normal();

        let _ = process_normal_key('d', text, 2, &mut state);
        let action = process_normal_key('d', text, 2, &mut state);
        assert!(matches!(action, NormalAction::Delete { from: 0, to: 6, linewise: true }));
    }

    #[test]
    fn test_count_motion() {
        let text = "one two three four";
        let mut state = VimState::new();
        state.enter_normal();

        let _ = process_normal_key('3', text, 0, &mut state);
        let action = process_normal_key('w', text, 0, &mut state);
        let target = resolve_motion('w', text, 0, 3);
        assert!(matches!(action, NormalAction::MoveCursor(t) if t == target));
    }

    #[test]
    fn test_vim_mode_display() {
        assert_eq!(VimMode::Normal.to_string(), "NORMAL");
        assert_eq!(VimMode::Insert.to_string(), "INSERT");
        assert_eq!(VimMode::Visual.to_string(), "VISUAL");
        assert_eq!(VimMode::Command.to_string(), "COMMAND");
    }
}
