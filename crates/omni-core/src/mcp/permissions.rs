use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::types::McpServerConfig;

// ---------------------------------------------------------------------------
// Policy entries (mirror the TS isMcpServerNameEntry / CommandEntry / UrlEntry)
// ---------------------------------------------------------------------------

/// An entry in an allowlist or denylist for MCP servers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerPolicyEntry {
    ByName { server_name: String },
    ByCommand { server_command: Vec<String> },
    ByUrl { server_url: String },
}

/// Policy settings that govern which MCP servers are permitted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPolicySettings {
    /// If present, only servers matching these entries are allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_mcp_servers: Option<Vec<McpServerPolicyEntry>>,

    /// Servers matching these entries are unconditionally blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied_mcp_servers: Option<Vec<McpServerPolicyEntry>>,

    /// When true, only managed (enterprise) servers are allowed.
    #[serde(default)]
    pub allow_managed_mcp_servers_only: bool,
}

// ---------------------------------------------------------------------------
// Channel permission relay
// ---------------------------------------------------------------------------

/// The response received from a channel when a permission prompt is relayed.
#[derive(Debug, Clone)]
pub struct ChannelPermissionResponse {
    pub behavior: PermissionBehavior,
    /// Which channel server the reply came from.
    pub from_server: String,
}

/// Whether a permission request was allowed or denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionBehavior {
    Allow,
    Deny,
}

impl std::fmt::Display for PermissionBehavior {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => write!(f, "allow"),
            Self::Deny => write!(f, "deny"),
        }
    }
}

/// Manages pending channel permission requests.
///
/// When a tool-use requires user approval, the system can relay the prompt
/// to channel servers (e.g. Telegram, Discord). This struct holds the
/// pending callbacks and resolves them when a response arrives.
pub struct ChannelPermissionCallbacks {
    pending: HashMap<String, tokio::sync::oneshot::Sender<ChannelPermissionResponse>>,
}

impl ChannelPermissionCallbacks {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Register a resolver for a permission request.
    ///
    /// Returns a receiver that will yield the response.
    pub fn on_response(
        &mut self,
        request_id: &str,
    ) -> tokio::sync::oneshot::Receiver<ChannelPermissionResponse> {
        let key = request_id.to_lowercase();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.insert(key, tx);
        rx
    }

    /// Cancel a pending request (e.g. on timeout).
    pub fn cancel(&mut self, request_id: &str) {
        self.pending.remove(&request_id.to_lowercase());
    }

    /// Resolve a pending request from a channel server notification.
    ///
    /// Returns `true` if the request_id was pending and the response was
    /// delivered.
    pub fn resolve(
        &mut self,
        request_id: &str,
        behavior: PermissionBehavior,
        from_server: &str,
    ) -> bool {
        let key = request_id.to_lowercase();
        if let Some(sender) = self.pending.remove(&key) {
            let _ = sender.send(ChannelPermissionResponse {
                behavior,
                from_server: from_server.to_string(),
            });
            true
        } else {
            false
        }
    }

