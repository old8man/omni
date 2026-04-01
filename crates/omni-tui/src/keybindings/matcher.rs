//! Keybinding matching and resolution.
//!
//! Matches crossterm [`KeyEvent`]s against parsed bindings, supporting both
//! single-keystroke and multi-keystroke chord bindings.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::parser::{keystrokes_equal, ParsedBinding, ParsedKeystroke};
use super::KeybindingContext;

/// Result of resolving a key event against bindings.
#[derive(Debug, Clone)]
pub enum ResolveResult {
    /// An action was matched.
    Match { action: String },
    /// No binding matched.
    None,
    /// A binding matched but was explicitly unbound (action = null).
    Unbound,
    /// The keystroke is a prefix of a longer chord — wait for more keys.
    ChordStarted { pending: Vec<ParsedKeystroke> },
    /// A pending chord was cancelled (no match after chord prefix).
    ChordCancelled,
}

/// Convert a crossterm [`KeyEvent`] to a [`ParsedKeystroke`] for comparison.
fn key_event_to_keystroke(key: KeyEvent) -> Option<ParsedKeystroke> {
    let key_name = match key.code {
        KeyCode::Char(c) => c.to_lowercase().to_string(),
        KeyCode::Esc => "escape".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => return Some(ParsedKeystroke {
            key: "tab".to_string(),
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
            alt: key.modifiers.contains(KeyModifiers::ALT),
            shift: true,
            meta: false,
        }),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        _ => return None,
    };

    Some(ParsedKeystroke {
        key: key_name,
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
        meta: false,
    })
}

/// Resolve a single key event (no chord state).
pub fn resolve_key(
    key: KeyEvent,
    active_contexts: &[KeybindingContext],
    bindings: &[ParsedBinding],
) -> ResolveResult {
    resolve_key_with_chord(key, active_contexts, bindings, None)
}

/// Resolve a key event with chord state support.
///
/// If `pending` is `Some`, we're in the middle of a multi-key chord.
/// The function checks whether the accumulated keystrokes match any binding,
/// could be a prefix of a longer chord, or should cancel the chord.
pub fn resolve_key_with_chord(
    key: KeyEvent,
    active_contexts: &[KeybindingContext],
    bindings: &[ParsedBinding],
    pending: Option<Vec<ParsedKeystroke>>,
) -> ResolveResult {
    // Cancel chord on escape
    if key.code == KeyCode::Esc && pending.is_some() {
        return ResolveResult::ChordCancelled;
    }

    let current = match key_event_to_keystroke(key) {
        Some(ks) => ks,
        None => {
            if pending.is_some() {
                return ResolveResult::ChordCancelled;
            }
            return ResolveResult::None;
        }
    };

    let mut test_chord = pending.unwrap_or_default();
    test_chord.push(current);

    // Filter bindings by active contexts
    let ctx_matches = |b: &ParsedBinding| active_contexts.contains(&b.context);

    // Check for prefix matches (longer chords that start with our sequence)
    let has_longer = bindings.iter().any(|b| {
        ctx_matches(b)
            && b.chord.len() > test_chord.len()
            && b.action.is_some()
            && chord_prefix_matches(&test_chord, &b.chord)
    });

    if has_longer {
        return ResolveResult::ChordStarted {
            pending: test_chord,
        };
    }

    // Check for exact matches (last one wins for user overrides)
    let mut exact_match: Option<&ParsedBinding> = None;
    for binding in bindings {
        if ctx_matches(binding) && chord_exactly_matches(&test_chord, &binding.chord) {
            exact_match = Some(binding);
        }
    }

    if let Some(binding) = exact_match {
        return match &binding.action {
            Some(action) => ResolveResult::Match {
                action: action.clone(),
            },
            None => ResolveResult::Unbound,
        };
    }

    // No match
    if test_chord.len() > 1 {
        ResolveResult::ChordCancelled
    } else {
        ResolveResult::None
    }
}

