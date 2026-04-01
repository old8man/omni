//! Permission rule parser.
//!
//! Handles the `ToolName(content)` format with proper escape handling,
//! glob/wildcard pattern matching, prefix syntax, and negation.
//!
//! Mirrors the TypeScript `permissionRuleParser.ts` and `shellRuleMatching.ts`.

use regex::Regex;

use super::types::{PermissionRuleValue, ShellPermissionRule};

// ---------------------------------------------------------------------------
// Legacy tool-name aliases (mirrors permissionRuleParser.ts)
// ---------------------------------------------------------------------------

/// Maps legacy tool names to their current canonical names.
fn normalize_legacy_tool_name(name: &str) -> String {
    match name {
        "Task" => "Agent".to_string(),
        "KillShell" => "TaskStop".to_string(),
        "AgentOutputTool" | "BashOutputTool" => "TaskOutput".to_string(),
        _ => name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Escape / unescape helpers
// ---------------------------------------------------------------------------

/// Escapes content for safe storage inside a `Tool(content)` rule string.
///
/// Order matters: backslashes first, then parentheses.
pub fn escape_rule_content(content: &str) -> String {
    content
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Reverses `escape_rule_content`.
///
/// Order matters (reverse of escaping): parentheses first, then backslashes.
pub fn unescape_rule_content(content: &str) -> String {
    content
        .replace("\\(", "(")
        .replace("\\)", ")")
        .replace("\\\\", "\\")
}

// ---------------------------------------------------------------------------
// Unescaped-char finders
// ---------------------------------------------------------------------------

/// Find the first unescaped occurrence of `ch` in `s`.
/// A character is unescaped when preceded by an even number (incl. 0) of backslashes.
fn find_first_unescaped(s: &str, ch: char) -> Option<usize> {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == ch as u8 {
            let mut backslash_count = 0usize;
            let mut j = i;
            while j > 0 {
                j -= 1;
                if bytes[j] == b'\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            if backslash_count.is_multiple_of(2) {
                return Some(i);
            }
        }
    }
    None
}

/// Find the last unescaped occurrence of `ch` in `s`.
fn find_last_unescaped(s: &str, ch: char) -> Option<usize> {
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).rev() {
        if bytes[i] == ch as u8 {
            let mut backslash_count = 0usize;
            let mut j = i;
            while j > 0 {
                j -= 1;
                if bytes[j] == b'\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            if backslash_count.is_multiple_of(2) {
                return Some(i);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Rule-string parsing: "ToolName" or "ToolName(content)"
// ---------------------------------------------------------------------------

/// Parse a rule string such as `"Bash(npm install)"` into its components.
///
/// Handles escaped parentheses in content, legacy tool-name aliases, and
/// edge cases like empty content or standalone wildcards.
pub fn permission_rule_value_from_string(rule_string: &str) -> PermissionRuleValue {
    let open = match find_first_unescaped(rule_string, '(') {
        Some(idx) => idx,
        None => {
            return PermissionRuleValue {
                tool_name: normalize_legacy_tool_name(rule_string),
                rule_content: None,
            };
        }
    };

    let close = match find_last_unescaped(rule_string, ')') {
        Some(idx) if idx > open => idx,
        _ => {
            return PermissionRuleValue {
                tool_name: normalize_legacy_tool_name(rule_string),
                rule_content: None,
            };
        }
    };

    // Closing paren must be at end of string.
    if close != rule_string.len() - 1 {
        return PermissionRuleValue {
            tool_name: normalize_legacy_tool_name(rule_string),
            rule_content: None,
        };
    }

    let tool_name = &rule_string[..open];
    let raw_content = &rule_string[open + 1..close];

    // Missing tool name (e.g. "(foo)") -> treat whole string as tool name.
    if tool_name.is_empty() {
        return PermissionRuleValue {
            tool_name: normalize_legacy_tool_name(rule_string),
            rule_content: None,
        };
    }

    // Empty content or standalone wildcard -> tool-wide rule.
    if raw_content.is_empty() || raw_content == "*" {
        return PermissionRuleValue {
            tool_name: normalize_legacy_tool_name(tool_name),
            rule_content: None,
        };
    }

    let content = unescape_rule_content(raw_content);
    PermissionRuleValue {
        tool_name: normalize_legacy_tool_name(tool_name),
        rule_content: Some(content),
    }
}

/// Convert a `PermissionRuleValue` back to its string representation.
pub fn permission_rule_value_to_string(value: &PermissionRuleValue) -> String {
    match &value.rule_content {
        None => value.tool_name.clone(),
        Some(content) => {
            let escaped = escape_rule_content(content);
            format!("{}({})", value.tool_name, escaped)
        }
    }
}

// ---------------------------------------------------------------------------
// Shell permission-rule parsing (prefix / wildcard / exact)
// ---------------------------------------------------------------------------

/// Extract prefix from legacy `:*` syntax (e.g. `"npm:*"` -> `Some("npm")`).
pub fn extract_prefix(rule: &str) -> Option<String> {
    if rule.ends_with(":*") && rule.len() > 2 {
        Some(rule[..rule.len() - 2].to_string())
    } else {
        None
    }
}

/// Returns `true` if `pattern` contains unescaped wildcards (not legacy `:*`).
pub fn has_wildcards(pattern: &str) -> bool {
    if pattern.ends_with(":*") {
        return false;
    }
    let bytes = pattern.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'*' {
            let mut backslash_count = 0usize;
            let mut j = i;
            while j > 0 {
                j -= 1;
                if bytes[j] == b'\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            if backslash_count.is_multiple_of(2) {
                return true;
            }
        }
    }
    false
}

/// Parse a shell permission rule string into its structured form.
pub fn parse_shell_permission_rule(rule: &str) -> ShellPermissionRule {
    if let Some(prefix) = extract_prefix(rule) {
        return ShellPermissionRule::Prefix { prefix };
    }
    if has_wildcards(rule) {
        return ShellPermissionRule::Wildcard {
            pattern: rule.to_string(),
        };
    }
    ShellPermissionRule::Exact {
        command: rule.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Wildcard pattern matching
// ---------------------------------------------------------------------------

// Sentinel placeholders for escape processing (mirroring TS null-byte sentinels).
const ESCAPED_STAR: &str = "\x00ESCAPED_STAR\x00";
const ESCAPED_BACKSLASH: &str = "\x00ESCAPED_BACKSLASH\x00";

/// Match a command against a wildcard pattern.
///
/// `*` matches any sequence of characters. `\*` matches a literal asterisk.
/// `\\` matches a literal backslash.
///
/// When a pattern ends with ` *` (space + single unescaped wildcard), the
/// trailing space-and-args become optional so `"git *"` matches both
/// `"git add"` and bare `"git"`.
pub fn match_wildcard_pattern(pattern: &str, command: &str, case_insensitive: bool) -> bool {
    let trimmed = pattern.trim();

    // Process escape sequences.
    let mut processed = String::with_capacity(trimmed.len());
    let chars: Vec<char> = trimmed.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                '*' => {
                    processed.push_str(ESCAPED_STAR);
                    i += 2;
                    continue;
                }
                '\\' => {
                    processed.push_str(ESCAPED_BACKSLASH);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        processed.push(chars[i]);
        i += 1;
    }

    // Count unescaped stars before regex-escaping.
    let unescaped_star_count = processed.matches('*').count();

    // Escape regex special chars except `*`.
    let escaped = regex::escape(&processed.replace('*', "\x01"))
        .replace("\\x01", "\x01") // undo escape of our placeholder
        .replace('\x01', ".*"); // placeholder -> .*

    // Restore sentinel placeholders.
    let mut regex_pattern = escaped
        .replace(ESCAPED_STAR, "\\*")
        .replace(ESCAPED_BACKSLASH, "\\\\");

    // Optional trailing space+args when pattern ends with ` *` and it is the
    // only unescaped wildcard.
    if regex_pattern.ends_with(" .*") && unescaped_star_count == 1 {
        let prefix = &regex_pattern[..regex_pattern.len() - 3];
        regex_pattern = format!("{}( .*)?", prefix);
    }

    let full = format!("^{}$", regex_pattern);
    let flags = if case_insensitive { "(?si)" } else { "(?s)" };
    let re = match Regex::new(&format!("{}{}", flags, full)) {
        Ok(r) => r,
        Err(_) => return false,
    };
    re.is_match(command)
}

/// Match a command against a parsed `ShellPermissionRule`.
pub fn matches_shell_rule(rule: &ShellPermissionRule, command: &str) -> bool {
    match rule {
        ShellPermissionRule::Exact { command: cmd } => command == cmd,
        ShellPermissionRule::Prefix { prefix } => {
            command == prefix.as_str() || command.starts_with(&format!("{} ", prefix))
        }
        ShellPermissionRule::Wildcard { pattern } => {
            match_wildcard_pattern(pattern, command, false)
        }
    }
}

// ---------------------------------------------------------------------------
// Negation patterns
// ---------------------------------------------------------------------------

/// Returns `true` if the rule string is a negation pattern (starts with `!`).
pub fn is_negation_pattern(rule: &str) -> bool {
    rule.starts_with('!')
}

/// Strip the leading `!` from a negation pattern, returning the inner pattern.
pub fn strip_negation(rule: &str) -> &str {
    rule.strip_prefix('!').unwrap_or(rule)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- escape / unescape ------------------------------------------------

    #[test]
    fn escape_and_unescape_roundtrip() {
        let original = r#"psycopg2.connect()"#;
        let escaped = escape_rule_content(original);
        assert_eq!(escaped, r"psycopg2.connect\(\)");
        assert_eq!(unescape_rule_content(&escaped), original);
    }

    #[test]
    fn escape_backslashes_and_parens() {
        let original = r#"echo "test\nvalue""#;
        let escaped = escape_rule_content(original);
        assert_eq!(escaped, r#"echo "test\\nvalue""#);
        assert_eq!(unescape_rule_content(&escaped), original);
    }

    // -- permission_rule_value_from_string ---------------------------------

    #[test]
    fn parse_bare_tool_name() {
        let v = permission_rule_value_from_string("Bash");
        assert_eq!(v.tool_name, "Bash");
        assert_eq!(v.rule_content, None);
    }

    #[test]
    fn parse_tool_with_content() {
        let v = permission_rule_value_from_string("Bash(npm install)");
        assert_eq!(v.tool_name, "Bash");
        assert_eq!(v.rule_content, Some("npm install".to_string()));
    }

    #[test]
    fn parse_tool_with_escaped_parens_in_content() {
        // Mirrors TS: 'Bash(python -c "print\\(1\\)")'
        // The rule content has escaped parens: \( and \) which unescape to ( and )
        let v = permission_rule_value_from_string("Bash(python -c \"print\\(1\\)\")");
        assert_eq!(v.tool_name, "Bash");
        assert_eq!(
            v.rule_content,
            Some("python -c \"print(1)\"".to_string())
        );
    }

    #[test]
    fn parse_empty_content_as_tool_wide() {
        let v = permission_rule_value_from_string("Bash()");
        assert_eq!(v.tool_name, "Bash");
        assert_eq!(v.rule_content, None);
    }

    #[test]
    fn parse_wildcard_content_as_tool_wide() {
        let v = permission_rule_value_from_string("Bash(*)");
        assert_eq!(v.tool_name, "Bash");
        assert_eq!(v.rule_content, None);
    }

    #[test]
    fn parse_legacy_tool_alias() {
        let v = permission_rule_value_from_string("Task");
        assert_eq!(v.tool_name, "Agent");
    }

    #[test]
    fn roundtrip_rule_value() {
        let v = PermissionRuleValue {
            tool_name: "Bash".to_string(),
            rule_content: Some("npm install".to_string()),
        };
        let s = permission_rule_value_to_string(&v);
        assert_eq!(s, "Bash(npm install)");
        let v2 = permission_rule_value_from_string(&s);
        assert_eq!(v, v2);
    }

    #[test]
    fn roundtrip_with_parens_in_content() {
        let v = PermissionRuleValue {
            tool_name: "Bash".to_string(),
            rule_content: Some("python -c \"print(1)\"".to_string()),
        };
        let s = permission_rule_value_to_string(&v);
        let v2 = permission_rule_value_from_string(&s);
        assert_eq!(v, v2);
    }

    // -- shell permission rule parsing -------------------------------------

    #[test]
    fn parse_exact_rule() {
        assert_eq!(
            parse_shell_permission_rule("ls -la"),
            ShellPermissionRule::Exact {
                command: "ls -la".to_string()
            }
        );
    }

    #[test]
    fn parse_prefix_rule() {
        assert_eq!(
            parse_shell_permission_rule("npm:*"),
            ShellPermissionRule::Prefix {
                prefix: "npm".to_string()
            }
        );
    }

    #[test]
    fn parse_wildcard_rule() {
        assert_eq!(
            parse_shell_permission_rule("git *"),
            ShellPermissionRule::Wildcard {
                pattern: "git *".to_string()
            }
        );
    }

    // -- wildcard matching -------------------------------------------------

    #[test]
    fn wildcard_basic() {
        assert!(match_wildcard_pattern("git *", "git add", false));
        assert!(match_wildcard_pattern("git *", "git", false)); // trailing * optional
        assert!(!match_wildcard_pattern("git *", "gitadd", false));
    }

    #[test]
    fn wildcard_multiple_stars() {
        assert!(match_wildcard_pattern("* run *", "npm run build", false));
        // Multiple wildcards: trailing * is NOT optional.
        assert!(!match_wildcard_pattern("* run *", "npm run", false));
    }

    #[test]
    fn wildcard_escaped_star() {
        assert!(match_wildcard_pattern(r"echo \*", "echo *", false));
        assert!(!match_wildcard_pattern(r"echo \*", "echo hello", false));
    }

    #[test]
    fn wildcard_case_insensitive() {
        assert!(match_wildcard_pattern("Git *", "git add", true));
        assert!(!match_wildcard_pattern("Git *", "git add", false));
    }

    #[test]
    fn wildcard_exact_match() {
        assert!(match_wildcard_pattern("ls -la", "ls -la", false));
        assert!(!match_wildcard_pattern("ls -la", "ls -la extra", false));
    }

    // -- matches_shell_rule ------------------------------------------------

    #[test]
    fn shell_rule_exact_match() {
        let rule = parse_shell_permission_rule("ls -la");
        assert!(matches_shell_rule(&rule, "ls -la"));
        assert!(!matches_shell_rule(&rule, "ls"));
    }

    #[test]
    fn shell_rule_prefix_match() {
        let rule = parse_shell_permission_rule("npm:*");
        assert!(matches_shell_rule(&rule, "npm install"));
        assert!(matches_shell_rule(&rule, "npm")); // bare command = prefix itself
        assert!(!matches_shell_rule(&rule, "npx foo"));
    }

    #[test]
    fn shell_rule_wildcard_match() {
        let rule = parse_shell_permission_rule("docker *");
        assert!(matches_shell_rule(&rule, "docker ps"));
        assert!(matches_shell_rule(&rule, "docker"));
    }

    // -- negation ----------------------------------------------------------

    #[test]
    fn negation_detection() {
        assert!(is_negation_pattern("!rm -rf"));
        assert!(!is_negation_pattern("rm -rf"));
    }

    #[test]
    fn negation_stripping() {
        assert_eq!(strip_negation("!rm -rf"), "rm -rf");
        assert_eq!(strip_negation("rm -rf"), "rm -rf");
    }

    // -- has_wildcards -----------------------------------------------------

    #[test]
    fn has_wildcards_detects_unescaped() {
        assert!(has_wildcards("git *"));
        assert!(has_wildcards("*"));
        assert!(!has_wildcards("npm:*")); // legacy prefix, not wildcard
        assert!(!has_wildcards(r"echo \*")); // escaped
    }
}
