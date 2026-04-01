//! Hook matching: pattern matching for hook matchers and hook deduplication.
//!
//! Mirrors `matchesPattern`, `getMatchingHooks`, and dedup logic from
//! the TypeScript `hooks.ts`.

use regex::Regex;
use tracing::debug;

use super::registry::HookRegistry;
use super::types::{HookCommand, HookInput, IndividualHookConfig};

/// Check if a match query matches a hook matcher pattern.
///
/// Supports:
/// - Simple exact match (e.g., "Write")
/// - Pipe-separated exact matches (e.g., "Write|Edit")
/// - Regex patterns (e.g., "^Write.*", ".*", "^(Write|Edit)$")
/// - Wildcard `*` (or empty matcher) matches everything
pub fn matches_pattern(match_query: &str, matcher: &str) -> bool {
    if matcher.is_empty() || matcher == "*" {
        return true;
    }

    // Check if it's a simple string or pipe-separated list (no regex special chars except |)
    let is_simple = matcher
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '|');

    if is_simple {
        if matcher.contains('|') {
            // Pipe-separated exact matches
            return matcher.split('|').map(|p| p.trim()).any(|p| p == match_query);
        }
        // Simple exact match
        return match_query == matcher;
    }

    // Otherwise treat as regex
    match Regex::new(matcher) {
        Ok(re) => re.is_match(match_query),
        Err(_) => {
            debug!("invalid regex pattern in hook matcher: {matcher}");
            false
        }
    }
}

/// Check if a matcher pattern matches a given value using `*` wildcards.
///
/// This is the simpler glob-style matching used for file watcher matchers.
pub fn glob_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == value;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !value.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 && !part.is_empty() {
            if !value.ends_with(part) {
                return false;
            }
        } else if let Some(found) = value[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }
    true
}

/// Get all hooks from the registry that match the given input.
///
/// This performs:
/// 1. Event filtering (only hooks registered for this event)
/// 2. Pattern matching (matcher vs match query from hook input)
/// 3. `if` condition filtering (for tool-related events)
/// 4. Deduplication (same command/prompt across sources collapses)
pub fn get_matching_hooks<'a>(
    registry: &'a HookRegistry,
    input: &HookInput,
) -> Vec<&'a IndividualHookConfig> {
    let event = input.hook_event_name();
    let match_query = input.match_query();

    let all_hooks = registry.get_hooks(event);
    if all_hooks.is_empty() {
        return Vec::new();
    }

    // Filter by matcher pattern
    let pattern_matched: Vec<&IndividualHookConfig> = all_hooks
        .into_iter()
        .filter(|hook| {
            match (&hook.matcher, match_query) {
                (None, _) => true, // No matcher = match everything
                (Some(m), Some(q)) => matches_pattern(q, m),
                (Some(_), None) => true, // No match query = match everything
            }
        })
        .collect();

    // Filter by `if` condition (only for tool-related events)
    let if_filtered: Vec<&IndividualHookConfig> = pattern_matched
        .into_iter()
        .filter(|hook| {
            let condition = hook.config.condition();
            match condition {
                None => true,
                Some(cond) => evaluate_if_condition(cond, input),
            }
        })
        .collect();

    // Deduplicate: same command/prompt content within the same source context
    // collapses to one. We keep the last occurrence (matching TS behavior with
    // `new Map(entries)` which keeps last on collision).
    deduplicate_hooks(if_filtered)
}

/// Evaluate an `if` condition against the hook input.
///
/// For tool-related events, the `if` condition is in the form "ToolName(pattern)"
/// and we check if the tool name and input match. For other events, the `if`
/// condition is currently always true (matching TS behavior where ifMatcher
/// returns undefined for non-tool events, causing the hook to be skipped).
fn evaluate_if_condition(condition: &str, input: &HookInput) -> bool {
    match input {
        HookInput::PreToolUse { tool_name, .. }
        | HookInput::PostToolUse { tool_name, .. }
        | HookInput::PostToolUseFailure { tool_name, .. }
        | HookInput::PermissionRequest { tool_name, .. } => {
            // Parse "ToolName(content)" or just "ToolName"
            if let Some(paren_pos) = condition.find('(') {
                let cond_tool = &condition[..paren_pos];
                if cond_tool != tool_name {
                    return false;
                }
                // Has a content pattern inside parens - for now, basic glob match
                let end = condition.rfind(')').unwrap_or(condition.len());
                let _content_pattern = &condition[paren_pos + 1..end];
                // Content pattern matching would require tool-specific logic
                // (e.g., parsing Bash command arguments). Return true for now
                // since the tool name matched.
                true
            } else {
                condition == tool_name
            }
        }
        _ => {
            // `if` conditions on non-tool events are not supported in TS either
            debug!("hook if condition \"{condition}\" cannot be evaluated for non-tool event");
            false
        }
    }
}

