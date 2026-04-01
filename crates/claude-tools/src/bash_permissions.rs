//! Bash command permission rules and matching.
//!
//! Ported from the TypeScript `bashPermissions.ts` and related modules.
//! Provides wildcard pattern matching for permission rules, safe wrapper
//! stripping, and permission evaluation for bash commands.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use crate::bash_security::{
    self, DestructiveCommandDetector, ReadOnlyValidator, SecurityVerdict,
};

// ---------------------------------------------------------------------------
// Permission rule types
// ---------------------------------------------------------------------------

/// A single permission rule for bash commands.
#[derive(Debug, Clone)]
pub struct BashPermissionRule {
    /// The pattern to match against, e.g. `"git status"`, `"git diff:*"`, `"npm run:*"`.
    pub pattern: String,
    /// Whether this rule allows or denies the command.
    pub value: PermissionRuleValue,
}

/// Whether a rule allows or denies execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionRuleValue {
    Allow,
    Deny,
}

/// The result of evaluating permission rules for a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResult {
    /// The command is allowed (possibly by a matching rule).
    Allow { reason: String },
    /// The command should be prompted to the user.
    Ask {
        message: String,
        suggestions: Vec<String>,
    },
    /// The command is denied by a rule.
    Deny { reason: String },
    /// No rule matched — fall through to other checks.
    Passthrough,
}

/// The current permission mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// Normal interactive mode — prompt for unknown commands.
    Default,
    /// Accept all edits automatically (filesystem write commands).
    AcceptEdits,
    /// Bypass all permission checks (developer/testing mode).
    BypassPermissions,
    /// Never prompt — deny anything not explicitly allowed.
    DontAsk,
}

// ---------------------------------------------------------------------------
// Safe env vars and wrappers
// ---------------------------------------------------------------------------

/// Environment variables that are safe to strip from command prefixes.
/// These don't affect what the command does (only formatting, locale, etc.).
const SAFE_ENV_VARS: &[&str] = &[
    "LANG",
    "LC_ALL",
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_NUMERIC",
    "LC_TIME",
    "TZ",
    "TERM",
    "COLORTERM",
    "FORCE_COLOR",
    "NO_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "NODE_ENV",
    "PYTHONDONTWRITEBYTECODE",
    "PYTHONUNBUFFERED",
    "RUST_LOG",
    "RUST_BACKTRACE",
    "DEBUG",
    "VERBOSE",
    "CI",
    "HOME",
    "PATH",
    "SHELL",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "LESS",
    "GREP_OPTIONS",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
];

/// Commands whose prefixes are safe wrappers (just pass through to the real command).
const SAFE_WRAPPER_PATTERNS: &[&str] = &["command", "builtin", "nice", "nohup", "time", "timeout"];

/// Shell commands that are dangerous as bare-prefix suggestions.
const BARE_SHELL_PREFIXES: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "csh", "tcsh"];

/// Commands auto-allowed in Accept Edits mode.
const ACCEPT_EDITS_ALLOWED: &[&str] = &["mkdir", "touch", "rm", "rmdir", "mv", "cp", "sed"];

// ---------------------------------------------------------------------------
// Permission evaluator
// ---------------------------------------------------------------------------

/// Evaluates bash command permissions against a set of rules.
pub struct BashPermissionEvaluator {
    rules: Vec<BashPermissionRule>,
    mode: PermissionMode,
    destructive_detector: DestructiveCommandDetector,
    read_only_validator: ReadOnlyValidator,
}

impl BashPermissionEvaluator {
    /// Create a new evaluator with the given rules and mode.
    pub fn new(rules: Vec<BashPermissionRule>, mode: PermissionMode) -> Self {
        Self {
            rules,
            mode,
            destructive_detector: DestructiveCommandDetector::new(),
            read_only_validator: ReadOnlyValidator::new(),
        }
    }

