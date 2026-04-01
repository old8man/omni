use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use super::env_expansion::expand_env_in_server_config;
use super::types::{ConfigScope, McpServerConfig, McpTransportType, ScopedMcpServerConfig};
use crate::config::paths::claude_dir;

// ---------------------------------------------------------------------------
// McpConfig
// ---------------------------------------------------------------------------

/// Top-level MCP configuration: a collection of named server configs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    pub servers: HashMap<String, ScopedMcpServerConfig>,
}

// ---------------------------------------------------------------------------
// On-disk config format
// ---------------------------------------------------------------------------

/// On-disk format of `~/.claude/mcp.json` and `.mcp.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpJsonFile {
    #[serde(default)]
    mcp_servers: HashMap<String, McpServerConfig>,
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load and merge MCP configuration from all available scopes:
///   1. Enterprise managed config
///   2. User config (~/.claude/mcp.json)
///   3. Project config (.mcp.json)
///   4. Local dynamic configs
///
/// Later scopes override earlier ones for the same server name.
pub fn load_mcp_config(project_root: &Path) -> Result<McpConfig> {
    let mut merged = McpConfig::default();

    // Enterprise managed config.
    if let Some(path) = enterprise_mcp_path() {
        if path.is_file() {
            match load_mcp_json_file(&path) {
                Ok(file) => {
                    debug!("loaded enterprise MCP config from {}", path.display());
                    add_servers(&mut merged, file.mcp_servers, ConfigScope::Enterprise);
                }
                Err(e) => warn!(
                    "failed to parse enterprise MCP config {}: {e}",
                    path.display()
                ),
            }
        }
    }

    // User config.
    if let Ok(dir) = claude_dir() {
        let user_path = dir.join("mcp.json");
        if user_path.is_file() {
            match load_mcp_json_file(&user_path) {
                Ok(file) => {
                    debug!("loaded user MCP config from {}", user_path.display());
                    add_servers(&mut merged, file.mcp_servers, ConfigScope::User);
                }
                Err(e) => warn!(
                    "failed to parse user MCP config {}: {e}",
                    user_path.display()
                ),
            }
        }
    }

    // Project config.
    let project_path = project_root.join(".mcp.json");
    if project_path.is_file() {
        match load_mcp_json_file(&project_path) {
            Ok(file) => {
                debug!(
                    "loaded project MCP config from {}",
                    project_path.display()
                );
                add_servers(&mut merged, file.mcp_servers, ConfigScope::Project);
            }
            Err(e) => warn!(
                "failed to parse project MCP config {}: {e}",
                project_path.display()
            ),
        }
    }

    Ok(merged)
}

/// Load MCP config from a specific file path, adding all servers with the
/// given scope.
pub fn load_mcp_config_from_file(
    path: &Path,
    scope: ConfigScope,
) -> Result<HashMap<String, ScopedMcpServerConfig>> {
    let file = load_mcp_json_file(&path.to_path_buf())?;
    let mut result = HashMap::new();
    for (name, raw) in file.mcp_servers {
        let (expanded, missing) = expand_env_in_server_config(&raw);
        if !missing.is_empty() {
            warn!(
                server = %name,
                "MCP server config has undefined env vars: {:?}",
                missing
            );
        }
        result.insert(
            name,
            ScopedMcpServerConfig {
                config: expanded,
                scope: scope.clone(),
            },
        );
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Writing (project .mcp.json)
// ---------------------------------------------------------------------------

/// Write MCP server configs to the project's `.mcp.json` file.
///
/// Preserves file permissions and uses atomic rename.
pub fn write_project_mcp_config(
    project_root: &Path,
    servers: &HashMap<String, McpServerConfig>,
) -> Result<()> {
    let path = project_root.join(".mcp.json");
    let file = McpJsonFile {
        mcp_servers: servers.clone(),
    };
    let json = serde_json::to_string_pretty(&file)?;

    // Atomic write via temp file + rename.
    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, &json).with_context(|| {
        format!("writing temp MCP config to {}", temp_path.display())
    })?;
    std::fs::rename(&temp_path, &path).with_context(|| {
        format!("renaming temp MCP config to {}", path.display())
    })?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Naming helpers
// ---------------------------------------------------------------------------

/// Normalize a server name for use in MCP tool name prefixes.
///
/// Replaces any invalid character with `_`. For claude.ai servers, also
/// collapses consecutive underscores and strips leading/trailing ones.
pub fn normalize_name_for_mcp(name: &str) -> String {
    let mut normalized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Claude.ai servers get extra normalization.
    if name.starts_with("claude.ai ") {
        // Collapse consecutive underscores.
        while normalized.contains("__") {
            normalized = normalized.replace("__", "_");
        }
        normalized = normalized.trim_matches('_').to_string();
    }

    normalized
}

/// Build the canonical MCP tool name: `mcp__<server>__<tool>`.
pub fn build_mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    let s = normalize_name_for_mcp(server_name);
    let t = normalize_name_for_mcp(tool_name);
    format!("mcp__{s}__{t}")
}

/// Parse an MCP tool name back into `(server_name, tool_name)`.
///
/// Known limitation: if a server name contains `__`, parsing will be
/// incorrect (server name gets truncated at the first `__`).
pub fn parse_mcp_tool_name(full_name: &str) -> Option<(String, String)> {
    let rest = full_name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server.to_string(), tool.to_string()))
}

