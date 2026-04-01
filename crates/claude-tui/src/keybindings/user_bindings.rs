//! User keybinding overrides loaded from `~/.claude/keybindings.json`.
//!
//! The file format follows the same structure as the TypeScript original:
//! ```json
//! {
//!   "bindings": [
//!     {
//!       "context": "Chat",
//!       "bindings": {
//!         "ctrl+k": "chat:submit",
//!         "ctrl+c": null
//!       }
//!     }
//!   ]
//! }
//! ```
//!
//! Setting an action to `null` unbinds the default shortcut.

use std::path::Path;

use tracing::warn;

use super::parser::KeybindingBlock;
use super::KeybindingContext;

/// Load user keybindings from a JSON file.
///
/// Supports two formats:
///
/// **Block format** (original):
/// ```json
/// {
///   "bindings": [
///     { "context": "Chat", "bindings": { "ctrl+k": "chat:submit", "ctrl+c": null } }
///   ]
/// }
/// ```
///
/// **Flat array format**:
/// ```json
/// [
///   { "keys": "ctrl+x ctrl+e", "command": "editor:open", "context": "Chat" }
/// ]
/// ```
///
/// In the flat format the `"context"` field defaults to `"Global"` when omitted.
/// Setting `"command"` to `null` unbinds the key.
///
/// Returns `None` if the file doesn't exist or can't be parsed.
/// Logs warnings for parse errors but does not propagate them.
pub fn load_user_bindings(path: &Path) -> Option<Vec<KeybindingBlock>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!("Failed to read keybindings file {}: {}", path.display(), e);
            }
            return None;
        }
    };

    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to parse keybindings JSON {}: {}", path.display(), e);
            return None;
        }
    };

    // Try flat array format first: [{"keys": "...", "command": "...", ...}]
    if let Some(arr) = json.as_array() {
        return Some(parse_flat_array(arr));
    }

    // Block format: {"bindings": [{...}, ...]}
    let blocks_array = json
        .get("bindings")
        .and_then(|v| v.as_array())?;

    let mut result = Vec::new();
    for block in blocks_array {
        let context_str = block.get("context").and_then(|v| v.as_str())?;
        let context = match KeybindingContext::parse(context_str) {
            Some(c) => c,
            None => {
                warn!("Unknown keybinding context: {}", context_str);
                continue;
            }
        };

        let bindings_obj = match block.get("bindings").and_then(|v| v.as_object()) {
            Some(obj) => obj,
            None => continue,
        };

        let mut bindings = Vec::new();
        for (key_str, action_val) in bindings_obj {
            let action = if action_val.is_null() {
                None
            } else {
                action_val.as_str().map(|s| s.to_string())
            };
            bindings.push((key_str.clone(), action));
        }

        result.push(KeybindingBlock {
            context,
            bindings,
        });
    }

    Some(result)
}

