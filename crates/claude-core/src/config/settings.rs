use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single permission rule referencing a tool, with an optional glob pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PermissionRuleConfig {
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

/// Allow/deny/ask lists of permission rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct SettingsPermissions {
    pub allow: Vec<PermissionRuleConfig>,
    pub deny: Vec<PermissionRuleConfig>,
    #[serde(default)]
    pub ask: Vec<PermissionRuleConfig>,
}

/// Attribution settings for commits and PRs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct AttributionSettings {
    /// Attribution text for git commits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Attribution text for PR descriptions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
}

/// Worktree configuration for --worktree flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct WorktreeSettings {
    /// Directories to symlink from main repo to worktrees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_directories: Option<Vec<String>>,
    /// Directories to include via git sparse-checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sparse_paths: Option<Vec<String>>,
}

/// Hook configuration for tool/event triggers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct HooksSettings {
    /// Hooks configuration — maps event names to hook entries.
    #[serde(flatten)]
    pub events: HashMap<String, Vec<HookEntry>>,
}

/// A single hook entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct HookEntry {
    /// Hook type: "command" or "url".
    #[serde(rename = "type")]
    pub hook_type: String,
    /// Shell command for command hooks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// URL for HTTP hooks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Timeout in ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// Custom spinner verb configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpinnerVerbsSettings {
    pub mode: String, // "append" | "replace"
    pub verbs: Vec<String>,
}

/// Status line display configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusLineSettings {
    #[serde(rename = "type")]
    pub status_type: String, // "command"
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub padding: Option<u32>,
}

/// Top-level settings structure matching the original TypeScript `SettingsJson`.
///
/// All fields are optional so that partial configurations can be layered.
/// Uses `camelCase` JSON field names for compatibility with the original format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    // ── Authentication ────────────────────────────────────────────────
    /// Path to a script that outputs authentication values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_helper: Option<String>,

    /// API key override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// Force a specific login method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_login_method: Option<String>,

    /// Organization UUID for OAuth login.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_login_org_uuid: Option<String>,

    // ── Model ─────────────────────────────────────────────────────────
    /// Override the default model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Enterprise allowlist of available models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_models: Option<Vec<String>>,

    /// Model ID override mapping (e.g. Anthropic → Bedrock ARN).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_overrides: Option<HashMap<String, String>>,

    /// Advisor model for server-side advisor tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advisor_model: Option<String>,

    // ── Behavior ──────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbose: Option<bool>,

    /// Maximum tokens for model response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Thinking behavior control.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub always_thinking_enabled: Option<bool>,

    /// Effort level for supported models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_level: Option<String>,

    /// Fast mode toggle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<bool>,

    /// If true, fast mode doesn't persist across sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode_per_session_opt_in: Option<bool>,

    /// Named agent for the main thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,

    /// Output style for assistant responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_style: Option<String>,

    /// Preferred language for responses and voice dictation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Prompt suggestion toggle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_suggestion_enabled: Option<bool>,

    // ── Permissions ───────────────────────────────────────────────────
    pub permissions: SettingsPermissions,

    /// Only use managed-settings permission rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_managed_permission_rules_only: Option<bool>,

    // ── MCP ───────────────────────────────────────────────────────────
    /// Auto-approve all project MCP servers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_all_project_mcp_servers: Option<bool>,

    /// Approved MCP servers from .mcp.json.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_mcpjson_servers: Option<Vec<String>>,

    /// Rejected MCP servers from .mcp.json.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_mcpjson_servers: Option<Vec<String>>,

    /// Enterprise allowlist of MCP servers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_mcp_servers: Option<Vec<Value>>,

    /// Enterprise denylist of MCP servers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denied_mcp_servers: Option<Vec<Value>>,

    /// Only read MCP allowlist from managed settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_managed_mcp_servers_only: Option<bool>,

    // ── Hooks ─────────────────────────────────────────────────────────
    /// Custom commands to run before/after tool executions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks: Option<HashMap<String, Vec<HookEntry>>>,

    /// Disable all hooks and status line execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_all_hooks: Option<bool>,

    /// Only run hooks from managed settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_managed_hooks_only: Option<bool>,

    /// Allowlist of URL patterns for HTTP hooks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_http_hook_urls: Option<Vec<String>>,

    // ── Attribution ───────────────────────────────────────────────────
    /// Attribution text for commits and PRs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<AttributionSettings>,

    /// Deprecated: use `attribution` instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_co_authored_by: Option<bool>,

    /// Include built-in git instructions in system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_git_instructions: Option<bool>,

    // ── File & Git ────────────────────────────────────────────────────
    /// Whether file picker respects .gitignore.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub respect_gitignore: Option<bool>,

    /// Worktree configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeSettings>,

    /// Number of days to retain chat transcripts (0 = no persistence).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_period_days: Option<u32>,

    // ── Environment ───────────────────────────────────────────────────
    /// Environment variables to set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,

    /// Default shell for input-box ! commands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_shell: Option<String>,

    // ── UI ────────────────────────────────────────────────────────────
    /// Status line display configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_line: Option<StatusLineSettings>,

    /// Whether to show tips in the spinner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spinner_tips_enabled: Option<bool>,

    /// Custom spinner verbs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spinner_verbs: Option<SpinnerVerbsSettings>,

    /// Disable syntax highlighting in diffs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syntax_highlighting_disabled: Option<bool>,

    /// Whether /rename updates the terminal tab title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_title_from_rename: Option<bool>,

    /// Show "clear context" option on plan accept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_clear_context_on_plan_accept: Option<bool>,

    /// Skip WebFetch blocklist check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_web_fetch_preflight: Option<bool>,

    /// Company announcements to display at startup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company_announcements: Option<Vec<String>>,

    // ── AWS/GCP ───────────────────────────────────────────────────────
    /// Path to script that exports AWS credentials.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_credential_export: Option<String>,

    /// Path to script that refreshes AWS auth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_auth_refresh: Option<String>,

    /// Command to refresh GCP auth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gcp_auth_refresh: Option<String>,


    // ── Plugins ───────────────────────────────────────────────────────
    /// Enabled plugins.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_plugins: Option<HashMap<String, Value>>,

    // ── Catch-all for forward compatibility ────────────────────────────
    /// Unknown fields are preserved for forward compatibility.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Settings {
    /// Merge `overlay` on top of `self`. Fields present in `overlay` win;
    /// fields absent in `overlay` fall back to `self`.
    ///
    /// For collections (permissions allow/deny, env, hooks), overlay replaces
    /// self when non-empty. Arrays like `allowedMcpServers` are merged
    /// (concatenated then deduped).
    pub fn merge(&self, overlay: &Settings) -> Settings {
        // Use serde to do a JSON-level deep merge, which handles all fields
        // including new ones without requiring manual field-by-field logic.
        let base = serde_json::to_value(self).unwrap_or_default();
        let over = serde_json::to_value(overlay).unwrap_or_default();
        let merged = json_merge(&base, &over);
        serde_json::from_value(merged).unwrap_or_default()
    }
}

