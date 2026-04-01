//! Mouse event handling and text selection support.
//!
//! Translates crossterm mouse events into application-level actions,
//! manages text selection state, and provides clipboard integration
//! via OSC 52 escape sequences (works in most modern terminals)
//! with fallback to the `copypasta` crate for X11/Wayland/macOS.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

/// Application-level mouse action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseAction {
    /// No action.
    None,
    /// Click at a position in the message area — could start a selection or
    /// focus a panel.
    Click { col: u16, row: u16 },
    /// Right-click at a position — opens context menu or copies selected text.
    RightClick { col: u16, row: u16 },
    /// Ctrl+Click — copy selected text (macOS-friendly alternative).
    CtrlClick { col: u16, row: u16 },
    /// Scroll up by the given number of lines.
    ScrollUp(u16),
    /// Scroll down by the given number of lines.
    ScrollDown(u16),
    /// Drag to extend a selection.
    Drag { col: u16, row: u16 },
    /// Mouse button released — finalize selection.
    Release { col: u16, row: u16 },
}

/// Translate a crossterm [`MouseEvent`] into a [`MouseAction`].
pub fn translate_mouse_event(event: MouseEvent) -> MouseAction {
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if event.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                || event.modifiers.contains(crossterm::event::KeyModifiers::SUPER)
            {
                MouseAction::CtrlClick {
                    col: event.column,
                    row: event.row,
                }
            } else {
                MouseAction::Click {
                    col: event.column,
                    row: event.row,
                }
            }
        }
        MouseEventKind::Down(MouseButton::Right) => MouseAction::RightClick {
            col: event.column,
            row: event.row,
        },
        MouseEventKind::Up(MouseButton::Left) => MouseAction::Release {
            col: event.column,
            row: event.row,
        },
        MouseEventKind::Drag(MouseButton::Left) => MouseAction::Drag {
            col: event.column,
            row: event.row,
        },
        MouseEventKind::ScrollUp => MouseAction::ScrollUp(3),
        MouseEventKind::ScrollDown => MouseAction::ScrollDown(3),
        _ => MouseAction::None,
    }
}

/// A text selection in the terminal.
#[derive(Debug, Clone, Default)]
pub struct TextSelection {
    /// Whether a selection is currently active (mouse button down).
    pub active: bool,
    /// Starting position of the selection.
    pub start: (u16, u16),
    /// Current end position of the selection.
    pub end: (u16, u16),
}

impl TextSelection {
    /// Create a new empty selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new selection at the given position.
    pub fn start_at(&mut self, col: u16, row: u16) {
        self.active = true;
        self.start = (col, row);
        self.end = (col, row);
    }

    /// Extend the selection to the given position.
    pub fn extend_to(&mut self, col: u16, row: u16) {
        if self.active {
            self.end = (col, row);
        }
    }

    /// Finalize the selection (mouse released).
    pub fn finalize(&mut self) {
        self.active = false;
    }

    /// Clear the selection.
    pub fn clear(&mut self) {
        self.active = false;
        self.start = (0, 0);
        self.end = (0, 0);
    }

    /// Whether there is a non-empty selection.
    pub fn has_selection(&self) -> bool {
        self.start != self.end
    }

    /// Return the normalized selection range as ((start_col, start_row), (end_col, end_row))
    /// where start is before end in reading order.
    pub fn normalized(&self) -> ((u16, u16), (u16, u16)) {
        let (sc, sr) = self.start; // (col, row)
        let (ec, er) = self.end;   // (col, row)
        if sr < er || (sr == er && sc <= ec) {
            ((sc, sr), (ec, er))
        } else {
            ((ec, er), (sc, sr))
        }
    }

    /// Check if a given (col, row) position is within the selection.
    pub fn contains(&self, col: u16, row: u16) -> bool {
        if !self.has_selection() {
            return false;
        }
        let ((sc, sr), (ec, er)) = self.normalized();
        if row < sr || row > er {
            return false;
        }
        if sr == er {
            // Single-line selection
            return col >= sc && col <= ec;
        }
        if row == sr {
            return col >= sc;
        }
        if row == er {
            return col <= ec;
        }
        // Middle lines are fully selected
        true
    }
}

/// Copy text to the system clipboard.
///
/// Attempts OSC 52 first (terminal-native, works over SSH), then falls
/// back to platform clipboard via `copypasta`.
pub fn copy_to_clipboard(text: &str) -> bool {
    // Try OSC 52 escape sequence (works in most modern terminals)
    if try_osc52_copy(text) {
        return true;
    }

    // Fallback to copypasta
    try_copypasta_copy(text)
}