    /// Evaluate whether a bash command should be allowed, denied, or prompted.
    ///
    /// This is the main entry point. It:
    /// 1. Runs security validation (structural checks, dangerous patterns).
    /// 2. Checks permission mode (bypass, accept-edits, etc.).
    /// 3. Evaluates explicit permission rules.
    /// 4. Falls back to read-only auto-allow or ask.
    pub fn evaluate(&self, command: &str, cwd: &Path) -> PermissionResult {
        // --- Step 0: Security validation (always runs, cannot be bypassed) ---
        let security_result = bash_security::validate_command(command, cwd);
        match &security_result {
            SecurityVerdict::Deny(reason) => {
                return PermissionResult::Deny {
                    reason: reason.clone(),
                };
            }
            SecurityVerdict::Ask(reason) => {
                // Security concerns always prompt, even in bypass mode,
                // for destructive commands.
                if let Some(warning) = self.destructive_detector.detect(command) {
                    return PermissionResult::Ask {
                        message: format!("{}\n{}", reason, warning),
                        suggestions: self.suggest_rules(command),
                    };
                }

                // In bypass mode, only truly dangerous things should still prompt.
                if self.mode == PermissionMode::BypassPermissions {
                    // Allow through in bypass mode for non-destructive security warnings.
                    // The validate_command function already filters the most dangerous cases.
                } else {
                    return PermissionResult::Ask {
                        message: reason.clone(),
                        suggestions: self.suggest_rules(command),
                    };
                }
            }
            SecurityVerdict::Allow => {}
        }

        // --- Step 1: Bypass mode ---
        if self.mode == PermissionMode::BypassPermissions {
            return PermissionResult::Allow {
                reason: "Bypass permissions mode".to_string(),
            };
        }

        // --- Step 2: Accept Edits mode ---
        if self.mode == PermissionMode::AcceptEdits {
            let segments = bash_security::split_command(command);
            let all_accept_edits = segments.iter().all(|seg| {
                let base = Self::extract_base_after_wrappers(seg);
                ACCEPT_EDITS_ALLOWED.contains(&base.as_str())
            });
            if all_accept_edits {
                return PermissionResult::Allow {
                    reason: "Accept Edits mode allows filesystem commands".to_string(),
                };
            }
        }

        // --- Step 3: Explicit rules ---
        let stripped = self.strip_safe_wrappers(command);
        if let Some(result) = self.check_rules(&stripped) {
            return result;
        }

        // --- Step 4: Read-only auto-allow ---
        if self.read_only_validator.is_read_only(command) {
            return PermissionResult::Allow {
                reason: "Command is read-only".to_string(),
            };
        }

        // --- Step 5: DontAsk mode — deny anything not explicitly allowed ---
        if self.mode == PermissionMode::DontAsk {
            return PermissionResult::Deny {
                reason: "Command not in allowlist and mode is DontAsk".to_string(),
            };
        }

        // --- Step 6: Ask ---
        let mut message = "Command requires approval".to_string();
        if let Some(warning) = self.destructive_detector.detect(command) {
            message = format!("{}\n{}", message, warning);
        }

        PermissionResult::Ask {
            message,
            suggestions: self.suggest_rules(command),
        }
    }

    /// Check explicit rules against the stripped command.
    fn check_rules(&self, command: &str) -> Option<PermissionResult> {
        for rule in &self.rules {
            if self.rule_matches(&rule.pattern, command) {
                return Some(match rule.value {
                    PermissionRuleValue::Allow => PermissionResult::Allow {
                        reason: format!("Matched allow rule: {}", rule.pattern),
                    },
                    PermissionRuleValue::Deny => PermissionResult::Deny {
                        reason: format!("Matched deny rule: {}", rule.pattern),
                    },
                });
            }
        }
        None
    }

    /// Check if a permission rule pattern matches a command.
    fn rule_matches(&self, pattern: &str, command: &str) -> bool {
        let parsed = parse_permission_rule(pattern);
        match parsed.match_type {
            RuleMatchType::Exact => command == parsed.command,
            RuleMatchType::Prefix => command.starts_with(&parsed.command),
            RuleMatchType::Glob => match_wildcard_pattern(&parsed.command, command),
        }
    }

    /// Strip safe environment variable assignments and wrapper commands.
    fn strip_safe_wrappers(&self, command: &str) -> String {
        let tokens: Vec<String> = bash_security::split_command(command)
            .into_iter()
            .map(|seg| Self::strip_segment_wrappers(&seg))
            .collect();
        tokens.join(" && ")
    }

    fn strip_segment_wrappers(segment: &str) -> String {
        let tokens = shell_tokenize_raw(segment);
        let mut result_tokens = Vec::new();
        let mut i = 0;

        // Skip safe env var assignments (VAR=value).
        let safe_vars: HashSet<&str> = SAFE_ENV_VARS.iter().copied().collect();
        while i < tokens.len() {
            if let Some(eq_pos) = tokens[i].find('=') {
                let var_name = &tokens[i][..eq_pos];
                if safe_vars.contains(var_name) {
                    i += 1;
                    continue;
                }
            }
            break;
        }

        // Skip safe wrapper commands (command, nice, nohup, time, timeout).
        while i < tokens.len() {
            let base = tokens[i].as_str();
            if SAFE_WRAPPER_PATTERNS.contains(&base) {
                i += 1;
                // Skip wrapper-specific flags.
                while i < tokens.len() && tokens[i].starts_with('-') {
                    i += 1;
                }
            } else {
                break;
            }
        }

        result_tokens.extend_from_slice(&tokens[i..]);
        result_tokens.join(" ")
    }