    /// Number of pending requests.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Default for ChannelPermissionCallbacks {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Short request ID generation (for channel permission prompts)
// ---------------------------------------------------------------------------

/// Alphabet for short request IDs: a-z minus 'l' (looks like 1/I).
const ID_ALPHABET: &[u8; 25] = b"abcdefghijkmnopqrstuvwxyz";

/// Substring blocklist for generated IDs.
const ID_AVOID_SUBSTRINGS: &[&str] = &[
    "fuck", "shit", "cunt", "cock", "dick", "twat", "piss", "crap", "ass", "tit", "cum", "fag",
    "nig", "kike", "rape", "nazi", "damn", "poo", "pee", "wank", "anus",
];

/// FNV-1a hash to a 5-letter ID.
fn hash_to_id(input: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in input.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let mut s = String::with_capacity(5);
    for _ in 0..5 {
        s.push(ID_ALPHABET[(h % 25) as usize] as char);
        h /= 25;
    }
    s
}

/// Generate a short (5-letter) request ID from a tool-use ID.
///
/// Uses FNV-1a hashing into a 25-letter alphabet (a-z minus 'l').
/// Re-hashes with a salt if the result contains a blocklisted substring.
pub fn short_request_id(tool_use_id: &str) -> String {
    let mut candidate = hash_to_id(tool_use_id);
    for salt in 0..10 {
        if !ID_AVOID_SUBSTRINGS.iter().any(|bad| candidate.contains(bad)) {
            return candidate;
        }
        candidate = hash_to_id(&format!("{tool_use_id}:{salt}"));
    }
    candidate
}

// ---------------------------------------------------------------------------
// Policy evaluation
// ---------------------------------------------------------------------------

/// Check if an MCP server is denied by policy.
pub fn is_server_denied(
    server_name: &str,
    config: Option<&McpServerConfig>,
    policy: &McpPolicySettings,
) -> bool {
    let denied = match &policy.denied_mcp_servers {
        Some(entries) => entries,
        None => return false,
    };

    for entry in denied {
        match entry {
            McpServerPolicyEntry::ByName { server_name: name } if name == server_name => {
                return true
            }
            McpServerPolicyEntry::ByCommand { server_command } => {
                if let Some(cfg) = config {
                    if let Some(ref cmd) = cfg.command {
                        let actual: Vec<&str> =
                            std::iter::once(cmd.as_str()).chain(cfg.args.iter().map(|s| s.as_str())).collect();
                        let expected: Vec<&str> = server_command.iter().map(|s| s.as_str()).collect();
                        if actual == expected {
                            return true;
                        }
                    }
                }
            }
            McpServerPolicyEntry::ByUrl { server_url: pattern } => {
                if let Some(cfg) = config {
                    if let Some(ref url) = cfg.url {
                        if url_matches_pattern(url, pattern) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    false
}

/// Check if an MCP server is allowed by policy.
///
/// Returns `true` if the server passes both the denylist and allowlist checks.
pub fn is_server_allowed(
    server_name: &str,
    config: Option<&McpServerConfig>,
    policy: &McpPolicySettings,
) -> bool {
    // Denylist takes absolute precedence.
    if is_server_denied(server_name, config, policy) {
        return false;
    }

    let allowed = match &policy.allowed_mcp_servers {
        Some(entries) => entries,
        None => return true, // No allowlist = everything allowed.
    };

    if allowed.is_empty() {
        return false; // Empty allowlist = nothing allowed.
    }

    let has_command_entries = allowed.iter().any(|e| matches!(e, McpServerPolicyEntry::ByCommand { .. }));
    let has_url_entries = allowed.iter().any(|e| matches!(e, McpServerPolicyEntry::ByUrl { .. }));

    if let Some(cfg) = config {
        // Stdio server
        if let Some(ref cmd) = cfg.command {
            if has_command_entries {
                let actual: Vec<&str> =
                    std::iter::once(cmd.as_str()).chain(cfg.args.iter().map(|s| s.as_str())).collect();
                return allowed.iter().any(|entry| {
                    if let McpServerPolicyEntry::ByCommand { server_command } = entry {
                        let expected: Vec<&str> = server_command.iter().map(|s| s.as_str()).collect();
                        actual == expected
                    } else {
                        false
                    }
                });
            }
        }
        // Remote server
        if let Some(ref url) = cfg.url {
            if has_url_entries {
                return allowed.iter().any(|entry| {
                    if let McpServerPolicyEntry::ByUrl { server_url: pattern } = entry {
                        url_matches_pattern(url, pattern)
                    } else {
                        false
                    }
                });
            }
        }
    }

    // Fall back to name-based matching.
    allowed.iter().any(|entry| {
        matches!(entry, McpServerPolicyEntry::ByName { server_name: name } if name == server_name)
    })
}

/// Filter a set of MCP server configs by policy.
///
/// Returns the allowed configs and the names of blocked ones.
pub fn filter_servers_by_policy<V: Clone>(
    configs: &HashMap<String, V>,
    config_accessor: impl Fn(&V) -> Option<&McpServerConfig>,
    policy: &McpPolicySettings,
) -> (HashMap<String, V>, Vec<String>) {
    let mut allowed = HashMap::new();
    let mut blocked = Vec::new();

    for (name, value) in configs {
        let cfg = config_accessor(value);
        if is_server_allowed(name, cfg, policy) {
            allowed.insert(name.clone(), value.clone());
        } else {
            debug!(server = %name, "MCP server blocked by policy");
            blocked.push(name.clone());
        }
    }

    (allowed, blocked)
}

// ---------------------------------------------------------------------------
// Per-server tool restrictions
// ---------------------------------------------------------------------------

/// Per-server tool restriction policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpToolRestrictions {
    /// Map from server name to the set of allowed tool names.
    /// If a server is present in this map, only tools in the set are permitted.
    /// If a server is absent, all its tools are allowed.
    #[serde(default)]
    pub server_allowed_tools: HashMap<String, HashSet<String>>,
}

impl McpToolRestrictions {
    /// Check whether a given tool from a given server is allowed.
    pub fn is_tool_allowed(&self, server_name: &str, tool_name: &str) -> bool {
        match self.server_allowed_tools.get(server_name) {
            Some(allowed) => allowed.contains(tool_name),
            None => true, // No restrictions for this server.
        }
    }
}

// ---------------------------------------------------------------------------
// Channel allowlist
// ---------------------------------------------------------------------------

/// An entry on the channel plugin allowlist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelAllowlistEntry {
    pub marketplace: String,
    pub plugin: String,
}

/// Check whether a plugin source string is on the channel allowlist.
pub fn is_channel_allowlisted(
    plugin_source: Option<&str>,
    allowlist: &[ChannelAllowlistEntry],
) -> bool {
    let source = match plugin_source {
        Some(s) => s,
        None => return false,
    };

    // Parse "name@marketplace" format.
    let (name, marketplace) = match source.rsplit_once('@') {
        Some((n, m)) => (n, m),
        None => return false, // No marketplace = can't match.
    };

    allowlist
        .iter()
        .any(|e| e.plugin == name && e.marketplace == marketplace)
}

// ---------------------------------------------------------------------------
// URL pattern matching
// ---------------------------------------------------------------------------

/// Match a URL against a glob-style pattern (supports `*` wildcard).
fn url_matches_pattern(url: &str, pattern: &str) -> bool {
    let escaped = regex::escape(pattern);
    let regex_str = format!("^{}$", escaped.replace(r"\*", ".*"));
    match regex::Regex::new(&regex_str) {
        Ok(re) => re.is_match(url),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_request_id() {
        let id = short_request_id("toolu_abc123");
        assert_eq!(id.len(), 5);
        assert!(id.chars().all(|c| c.is_ascii_lowercase() && c != 'l'));
    }

    #[test]
    fn test_short_request_id_deterministic() {
        let a = short_request_id("toolu_xyz");
        let b = short_request_id("toolu_xyz");
        assert_eq!(a, b);
    }

    #[test]
    fn test_url_pattern_matching() {
        assert!(url_matches_pattern(
            "https://api.example.com/path",
            "https://api.example.com/*"
        ));
        assert!(url_matches_pattern(
            "https://foo.example.com/bar",
            "https://*.example.com/*"
        ));
        assert!(!url_matches_pattern(
            "https://other.com/path",
            "https://api.example.com/*"
        ));
    }

    #[test]
    fn test_server_denied_by_name() {
        let policy = McpPolicySettings {
            denied_mcp_servers: Some(vec![McpServerPolicyEntry::ByName {
                server_name: "evil".into(),
            }]),
            ..Default::default()
        };
        assert!(is_server_denied("evil", None, &policy));
        assert!(!is_server_denied("good", None, &policy));
    }

    #[test]
    fn test_server_allowed_with_no_policy() {
        let policy = McpPolicySettings::default();
        assert!(is_server_allowed("anything", None, &policy));
    }

    #[test]
    fn test_server_blocked_by_empty_allowlist() {
        let policy = McpPolicySettings {
            allowed_mcp_servers: Some(vec![]),
            ..Default::default()
        };
        assert!(!is_server_allowed("anything", None, &policy));
    }

    #[test]
    fn test_channel_allowlist() {
        let list = vec![ChannelAllowlistEntry {
            marketplace: "anthropic".into(),
            plugin: "telegram".into(),
        }];
        assert!(is_channel_allowlisted(
            Some("telegram@anthropic"),
            &list
        ));
        assert!(!is_channel_allowlisted(
            Some("discord@anthropic"),
            &list
        ));
        assert!(!is_channel_allowlisted(None, &list));
        assert!(!is_channel_allowlisted(Some("telegram"), &list));
    }

    #[test]
    fn test_tool_restrictions() {
        let mut restrictions = McpToolRestrictions::default();
        restrictions.server_allowed_tools.insert(
            "locked-server".into(),
            HashSet::from(["read".into(), "list".into()]),
        );
        assert!(restrictions.is_tool_allowed("locked-server", "read"));
        assert!(!restrictions.is_tool_allowed("locked-server", "write"));
        assert!(restrictions.is_tool_allowed("open-server", "anything"));
    }

    #[test]
    fn test_channel_permission_callbacks() {
        let mut cbs = ChannelPermissionCallbacks::new();
        let mut rx = cbs.on_response("abc");
        assert_eq!(cbs.pending_count(), 1);

        let resolved = cbs.resolve("ABC", PermissionBehavior::Allow, "telegram");
        assert!(resolved);
        assert_eq!(cbs.pending_count(), 0);

        let resp = rx.try_recv().unwrap();
        assert_eq!(resp.behavior, PermissionBehavior::Allow);
        assert_eq!(resp.from_server, "telegram");
    }

    #[test]
    fn test_channel_permission_duplicate_resolve() {
        let mut cbs = ChannelPermissionCallbacks::new();
        let _rx = cbs.on_response("abc");
        assert!(cbs.resolve("abc", PermissionBehavior::Allow, "tg"));
        // Second resolve should return false (already consumed).
        assert!(!cbs.resolve("abc", PermissionBehavior::Deny, "tg"));
    }
}
