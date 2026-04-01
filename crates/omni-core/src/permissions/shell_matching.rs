//! Enhanced shell rule matching utilities.
//!
//! Extends the rule_parser with higher-level matching functions:
//! - Permission suggestion generation for shell commands
//! - Command prefix extraction for suggestion UI
//! - Compound command splitting and per-segment matching
//!
//! Mirrors the TypeScript `shellRuleMatching.ts` suggestion helpers.

use super::rule_parser::{matches_shell_rule, parse_shell_permission_rule};

// ---------------------------------------------------------------------------
// Permission suggestion types
// ---------------------------------------------------------------------------

/// A suggestion for a permission rule that the user can add.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionSuggestion {
    /// The tool name (e.g. "Bash").
    pub tool_name: String,
    /// The rule content (e.g. "npm install" or "git:*").
    pub rule_content: String,
    /// Human-readable label for the suggestion.
    pub label: String,
    /// Whether this is an exact match or a prefix/wildcard.
    pub suggestion_type: SuggestionType,
}

/// Type of permission suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuggestionType {
    /// Match only this exact command.
    Exact,
    /// Match this command and any arguments.
    Prefix,
}

// ---------------------------------------------------------------------------
// Suggestion generation
// ---------------------------------------------------------------------------

/// Generate permission update suggestions for a shell command.
///
/// Returns suggestions in order from most specific to most broad:
/// 1. Exact command match
/// 2. First-word prefix match (e.g. `git:*` for `git add`)
///
/// For compound commands (containing `&&` or `||`), suggestions are generated
/// for the first segment only.
pub fn suggest_shell_permissions(
    tool_name: &str,
    command: &str,
) -> Vec<PermissionSuggestion> {
    let mut suggestions = Vec::new();
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return suggestions;
    }

    // For compound commands, work with the first segment.
    let first_segment = extract_first_segment(trimmed);

    // Exact match suggestion.
    suggestions.push(PermissionSuggestion {
        tool_name: tool_name.to_string(),
        rule_content: first_segment.to_string(),
        label: format!("Allow exactly: {}", truncate(first_segment, 60)),
        suggestion_type: SuggestionType::Exact,
    });

    // Prefix suggestion: first word(s).
    let prefix = extract_command_prefix(first_segment);
    if !prefix.is_empty() && prefix != first_segment {
        suggestions.push(PermissionSuggestion {
            tool_name: tool_name.to_string(),
            rule_content: format!("{}:*", prefix),
            label: format!("Allow all '{}' commands", prefix),
            suggestion_type: SuggestionType::Prefix,
        });
    }

    // Two-word prefix for compound commands like "git add", "npm run".
    let two_word_prefix = extract_two_word_prefix(first_segment);
    if let Some(twp) = two_word_prefix {
        if twp != first_segment && twp != prefix {
            suggestions.push(PermissionSuggestion {
                tool_name: tool_name.to_string(),
                rule_content: format!("{} *", twp),
                label: format!("Allow '{}' with any arguments", twp),
                suggestion_type: SuggestionType::Prefix,
            });
        }
    }

    suggestions
}

/// Check if a command matches any of the given permission rule strings.
///
/// Returns the first matching rule string, or `None` if no match.
pub fn find_matching_rule<'a>(
    command: &str,
    rules: &'a [String],
) -> Option<&'a String> {
    for rule_str in rules {
        let parsed = parse_shell_permission_rule(rule_str);
        if matches_shell_rule(&parsed, command) {
            return Some(rule_str);
        }
    }
    None
}

/// Check if a command matches a specific permission rule string.
pub fn command_matches_rule(command: &str, rule: &str) -> bool {
    let parsed = parse_shell_permission_rule(rule);
    matches_shell_rule(&parsed, command)
}

// ---------------------------------------------------------------------------
// Compound command handling
// ---------------------------------------------------------------------------

/// Split a compound command into its segments.
///
/// Handles `&&`, `||`, `;`, and `|` operators. Respects quoting so that
/// pipes/operators inside quotes are not treated as separators.
pub fn split_compound_command(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < chars.len() {
        let ch = chars[i];

        // Handle quoting
        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(ch);
            i += 1;
            continue;
        }
        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(ch);
            i += 1;
            continue;
        }

        // Skip separators inside quotes
        if in_single_quote || in_double_quote {
            current.push(ch);
            i += 1;
            continue;
        }

        // Check for && and ||
        if ch == '&' && i + 1 < chars.len() && chars[i + 1] == '&' {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                segments.push(trimmed);
            }
            current.clear();
            i += 2;
            continue;
        }
        if ch == '|' && i + 1 < chars.len() && chars[i + 1] == '|' {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                segments.push(trimmed);
            }
            current.clear();
            i += 2;
            continue;
        }

        // Semicolon separator
        if ch == ';' {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                segments.push(trimmed);
            }
            current.clear();
            i += 1;
            continue;
        }

        // Single pipe (command pipeline) - treat as separator for permission purposes
        if ch == '|' {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                segments.push(trimmed);
            }
            current.clear();
            i += 1;
            continue;
        }

        current.push(ch);
        i += 1;
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        segments.push(trimmed);
    }

    segments
}