    /// Extract the base command after stripping safe wrappers.
    fn extract_base_after_wrappers(segment: &str) -> String {
        let stripped = Self::strip_segment_wrappers(segment);
        stripped.split_whitespace().next().unwrap_or("").to_string()
    }

    /// Suggest permission rules for a command.
    fn suggest_rules(&self, command: &str) -> Vec<String> {
        let mut suggestions = Vec::new();
        let stripped = self.strip_safe_wrappers(command);
        let tokens: Vec<&str> = stripped.split_whitespace().collect();

        if tokens.is_empty() {
            return suggestions;
        }

        let base = tokens[0];

        // Don't suggest rules for bare shell prefixes.
        if BARE_SHELL_PREFIXES.contains(&base) {
            return suggestions;
        }

        // Exact match suggestion.
        suggestions.push(stripped.clone());

        // Prefix suggestion for multi-word commands.
        if let Some(prefix) = get_simple_command_prefix(&stripped) {
            suggestions.push(format!("{}:*", prefix));
        }

        suggestions
    }
}

// ---------------------------------------------------------------------------
// Wildcard pattern matching
// ---------------------------------------------------------------------------

/// Match a wildcard pattern against a string.
/// Supports `*` (matches any sequence of characters) and `?` (matches any single character).
pub fn match_wildcard_pattern(pattern: &str, value: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let value_chars: Vec<char> = value.chars().collect();
    match_wildcard_recursive(&pattern_chars, &value_chars, 0, 0)
}

fn match_wildcard_recursive(pattern: &[char], value: &[char], pi: usize, vi: usize) -> bool {
    if pi == pattern.len() && vi == value.len() {
        return true;
    }
    if pi == pattern.len() {
        return false;
    }

    if pattern[pi] == '*' {
        // Try matching * against zero or more characters.
        for skip in 0..=(value.len() - vi) {
            if match_wildcard_recursive(pattern, value, pi + 1, vi + skip) {
                return true;
            }
        }
        return false;
    }

    if vi >= value.len() {
        return false;
    }

    if pattern[pi] == '?' || pattern[pi] == value[vi] {
        return match_wildcard_recursive(pattern, value, pi + 1, vi + 1);
    }

    false
}

// ---------------------------------------------------------------------------
// Permission rule parsing
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum RuleMatchType {
    Exact,
    Prefix,
    Glob,
}

#[derive(Debug)]
struct ParsedRule {
    command: String,
    match_type: RuleMatchType,
}

/// Parse a permission rule pattern into its components.
///
/// Patterns can be:
/// - `"git status"` — exact match
/// - `"git diff:*"` — prefix match (everything after `git diff`)
/// - `"npm *"` — glob match
fn parse_permission_rule(pattern: &str) -> ParsedRule {
    // Check for `prefix:*` pattern.
    if let Some(prefix) = pattern.strip_suffix(":*") {
        return ParsedRule {
            command: format!("{} ", prefix),
            match_type: RuleMatchType::Prefix,
        };
    }

    // Check for glob characters.
    if pattern.contains('*') || pattern.contains('?') {
        return ParsedRule {
            command: pattern.to_string(),
            match_type: RuleMatchType::Glob,
        };
    }

    // Exact match.
    ParsedRule {
        command: pattern.to_string(),
        match_type: RuleMatchType::Exact,
    }
}

/// Regex for validating subcommand-like tokens (e.g. `run`, `build`, `test-e2e`).
static RE_SUBCMD: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$").expect("static regex")
});

/// Extract a stable command prefix (command + subcommand) from a raw command string.
fn get_simple_command_prefix(command: &str) -> Option<String> {
    let tokens: Vec<&str> = command
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.len() < 2 {
        return None;
    }

    let subcmd = tokens[1];
    // Second token must look like a subcommand.
    if !RE_SUBCMD.is_match(subcmd) {
        return None;
    }

    Some(format!("{} {}", tokens[0], tokens[1]))
}