/// Attempt clipboard copy via OSC 52 escape sequence.
///
/// OSC 52 sends the base64-encoded text to the terminal, which
/// puts it on the system clipboard.  Works over SSH sessions.
fn try_osc52_copy(text: &str) -> bool {
    use std::io::Write;

    let encoded = base64_encode_simple(text);
    let seq = format!("\x1b]52;c;{}\x07", encoded);

    // Write to stdout
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(seq.as_bytes()).is_ok() && stdout.flush().is_ok()
}

/// Simple base64 encoding (no external dependency needed).
fn base64_encode_simple(input: &str) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = if i + 1 < bytes.len() { bytes[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < bytes.len() { bytes[i + 2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        output.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        output.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if i + 1 < bytes.len() {
            output.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            output.push('=');
        }
        if i + 2 < bytes.len() {
            output.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            output.push('=');
        }
        i += 3;
    }
    output
}

/// Attempt clipboard copy via copypasta crate.
fn try_copypasta_copy(text: &str) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        if let Ok(mut ctx) = copypasta::ClipboardContext::new() {
            use copypasta::ClipboardProvider;
            return ctx.set_contents(text.to_string()).is_ok();
        }
    }
    false
}

/// Focus target for keyboard/mouse input routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    /// The main prompt input.
    Prompt,
    /// The message list / output area.
    Messages,
    /// The search overlay.
    Search,
    /// The context panel.
    ContextPanel,
    /// The agent status panel.
    AgentPanel,
    /// A permission dialog.
    PermissionDialog,
}

/// Focus manager that tracks which panel has keyboard focus.
pub struct FocusManager {
    /// Current focus target.
    current: FocusTarget,
    /// Previous focus target (for restoring after overlay closes).
    previous: FocusTarget,
}

impl FocusManager {
    /// Create a new focus manager with focus on the prompt.
    pub fn new() -> Self {
        Self {
            current: FocusTarget::Prompt,
            previous: FocusTarget::Prompt,
        }
    }

    /// Get the current focus target.
    pub fn current(&self) -> FocusTarget {
        self.current
    }

    /// Set focus to a new target.
    pub fn set(&mut self, target: FocusTarget) {
        self.previous = self.current;
        self.current = target;
    }

    /// Restore focus to the previous target.
    pub fn restore(&mut self) {
        std::mem::swap(&mut self.current, &mut self.previous);
    }

    /// Check if the given target currently has focus.
    pub fn is_focused(&self, target: FocusTarget) -> bool {
        self.current == target
    }

    /// Cycle focus to the next panel in order.
    pub fn cycle_next(&mut self) {
        self.previous = self.current;
        self.current = match self.current {
            FocusTarget::Prompt => FocusTarget::Messages,
            FocusTarget::Messages => FocusTarget::Prompt,
            // Overlays don't participate in cycling
            other => other,
        };
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_scroll() {
        let event = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        assert_eq!(translate_mouse_event(event), MouseAction::ScrollUp(3));
    }

    #[test]
    fn test_translate_click() {
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 5,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        assert_eq!(
            translate_mouse_event(event),
            MouseAction::Click { col: 10, row: 5 }
        );
    }

    #[test]
    fn test_selection_basic() {
        let mut sel = TextSelection::new();
        assert!(!sel.has_selection());
        sel.start_at(5, 1);
        sel.extend_to(10, 1);
        assert!(sel.has_selection());
        assert!(sel.contains(7, 1));
        assert!(!sel.contains(3, 1));
    }

    #[test]
    fn test_selection_multiline() {
        let mut sel = TextSelection::new();
        sel.start_at(5, 1);
        sel.extend_to(3, 3);
        assert!(sel.contains(10, 2)); // middle line, any column
        assert!(sel.contains(7, 1)); // first line, after start
        assert!(!sel.contains(2, 1)); // first line, before start
    }

    #[test]
    fn test_selection_clear() {
        let mut sel = TextSelection::new();
        sel.start_at(0, 0);
        sel.extend_to(10, 5);
        sel.clear();
        assert!(!sel.has_selection());
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode_simple("hello"), "aGVsbG8=");
        assert_eq!(base64_encode_simple("hi"), "aGk=");
        assert_eq!(base64_encode_simple("abc"), "YWJj");
    }

    #[test]
    fn test_focus_manager() {
        let mut fm = FocusManager::new();
        assert_eq!(fm.current(), FocusTarget::Prompt);
        fm.set(FocusTarget::Messages);
        assert_eq!(fm.current(), FocusTarget::Messages);
        fm.restore();
        assert_eq!(fm.current(), FocusTarget::Prompt);
    }

    #[test]
    fn test_focus_cycle() {
        let mut fm = FocusManager::new();
        fm.cycle_next();
        assert_eq!(fm.current(), FocusTarget::Messages);
        fm.cycle_next();
        assert_eq!(fm.current(), FocusTarget::Prompt);
    }
}