/// Parse the flat array format into keybinding blocks.
///
/// Groups entries by context so they merge cleanly with the existing system.
fn parse_flat_array(arr: &[serde_json::Value]) -> Vec<KeybindingBlock> {
    use std::collections::HashMap;

    let mut by_context: HashMap<KeybindingContext, Vec<(String, Option<String>)>> = HashMap::new();

    for entry in arr {
        let keys = match entry.get("keys").and_then(|v| v.as_str()) {
            Some(k) => k.to_string(),
            None => {
                warn!("Keybinding entry missing 'keys' field: {:?}", entry);
                continue;
            }
        };

        let command = if entry.get("command").map_or(false, |v| v.is_null()) {
            None
        } else {
            entry.get("command").and_then(|v| v.as_str()).map(|s| s.to_string())
        };

        let context = entry
            .get("context")
            .and_then(|v| v.as_str())
            .and_then(KeybindingContext::parse)
            .unwrap_or(KeybindingContext::Global);

        by_context.entry(context).or_default().push((keys, command));
    }

    by_context
        .into_iter()
        .map(|(context, bindings)| KeybindingBlock { context, bindings })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn test_load_nonexistent() {
        let result = load_user_bindings(Path::new("/nonexistent/keybindings.json"));
        assert!(result.is_none());
    }

    #[test]
    fn test_load_valid() {
        let f = write_temp(r#"{
            "bindings": [
                {
                    "context": "Chat",
                    "bindings": {
                        "ctrl+k": "chat:submit",
                        "ctrl+c": null
                    }
                }
            ]
        }"#);
        let result = load_user_bindings(f.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].context, KeybindingContext::Chat);
        assert_eq!(result[0].bindings.len(), 2);
        // JSON object order is not guaranteed, so check both entries exist
        let has_submit = result[0].bindings.iter().any(|(k, a)| k == "ctrl+k" && *a == Some("chat:submit".to_string()));
        let has_null = result[0].bindings.iter().any(|(k, a)| k == "ctrl+c" && a.is_none());
        assert!(has_submit, "Expected ctrl+k -> chat:submit binding");
        assert!(has_null, "Expected ctrl+c -> null unbind");
    }

    #[test]
    fn test_load_invalid_json() {
        let f = write_temp("not json");
        let result = load_user_bindings(f.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_load_unknown_context() {
        let f = write_temp(r#"{
            "bindings": [
                {
                    "context": "UnknownContext",
                    "bindings": { "ctrl+k": "foo" }
                }
            ]
        }"#);
        let result = load_user_bindings(f.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_load_flat_array_format() {
        let f = write_temp(r#"[
            {"keys": "ctrl+x ctrl+e", "command": "editor:open", "context": "Chat"},
            {"keys": "ctrl+s", "command": "chat:stash", "context": "Chat"}
        ]"#);
        let result = load_user_bindings(f.path()).unwrap();
        assert_eq!(result.len(), 1, "Both entries are Chat context, grouped into one block");
        let chat_block = &result[0];
        assert_eq!(chat_block.context, KeybindingContext::Chat);
        assert_eq!(chat_block.bindings.len(), 2);
        let has_editor = chat_block.bindings.iter().any(|(k, a)| {
            k == "ctrl+x ctrl+e" && *a == Some("editor:open".to_string())
        });
        let has_stash = chat_block.bindings.iter().any(|(k, a)| {
            k == "ctrl+s" && *a == Some("chat:stash".to_string())
        });
        assert!(has_editor, "Expected ctrl+x ctrl+e -> editor:open");
        assert!(has_stash, "Expected ctrl+s -> chat:stash");
    }

    #[test]
    fn test_load_flat_array_default_context() {
        let f = write_temp(r#"[
            {"keys": "ctrl+q", "command": "app:exit"}
        ]"#);
        let result = load_user_bindings(f.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].context, KeybindingContext::Global);
        assert_eq!(result[0].bindings[0].0, "ctrl+q");
        assert_eq!(result[0].bindings[0].1, Some("app:exit".to_string()));
    }

    #[test]
    fn test_load_flat_array_unbind() {
        let f = write_temp(r#"[
            {"keys": "ctrl+c", "command": null, "context": "Global"}
        ]"#);
        let result = load_user_bindings(f.path()).unwrap();
        assert_eq!(result.len(), 1);
        let binding = &result[0].bindings[0];
        assert_eq!(binding.0, "ctrl+c");
        assert!(binding.1.is_none(), "null command should unbind");
    }

    #[test]
    fn test_load_flat_array_multiple_contexts() {
        let f = write_temp(r#"[
            {"keys": "ctrl+a", "command": "foo", "context": "Chat"},
            {"keys": "ctrl+b", "command": "bar", "context": "Global"}
        ]"#);
        let result = load_user_bindings(f.path()).unwrap();
        assert_eq!(result.len(), 2, "Two different contexts produce two blocks");
    }
}