/// JSON-level merge: overlay values replace base values; objects are merged
/// recursively; null/absent fields in overlay don't overwrite base.
fn json_merge(base: &Value, overlay: &Value) -> Value {
    match (base, overlay) {
        (Value::Object(b), Value::Object(o)) => {
            let mut result = b.clone();
            for (key, ov) in o {
                if ov.is_null() {
                    continue;
                }
                let merged = if let Some(bv) = b.get(key) {
                    json_merge(bv, ov)
                } else {
                    ov.clone()
                };
                result.insert(key.clone(), merged);
            }
            Value::Object(result)
        }
        (_, ov) if !ov.is_null() => ov.clone(),
        _ => base.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_default() {
        let s = Settings::default();
        assert!(s.model.is_none());
        assert!(s.permissions.allow.is_empty());
    }

    #[test]
    fn test_settings_merge_basic() {
        let base = Settings {
            model: Some("opus".to_string()),
            verbose: Some(false),
            ..Default::default()
        };
        let overlay = Settings {
            model: Some("sonnet".to_string()),
            ..Default::default()
        };
        let merged = base.merge(&overlay);
        assert_eq!(merged.model, Some("sonnet".to_string()));
        assert_eq!(merged.verbose, Some(false));
    }

    #[test]
    fn test_settings_camelcase_json() {
        let json = r#"{"apiKeyHelper":"script.sh","enableAllProjectMcpServers":true}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.api_key_helper, Some("script.sh".to_string()));
        assert_eq!(settings.enable_all_project_mcp_servers, Some(true));
    }

    #[test]
    fn test_settings_roundtrip() {
        let settings = Settings {
            model: Some("claude-opus-4-6".to_string()),
            fast_mode: Some(true),
            language: Some("japanese".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let parsed: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model, settings.model);
        assert_eq!(parsed.fast_mode, settings.fast_mode);
        assert_eq!(parsed.language, settings.language);
    }

    #[test]
    fn test_settings_extra_fields_preserved() {
        let json = r#"{"model":"opus","futureField":"hello","nestedFuture":{"a":1}}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.model, Some("opus".to_string()));
        assert_eq!(settings.extra.get("futureField").unwrap(), "hello");
    }

    #[test]
    fn test_permissions_with_ask() {
        let json = r#"{"permissions":{"allow":[{"tool":"Bash"}],"deny":[],"ask":[{"tool":"Edit","pattern":"*.rs"}]}}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.permissions.allow.len(), 1);
        assert_eq!(settings.permissions.ask.len(), 1);
        assert_eq!(settings.permissions.ask[0].tool, "Edit");
    }
}
