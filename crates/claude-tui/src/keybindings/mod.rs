//! Keybinding system with chord support and user overrides.
//!
//! Keybindings are organized by context (e.g. Global, Chat, Confirmation).
//! Each binding maps a chord (one or more keystrokes) to an action string.
//! User bindings from `~/.claude/keybindings.json` override the defaults.

mod default_bindings;
mod matcher;
mod parser;
mod user_bindings;

pub use default_bindings::default_bindings;
pub use matcher::{ResolveResult, resolve_key, resolve_key_with_chord};
pub use parser::{Chord, ParsedBinding, ParsedKeystroke, parse_chord, parse_bindings};
pub use user_bindings::load_user_bindings;

use crossterm::event::KeyEvent;
use serde::{Deserialize, Serialize};

/// UI context where keybindings apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeybindingContext {
    Global,
    Chat,
    Autocomplete,
    Confirmation,
    Help,
    Transcript,
    HistorySearch,
    Task,
    Settings,
}

impl KeybindingContext {
    /// Parse a context name string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "global" => Some(Self::Global),
            "chat" => Some(Self::Chat),
            "autocomplete" => Some(Self::Autocomplete),
            "confirmation" => Some(Self::Confirmation),
            "help" => Some(Self::Help),
            "transcript" => Some(Self::Transcript),
            "historysearch" => Some(Self::HistorySearch),
            "task" => Some(Self::Task),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }
}

impl std::fmt::Display for KeybindingContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Global => write!(f, "Global"),
            Self::Chat => write!(f, "Chat"),
            Self::Autocomplete => write!(f, "Autocomplete"),
            Self::Confirmation => write!(f, "Confirmation"),
            Self::Help => write!(f, "Help"),
            Self::Transcript => write!(f, "Transcript"),
            Self::HistorySearch => write!(f, "HistorySearch"),
            Self::Task => write!(f, "Task"),
            Self::Settings => write!(f, "Settings"),
        }
    }
}

/// Manages keybindings: default + user overrides, chord state.
pub struct KeybindingManager {
    /// All parsed bindings (defaults first, then user overrides).
    bindings: Vec<ParsedBinding>,
    /// Pending chord keystrokes (for multi-key combos like ctrl+x ctrl+k).
    pending_chord: Option<Vec<ParsedKeystroke>>,
}

impl KeybindingManager {
    /// Create a new manager with default bindings.
    pub fn new() -> Self {
        let bindings = parse_bindings(&default_bindings());
        Self {
            bindings,
            pending_chord: None,
        }
    }

    /// Load and merge user bindings from the given path.
    pub fn load_user_overrides(&mut self, path: &std::path::Path) {
        if let Some(user_blocks) = load_user_bindings(path) {
            let user_parsed = parse_bindings(&user_blocks);
            self.bindings.extend(user_parsed);
        }
    }

    /// All parsed bindings.
    pub fn bindings(&self) -> &[ParsedBinding] {
        &self.bindings
    }

    /// Whether we're in the middle of a chord sequence.
    pub fn has_pending_chord(&self) -> bool {
        self.pending_chord.is_some()
    }

    /// Cancel any pending chord.
    pub fn cancel_chord(&mut self) {
        self.pending_chord = None;
    }

    /// Resolve a crossterm key event against the active contexts.
    ///
    /// Returns a [`ResolveResult`] indicating whether an action was matched,
    /// a chord was started, or nothing matched.
    pub fn resolve(
        &mut self,
        key: KeyEvent,
        active_contexts: &[KeybindingContext],
    ) -> ResolveResult {
        let result = resolve_key_with_chord(
            key,
            active_contexts,
            &self.bindings,
            self.pending_chord.take(),
        );

        match &result {
            ResolveResult::ChordStarted { pending } => {
                self.pending_chord = Some(pending.clone());
            }
            _ => {
                self.pending_chord = None;
            }
        }

        result
    }

    /// Look up the display string for an action in a given context.
    pub fn display_for_action(
        &self,
        action: &str,
        context: KeybindingContext,
    ) -> Option<String> {
        self.bindings
            .iter()
            .rev()
            .find(|b| b.action.as_deref() == Some(action) && b.context == context)
            .map(|b| parser::chord_to_string(&b.chord))
    }
}

impl Default for KeybindingManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_from_str() {
        assert_eq!(KeybindingContext::parse("Global"), Some(KeybindingContext::Global));
        assert_eq!(KeybindingContext::parse("chat"), Some(KeybindingContext::Chat));
        assert_eq!(KeybindingContext::parse("CONFIRMATION"), Some(KeybindingContext::Confirmation));
        assert_eq!(KeybindingContext::parse("unknown"), None);
    }

    #[test]
    fn test_manager_has_default_bindings() {
        let mgr = KeybindingManager::new();
        assert!(!mgr.bindings().is_empty());
    }

    #[test]
    fn test_context_display() {
        assert_eq!(KeybindingContext::Global.to_string(), "Global");
        assert_eq!(KeybindingContext::Chat.to_string(), "Chat");
    }
}