/// Get the `mcp__<server>__` prefix for a given server.
pub fn get_mcp_prefix(server_name: &str) -> String {
    format!("mcp__{}__", normalize_name_for_mcp(server_name))
}

// ---------------------------------------------------------------------------
// Server signatures (for dedup)
// ---------------------------------------------------------------------------

/// Compute a dedup signature for an MCP server config.
///
/// Two configs with the same signature are considered "the same server" for
/// plugin deduplication. Ignores env and headers.
pub fn get_server_signature(config: &McpServerConfig) -> Option<String> {
    if let Some(ref cmd) = config.command {
        let args_json = serde_json::to_string(&config.args).unwrap_or_default();
        return Some(format!("stdio:[\"{cmd}\",{args_json}]"));
    }
    if let Some(ref url) = config.url {
        return Some(format!("url:{url}"));
    }
    None
}

/// Filter plugin MCP servers, dropping any whose signature matches a
/// manually-configured server or an earlier-loaded plugin server.
pub fn dedup_plugin_servers(
    plugin_servers: &HashMap<String, ScopedMcpServerConfig>,
    manual_servers: &HashMap<String, ScopedMcpServerConfig>,
) -> (
    HashMap<String, ScopedMcpServerConfig>,
    Vec<SuppressedServer>,
) {
    let mut manual_sigs: HashMap<String, String> = HashMap::new();
    for (name, scoped) in manual_servers {
        if let Some(sig) = get_server_signature(&scoped.config) {
            manual_sigs.entry(sig).or_insert_with(|| name.clone());
        }
    }

    let mut result = HashMap::new();
    let mut suppressed = Vec::new();
    let mut seen_plugin_sigs: HashMap<String, String> = HashMap::new();

    for (name, config) in plugin_servers {
        let sig = get_server_signature(&config.config);
        if let Some(ref s) = sig {
            if let Some(dup) = manual_sigs.get(s) {
                suppressed.push(SuppressedServer {
                    name: name.clone(),
                    duplicate_of: dup.clone(),
                });
                continue;
            }
            if let Some(dup) = seen_plugin_sigs.get(s) {
                suppressed.push(SuppressedServer {
                    name: name.clone(),
                    duplicate_of: dup.clone(),
                });
                continue;
            }
            seen_plugin_sigs.insert(s.clone(), name.clone());
        }
        result.insert(name.clone(), config.clone());
    }

    (result, suppressed)
}

/// Record of a plugin server that was suppressed during dedup.
#[derive(Debug, Clone)]
pub struct SuppressedServer {
    pub name: String,
    pub duplicate_of: String,
}

// ---------------------------------------------------------------------------
// Config validation
// ---------------------------------------------------------------------------

/// Validate an MCP server config, returning a list of issues.
pub fn validate_server_config(name: &str, config: &McpServerConfig) -> Vec<String> {
    let mut issues = Vec::new();

    match config.transport {
        McpTransportType::Stdio => {
            if config.command.is_none() || config.command.as_deref() == Some("") {
                issues.push(format!("{name}: stdio transport requires a non-empty command"));
            }
        }
        McpTransportType::Sse | McpTransportType::SseIde | McpTransportType::Http | McpTransportType::Ws => {
            if config.url.is_none() || config.url.as_deref() == Some("") {
                issues.push(format!(
                    "{name}: {:?} transport requires a URL",
                    config.transport
                ));
            }
        }
        McpTransportType::Sdk => {
            // SDK servers are managed out-of-band.
        }
    }

    if let Some(ref url) = config.url {
        if url::Url::parse(url).is_err() {
            issues.push(format!("{name}: invalid URL: {url}"));
        }
    }

    if let Some(ref meta_url) = config.oauth.as_ref().and_then(|o| o.auth_server_metadata_url.clone()) {
        if !meta_url.starts_with("https://") {
            issues.push(format!(
                "{name}: authServerMetadataUrl must use https:// (got: {meta_url})"
            ));
        }
    }

    issues
}

