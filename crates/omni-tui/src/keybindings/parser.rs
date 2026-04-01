//! Keystroke and chord parsing.
//!
//! Parses human-readable keystroke strings like `"ctrl+shift+k"` and chord
//! sequences like `"ctrl+x ctrl+k"` into structured types that the matcher
//! can compare against crossterm key events.

use super::KeybindingContext;

/// A single parsed keystroke with modifier flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedKeystroke {
    /// The base key name (lowercase), e.g. `"k"`, `"escape"`, `"enter"`.
    pub key: String,
    /// Ctrl modifier.
    pub ctrl: bool,
    /// Alt/Option modifier.
    pub alt: bool,
    /// Shift modifier.
    pub shift: bool,
    /// Meta modifier (treated as alias for alt in most terminals).
    pub meta: bool,
}

/// A chord is a sequence of one or more keystrokes.
pub type Chord = Vec<ParsedKeystroke>;

/// A parsed keybinding: chord -> action, scoped to a context.
#[derive(Debug, Clone)]
pub struct ParsedBinding {
    /// The keystroke sequence that triggers this binding.
    pub chord: Chord,
    /// The action to trigger, or None to unbind.
    pub action: Option<String>,
    /// The UI context where this binding applies.
    pub context: KeybindingContext,
}

/// A raw keybinding block from configuration (before parsing).
#[derive(Debug, Clone)]
pub struct KeybindingBlock {
    /// Context name.
    pub context: KeybindingContext,
    /// Map of keystroke pattern -> action (None = unbind).
    pub bindings: Vec<(String, Option<String>)>,
}

/// Parse a keystroke string like `"ctrl+shift+k"` into a [`ParsedKeystroke`].
///
/// Supports modifier aliases:
/// - `ctrl` / `control`
/// - `alt` / `opt` / `option`
/// - `shift`
/// - `meta`
///
/// Special key names: `esc`/`escape`, `return`/`enter`, `space`, `tab`,
/// `backspace`, `delete`, arrow keys (`up`, `down`, `left`, `right`),
/// `pageup`, `pagedown`, `home`, `end`.
pub fn parse_keystroke(input: &str) -> ParsedKeystroke {
    let parts: Vec<&str> = input.split('+').collect();
    let mut ks = ParsedKeystroke {
        key: String::new(),
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };

    for part in parts {
        let lower = part.to_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => ks.ctrl = true,
            "alt" | "opt" | "option" => ks.alt = true,
            "shift" => ks.shift = true,
            "meta" => ks.meta = true,
            "esc" => ks.key = "escape".to_string(),
            "return" => ks.key = "enter".to_string(),
            "space" => ks.key = " ".to_string(),
            _ => ks.key = lower,
        }
    }

    ks
}

/// Parse a chord string like `"ctrl+k ctrl+s"` into a [`Chord`].
///
/// A lone space character `" "` is treated as the space key binding, not a separator.
pub fn parse_chord(input: &str) -> Chord {
    if input == " " {
        return vec![parse_keystroke("space")];
    }
    input
        .split_whitespace()
        .map(parse_keystroke)
        .collect()
}

/// Convert a [`ParsedKeystroke`] to its canonical string representation.
pub fn keystroke_to_string(ks: &ParsedKeystroke) -> String {
    let mut parts = Vec::new();
    if ks.ctrl {
        parts.push("ctrl");
    }
    if ks.alt {
        parts.push("alt");
    }
    if ks.shift {
        parts.push("shift");
    }
    if ks.meta {
        parts.push("meta");
    }
    let display = match ks.key.as_str() {
        "escape" => "Esc",
        " " => "Space",
        "enter" => "Enter",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "home" => "Home",
        "end" => "End",
        "tab" => "Tab",
        other => other,
    };
    parts.push(display);
    parts.join("+")
}

/// Convert a [`Chord`] to its canonical display string.
pub fn chord_to_string(chord: &Chord) -> String {
    chord
        .iter()
        .map(keystroke_to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse keybinding blocks into a flat list of [`ParsedBinding`]s.
pub fn parse_bindings(blocks: &[KeybindingBlock]) -> Vec<ParsedBinding> {
    let mut bindings = Vec::new();
    for block in blocks {
        for (key_str, action) in &block.bindings {
            bindings.push(ParsedBinding {
                chord: parse_chord(key_str),
                action: action.clone(),
                context: block.context,
            });
        }
    }
    bindings
}

/// Check if two keystrokes are equal. Collapses alt/meta into one modifier.
pub fn keystrokes_equal(a: &ParsedKeystroke, b: &ParsedKeystroke) -> bool {
    a.key == b.key
        && a.ctrl == b.ctrl
        && a.shift == b.shift
        && (a.alt || a.meta) == (b.alt || b.meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_keystroke_simple() {
        let ks = parse_keystroke("k");
        assert_eq!(ks.key, "k");
        assert!(!ks.ctrl);
        assert!(!ks.alt);
        assert!(!ks.shift);
    }

    #[test]
    fn test_parse_keystroke_modifiers() {
        let ks = parse_keystroke("ctrl+shift+k");
        assert_eq!(ks.key, "k");
        assert!(ks.ctrl);
        assert!(ks.shift);
        assert!(!ks.alt);
    }

    #[test]
    fn test_parse_keystroke_aliases() {
        let ks = parse_keystroke("opt+esc");
        assert_eq!(ks.key, "escape");
        assert!(ks.alt);
    }

    #[test]
    fn test_parse_chord_single() {
        let chord = parse_chord("ctrl+k");
        assert_eq!(chord.len(), 1);
        assert!(chord[0].ctrl);
        assert_eq!(chord[0].key, "k");
    }

    #[test]
    fn test_parse_chord_multi() {
        let chord = parse_chord("ctrl+x ctrl+k");
        assert_eq!(chord.len(), 2);
        assert!(chord[0].ctrl);
        assert_eq!(chord[0].key, "x");
        assert!(chord[1].ctrl);
        assert_eq!(chord[1].key, "k");
    }

    #[test]
    fn test_parse_chord_space() {
        let chord = parse_chord(" ");
        assert_eq!(chord.len(), 1);
        assert_eq!(chord[0].key, " ");
    }

    #[test]
    fn test_keystroke_to_string() {
        let ks = ParsedKeystroke {
            key: "k".to_string(),
            ctrl: true,
            alt: false,
            shift: true,
            meta: false,
        };
        assert_eq!(keystroke_to_string(&ks), "ctrl+shift+k");
    }

    #[test]
    fn test_chord_to_string() {
        let chord = parse_chord("ctrl+x ctrl+k");
        assert_eq!(chord_to_string(&chord), "ctrl+x ctrl+k");
    }

    #[test]
    fn test_keystrokes_equal_alt_meta() {
        let a = ParsedKeystroke {
            key: "k".to_string(),
            ctrl: false,
            alt: true,
            shift: false,
            meta: false,
        };
        let b = ParsedKeystroke {
            key: "k".to_string(),
            ctrl: false,
            alt: false,
            shift: false,
            meta: true,
        };
        assert!(keystrokes_equal(&a, &b));
    }
}
