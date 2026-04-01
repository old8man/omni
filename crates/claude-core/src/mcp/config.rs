use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::types::{ConfigScope, McpServerConfig, ScopedMcpServerConfig};
use crate::config::paths::claude_dir;

/// Top-level MCP configuration: a collection of named server configs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    pub servers: HashMap<String, ScopedMcpServerConfig>,
}

/// On-disk format of `~/.claude/mcp.json` and `.mcp.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpJsonFile {
    #[serde(default)]
    mcp_servers: HashMap<String, McpServerConfig>,
}

/// Load and merge MCP configuration from user and project scopes.
pub fn load_mcp_config(project_root: &Path) -> Result<McpConfig> {
    let mut merged = McpConfig::default();

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

    let project_path = project_root.join(".mcp.json");
    if project_path.is_file() {
        match load_mcp_json_file(&project_path) {
            Ok(file) => {
                debug!("loaded project MCP config from {}", project_path.display());
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
        let expanded = expand_env_in_config(raw);
        config.servers.insert(
            name,
            ScopedMcpServerConfig {
                config: expanded,
                scope: scope.clone(),
            },
        );
    }
}

fn expand_env_in_config(mut cfg: McpServerConfig) -> McpServerConfig {
    if let Some(ref mut cmd) = cfg.command {
        *cmd = expand_env_vars(cmd);
    }
    cfg.args = cfg.args.into_iter().map(|a| expand_env_vars(&a)).collect();
    if let Some(ref mut url) = cfg.url {
        *url = expand_env_vars(url);
    }
    cfg.env = cfg
        .env
        .into_iter()
        .map(|(k, v)| (k, expand_env_vars(&v)))
        .collect();
    cfg.headers = cfg
        .headers
        .into_iter()
        .map(|(k, v)| (k, expand_env_vars(&v)))
        .collect();
    cfg
}

/// Expand `$VAR`, `${VAR}`, and `${VAR:-default}` references in a string.
fn expand_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' {
            let braced = chars.peek() == Some(&'{');
            if braced {
                chars.next();
            }
            let mut var_content = String::new();
            while let Some(&c) = chars.peek() {
                if braced {
                    if c == '}' {
                        chars.next();
                        break;
                    }
                } else if !c.is_alphanumeric() && c != '_' {
                    break;
                }
                var_content.push(c);
                chars.next();
            }
            if !var_content.is_empty() {
                if braced {
                    // Support ${VAR:-default} syntax
                    if let Some((var_name, default_val)) = var_content.split_once(":-") {
                        match std::env::var(var_name) {
                            Ok(val) => result.push_str(&val),
                            Err(_) => result.push_str(default_val),
                        }
                    } else if let Ok(val) = std::env::var(&var_content) {
                        result.push_str(&val);
                    }
                } else if let Ok(val) = std::env::var(&var_content) {
                    result.push_str(&val);
                }
            } else if braced {
                result.push_str("${}");
            } else {
                result.push('$');
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Normalize a server name for use in MCP tool name prefixes.
pub fn normalize_name_for_mcp(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Build the canonical MCP tool name: `mcp__<server>__<tool>`.
pub fn build_mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    let s = normalize_name_for_mcp(server_name);
    let t = normalize_name_for_mcp(tool_name);
    format!("mcp__{s}__{t}")
}

/// Parse an MCP tool name back into `(server_name, tool_name)`.
pub fn parse_mcp_tool_name(full_name: &str) -> Option<(String, String)> {
    let rest = full_name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server.to_string(), tool.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env_vars_basic() {
        std::env::set_var("TEST_MCP_VAR", "hello");
        assert_eq!(expand_env_vars("$TEST_MCP_VAR"), "hello");
        assert_eq!(expand_env_vars("${TEST_MCP_VAR}"), "hello");
        std::env::remove_var("TEST_MCP_VAR");
    }

    #[test]
    fn test_expand_env_vars_default_value() {
        std::env::remove_var("TEST_MCP_MISSING");
        assert_eq!(expand_env_vars("${TEST_MCP_MISSING:-fallback}"), "fallback");
        std::env::set_var("TEST_MCP_PRESENT", "real");
        assert_eq!(expand_env_vars("${TEST_MCP_PRESENT:-fallback}"), "real");
        std::env::remove_var("TEST_MCP_PRESENT");
    }

    #[test]
    fn test_normalize_name() {
        assert_eq!(normalize_name_for_mcp("my-server"), "my_server");
        assert_eq!(normalize_name_for_mcp("simple"), "simple");
    }

    #[test]
    fn test_build_parse_tool_name() {
        let name = build_mcp_tool_name("my-server", "read_file");
        assert_eq!(name, "mcp__my_server__read_file");
        let (s, t) = parse_mcp_tool_name(&name).unwrap();
        assert_eq!(s, "my_server");
        assert_eq!(t, "read_file");
    }
}
