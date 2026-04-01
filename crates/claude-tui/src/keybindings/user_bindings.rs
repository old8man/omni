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
}