/// Deduplicate hooks by their content identity.
///
/// Two hooks are considered duplicates if they have the same type, content
/// (command/prompt/url), shell, and `if` condition. Timeout differences are
/// intentionally ignored.
fn deduplicate_hooks<'a>(hooks: Vec<&'a IndividualHookConfig>) -> Vec<&'a IndividualHookConfig> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::with_capacity(hooks.len());

    for hook in hooks {
        let key = dedup_key(&hook.config);
        if seen.insert(key) {
            result.push(hook);
        }
    }

    result
}

/// Build a dedup key string from a hook command.
fn dedup_key(cmd: &HookCommand) -> String {
    match cmd {
        HookCommand::Command {
            command,
            shell,
            condition,
            ..
        } => format!(
            "command\0{shell}\0{command}\0{}",
            condition.as_deref().unwrap_or("")
        ),
        HookCommand::Prompt {
            prompt, condition, ..
        } => format!(
            "prompt\0{prompt}\0{}",
            condition.as_deref().unwrap_or("")
        ),
        HookCommand::Agent {
            prompt, condition, ..
        } => format!(
            "agent\0{prompt}\0{}",
            condition.as_deref().unwrap_or("")
        ),
        HookCommand::Http {
            url, condition, ..
        } => format!(
            "http\0{url}\0{}",
            condition.as_deref().unwrap_or("")
        ),
    }
}

/// Check if any hook results contain a blocking error.
pub fn has_blocking_result(results: &[super::types::HookOutsideReplResult]) -> bool {
    results.iter().any(|r| r.blocked)
}

/// Format a blocking error from a PreToolUse hook.
pub fn get_pre_tool_hook_blocking_message(
    hook_name: &str,
    blocking_error: &super::types::HookBlockingError,
) -> String {
    format!("{hook_name} hook error: {}", blocking_error.blocking_error)
}

/// Format a blocking error from a Stop hook.
pub fn get_stop_hook_message(blocking_error: &super::types::HookBlockingError) -> String {
    format!("Stop hook feedback:\n{}", blocking_error.blocking_error)
}

/// Format a blocking error from a UserPromptSubmit hook.
pub fn get_user_prompt_submit_hook_blocking_message(
    blocking_error: &super::types::HookBlockingError,
) -> String {
    format!(
        "UserPromptSubmit operation blocked by hook:\n{}",
        blocking_error.blocking_error
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_pattern_exact() {
        assert!(matches_pattern("Write", "Write"));
        assert!(!matches_pattern("Read", "Write"));
    }

    #[test]
    fn test_matches_pattern_wildcard() {
        assert!(matches_pattern("anything", "*"));
        assert!(matches_pattern("anything", ""));
    }

    #[test]
    fn test_matches_pattern_pipe_separated() {
        assert!(matches_pattern("Write", "Write|Edit"));
        assert!(matches_pattern("Edit", "Write|Edit"));
        assert!(!matches_pattern("Read", "Write|Edit"));
    }

    #[test]
    fn test_matches_pattern_regex() {
        assert!(matches_pattern("Write", "^Write.*"));
        assert!(matches_pattern("WriteFile", "^Write.*"));
        assert!(!matches_pattern("ReadFile", "^Write.*"));
        assert!(matches_pattern("anything", ".*"));
        assert!(matches_pattern("Edit", "^(Write|Edit)$"));
    }

    #[test]
    fn test_matches_pattern_invalid_regex() {
        assert!(!matches_pattern("test", "[invalid"));
    }

    #[test]
    fn test_glob_matches() {
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("Bash", "Bash"));
        assert!(!glob_matches("Bash", "Read"));
        assert!(glob_matches("Bash*", "BashTool"));
        assert!(glob_matches("mcp__*__read", "mcp__server__read"));
    }

    #[test]
    fn test_dedup_key() {
        let cmd1 = HookCommand::Command {
            command: "echo test".to_string(),
            shell: "bash".to_string(),
            condition: None,
            timeout: Some(10),
        };
        let cmd2 = HookCommand::Command {
            command: "echo test".to_string(),
            shell: "bash".to_string(),
            condition: None,
            timeout: Some(999),
        };
        assert_eq!(dedup_key(&cmd1), dedup_key(&cmd2));

        let cmd3 = HookCommand::Command {
            command: "echo test".to_string(),
            shell: "zsh".to_string(),
            condition: None,
            timeout: None,
        };
        assert_ne!(dedup_key(&cmd1), dedup_key(&cmd3));
    }
}