/// Check if `prefix` is a prefix of `chord`.
fn chord_prefix_matches(prefix: &[ParsedKeystroke], chord: &[ParsedKeystroke]) -> bool {
    if prefix.len() >= chord.len() {
        return false;
    }
    prefix
        .iter()
        .zip(chord.iter())
        .all(|(a, b)| keystrokes_equal(a, b))
}

/// Check if two chords are exactly equal.
fn chord_exactly_matches(a: &[ParsedKeystroke], b: &[ParsedKeystroke]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| keystrokes_equal(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::parser::parse_chord;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn binding(chord_str: &str, action: &str, context: KeybindingContext) -> ParsedBinding {
        ParsedBinding {
            chord: parse_chord(chord_str),
            action: Some(action.to_string()),
            context,
        }
    }

    #[test]
    fn test_simple_match() {
        let bindings = vec![binding("ctrl+c", "app:interrupt", KeybindingContext::Global)];
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let result = resolve_key(key, &[KeybindingContext::Global], &bindings);
        assert!(matches!(result, ResolveResult::Match { action } if action == "app:interrupt"));
    }

    #[test]
    fn test_no_match() {
        let bindings = vec![binding("ctrl+c", "app:interrupt", KeybindingContext::Global)];
        let key = make_key(KeyCode::Char('k'), KeyModifiers::empty());
        let result = resolve_key(key, &[KeybindingContext::Global], &bindings);
        assert!(matches!(result, ResolveResult::None));
    }

    #[test]
    fn test_context_filtering() {
        let bindings = vec![binding("enter", "chat:submit", KeybindingContext::Chat)];
        let key = make_key(KeyCode::Enter, KeyModifiers::empty());
        // Wrong context
        let result = resolve_key(key, &[KeybindingContext::Global], &bindings);
        assert!(matches!(result, ResolveResult::None));
        // Right context
        let result = resolve_key(key, &[KeybindingContext::Chat], &bindings);
        assert!(matches!(result, ResolveResult::Match { .. }));
    }

    #[test]
    fn test_chord_prefix() {
        let bindings = vec![binding(
            "ctrl+x ctrl+k",
            "chat:killAgents",
            KeybindingContext::Chat,
        )];
        let key1 = make_key(KeyCode::Char('x'), KeyModifiers::CONTROL);
        let result = resolve_key(key1, &[KeybindingContext::Chat], &bindings);
        assert!(matches!(result, ResolveResult::ChordStarted { .. }));
    }

    #[test]
    fn test_chord_complete() {
        let bindings = vec![binding(
            "ctrl+x ctrl+k",
            "chat:killAgents",
            KeybindingContext::Chat,
        )];
        let key1 = make_key(KeyCode::Char('x'), KeyModifiers::CONTROL);
        let result = resolve_key_with_chord(
            key1,
            &[KeybindingContext::Chat],
            &bindings,
            None,
        );
        let pending = match result {
            ResolveResult::ChordStarted { pending } => pending,
            _ => panic!("Expected ChordStarted"),
        };
        let key2 = make_key(KeyCode::Char('k'), KeyModifiers::CONTROL);
        let result = resolve_key_with_chord(
            key2,
            &[KeybindingContext::Chat],
            &bindings,
            Some(pending),
        );
        assert!(matches!(result, ResolveResult::Match { action } if action == "chat:killAgents"));
    }

    #[test]
    fn test_last_binding_wins() {
        let bindings = vec![
            binding("ctrl+c", "app:interrupt", KeybindingContext::Global),
            binding("ctrl+c", "app:custom", KeybindingContext::Global),
        ];
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let result = resolve_key(key, &[KeybindingContext::Global], &bindings);
        assert!(matches!(result, ResolveResult::Match { action } if action == "app:custom"));
    }

    #[test]
    fn test_unbound() {
        let bindings = vec![ParsedBinding {
            chord: parse_chord("ctrl+c"),
            action: None,
            context: KeybindingContext::Global,
        }];
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let result = resolve_key(key, &[KeybindingContext::Global], &bindings);
        assert!(matches!(result, ResolveResult::Unbound));
    }
}