/// Validate all servers in a config, returning all issues found.
pub fn validate_config(config: &McpConfig) -> Vec<String> {
    let mut all_issues = Vec::new();
    for (name, scoped) in &config.servers {
        all_issues.extend(validate_server_config(name, &scoped.config));
    }
    all_issues
}

// ---------------------------------------------------------------------------
// Headers helper
// ---------------------------------------------------------------------------

/// Execute a headers helper command and parse the JSON output as headers.
///
/// The command receives the server name and URL via environment variables
/// so a single script can serve multiple MCP servers.
pub async fn get_headers_from_helper(
    server_name: &str,
    helper_command: &str,
    server_url: Option<&str>,
) -> Result<HashMap<String, String>> {
    use tokio::process::Command;

    let mut cmd = Command::new("sh");
    cmd.args(["-c", helper_command]);
    cmd.env("CLAUDE_CODE_MCP_SERVER_NAME", server_name);
    if let Some(url) = server_url {
        cmd.env("CLAUDE_CODE_MCP_SERVER_URL", url);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        cmd.output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("headersHelper timed out after 10s"))?
    .context("failed to execute headersHelper")?;

    if !output.status.success() {
        anyhow::bail!(
            "headersHelper exited with status {}",
            output.status
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let headers: HashMap<String, String> =
        serde_json::from_str(stdout.trim()).context("headersHelper must return a JSON object")?;

    // Validate all values are strings.
    for (key, value) in &headers {
        if value.is_empty() {
            warn!("headersHelper returned empty value for key {key}");
        }
    }

    debug!(
        server = %server_name,
        count = headers.len(),
        "headersHelper returned headers"
    );
    Ok(headers)
}

/// Get combined headers for an MCP server (static from config + dynamic
/// from headersHelper). Dynamic headers override static ones.
pub async fn get_server_headers(
    server_name: &str,
    config: &McpServerConfig,
) -> HashMap<String, String> {
    let mut headers = config.headers.clone();

    if let Some(ref helper) = config.headers_helper {
        match get_headers_from_helper(server_name, helper, config.url.as_deref()).await {
            Ok(dynamic) => {
                for (k, v) in dynamic {
                    headers.insert(k, v);
                }
            }
            Err(e) => {
                warn!(
                    server = %server_name,
                    "headersHelper failed: {e:#}"
                );
            }
        }
    }

    headers
}

// ---------------------------------------------------------------------------
// Config hashing (for change detection)
// ---------------------------------------------------------------------------

/// Stable hash of an MCP server config for change detection.
///
/// Excludes `scope` so that moving a server between config files doesn't
/// trigger a reconnect.
pub fn hash_server_config(config: &ScopedMcpServerConfig) -> String {
    let json = serde_json::to_string(&config.config).unwrap_or_default();
    let hash = Sha256::digest(json.as_bytes());
    hex::encode(&hash[..8])
}

// ---------------------------------------------------------------------------
// Scope/path helpers
// ---------------------------------------------------------------------------

/// Path to the enterprise managed MCP config file.
pub fn enterprise_mcp_path() -> Option<PathBuf> {
    claude_dir()
        .ok()
        .map(|d| d.join("managed").join("managed-mcp.json"))
}

/// Describe the file path for a given config scope.
pub fn describe_config_path(scope: &ConfigScope, project_root: &Path) -> String {
    match scope {
        ConfigScope::User => claude_dir()
            .map(|d| d.join("mcp.json").display().to_string())
            .unwrap_or_else(|_| "~/.claude/mcp.json".to_string()),
        ConfigScope::Project => project_root.join(".mcp.json").display().to_string(),
        ConfigScope::Local => format!(
            "{} [project: {}]",
            claude_dir()
                .map(|d| d.join("mcp.json").display().to_string())
                .unwrap_or_else(|_| "~/.claude/mcp.json".to_string()),
            project_root.display()
        ),
        ConfigScope::Dynamic => "Dynamically configured".to_string(),
        ConfigScope::Enterprise => enterprise_mcp_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "enterprise config".to_string()),
        ConfigScope::ClaudeAi => "claude.ai".to_string(),
        ConfigScope::Managed => "managed config".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn load_mcp_json_file(path: &PathBuf) -> Result<McpJsonFile> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let file: McpJsonFile =
        serde_json::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;
    Ok(file)
}

fn add_servers(
    config: &mut McpConfig,
    servers: HashMap<String, McpServerConfig>,
    scope: ConfigScope,
) {
    for (name, raw) in servers {
        let (expanded, missing) = expand_env_in_server_config(&raw);
        if !missing.is_empty() {
            warn!(
                server = %name,
                "MCP config has undefined env vars: {:?}",
                missing
            );
        }
        config.servers.insert(
            name,
            ScopedMcpServerConfig {
                config: expanded,
                scope: scope.clone(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_name() {
        assert_eq!(normalize_name_for_mcp("my-server"), "my-server");
        assert_eq!(normalize_name_for_mcp("my server"), "my_server");
        assert_eq!(normalize_name_for_mcp("simple"), "simple");
        assert_eq!(normalize_name_for_mcp("dots.and.stuff"), "dots_and_stuff");
    }

    #[test]
    fn test_normalize_claudeai_name() {
        assert_eq!(
            normalize_name_for_mcp("claude.ai My Server"),
            "claude_ai_My_Server"
        );
    }

    #[test]
    fn test_build_parse_tool_name() {
        let name = build_mcp_tool_name("my-server", "read_file");
        assert_eq!(name, "mcp__my-server__read_file");
        let (s, t) = parse_mcp_tool_name(&name).unwrap();
        assert_eq!(s, "my-server");
        assert_eq!(t, "read_file");
    }

    #[test]
    fn test_parse_invalid_tool_names() {
        assert!(parse_mcp_tool_name("not_mcp").is_none());
        assert!(parse_mcp_tool_name("mcp__").is_none());
        assert!(parse_mcp_tool_name("mcp____").is_none());
    }

    #[test]
    fn test_get_mcp_prefix() {
        assert_eq!(get_mcp_prefix("github"), "mcp__github__");
    }

    #[test]
    fn test_server_signature_stdio() {
        let config = McpServerConfig {
            command: Some("npx".into()),
            args: vec!["@modelcontextprotocol/server-github".into()],
            ..Default::default()
        };
        let sig = get_server_signature(&config).unwrap();
        assert!(sig.starts_with("stdio:"));
    }

    #[test]
    fn test_server_signature_url() {
        let config = McpServerConfig {
            transport: McpTransportType::Http,
            url: Some("https://example.com/mcp".into()),
            ..Default::default()
        };
        let sig = get_server_signature(&config).unwrap();
        assert_eq!(sig, "url:https://example.com/mcp");
    }

    #[test]
    fn test_validate_stdio_no_command() {
        let config = McpServerConfig {
            transport: McpTransportType::Stdio,
            ..Default::default()
        };
        let issues = validate_server_config("test", &config);
        assert!(!issues.is_empty());
    }

    #[test]
    fn test_validate_http_no_url() {
        let config = McpServerConfig {
            transport: McpTransportType::Http,
            ..Default::default()
        };
        let issues = validate_server_config("test", &config);
        assert!(!issues.is_empty());
    }

    #[test]
    fn test_validate_valid_config() {
        let config = McpServerConfig {
            transport: McpTransportType::Stdio,
            command: Some("node".into()),
            args: vec!["server.js".into()],
            ..Default::default()
        };
        let issues = validate_server_config("test", &config);
        assert!(issues.is_empty());
    }

    #[test]
    fn test_hash_server_config() {
        let cfg1 = ScopedMcpServerConfig {
            config: McpServerConfig {
                command: Some("node".into()),
                ..Default::default()
            },
            scope: ConfigScope::User,
        };
        let cfg2 = ScopedMcpServerConfig {
            config: McpServerConfig {
                command: Some("node".into()),
                ..Default::default()
            },
            scope: ConfigScope::Project, // Different scope, same hash.
        };
        assert_eq!(hash_server_config(&cfg1), hash_server_config(&cfg2));

        let cfg3 = ScopedMcpServerConfig {
            config: McpServerConfig {
                command: Some("python".into()),
                ..Default::default()
            },
            scope: ConfigScope::User,
        };
        assert_ne!(hash_server_config(&cfg1), hash_server_config(&cfg3));
    }
}