/// Raw shell tokenizer that preserves quotes for pattern matching.
fn shell_tokenize_raw(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    for c in command.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }

        if c == '\\' && !in_single_quote {
            escaped = true;
            current.push(c);
            continue;
        }

        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(c);
            continue;
        }

        if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(c);
            continue;
        }

        if c.is_whitespace() && !in_single_quote && !in_double_quote {
            if !current.is_empty() {
                tokens.push(current.clone());
                current.clear();
            }
            continue;
        }

        current.push(c);
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_pattern() {
        assert!(match_wildcard_pattern("git *", "git status"));
        assert!(match_wildcard_pattern("git *", "git diff --stat"));
        assert!(!match_wildcard_pattern("git *", "npm install"));
        assert!(match_wildcard_pattern("*", "anything"));
        assert!(match_wildcard_pattern("git diff*", "git diff --stat"));
        assert!(match_wildcard_pattern("test?file", "test.file"));
        assert!(!match_wildcard_pattern("test?file", "testfile"));
    }

    #[test]
    fn test_parse_permission_rule() {
        let rule = parse_permission_rule("git diff:*");
        assert!(matches!(rule.match_type, RuleMatchType::Prefix));
        assert_eq!(rule.command, "git diff ");

        let rule = parse_permission_rule("git status");
        assert!(matches!(rule.match_type, RuleMatchType::Exact));
        assert_eq!(rule.command, "git status");

        let rule = parse_permission_rule("npm *");
        assert!(matches!(rule.match_type, RuleMatchType::Glob));
        assert_eq!(rule.command, "npm *");
    }

    #[test]
    fn test_evaluator_bypass_mode() {
        let evaluator = BashPermissionEvaluator::new(vec![], PermissionMode::BypassPermissions);
        let cwd = std::path::PathBuf::from("/tmp/test");

        // Most commands are allowed in bypass mode.
        assert!(matches!(
            evaluator.evaluate("echo hello", &cwd),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn test_evaluator_explicit_rules() {
        let rules = vec![
            BashPermissionRule {
                pattern: "git status".to_string(),
                value: PermissionRuleValue::Allow,
            },
            BashPermissionRule {
                pattern: "rm -rf /".to_string(),
                value: PermissionRuleValue::Deny,
            },
        ];
        let evaluator = BashPermissionEvaluator::new(rules, PermissionMode::Default);
        let cwd = std::path::PathBuf::from("/tmp/test");

        // git status is read-only (auto-allowed even without rule), but the rule also matches.
        assert!(matches!(
            evaluator.evaluate("git status", &cwd),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn test_evaluator_prefix_rule() {
        let rules = vec![BashPermissionRule {
            pattern: "npm run:*".to_string(),
            value: PermissionRuleValue::Allow,
        }];
        let evaluator = BashPermissionEvaluator::new(rules, PermissionMode::Default);
        let cwd = std::path::PathBuf::from("/tmp/test");

        assert!(matches!(
            evaluator.evaluate("npm run build", &cwd),
            PermissionResult::Allow { .. }
        ));
        assert!(matches!(
            evaluator.evaluate("npm run test", &cwd),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn test_evaluator_accept_edits_mode() {
        let evaluator = BashPermissionEvaluator::new(vec![], PermissionMode::AcceptEdits);
        let cwd = std::path::PathBuf::from("/tmp/test");

        assert!(matches!(
            evaluator.evaluate("mkdir -p src/new", &cwd),
            PermissionResult::Allow { .. }
        ));
        assert!(matches!(
            evaluator.evaluate("touch newfile.txt", &cwd),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn test_evaluator_dont_ask_mode() {
        let evaluator = BashPermissionEvaluator::new(vec![], PermissionMode::DontAsk);
        let cwd = std::path::PathBuf::from("/tmp/test");

        // Read-only commands are still allowed.
        assert!(matches!(
            evaluator.evaluate("echo hello", &cwd),
            PermissionResult::Allow { .. }
        ));

        // Write commands are denied.
        assert!(matches!(
            evaluator.evaluate("touch newfile.txt", &cwd),
            PermissionResult::Deny { .. }
        ));
    }

    #[test]
    fn test_get_simple_command_prefix() {
        assert_eq!(
            get_simple_command_prefix("git commit -m 'fix'"),
            Some("git commit".to_string())
        );
        assert_eq!(
            get_simple_command_prefix("npm run build"),
            Some("npm run".to_string())
        );
        assert_eq!(get_simple_command_prefix("ls -la"), None);
        assert_eq!(get_simple_command_prefix("cat file.txt"), None);
    }

    #[test]
    fn test_strip_segment_wrappers() {
        assert_eq!(
            BashPermissionEvaluator::strip_segment_wrappers("NODE_ENV=prod npm run build"),
            "npm run build"
        );
        assert_eq!(
            BashPermissionEvaluator::strip_segment_wrappers("command git status"),
            "git status"
        );
        assert_eq!(
            BashPermissionEvaluator::strip_segment_wrappers("nice -n 10 make"),
            "make"
        );
    }
}
