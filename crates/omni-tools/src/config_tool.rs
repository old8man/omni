use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;

/// Supported configuration settings with their metadata.
struct SettingDef {
    key: &'static str,
    setting_type: SettingType,
    description: &'static str,
    options: Option<&'static [&'static str]>,
}

enum SettingType {
    StringType,
    BoolType,
}

const SUPPORTED_SETTINGS: &[SettingDef] = &[
    SettingDef {
        key: "theme",
        setting_type: SettingType::StringType,
        description: "Color theme for the UI",
        options: Some(&["dark", "light", "light-daltonized", "dark-daltonized", "auto-dark", "auto-light"]),
    },
    SettingDef {
        key: "editorMode",
        setting_type: SettingType::StringType,
        description: "Key binding mode",
        options: Some(&["normal", "vim", "emacs"]),
    },
    SettingDef {
        key: "verbose",
        setting_type: SettingType::BoolType,
        description: "Show detailed debug output",
        options: None,
    },
    SettingDef {
        key: "autoCompactEnabled",
        setting_type: SettingType::BoolType,
        description: "Auto-compact when context is full",
        options: None,
    },
    SettingDef {
        key: "autoMemoryEnabled",
        setting_type: SettingType::BoolType,
        description: "Automatically save memories",
        options: None,
    },
    SettingDef {
        key: "model",
        setting_type: SettingType::StringType,
        description: "Default model to use",
        options: None,
    },
    SettingDef {
        key: "thinkingEnabled",
        setting_type: SettingType::BoolType,
        description: "Enable extended thinking",
        options: None,
    },
    SettingDef {
        key: "voiceEnabled",
        setting_type: SettingType::BoolType,
        description: "Enable voice input mode",
        options: None,
    },
    SettingDef {
        key: "defaultView",
        setting_type: SettingType::StringType,
        description: "Default view mode",
        options: Some(&["code", "chat"]),
    },
    SettingDef {
        key: "permissions.defaultMode",
        setting_type: SettingType::StringType,
        description: "Default permission mode",
        options: Some(&["default", "auto", "bypassPermissions"]),
    },
    SettingDef {
        key: "teammateMode",
        setting_type: SettingType::StringType,
        description: "Agent coordination mode",
        options: Some(&["off", "plan_mode_required", "plan_mode_voluntary"]),
    },
];

/// Reads and writes configuration settings programmatically.
///
/// Supports getting the current value of a setting or changing it. Settings
/// are persisted to `~/.claude/settings.json` (global config). Only
/// supported settings can be read or written.
pub struct ConfigTool;

