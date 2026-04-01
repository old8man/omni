//! Default keybinding configuration.
//!
//! Defines the built-in keybindings that match Claude Code's standard behavior.
//! User overrides from `~/.claude/keybindings.json` are layered on top.

use super::parser::KeybindingBlock;
use super::KeybindingContext;

/// Build the default keybinding blocks.
pub fn default_bindings() -> Vec<KeybindingBlock> {
    vec![
        KeybindingBlock {
            context: KeybindingContext::Global,
            bindings: vec![
                ("ctrl+c".into(), Some("app:interrupt".into())),
                ("ctrl+d".into(), Some("app:exit".into())),
                ("ctrl+l".into(), Some("app:redraw".into())),
                ("ctrl+r".into(), Some("history:search".into())),
            ],
        },
        KeybindingBlock {
            context: KeybindingContext::Chat,
            bindings: vec![
                ("escape".into(), Some("chat:cancel".into())),
                ("ctrl+x ctrl+k".into(), Some("chat:killAgents".into())),
                ("shift+tab".into(), Some("chat:cycleMode".into())),
                ("enter".into(), Some("chat:submit".into())),
                ("up".into(), Some("history:previous".into())),
                ("down".into(), Some("history:next".into())),
                ("ctrl+_".into(), Some("chat:undo".into())),
                ("ctrl+x ctrl+e".into(), Some("chat:externalEditor".into())),
                ("ctrl+g".into(), Some("chat:externalEditor".into())),
                ("ctrl+s".into(), Some("chat:stash".into())),
                ("ctrl+v".into(), Some("chat:imagePaste".into())),
            ],
        },
        KeybindingBlock {
            context: KeybindingContext::Autocomplete,
            bindings: vec![
                ("tab".into(), Some("autocomplete:accept".into())),
                ("escape".into(), Some("autocomplete:dismiss".into())),
                ("up".into(), Some("autocomplete:previous".into())),
                ("down".into(), Some("autocomplete:next".into())),
            ],
        },
        KeybindingBlock {
            context: KeybindingContext::Confirmation,
            bindings: vec![
                ("y".into(), Some("confirm:yes".into())),
                ("n".into(), Some("confirm:no".into())),
                ("enter".into(), Some("confirm:yes".into())),
                ("escape".into(), Some("confirm:no".into())),
                ("up".into(), Some("confirm:previous".into())),
                ("down".into(), Some("confirm:next".into())),
                ("tab".into(), Some("confirm:nextField".into())),
                ("space".into(), Some("confirm:toggle".into())),
                ("shift+tab".into(), Some("confirm:cycleMode".into())),
            ],
        },
        KeybindingBlock {
            context: KeybindingContext::Transcript,
            bindings: vec![
                ("ctrl+c".into(), Some("transcript:exit".into())),
                ("escape".into(), Some("transcript:exit".into())),
                ("q".into(), Some("transcript:exit".into())),
            ],
        },
        KeybindingBlock {
            context: KeybindingContext::HistorySearch,
            bindings: vec![
                ("ctrl+r".into(), Some("historySearch:next".into())),
                ("escape".into(), Some("historySearch:accept".into())),
                ("tab".into(), Some("historySearch:accept".into())),
                ("ctrl+c".into(), Some("historySearch:cancel".into())),
                ("enter".into(), Some("historySearch:execute".into())),
            ],
        },
        KeybindingBlock {
            context: KeybindingContext::Task,
            bindings: vec![
                ("ctrl+b".into(), Some("task:background".into())),
            ],
        },
        KeybindingBlock {
            context: KeybindingContext::Help,
            bindings: vec![
                ("escape".into(), Some("help:dismiss".into())),
            ],
        },
        KeybindingBlock {
            context: KeybindingContext::Settings,
            bindings: vec![
                ("escape".into(), Some("confirm:no".into())),
                ("up".into(), Some("select:previous".into())),
                ("down".into(), Some("select:next".into())),
                ("k".into(), Some("select:previous".into())),
                ("j".into(), Some("select:next".into())),
                ("space".into(), Some("select:accept".into())),
                ("enter".into(), Some("settings:close".into())),
                ("/".into(), Some("settings:search".into())),
                ("r".into(), Some("settings:retry".into())),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_bindings_not_empty() {
        let blocks = default_bindings();
        assert!(!blocks.is_empty());
    }

    #[test]
    fn test_global_has_ctrl_c() {
        let blocks = default_bindings();
        let global = blocks.iter().find(|b| b.context == KeybindingContext::Global).unwrap();
        let has_ctrl_c = global
            .bindings
            .iter()
            .any(|(k, a)| k == "ctrl+c" && a.as_deref() == Some("app:interrupt"));
        assert!(has_ctrl_c);
    }

    #[test]
    fn test_chat_has_chord() {
        let blocks = default_bindings();
        let chat = blocks.iter().find(|b| b.context == KeybindingContext::Chat).unwrap();
        let has_chord = chat
            .bindings
            .iter()
            .any(|(k, _)| k.contains(' '));
        assert!(has_chord, "Chat context should have multi-key chord bindings");
    }
}
