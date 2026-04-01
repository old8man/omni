use std::collections::HashMap;

/// Result of expanding environment variables in a string.
#[derive(Debug, Clone)]
pub struct EnvExpansionResult {
    /// The expanded string.
    pub expanded: String,
    /// Names of environment variables that were referenced but not found
    /// (and had no default value).
    pub missing_vars: Vec<String>,
}

/// Expand `${VAR}` and `${VAR:-default}` references in a string.
///
/// Only the `${...}` brace syntax is handled for full fidelity with the
/// TypeScript original.  Bare `$VAR` references (no braces) are left
/// untouched so that they can be forwarded to child processes.
///
/// Missing variables with no default are left as the original `${VAR}`
/// text and recorded in `missing_vars` for diagnostic reporting.
pub fn expand_env_vars_in_string(value: &str) -> EnvExpansionResult {
    expand_env_vars_with_provider(value, |name| std::env::var(name).ok())
}

/// Same as [`expand_env_vars_in_string`] but with a pluggable variable
/// provider (useful for testing without mutating the real environment).
pub fn expand_env_vars_with_provider(
    value: &str,
    provider: impl Fn(&str) -> Option<String>,
) -> EnvExpansionResult {
    let mut missing_vars = Vec::new();
    // Regex: match ${...} blocks (non-greedy content match).
    let expanded = replace_env_refs(value, &provider, &mut missing_vars);
    EnvExpansionResult {
        expanded,
        missing_vars,
    }
}

fn replace_env_refs(
    input: &str,
    provider: &dyn Fn(&str) -> Option<String>,
    missing: &mut Vec<String>,
) -> String {
    let mut result = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'$' && i + 1 < len && bytes[i + 1] == b'{' {
            // Find the matching '}'
            let start = i;
            let content_start = i + 2;
            let mut j = content_start;
            while j < len && bytes[j] != b'}' {
                j += 1;
            }
            if j < len {
                // We found '}'
                let var_content = &input[content_start..j];
                // Split on :- to support default values (limit to 2 parts)
                if let Some((var_name, default_value)) = var_content.split_once(":-") {
                    match provider(var_name) {
                        Some(val) => result.push_str(&val),
                        None => result.push_str(default_value),
                    }
                } else {
                    // No default
                    match provider(var_content) {
                        Some(val) => result.push_str(&val),
                        None => {
                            missing.push(var_content.to_string());
                            // Keep original for debugging
                            result.push_str(&input[start..=j]);
                        }
                    }
                }
                i = j + 1;
            } else {
                // No closing brace, output literally
                result.push('$');
                i += 1;
            }
        } else {
            result.push(input[i..].chars().next().unwrap());
            i += input[i..].chars().next().unwrap().len_utf8();
        }
    }

    result
}

/// Expand environment variables in all string fields of an MCP server config.
///
/// Returns the expanded config plus a deduplicated list of any variable names
/// that were referenced but not found in the environment.
pub fn expand_env_in_server_config(
    config: &super::types::McpServerConfig,
) -> (super::types::McpServerConfig, Vec<String>) {
    let mut all_missing = Vec::new();

    let command = config.command.as_ref().map(|cmd| {
        let r = expand_env_vars_in_string(cmd);
        all_missing.extend(r.missing_vars);
        r.expanded
    });

    let args: Vec<String> = config
        .args
        .iter()
        .map(|a| {
            let r = expand_env_vars_in_string(a);
            all_missing.extend(r.missing_vars);
            r.expanded
        })
        .collect();

    let url = config.url.as_ref().map(|u| {
        let r = expand_env_vars_in_string(u);
        all_missing.extend(r.missing_vars);
        r.expanded
    });

    let env: HashMap<String, String> = config
        .env
        .iter()
        .map(|(k, v)| {
            let r = expand_env_vars_in_string(v);
            all_missing.extend(r.missing_vars);
            (k.clone(), r.expanded)
        })
        .collect();

    let headers: HashMap<String, String> = config
        .headers
        .iter()
        .map(|(k, v)| {
            let r = expand_env_vars_in_string(v);
            all_missing.extend(r.missing_vars);
            (k.clone(), r.expanded)
        })
        .collect();

    // Deduplicate missing vars while preserving order.
    let mut seen = std::collections::HashSet::new();
    all_missing.retain(|v| seen.insert(v.clone()));

    let expanded = super::types::McpServerConfig {
        transport: config.transport.clone(),
        command,
        args,
        env,
        url,
        headers,
        headers_helper: config.headers_helper.clone(),
        ide_name: config.ide_name.clone(),
        oauth: config.oauth.clone(),
    };

    (expanded, all_missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_expansion() {
        let env = |name: &str| -> Option<String> {
            match name {
                "HOME" => Some("/home/user".into()),
                "PORT" => Some("8080".into()),
                _ => None,
            }
        };
        let r = expand_env_vars_with_provider("${HOME}/app:${PORT}", env);
        assert_eq!(r.expanded, "/home/user/app:8080");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn test_default_value() {
        let env = |_: &str| -> Option<String> { None };
        let r = expand_env_vars_with_provider("${MISSING:-fallback}", env);
        assert_eq!(r.expanded, "fallback");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn test_default_overridden_by_env() {
        let env = |name: &str| -> Option<String> {
            if name == "PRESENT" {
                Some("real".into())
            } else {
                None
            }
        };
        let r = expand_env_vars_with_provider("${PRESENT:-fallback}", env);
        assert_eq!(r.expanded, "real");
    }

    #[test]
    fn test_missing_tracked() {
        let env = |_: &str| -> Option<String> { None };
        let r = expand_env_vars_with_provider("${NOPE}", env);
        assert_eq!(r.expanded, "${NOPE}");
        assert_eq!(r.missing_vars, vec!["NOPE"]);
    }

    #[test]
    fn test_no_expansion_needed() {
        let env = |_: &str| -> Option<String> { None };
        let r = expand_env_vars_with_provider("plain text", env);
        assert_eq!(r.expanded, "plain text");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn test_unclosed_brace() {
        let env = |_: &str| -> Option<String> { None };
        let r = expand_env_vars_with_provider("${OPEN", env);
        assert_eq!(r.expanded, "${OPEN");
    }

    #[test]
    fn test_default_with_colon_in_value() {
        let env = |_: &str| -> Option<String> { None };
        // Only the first :- is the delimiter
        let r = expand_env_vars_with_provider("${X:-a:-b}", env);
        assert_eq!(r.expanded, "a:-b");
    }
}