#[async_trait]
impl ToolExecutor for ConfigTool {
    fn name(&self) -> &str {
        "Config"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "setting": {
                    "type": "string",
                    "description": "The setting key (e.g., \"theme\", \"model\", \"permissions.defaultMode\")"
                },
                "value": {
                    "description": "The new value. Omit to get the current value."
                }
            },
            "required": ["setting"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let setting = input["setting"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'setting' field"))?;

        // Find the setting definition
        let def = match SUPPORTED_SETTINGS.iter().find(|s| s.key == setting) {
            Some(d) => d,
            None => {
                let known: Vec<&str> = SUPPORTED_SETTINGS.iter().map(|s| s.key).collect();
                return Ok(ToolResultData {
                    data: json!({
                        "success": false,
                        "error": format!("Unknown setting: \"{}\". Supported: {}", setting, known.join(", ")),
                    }),
                    is_error: false,
                });
            }
        };

        // GET operation
        if input.get("value").is_none() || input["value"].is_null() {
            let config_path = config_file_path()?;
            let current = read_setting_from_file(&config_path, setting).await;
            return Ok(ToolResultData {
                data: json!({
                    "success": true,
                    "operation": "get",
                    "setting": setting,
                    "value": current,
                    "description": def.description,
                }),
                is_error: false,
            });
        }

        // SET operation
        let value = &input["value"];

        // Validate boolean settings
        let final_value = match def.setting_type {
            SettingType::BoolType => {
                match value {
                    Value::Bool(b) => Value::Bool(*b),
                    Value::String(s) => {
                        let lower = s.to_lowercase();
                        match lower.as_str() {
                            "true" => Value::Bool(true),
                            "false" => Value::Bool(false),
                            _ => {
                                return Ok(ToolResultData {
                                    data: json!({
                                        "success": false,
                                        "operation": "set",
                                        "setting": setting,
                                        "error": format!("{} requires true or false.", setting),
                                    }),
                                    is_error: false,
                                });
                            }
                        }
                    }
                    _ => {
                        return Ok(ToolResultData {
                            data: json!({
                                "success": false,
                                "operation": "set",
                                "setting": setting,
                                "error": format!("{} requires true or false.", setting),
                            }),
                            is_error: false,
                        });
                    }
                }
            }
            SettingType::StringType => {
                let s = value.as_str().map(|s| s.to_string()).unwrap_or_else(|| value.to_string());
                Value::String(s)
            }
        };

        // Check valid options
        if let Some(options) = def.options {
            let val_str = match &final_value {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                other => other.to_string(),
            };
            if !options.iter().any(|o| *o == val_str) {
                return Ok(ToolResultData {
                    data: json!({
                        "success": false,
                        "operation": "set",
                        "setting": setting,
                        "error": format!(
                            "Invalid value \"{}\". Options: {}",
                            val_str,
                            options.join(", ")
                        ),
                    }),
                    is_error: false,
                });
            }
        }

        // Read existing config, update, and write back
        let config_path = config_file_path()?;
        let previous = read_setting_from_file(&config_path, setting).await;

        if let Err(e) = write_setting_to_file(&config_path, setting, &final_value).await {
            return Ok(ToolResultData {
                data: json!({
                    "success": false,
                    "operation": "set",
                    "setting": setting,
                    "error": format!("Failed to save setting: {}", e),
                }),
                is_error: true,
            });
        }

        Ok(ToolResultData {
            data: json!({
                "success": true,
                "operation": "set",
                "setting": setting,
                "previousValue": previous,
                "newValue": final_value,
            }),
            is_error: false,
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn is_read_only(&self, input: &Value) -> bool {
        input.get("value").is_none() || input["value"].is_null()
    }
}

/// Get the path to the global config file.
fn config_file_path() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    Ok(home.join(".claude-omni").join("settings.json"))
}

/// Read a setting value from the config file.
async fn read_setting_from_file(path: &std::path::Path, setting: &str) -> Value {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return Value::Null,
    };
    let config: Value = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(_) => return Value::Null,
    };
    navigate_json(&config, setting)
}

/// Write a setting value to the config file.
async fn write_setting_to_file(
    path: &std::path::Path,
    setting: &str,
    value: &Value,
) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Read existing config or start fresh
    let content = tokio::fs::read_to_string(path).await.unwrap_or_else(|_| "{}".to_string());
    let mut config: Value = serde_json::from_str(&content).unwrap_or(json!({}));

    // Navigate and set the value
    let parts: Vec<&str> = setting.split('.').collect();
    set_nested_value(&mut config, &parts, value.clone());

    let output = serde_json::to_string_pretty(&config)?;
    tokio::fs::write(path, output).await?;

    Ok(())
}

/// Navigate a JSON value by dot-separated path.
fn navigate_json(value: &Value, path: &str) -> Value {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;
    for part in parts {
        match current.get(part) {
            Some(v) => current = v,
            None => return Value::Null,
        }
    }
    current.clone()
}

/// Set a value at a dot-separated path in a JSON object.
fn set_nested_value(root: &mut Value, path: &[&str], value: Value) {
    if path.is_empty() {
        return;
    }
    if path.len() == 1 {
        if let Value::Object(map) = root {
            map.insert(path[0].to_string(), value);
        }
        return;
    }
    if let Value::Object(map) = root {
        let child = map
            .entry(path[0].to_string())
            .or_insert_with(|| json!({}));
        set_nested_value(child, &path[1..], value);
    }
}