/// Check if every segment of a compound command matches at least one
/// of the given permission rules.
pub fn all_segments_match(command: &str, rules: &[String]) -> bool {
    let segments = split_compound_command(command);
    if segments.is_empty() {
        return false;
    }
    segments.iter().all(|seg| find_matching_rule(seg, rules).is_some())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the first segment of a compound command (before `&&`, `||`, `;`, `|`).
fn extract_first_segment(command: &str) -> &str {
    let segments = [" && ", " || ", "; ", " | "];
    let mut earliest = command.len();
    for sep in &segments {
        if let Some(pos) = command.find(sep) {
            if pos < earliest {
                earliest = pos;
            }
        }
    }
    command[..earliest].trim()
}

/// Extract the command prefix (first word) from a command string.
fn extract_command_prefix(command: &str) -> &str {
    let trimmed = command.trim();
    match trimmed.find(char::is_whitespace) {
        Some(pos) => &trimmed[..pos],
        None => trimmed,
    }
}

/// Extract a two-word prefix (e.g. "git add" from "git add -A src/").
fn extract_two_word_prefix(command: &str) -> Option<String> {
    let words: Vec<&str> = command.trim().splitn(3, char::is_whitespace).collect();
    if words.len() >= 3 {
        Some(format!("{} {}", words[0], words[1]))
    } else {
        None
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_exact_and_prefix() {
        let suggestions = suggest_shell_permissions("Bash", "git add -A");
        assert!(suggestions.len() >= 2);
        assert_eq!(suggestions[0].suggestion_type, SuggestionType::Exact);
        assert_eq!(suggestions[0].rule_content, "git add -A");
        assert_eq!(suggestions[1].suggestion_type, SuggestionType::Prefix);
        assert_eq!(suggestions[1].rule_content, "git:*");
    }

    #[test]
    fn suggest_two_word_prefix() {
        let suggestions = suggest_shell_permissions("Bash", "npm run build");
        assert!(suggestions.len() >= 3);
        assert_eq!(suggestions[2].rule_content, "npm run *");
    }

    #[test]
    fn suggest_single_word_command() {
        let suggestions = suggest_shell_permissions("Bash", "ls");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].rule_content, "ls");
    }

    #[test]
    fn suggest_compound_command_uses_first_segment() {
        let suggestions = suggest_shell_permissions("Bash", "cd /tmp && ls -la");
        assert_eq!(suggestions[0].rule_content, "cd /tmp");
    }

    #[test]
    fn find_matching_rule_exact() {
        let rules = vec!["ls -la".to_string(), "git:*".to_string()];
        assert_eq!(find_matching_rule("ls -la", &rules), Some(&rules[0]));
    }

    #[test]
    fn find_matching_rule_prefix() {
        let rules = vec!["git:*".to_string()];
        assert_eq!(find_matching_rule("git status", &rules), Some(&rules[0]));
    }

    #[test]
    fn find_matching_rule_no_match() {
        let rules = vec!["git:*".to_string()];
        assert_eq!(find_matching_rule("npm install", &rules), None);
    }

    #[test]
    fn split_compound_and() {
        let segments = split_compound_command("cd /tmp && ls -la && pwd");
        assert_eq!(segments, vec!["cd /tmp", "ls -la", "pwd"]);
    }

    #[test]
    fn split_compound_or() {
        let segments = split_compound_command("test -f foo || echo missing");
        assert_eq!(segments, vec!["test -f foo", "echo missing"]);
    }

    #[test]
    fn split_compound_semicolon() {
        let segments = split_compound_command("echo a; echo b; echo c");
        assert_eq!(segments, vec!["echo a", "echo b", "echo c"]);
    }

    #[test]
    fn split_compound_pipe() {
        let segments = split_compound_command("cat file | grep foo | wc -l");
        assert_eq!(segments, vec!["cat file", "grep foo", "wc -l"]);
    }

    #[test]
    fn split_respects_quotes() {
        let segments = split_compound_command(r#"echo "hello && world" && ls"#);
        assert_eq!(segments, vec![r#"echo "hello && world""#, "ls"]);
    }

    #[test]
    fn all_segments_match_true() {
        let rules = vec!["git:*".to_string(), "ls:*".to_string()];
        assert!(all_segments_match("git status && ls -la", &rules));
    }

    #[test]
    fn all_segments_match_false() {
        let rules = vec!["git:*".to_string()];
        assert!(!all_segments_match("git status && rm -rf /", &rules));
    }

    #[test]
    fn command_matches_rule_works() {
        assert!(command_matches_rule("git status", "git:*"));
        assert!(command_matches_rule("ls -la", "ls *"));
        assert!(command_matches_rule("npm install", "npm install"));
        assert!(!command_matches_rule("rm -rf /", "git:*"));
    }
}
