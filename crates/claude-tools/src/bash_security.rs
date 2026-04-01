//! Bash command security validation.
//!
//! Ported from the TypeScript implementation in `tools/BashTool/bashSecurity.ts`,
//! `destructiveCommandWarning.ts`, `sedValidation.ts`, `readOnlyValidation.ts`,
//! and `pathValidation.ts`.
//!
//! Provides detection of dangerous/destructive commands, read-only validation,
//! sed command validation, and path safety checking.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// Static regexes — compiled once at first use, panics surface at startup.
// ---------------------------------------------------------------------------

// DestructiveCommandDetector patterns
static RE_GIT_RESET_HARD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bgit\s+reset\s+--hard\b").expect("static regex"));
static RE_GIT_PUSH_FORCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bgit\s+push\b[^;&|\n]*[ \t](--force|--force-with-lease|-f)\b")
        .expect("static regex")
});
static RE_GIT_CLEAN_F: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bgit\s+clean\b[^;&|\n]*-[a-zA-Z]*f").expect("static regex")
});
static RE_GIT_CHECKOUT_DOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bgit\s+checkout\s+(--\s+)?\.[ \t]*($|[;&|\n])").expect("static regex")
});
static RE_GIT_RESTORE_DOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bgit\s+restore\s+(--\s+)?\.[ \t]*($|[;&|\n])").expect("static regex")
});
static RE_GIT_STASH_DROP_CLEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bgit\s+stash[ \t]+(drop|clear)\b").expect("static regex"));
static RE_GIT_BRANCH_FORCE_DELETE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bgit\s+branch\s+(-D[ \t]|--delete\s+--force|--force\s+--delete)\b")
        .expect("static regex")
});
static RE_GIT_NO_VERIFY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bgit\s+(commit|push|merge)\b[^;&|\n]*--no-verify\b").expect("static regex")
});
static RE_GIT_COMMIT_AMEND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bgit\s+commit\b[^;&|\n]*--amend\b").expect("static regex"));
static RE_RM_RF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*[rR][a-zA-Z]*f|(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*f[a-zA-Z]*[rR]",
    )
    .expect("static regex")
});
static RE_RM_R: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*[rR]").expect("static regex")
});
static RE_RM_F: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*f").expect("static regex")
});
static RE_DROP_TRUNCATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(DROP|TRUNCATE)\s+(TABLE|DATABASE|SCHEMA)\b").expect("static regex")
});
static RE_DELETE_FROM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bDELETE\s+FROM\s+\w+[ \t]*(;|"|'|\n|$)"#).expect("static regex")
});
static RE_KUBECTL_DELETE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bkubectl\s+delete\b").expect("static regex"));
static RE_TERRAFORM_DESTROY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bterraform\s+destroy\b").expect("static regex"));

// detect() helper patterns
static RE_GIT_CLEAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bgit\s+clean\b").expect("static regex"));
static RE_FLAG_N: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-[a-zA-Z]*n").expect("static regex"));

// hostname config regex
static RE_HOSTNAME_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^hostname(?:\s+(?:-[a-zA-Z]|--[a-zA-Z-]+))*\s*$").expect("static regex")
});

// SedValidator patterns
static RE_SED_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*sed\s+").expect("static regex"));
static RE_PRINT_CMD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\d+|\d+,\d+)?p$").expect("static regex"));
static RE_SUBST_CMD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^s/(.*)$").expect("static regex"));
static RE_SUBST_FLAGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[gpimIM]*[1-9]?[gpimIM]*$").expect("static regex"));
static RE_DANGER_FLAGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-e[wWe]|-w[eE]").expect("static regex"));
static RE_NEGATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[/\d$]!").expect("static regex"));
static RE_TILDE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d\s*~\s*\d|,\s*~\s*\d|\$\s*~\s*\d").expect("static regex")
});
static RE_COMMA_OFFSET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r",\s*[+-]").expect("static regex"));
static RE_BS_TRICKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"s\\|\\[|#%@]").expect("static regex"));
static RE_ESCAPED_SLASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\/.*[wW]").expect("static regex"));
static RE_SLASH_WS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/[^/]*\s+[wWeE]").expect("static regex"));
static RE_PROPER_SUBST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^s/[^/]*/[^/]*/[^/]*$").expect("static regex"));
static RE_Y_CMD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"y([^\\\n])").expect("static regex"));
static RE_DANGEROUS_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[wWeE]").expect("static regex"));

// Write-command patterns for contains_dangerous_operations
static RE_WRITE_BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[wW]\s*\S+").expect("static regex"));
static RE_WRITE_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+\s*[wW]\s*\S+").expect("static regex"));
static RE_WRITE_DOLLAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\$\s*[wW]\s*\S+").expect("static regex"));
static RE_WRITE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/[^/]*/[IMim]*\s*[wW]\s*\S+").expect("static regex"));
static RE_WRITE_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+,\d+\s*[wW]\s*\S+").expect("static regex"));
static RE_WRITE_RANGE_DOLLAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+,\$\s*[wW]\s*\S+").expect("static regex"));

// Exec-command patterns for contains_dangerous_operations
static RE_EXEC_BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^e").expect("static regex"));
static RE_EXEC_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+\s*e").expect("static regex"));
static RE_EXEC_DOLLAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\$\s*e").expect("static regex"));
static RE_EXEC_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/[^/]*/[IMim]*\s*e").expect("static regex"));
static RE_EXEC_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+,\d+\s*e").expect("static regex"));
static RE_EXEC_RANGE_DOLLAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+,\$\s*e").expect("static regex"));

// validate_command patterns
static RE_OPERATOR_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(&&|\|\||;|>>?|<)").expect("static regex"));
static RE_IFS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$IFS|\$\{[^}]*IFS").expect("static regex"));

// strip_safe_redirections patterns
static RE_REDIR_2_TO_1: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+2\s*>&\s*1(?:\s|$)").expect("static regex"));
static RE_REDIR_DEV_NULL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[012]?\s*>\s*/dev/null(?:\s|$)").expect("static regex"));
static RE_REDIR_IN_DEV_NULL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*<\s*/dev/null(?:\s|$)").expect("static regex"));

// Command substitution / shell expansion patterns (validate_command)
static RE_PROC_SUBST_IN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<\(").expect("static regex"));
static RE_PROC_SUBST_OUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r">\(").expect("static regex"));
static RE_ZSH_PROC_SUBST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"=\(").expect("static regex"));
static RE_CMD_SUBST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\(").expect("static regex"));
static RE_PARAM_SUBST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{").expect("static regex"));
static RE_ARITH_SUBST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\[").expect("static regex"));
static RE_ZSH_PARAM_EXP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"~\[").expect("static regex"));
static RE_ZSH_GLOB_QUAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(e:").expect("static regex"));
static RE_ZSH_GLOB_EXEC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\+").expect("static regex"));
static RE_ZSH_ALWAYS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\}\s*always\s*\{").expect("static regex"));
static RE_PS_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<#").expect("static regex"));

// ---------------------------------------------------------------------------
// Public result types
// ---------------------------------------------------------------------------

/// Outcome of a security validation check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityVerdict {
    /// Command is safe to proceed (or no opinion).
    Allow,
    /// Command requires explicit user approval — includes a reason.
    Ask(String),
    /// Command is outright blocked.
    Deny(String),
}

/// Classification of a bash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandClassification {
    ReadOnly,
    Write,
    Destructive,
}

// ---------------------------------------------------------------------------
// DestructiveCommandDetector
// ---------------------------------------------------------------------------

/// Regex-based detection of dangerous/destructive bash commands.
///
/// Returns a human-readable warning string when a known destructive pattern is
/// detected. This is informational — it does not block execution on its own.
pub struct DestructiveCommandDetector {
    patterns: Vec<(&'static LazyLock<Regex>, &'static str)>,
}

impl Default for DestructiveCommandDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl DestructiveCommandDetector {
    pub fn new() -> Self {
        // References to LazyLock statics — compiled once, reused across all instances.
        let patterns: Vec<(&'static LazyLock<Regex>, &'static str)> = vec![
            // --- Git — data loss / hard to reverse ---
            (&RE_GIT_RESET_HARD, "Note: may discard uncommitted changes"),
            (&RE_GIT_PUSH_FORCE, "Note: may overwrite remote history"),
            (
                // git clean -f without --dry-run/-n (checked in two steps since
                // the `regex` crate does not support lookahead assertions)
                &RE_GIT_CLEAN_F,
                "Note: may permanently delete untracked files",
            ),
            (
                &RE_GIT_CHECKOUT_DOT,
                "Note: may discard all working tree changes",
            ),
            (
                &RE_GIT_RESTORE_DOT,
                "Note: may discard all working tree changes",
            ),
            (
                &RE_GIT_STASH_DROP_CLEAR,
                "Note: may permanently remove stashed changes",
            ),
            (
                &RE_GIT_BRANCH_FORCE_DELETE,
                "Note: may force-delete a branch",
            ),
            // --- Git — safety bypass ---
            (&RE_GIT_NO_VERIFY, "Note: may skip safety hooks"),
            (&RE_GIT_COMMIT_AMEND, "Note: may rewrite the last commit"),
            // --- File deletion ---
            (&RE_RM_RF, "Note: may recursively force-remove files"),
            (&RE_RM_R, "Note: may recursively remove files"),
            (&RE_RM_F, "Note: may force-remove files"),
            // --- Database ---
            (
                &RE_DROP_TRUNCATE,
                "Note: may drop or truncate database objects",
            ),
            (
                &RE_DELETE_FROM,
                "Note: may delete all rows from a database table",
            ),
            // --- Infrastructure ---
            (&RE_KUBECTL_DELETE, "Note: may delete Kubernetes resources"),
            (
                &RE_TERRAFORM_DESTROY,
                "Note: may destroy Terraform infrastructure",
            ),
        ];

        Self { patterns }
    }

    /// Returns a warning message if the command matches a known destructive
    /// pattern, or `None` if no match is found.
    pub fn detect(&self, command: &str) -> Option<&'static str> {
        // Pre-check: git clean -f with --dry-run or -n is safe
        let has_dry_run = RE_GIT_CLEAN.is_match(command)
            && (command.contains("--dry-run") || RE_FLAG_N.is_match(command));

        for (pattern, warning) in &self.patterns {
            if pattern.is_match(command) {
                // Skip git clean -f warning if --dry-run/-n is present
                if has_dry_run && warning.contains("untracked files") {
                    continue;
                }
                return Some(warning);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// CommandClassifier
// ---------------------------------------------------------------------------

/// Classifies commands as read-only, write, or destructive.
pub struct CommandClassifier {
    destructive: DestructiveCommandDetector,
    read_only: ReadOnlyValidator,
}

impl Default for CommandClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandClassifier {
    pub fn new() -> Self {
        Self {
            destructive: DestructiveCommandDetector::new(),
            read_only: ReadOnlyValidator::new(),
        }
    }

    /// Classify the given command string.
    pub fn classify(&self, command: &str) -> CommandClassification {
        if self.destructive.detect(command).is_some() {
            return CommandClassification::Destructive;
        }
        if self.read_only.is_read_only(command) {
            return CommandClassification::ReadOnly;
        }
        CommandClassification::Write
    }
}

// ---------------------------------------------------------------------------
// ReadOnlyValidator
// ---------------------------------------------------------------------------

/// Argument type expected by a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagArgType {
    /// Flag takes no argument.
    NoArg,
    /// Flag takes a string argument.
    StringArg,
    /// Flag takes a numeric argument.
    NumberArg,
}

/// Per-command read-only configuration.
struct CommandConfig {
    /// Mapping from flag (e.g. `"-n"`, `"--help"`) to whether it takes an arg.
    safe_flags: HashMap<String, FlagArgType>,
    /// Optional regex the *whole* command must match.
    regex: Option<Regex>,
    /// Optional callback: returns `true` if the command is dangerous.
    additional_dangerous_check: Option<fn(&str, &[String]) -> bool>,
    /// Whether the command respects POSIX `--`.
    respects_double_dash: bool,
}

/// Validates that a bash command is read-only.
///
/// Implements the allowlist-based approach from the TypeScript
/// `readOnlyValidation.ts` / `COMMAND_ALLOWLIST`.
pub struct ReadOnlyValidator {
    allowlist: HashMap<String, CommandConfig>,
    simple_readonly: HashSet<&'static str>,
}

impl Default for ReadOnlyValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadOnlyValidator {
    pub fn new() -> Self {
        let mut allowlist = HashMap::new();

        // --- fd / fdfind ---
        let fd_safe_flags = Self::fd_safe_flags();
        allowlist.insert(
            "fd".to_string(),
            CommandConfig {
                safe_flags: fd_safe_flags.clone(),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        );
        allowlist.insert(
            "fdfind".to_string(),
            CommandConfig {
                safe_flags: fd_safe_flags,
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        );

        // --- grep ---
        allowlist.insert("grep".to_string(), Self::grep_config());

        // --- rg ---
        allowlist.insert("rg".to_string(), Self::rg_config());

        // --- git read-only subcommands ---
        for (name, cfg) in Self::git_read_only_configs() {
            allowlist.insert(name, cfg);
        }

        // --- sed ---
        allowlist.insert("sed".to_string(), Self::sed_config());

        // --- sort ---
        allowlist.insert("sort".to_string(), Self::sort_config());

        // --- file ---
        allowlist.insert("file".to_string(), Self::file_config());

        // --- ps ---
        allowlist.insert("ps".to_string(), Self::ps_config());

        // --- base64 ---
        allowlist.insert("base64".to_string(), Self::base64_config());

        // --- xargs ---
        allowlist.insert("xargs".to_string(), Self::xargs_config());

        // --- man ---
        allowlist.insert("man".to_string(), Self::man_config());

        // --- netstat ---
        allowlist.insert("netstat".to_string(), Self::netstat_config());

        // --- tree ---
        allowlist.insert("tree".to_string(), Self::tree_config());

        // --- date ---
        allowlist.insert("date".to_string(), Self::date_config());

        // --- pgrep ---
        allowlist.insert("pgrep".to_string(), Self::pgrep_config());

        // --- lsof ---
        allowlist.insert("lsof".to_string(), Self::lsof_config());

        // --- ss ---
        allowlist.insert("ss".to_string(), Self::ss_config());

        // --- hostname ---
        allowlist.insert("hostname".to_string(), Self::hostname_config());

        // --- checksum commands ---
        for name in &["sha256sum", "sha1sum", "md5sum"] {
            allowlist.insert(name.to_string(), Self::checksum_config());
        }

        // Simple commands that are always safe with any flags.
        let simple_readonly: HashSet<&'static str> = [
            "echo", "printf", "true", "false", "pwd", "whoami", "uname", "id", "type", "which",
            "env", "printenv",
        ]
        .into_iter()
        .collect();

        Self {
            allowlist,
            simple_readonly,
        }
    }

    /// Returns `true` if the command is read-only safe.
    pub fn is_read_only(&self, command: &str) -> bool {
        // Split on command operators (&&, ||, ;, |) — each segment must be safe.
        let segments = split_command(command);
        for segment in &segments {
            if !self.is_segment_read_only(segment.trim()) {
                return false;
            }
        }
        true
    }

    fn is_segment_read_only(&self, command: &str) -> bool {
        if command.is_empty() {
            return true;
        }

        let tokens = shell_tokenize(command);
        if tokens.is_empty() {
            return true;
        }

        // Reject any token containing `$` (variable expansion can defeat analysis).
        if tokens.iter().any(|t| t.contains('$')) {
            return false;
        }

        // Check simple read-only commands.
        if self.simple_readonly.contains(tokens[0].as_str()) {
            return true;
        }

        // Try multi-word match first ("git diff", etc.), then single-word.
        let (config, cmd_token_count) = if tokens.len() >= 2 {
            let two_word = format!("{} {}", tokens[0], tokens[1]);
            if let Some(cfg) = self.allowlist.get(&two_word) {
                (cfg, 2usize)
            } else if let Some(cfg) = self.allowlist.get(&tokens[0]) {
                (cfg, 1usize)
            } else {
                return false;
            }
        } else if let Some(cfg) = self.allowlist.get(&tokens[0]) {
            (cfg, 1usize)
        } else {
            return false;
        };

        // Validate flags.
        let args: Vec<String> = tokens[cmd_token_count..].to_vec();
        if !self.validate_flags(&args, config) {
            return false;
        }

        // Optional regex check.
        if let Some(ref re) = config.regex {
            if !re.is_match(command) {
                return false;
            }
        }

        // Reject backticks.
        if command.contains('`') && config.regex.is_none() {
            return false;
        }

        // Additional callback.
        if let Some(check) = config.additional_dangerous_check {
            if check(command, &args) {
                return false;
            }
        }

        true
    }

    /// Validate flags against an allowlist.
    fn validate_flags(&self, args: &[String], config: &CommandConfig) -> bool {
        let mut i = 0;
        let mut after_double_dash = false;

        while i < args.len() {
            let arg = &args[i];

            if !after_double_dash && arg == "--" {
                if config.respects_double_dash {
                    after_double_dash = true;
                }
                i += 1;
                continue;
            }

            if after_double_dash || !arg.starts_with('-') {
                // Positional argument — allowed (path validation is separate).
                i += 1;
                continue;
            }

            // Long flag with = (e.g., --foo=bar).
            if arg.starts_with("--") && arg.contains('=') {
                let flag_name: String = arg.split('=').next().unwrap().to_string();
                if config.safe_flags.contains_key(&flag_name) {
                    i += 1;
                    continue;
                }
                return false;
            }

            // Long flag.
            if arg.starts_with("--") {
                if let Some(&arg_type) = config.safe_flags.get(arg.as_str()) {
                    match arg_type {
                        FlagArgType::NoArg => {
                            i += 1;
                        }
                        FlagArgType::StringArg | FlagArgType::NumberArg => {
                            i += 2; // skip flag + value
                        }
                    }
                    continue;
                }
                return false;
            }

            // Short flag — may be combined (e.g., -nE).
            if arg.len() == 2 {
                // Single short flag like -n.
                if let Some(&arg_type) = config.safe_flags.get(arg.as_str()) {
                    match arg_type {
                        FlagArgType::NoArg => {
                            i += 1;
                        }
                        FlagArgType::StringArg | FlagArgType::NumberArg => {
                            i += 2;
                        }
                    }
                    continue;
                }
                return false;
            }

            // Combined short flags (e.g., -nE).
            // Check each character.
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut all_valid = true;
            for ch in &chars {
                let flag = format!("-{}", ch);
                if !config.safe_flags.contains_key(&flag) {
                    all_valid = false;
                    break;
                }
            }
            if !all_valid {
                return false;
            }
            i += 1;
        }

        true
    }

    // --- Command config builders ---

    fn flags(entries: &[(&str, FlagArgType)]) -> HashMap<String, FlagArgType> {
        entries.iter().map(|&(k, v)| (k.to_string(), v)).collect()
    }

    fn fd_safe_flags() -> HashMap<String, FlagArgType> {
        use FlagArgType::*;
        Self::flags(&[
            ("-h", NoArg),
            ("--help", NoArg),
            ("-V", NoArg),
            ("--version", NoArg),
            ("-H", NoArg),
            ("--hidden", NoArg),
            ("-I", NoArg),
            ("--no-ignore", NoArg),
            ("--no-ignore-vcs", NoArg),
            ("--no-ignore-parent", NoArg),
            ("-s", NoArg),
            ("--case-sensitive", NoArg),
            ("-i", NoArg),
            ("--ignore-case", NoArg),
            ("-g", NoArg),
            ("--glob", NoArg),
            ("--regex", NoArg),
            ("-F", NoArg),
            ("--fixed-strings", NoArg),
            ("-a", NoArg),
            ("--absolute-path", NoArg),
            ("-L", NoArg),
            ("--follow", NoArg),
            ("-p", NoArg),
            ("--full-path", NoArg),
            ("-0", NoArg),
            ("--print0", NoArg),
            ("-d", NumberArg),
            ("--max-depth", NumberArg),
            ("--min-depth", NumberArg),
            ("--exact-depth", NumberArg),
            ("-t", StringArg),
            ("--type", StringArg),
            ("-e", StringArg),
            ("--extension", StringArg),
            ("-S", StringArg),
            ("--size", StringArg),
            ("--changed-within", StringArg),
            ("--changed-before", StringArg),
            ("-o", StringArg),
            ("--owner", StringArg),
            ("-E", StringArg),
            ("--exclude", StringArg),
            ("--ignore-file", StringArg),
            ("-c", StringArg),
            ("--color", StringArg),
            ("-j", NumberArg),
            ("--threads", NumberArg),
            ("--max-buffer-time", StringArg),
            ("--max-results", NumberArg),
            ("-1", NoArg),
            ("-q", NoArg),
            ("--quiet", NoArg),
            ("--show-errors", NoArg),
            ("--strip-cwd-prefix", NoArg),
            ("--one-file-system", NoArg),
            ("--prune", NoArg),
            ("--search-path", StringArg),
            ("--base-directory", StringArg),
            ("--path-separator", StringArg),
            ("--batch-size", NumberArg),
            ("--no-require-git", NoArg),
            ("--hyperlink", StringArg),
            ("--and", StringArg),
            ("--format", StringArg),
        ])
    }

    fn grep_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-e", StringArg),
                ("--regexp", StringArg),
                ("-f", StringArg),
                ("--file", StringArg),
                ("-F", NoArg),
                ("--fixed-strings", NoArg),
                ("-G", NoArg),
                ("--basic-regexp", NoArg),
                ("-E", NoArg),
                ("--extended-regexp", NoArg),
                ("-P", NoArg),
                ("--perl-regexp", NoArg),
                ("-i", NoArg),
                ("--ignore-case", NoArg),
                ("--no-ignore-case", NoArg),
                ("-v", NoArg),
                ("--invert-match", NoArg),
                ("-w", NoArg),
                ("--word-regexp", NoArg),
                ("-x", NoArg),
                ("--line-regexp", NoArg),
                ("-c", NoArg),
                ("--count", NoArg),
                ("--color", StringArg),
                ("--colour", StringArg),
                ("-L", NoArg),
                ("--files-without-match", NoArg),
                ("-l", NoArg),
                ("--files-with-matches", NoArg),
                ("-m", NumberArg),
                ("--max-count", NumberArg),
                ("-o", NoArg),
                ("--only-matching", NoArg),
                ("-q", NoArg),
                ("--quiet", NoArg),
                ("--silent", NoArg),
                ("-s", NoArg),
                ("--no-messages", NoArg),
                ("-b", NoArg),
                ("--byte-offset", NoArg),
                ("-H", NoArg),
                ("--with-filename", NoArg),
                ("-h", NoArg),
                ("--no-filename", NoArg),
                ("--label", StringArg),
                ("-n", NoArg),
                ("--line-number", NoArg),
                ("-T", NoArg),
                ("--initial-tab", NoArg),
                ("-u", NoArg),
                ("--unix-byte-offsets", NoArg),
                ("-Z", NoArg),
                ("--null", NoArg),
                ("-z", NoArg),
                ("--null-data", NoArg),
                ("-A", NumberArg),
                ("--after-context", NumberArg),
                ("-B", NumberArg),
                ("--before-context", NumberArg),
                ("-C", NumberArg),
                ("--context", NumberArg),
                ("--group-separator", StringArg),
                ("--no-group-separator", NoArg),
                ("-a", NoArg),
                ("--text", NoArg),
                ("--binary-files", StringArg),
                ("-D", StringArg),
                ("--devices", StringArg),
                ("-d", StringArg),
                ("--directories", StringArg),
                ("--exclude", StringArg),
                ("--exclude-from", StringArg),
                ("--exclude-dir", StringArg),
                ("--include", StringArg),
                ("-r", NoArg),
                ("--recursive", NoArg),
                ("-R", NoArg),
                ("--dereference-recursive", NoArg),
                ("--line-buffered", NoArg),
                ("-U", NoArg),
                ("--binary", NoArg),
                ("--help", NoArg),
                ("-V", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn rg_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-e", StringArg),
                ("--regexp", StringArg),
                ("-f", StringArg),
                ("--file", StringArg),
                ("-F", NoArg),
                ("--fixed-strings", NoArg),
                ("-i", NoArg),
                ("--ignore-case", NoArg),
                ("-S", NoArg),
                ("--smart-case", NoArg),
                ("-s", NoArg),
                ("--case-sensitive", NoArg),
                ("-v", NoArg),
                ("--invert-match", NoArg),
                ("-w", NoArg),
                ("--word-regexp", NoArg),
                ("-x", NoArg),
                ("--line-regexp", NoArg),
                ("-c", NoArg),
                ("--count", NoArg),
                ("--count-matches", NoArg),
                ("--color", StringArg),
                ("--colours", StringArg),
                ("-l", NoArg),
                ("--files-with-matches", NoArg),
                ("--files-without-match", NoArg),
                ("-m", NumberArg),
                ("--max-count", NumberArg),
                ("-o", NoArg),
                ("--only-matching", NoArg),
                ("-q", NoArg),
                ("--quiet", NoArg),
                ("-n", NoArg),
                ("--line-number", NoArg),
                ("-N", NoArg),
                ("--no-line-number", NoArg),
                ("-H", NoArg),
                ("--with-filename", NoArg),
                ("--no-filename", NoArg),
                ("-b", NoArg),
                ("--byte-offset", NoArg),
                ("-0", NoArg),
                ("--null", NoArg),
                ("--null-data", NoArg),
                ("-A", NumberArg),
                ("--after-context", NumberArg),
                ("-B", NumberArg),
                ("--before-context", NumberArg),
                ("-C", NumberArg),
                ("--context", NumberArg),
                ("-U", NoArg),
                ("--multiline", NoArg),
                ("--multiline-dotall", NoArg),
                ("-P", NoArg),
                ("--pcre2", NoArg),
                ("-a", NoArg),
                ("--text", NoArg),
                ("-z", NoArg),
                ("--search-zip", NoArg),
                ("--binary", NoArg),
                ("-L", NoArg),
                ("--follow", NoArg),
                ("--hidden", NoArg),
                ("--no-hidden", NoArg),
                ("--no-ignore", NoArg),
                ("--no-ignore-dot", NoArg),
                ("--no-ignore-exclude", NoArg),
                ("--no-ignore-files", NoArg),
                ("--no-ignore-global", NoArg),
                ("--no-ignore-parent", NoArg),
                ("--no-ignore-vcs", NoArg),
                ("-g", StringArg),
                ("--glob", StringArg),
                ("--iglob", StringArg),
                ("-t", StringArg),
                ("--type", StringArg),
                ("-T", StringArg),
                ("--type-not", StringArg),
                ("--type-add", StringArg),
                ("--type-clear", StringArg),
                ("--type-list", NoArg),
                ("--max-depth", NumberArg),
                ("-d", NumberArg),
                ("--max-filesize", StringArg),
                ("-j", NumberArg),
                ("--threads", NumberArg),
                ("--sort", StringArg),
                ("--sortr", StringArg),
                ("--stats", NoArg),
                ("--trim", NoArg),
                ("--no-unicode", NoArg),
                ("--unicode", NoArg),
                ("--one-file-system", NoArg),
                ("--heading", NoArg),
                ("--no-heading", NoArg),
                ("--vimgrep", NoArg),
                ("--json", NoArg),
                ("-p", NoArg),
                ("--pretty", NoArg),
                ("--help", NoArg),
                ("-V", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn git_read_only_configs() -> Vec<(String, CommandConfig)> {
        use FlagArgType::*;
        let mut configs = Vec::new();

        // git diff
        configs.push((
            "git diff".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("--stat", NoArg),
                    ("--numstat", NoArg),
                    ("--shortstat", NoArg),
                    ("--name-only", NoArg),
                    ("--name-status", NoArg),
                    ("--no-renames", NoArg),
                    ("--check", NoArg),
                    ("--cached", NoArg),
                    ("--staged", NoArg),
                    ("--no-index", NoArg),
                    ("--merge-base", NoArg),
                    ("-p", NoArg),
                    ("-u", NoArg),
                    ("--patch", NoArg),
                    ("-s", NoArg),
                    ("--no-patch", NoArg),
                    ("--raw", NoArg),
                    ("--minimal", NoArg),
                    ("--patience", NoArg),
                    ("--histogram", NoArg),
                    ("--diff-algorithm", StringArg),
                    ("-U", NumberArg),
                    ("--unified", NumberArg),
                    ("-W", NoArg),
                    ("--function-context", NoArg),
                    ("--ignore-space-change", NoArg),
                    ("-b", NoArg),
                    ("--ignore-all-space", NoArg),
                    ("-w", NoArg),
                    ("--ignore-blank-lines", NoArg),
                    ("--color", StringArg),
                    ("--no-color", NoArg),
                    ("--word-diff", NoArg),
                    ("--word-diff-regex", StringArg),
                    ("--color-moved", NoArg),
                    ("--color-words", NoArg),
                    ("-R", NoArg),
                    ("--relative", NoArg),
                    ("--src-prefix", StringArg),
                    ("--dst-prefix", StringArg),
                    ("--no-prefix", NoArg),
                    ("--abbrev", NumberArg),
                    ("--full-index", NoArg),
                    ("--binary", NoArg),
                    ("--find-renames", NoArg),
                    ("-M", NoArg),
                    ("--find-copies", NoArg),
                    ("-C", NoArg),
                    ("--diff-filter", StringArg),
                    ("-z", NoArg),
                ]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        // git log
        configs.push((
            "git log".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("-n", NumberArg),
                    ("--max-count", NumberArg),
                    ("--oneline", NoArg),
                    ("--graph", NoArg),
                    ("--all", NoArg),
                    ("--stat", NoArg),
                    ("--shortstat", NoArg),
                    ("--name-only", NoArg),
                    ("--name-status", NoArg),
                    ("-p", NoArg),
                    ("--patch", NoArg),
                    ("-s", NoArg),
                    ("--no-patch", NoArg),
                    ("--format", StringArg),
                    ("--pretty", StringArg),
                    ("--abbrev-commit", NoArg),
                    ("--decorate", NoArg),
                    ("--no-decorate", NoArg),
                    ("--first-parent", NoArg),
                    ("--merges", NoArg),
                    ("--no-merges", NoArg),
                    ("--author", StringArg),
                    ("--committer", StringArg),
                    ("--grep", StringArg),
                    ("--since", StringArg),
                    ("--after", StringArg),
                    ("--until", StringArg),
                    ("--before", StringArg),
                    ("--follow", NoArg),
                    ("--diff-filter", StringArg),
                    ("-S", StringArg),
                    ("-G", StringArg),
                    ("--reverse", NoArg),
                    ("--date", StringArg),
                    ("--color", StringArg),
                    ("--no-color", NoArg),
                    ("-z", NoArg),
                ]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        // git status
        configs.push((
            "git status".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("-s", NoArg),
                    ("--short", NoArg),
                    ("-b", NoArg),
                    ("--branch", NoArg),
                    ("--porcelain", NoArg),
                    ("--long", NoArg),
                    ("-v", NoArg),
                    ("--verbose", NoArg),
                    ("-u", StringArg),
                    ("--untracked-files", StringArg),
                    ("--ignored", NoArg),
                    ("--ignore-submodules", StringArg),
                    ("-z", NoArg),
                    ("--column", NoArg),
                    ("--no-column", NoArg),
                    ("--ahead-behind", NoArg),
                    ("--no-ahead-behind", NoArg),
                    ("--renames", NoArg),
                    ("--no-renames", NoArg),
                ]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        // git show
        configs.push((
            "git show".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("--stat", NoArg),
                    ("--numstat", NoArg),
                    ("--shortstat", NoArg),
                    ("--name-only", NoArg),
                    ("--name-status", NoArg),
                    ("-p", NoArg),
                    ("--patch", NoArg),
                    ("-s", NoArg),
                    ("--no-patch", NoArg),
                    ("--format", StringArg),
                    ("--pretty", StringArg),
                    ("--oneline", NoArg),
                    ("--abbrev-commit", NoArg),
                    ("--color", StringArg),
                    ("--no-color", NoArg),
                    ("-z", NoArg),
                ]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        // git branch (list mode only)
        configs.push((
            "git branch".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("-a", NoArg),
                    ("--all", NoArg),
                    ("-r", NoArg),
                    ("--remotes", NoArg),
                    ("-l", NoArg),
                    ("--list", NoArg),
                    ("-v", NoArg),
                    ("--verbose", NoArg),
                    ("--no-color", NoArg),
                    ("--color", StringArg),
                    ("--sort", StringArg),
                    ("--format", StringArg),
                    ("--contains", StringArg),
                    ("--no-contains", StringArg),
                    ("--merged", StringArg),
                    ("--no-merged", StringArg),
                    ("--points-at", StringArg),
                    ("--column", NoArg),
                    ("--no-column", NoArg),
                    ("--abbrev", NumberArg),
                ]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        // git stash list
        configs.push((
            "git stash list".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("--oneline", NoArg),
                    ("-p", NoArg),
                    ("--patch", NoArg),
                    ("--stat", NoArg),
                    ("--format", StringArg),
                    ("--pretty", StringArg),
                    ("--date", StringArg),
                ]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        // git ls-files
        configs.push((
            "git ls-files".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("-c", NoArg),
                    ("--cached", NoArg),
                    ("-d", NoArg),
                    ("--deleted", NoArg),
                    ("-m", NoArg),
                    ("--modified", NoArg),
                    ("-o", NoArg),
                    ("--others", NoArg),
                    ("-i", NoArg),
                    ("--ignored", NoArg),
                    ("-s", NoArg),
                    ("--stage", NoArg),
                    ("-u", NoArg),
                    ("--unmerged", NoArg),
                    ("-z", NoArg),
                    ("--exclude", StringArg),
                    ("-x", StringArg),
                    ("--exclude-from", StringArg),
                    ("-X", StringArg),
                    ("--exclude-per-directory", StringArg),
                    ("--exclude-standard", NoArg),
                    ("--error-unmatch", NoArg),
                    ("--full-name", NoArg),
                    ("--abbrev", NumberArg),
                ]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        // git rev-parse
        configs.push((
            "git rev-parse".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("--abbrev-ref", NoArg),
                    ("--short", NoArg),
                    ("--verify", NoArg),
                    ("--symbolic-full-name", NoArg),
                    ("--show-toplevel", NoArg),
                    ("--show-cdup", NoArg),
                    ("--show-prefix", NoArg),
                    ("--git-dir", NoArg),
                    ("--git-common-dir", NoArg),
                    ("--is-inside-work-tree", NoArg),
                    ("--is-inside-git-dir", NoArg),
                    ("--is-bare-repository", NoArg),
                    ("--absolute-git-dir", NoArg),
                    ("--all", NoArg),
                ]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        // git remote
        configs.push((
            "git remote".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[("-v", NoArg), ("--verbose", NoArg)]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        // git ls-remote
        configs.push((
            "git ls-remote".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("--heads", NoArg),
                    ("--tags", NoArg),
                    ("--refs", NoArg),
                    ("--quiet", NoArg),
                    ("-q", NoArg),
                    ("--sort", StringArg),
                ]),
                regex: None,
                // Block URLs to prevent exfiltration
                additional_dangerous_check: Some(|_raw, args| {
                    for arg in args {
                        if !arg.starts_with('-')
                            && (arg.contains("://") || arg.contains('@') || arg.contains(':')) {
                                return true;
                            }
                    }
                    false
                }),
                respects_double_dash: true,
            },
        ));

        // git config (read-only)
        configs.push((
            "git config".to_string(),
            CommandConfig {
                safe_flags: Self::flags(&[
                    ("--get", StringArg),
                    ("--get-all", StringArg),
                    ("--get-regexp", StringArg),
                    ("-l", NoArg),
                    ("--list", NoArg),
                    ("--local", NoArg),
                    ("--global", NoArg),
                    ("--system", NoArg),
                    ("--show-origin", NoArg),
                    ("--show-scope", NoArg),
                    ("-z", NoArg),
                    ("--null", NoArg),
                    ("--name-only", NoArg),
                    ("--type", StringArg),
                    ("--default", StringArg),
                ]),
                regex: None,
                additional_dangerous_check: None,
                respects_double_dash: true,
            },
        ));

        configs
    }

    fn sed_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("--expression", StringArg),
                ("-e", StringArg),
                ("--quiet", NoArg),
                ("--silent", NoArg),
                ("-n", NoArg),
                ("--regexp-extended", NoArg),
                ("-r", NoArg),
                ("--posix", NoArg),
                ("-E", NoArg),
                ("--line-length", NumberArg),
                ("-l", NumberArg),
                ("--zero-terminated", NoArg),
                ("-z", NoArg),
                ("--separate", NoArg),
                ("-s", NoArg),
                ("--unbuffered", NoArg),
                ("-u", NoArg),
                ("--debug", NoArg),
                ("--help", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: Some(|raw, _args| {
                !SedValidator::is_allowed_by_allowlist(raw, false)
            }),
            respects_double_dash: true,
        }
    }

    fn sort_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("--ignore-leading-blanks", NoArg),
                ("-b", NoArg),
                ("--dictionary-order", NoArg),
                ("-d", NoArg),
                ("--ignore-case", NoArg),
                ("-f", NoArg),
                ("--general-numeric-sort", NoArg),
                ("-g", NoArg),
                ("--human-numeric-sort", NoArg),
                ("-h", NoArg),
                ("--ignore-nonprinting", NoArg),
                ("-i", NoArg),
                ("--month-sort", NoArg),
                ("-M", NoArg),
                ("--numeric-sort", NoArg),
                ("-n", NoArg),
                ("--random-sort", NoArg),
                ("-R", NoArg),
                ("--reverse", NoArg),
                ("-r", NoArg),
                ("--sort", StringArg),
                ("--stable", NoArg),
                ("-s", NoArg),
                ("--unique", NoArg),
                ("-u", NoArg),
                ("--version-sort", NoArg),
                ("-V", NoArg),
                ("--zero-terminated", NoArg),
                ("-z", NoArg),
                ("--key", StringArg),
                ("-k", StringArg),
                ("--field-separator", StringArg),
                ("-t", StringArg),
                ("--check", NoArg),
                ("-c", NoArg),
                ("--check-char-order", NoArg),
                ("-C", NoArg),
                ("--merge", NoArg),
                ("-m", NoArg),
                ("--buffer-size", StringArg),
                ("-S", StringArg),
                ("--parallel", NumberArg),
                ("--batch-size", NumberArg),
                ("--help", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn file_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("--brief", NoArg),
                ("-b", NoArg),
                ("--mime", NoArg),
                ("-i", NoArg),
                ("--mime-type", NoArg),
                ("--mime-encoding", NoArg),
                ("--apple", NoArg),
                ("--check-encoding", NoArg),
                ("-c", NoArg),
                ("--exclude", StringArg),
                ("--exclude-quiet", StringArg),
                ("--print0", NoArg),
                ("-0", NoArg),
                ("-f", StringArg),
                ("-F", StringArg),
                ("--separator", StringArg),
                ("--help", NoArg),
                ("--version", NoArg),
                ("-v", NoArg),
                ("--no-dereference", NoArg),
                ("-h", NoArg),
                ("--dereference", NoArg),
                ("-L", NoArg),
                ("--magic-file", StringArg),
                ("-m", StringArg),
                ("--keep-going", NoArg),
                ("-k", NoArg),
                ("--list", NoArg),
                ("-l", NoArg),
                ("--no-buffer", NoArg),
                ("-n", NoArg),
                ("--preserve-date", NoArg),
                ("-p", NoArg),
                ("--raw", NoArg),
                ("-r", NoArg),
                ("-s", NoArg),
                ("--special-files", NoArg),
                ("--uncompress", NoArg),
                ("-z", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn ps_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-e", NoArg),
                ("-A", NoArg),
                ("-a", NoArg),
                ("-d", NoArg),
                ("-N", NoArg),
                ("--deselect", NoArg),
                ("-f", NoArg),
                ("-F", NoArg),
                ("-l", NoArg),
                ("-j", NoArg),
                ("-y", NoArg),
                ("-w", NoArg),
                ("-ww", NoArg),
                ("--width", NumberArg),
                ("-c", NoArg),
                ("-H", NoArg),
                ("--forest", NoArg),
                ("--headers", NoArg),
                ("--no-headers", NoArg),
                ("-n", StringArg),
                ("--sort", StringArg),
                ("-L", NoArg),
                ("-T", NoArg),
                ("-m", NoArg),
                ("-C", StringArg),
                ("-G", StringArg),
                ("-g", StringArg),
                ("-p", StringArg),
                ("--pid", StringArg),
                ("-q", StringArg),
                ("--quick-pid", StringArg),
                ("-s", StringArg),
                ("--sid", StringArg),
                ("-t", StringArg),
                ("--tty", StringArg),
                ("-U", StringArg),
                ("-u", StringArg),
                ("--user", StringArg),
                ("--help", NoArg),
                ("--info", NoArg),
                ("-V", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            // Block BSD-style 'e' modifier which shows environment variables
            additional_dangerous_check: Some(|_raw, args| {
                args.iter().any(|a| {
                    !a.starts_with('-')
                        && a.chars().all(|c| c.is_ascii_alphabetic())
                        && a.contains('e')
                })
            }),
            respects_double_dash: true,
        }
    }

    fn base64_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-d", NoArg),
                ("-D", NoArg),
                ("--decode", NoArg),
                ("-b", NumberArg),
                ("--break", NumberArg),
                ("-w", NumberArg),
                ("--wrap", NumberArg),
                ("-i", StringArg),
                ("--input", StringArg),
                ("--ignore-garbage", NoArg),
                ("-h", NoArg),
                ("--help", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: false,
        }
    }

    fn xargs_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-I", StringArg),
                ("-n", NumberArg),
                ("-P", NumberArg),
                ("-L", NumberArg),
                ("-s", NumberArg),
                ("-E", StringArg),
                ("-0", NoArg),
                ("-t", NoArg),
                ("-r", NoArg),
                ("-x", NoArg),
                ("-d", StringArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn man_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-a", NoArg),
                ("--all", NoArg),
                ("-d", NoArg),
                ("-f", NoArg),
                ("--whatis", NoArg),
                ("-h", NoArg),
                ("-k", NoArg),
                ("--apropos", NoArg),
                ("-l", StringArg),
                ("-w", NoArg),
                ("-S", StringArg),
                ("-s", StringArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn netstat_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-a", NoArg),
                ("-L", NoArg),
                ("-l", NoArg),
                ("-n", NoArg),
                ("-f", StringArg),
                ("-g", NoArg),
                ("-i", NoArg),
                ("-I", StringArg),
                ("-s", NoArg),
                ("-r", NoArg),
                ("-m", NoArg),
                ("-v", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn tree_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-a", NoArg),
                ("-d", NoArg),
                ("-l", NoArg),
                ("-f", NoArg),
                ("-x", NoArg),
                ("-L", NumberArg),
                ("-P", StringArg),
                ("-I", StringArg),
                ("--gitignore", NoArg),
                ("--gitfile", StringArg),
                ("--ignore-case", NoArg),
                ("--matchdirs", NoArg),
                ("--metafirst", NoArg),
                ("--prune", NoArg),
                ("--info", NoArg),
                ("--infofile", StringArg),
                ("--noreport", NoArg),
                ("--charset", StringArg),
                ("--filelimit", NumberArg),
                ("-q", NoArg),
                ("-N", NoArg),
                ("-Q", NoArg),
                ("-p", NoArg),
                ("-u", NoArg),
                ("-g", NoArg),
                ("-s", NoArg),
                ("-h", NoArg),
                ("--si", NoArg),
                ("--du", NoArg),
                ("-D", NoArg),
                ("--timefmt", StringArg),
                ("-F", NoArg),
                ("--inodes", NoArg),
                ("--device", NoArg),
                ("-v", NoArg),
                ("-t", NoArg),
                ("-c", NoArg),
                ("-U", NoArg),
                ("-r", NoArg),
                ("--dirsfirst", NoArg),
                ("--filesfirst", NoArg),
                ("--sort", StringArg),
                ("-i", NoArg),
                ("-A", NoArg),
                ("-S", NoArg),
                ("-n", NoArg),
                ("-C", NoArg),
                ("-X", NoArg),
                ("-J", NoArg),
                ("-H", StringArg),
                ("--nolinks", NoArg),
                ("--hintro", StringArg),
                ("--houtro", StringArg),
                ("-T", StringArg),
                ("--hyperlink", NoArg),
                ("--scheme", StringArg),
                ("--authority", StringArg),
                ("--fromfile", NoArg),
                ("--fromtabfile", NoArg),
                ("--fflinks", NoArg),
                ("--help", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn date_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-d", StringArg),
                ("--date", StringArg),
                ("-r", StringArg),
                ("--reference", StringArg),
                ("-u", NoArg),
                ("--utc", NoArg),
                ("--universal", NoArg),
                ("-I", NoArg),
                ("--iso-8601", StringArg),
                ("-R", NoArg),
                ("--rfc-email", NoArg),
                ("--rfc-3339", StringArg),
                ("--debug", NoArg),
                ("--help", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            // Block positional args that don't start with + (format strings)
            additional_dangerous_check: Some(|_raw, args| {
                let flags_with_args: HashSet<&str> = [
                    "-d",
                    "--date",
                    "-r",
                    "--reference",
                    "--iso-8601",
                    "--rfc-3339",
                ]
                .into_iter()
                .collect();
                let mut i = 0;
                while i < args.len() {
                    let token = &args[i];
                    if token.starts_with("--") && token.contains('=') {
                        i += 1;
                    } else if token.starts_with('-') {
                        if flags_with_args.contains(token.as_str()) {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    } else {
                        if !token.starts_with('+') {
                            return true; // Dangerous
                        }
                        i += 1;
                    }
                }
                false
            }),
            respects_double_dash: true,
        }
    }

    fn pgrep_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-d", StringArg),
                ("--delimiter", StringArg),
                ("-l", NoArg),
                ("--list-name", NoArg),
                ("-a", NoArg),
                ("--list-full", NoArg),
                ("-v", NoArg),
                ("--inverse", NoArg),
                ("-w", NoArg),
                ("--lightweight", NoArg),
                ("-c", NoArg),
                ("--count", NoArg),
                ("-f", NoArg),
                ("--full", NoArg),
                ("-g", StringArg),
                ("--pgroup", StringArg),
                ("-G", StringArg),
                ("--group", StringArg),
                ("-i", NoArg),
                ("--ignore-case", NoArg),
                ("-n", NoArg),
                ("--newest", NoArg),
                ("-o", NoArg),
                ("--oldest", NoArg),
                ("-O", StringArg),
                ("--older", StringArg),
                ("-P", StringArg),
                ("--parent", StringArg),
                ("-s", StringArg),
                ("--session", StringArg),
                ("-t", StringArg),
                ("--terminal", StringArg),
                ("-u", StringArg),
                ("--euid", StringArg),
                ("-U", StringArg),
                ("--uid", StringArg),
                ("-x", NoArg),
                ("--exact", NoArg),
                ("-F", StringArg),
                ("--pidfile", StringArg),
                ("-L", NoArg),
                ("--logpidfile", NoArg),
                ("-r", StringArg),
                ("--runstates", StringArg),
                ("--ns", StringArg),
                ("--nslist", StringArg),
                ("--help", NoArg),
                ("-V", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn lsof_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-?", NoArg),
                ("-h", NoArg),
                ("-v", NoArg),
                ("-a", NoArg),
                ("-b", NoArg),
                ("-C", NoArg),
                ("-l", NoArg),
                ("-n", NoArg),
                ("-N", NoArg),
                ("-O", NoArg),
                ("-P", NoArg),
                ("-Q", NoArg),
                ("-R", NoArg),
                ("-t", NoArg),
                ("-U", NoArg),
                ("-V", NoArg),
                ("-X", NoArg),
                ("-H", NoArg),
                ("-E", NoArg),
                ("-F", NoArg),
                ("-g", NoArg),
                ("-i", NoArg),
                ("-K", NoArg),
                ("-L", NoArg),
                ("-o", NoArg),
                ("-r", NoArg),
                ("-s", NoArg),
                ("-S", NoArg),
                ("-T", NoArg),
                ("-x", NoArg),
                ("-A", StringArg),
                ("-c", StringArg),
                ("-d", StringArg),
                ("-e", StringArg),
                ("-k", StringArg),
                ("-p", StringArg),
                ("-u", StringArg),
            ]),
            regex: None,
            // Block +m (create mount supplement file) — writes to disk.
            additional_dangerous_check: Some(|_raw, args| {
                args.iter().any(|a| a == "+m" || a.starts_with("+m"))
            }),
            respects_double_dash: true,
        }
    }

    fn ss_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-h", NoArg),
                ("--help", NoArg),
                ("-V", NoArg),
                ("--version", NoArg),
                ("-n", NoArg),
                ("--numeric", NoArg),
                ("-r", NoArg),
                ("--resolve", NoArg),
                ("-a", NoArg),
                ("--all", NoArg),
                ("-l", NoArg),
                ("--listening", NoArg),
                ("-o", NoArg),
                ("--options", NoArg),
                ("-e", NoArg),
                ("--extended", NoArg),
                ("-m", NoArg),
                ("--memory", NoArg),
                ("-p", NoArg),
                ("--processes", NoArg),
                ("-i", NoArg),
                ("--info", NoArg),
                ("-s", NoArg),
                ("--summary", NoArg),
                ("-4", NoArg),
                ("--ipv4", NoArg),
                ("-6", NoArg),
                ("--ipv6", NoArg),
                ("-0", NoArg),
                ("--packet", NoArg),
                ("-t", NoArg),
                ("--tcp", NoArg),
                ("-M", NoArg),
                ("--mptcp", NoArg),
                ("-S", NoArg),
                ("--sctp", NoArg),
                ("-u", NoArg),
                ("--udp", NoArg),
                ("-d", NoArg),
                ("--dccp", NoArg),
                ("-w", NoArg),
                ("--raw", NoArg),
                ("-x", NoArg),
                ("--unix", NoArg),
                ("--tipc", NoArg),
                ("--vsock", NoArg),
                ("-f", StringArg),
                ("--family", StringArg),
                ("-A", StringArg),
                ("--query", StringArg),
                ("--socket", StringArg),
                ("-Z", NoArg),
                ("--context", NoArg),
                ("-z", NoArg),
                ("--contexts", NoArg),
                ("-b", NoArg),
                ("--bpf", NoArg),
                ("-E", NoArg),
                ("--events", NoArg),
                ("-H", NoArg),
                ("--no-header", NoArg),
                ("-O", NoArg),
                ("--oneline", NoArg),
                ("--tipcinfo", NoArg),
                ("--tos", NoArg),
                ("--cgroup", NoArg),
                ("--inet-sockopt", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn hostname_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-f", NoArg),
                ("--fqdn", NoArg),
                ("--long", NoArg),
                ("-s", NoArg),
                ("--short", NoArg),
                ("-i", NoArg),
                ("--ip-address", NoArg),
                ("-I", NoArg),
                ("--all-ip-addresses", NoArg),
                ("-a", NoArg),
                ("--alias", NoArg),
                ("-d", NoArg),
                ("--domain", NoArg),
                ("-A", NoArg),
                ("--all-fqdns", NoArg),
                ("-v", NoArg),
                ("--verbose", NoArg),
                ("-h", NoArg),
                ("--help", NoArg),
                ("-V", NoArg),
                ("--version", NoArg),
            ]),
            regex: Some(RE_HOSTNAME_ONLY.clone()),
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }

    fn checksum_config() -> CommandConfig {
        use FlagArgType::*;
        CommandConfig {
            safe_flags: Self::flags(&[
                ("-b", NoArg),
                ("--binary", NoArg),
                ("-t", NoArg),
                ("--text", NoArg),
                ("-c", NoArg),
                ("--check", NoArg),
                ("--ignore-missing", NoArg),
                ("--quiet", NoArg),
                ("--status", NoArg),
                ("--strict", NoArg),
                ("-w", NoArg),
                ("--warn", NoArg),
                ("--tag", NoArg),
                ("-z", NoArg),
                ("--zero", NoArg),
                ("--help", NoArg),
                ("--version", NoArg),
            ]),
            regex: None,
            additional_dangerous_check: None,
            respects_double_dash: true,
        }
    }
}

// ---------------------------------------------------------------------------
// SedValidator
// ---------------------------------------------------------------------------

/// Validates sed commands for safety.
pub struct SedValidator;

impl SedValidator {
    /// Check if a sed command is allowed by the allowlist.
    /// When `allow_file_writes` is true, `-i` flag and file arguments are permitted
    /// for substitution commands.
    pub fn is_allowed_by_allowlist(command: &str, allow_file_writes: bool) -> bool {
        let expressions = match Self::extract_expressions(command) {
            Some(exprs) => exprs,
            None => return false,
        };

        let has_file_args = Self::has_file_args(command);

        let is_pattern1 = if allow_file_writes {
            false
        } else {
            Self::is_line_printing_command(command, &expressions)
        };

        let is_pattern2 =
            Self::is_substitution_command(command, &expressions, has_file_args, allow_file_writes);

        if !is_pattern1 && !is_pattern2 {
            return false;
        }

        // Pattern 2 does not allow semicolons
        if is_pattern2 {
            for expr in &expressions {
                if expr.contains(';') {
                    return false;
                }
            }
        }

        // Defense-in-depth denylist check
        for expr in &expressions {
            if Self::contains_dangerous_operations(expr) {
                return false;
            }
        }

        true
    }

    /// Check if a sed command is a line-printing command (Pattern 1).
    fn is_line_printing_command(command: &str, expressions: &[String]) -> bool {
        if !RE_SED_PREFIX.is_match(command) {
            return false;
        }

        let without_sed = RE_SED_PREFIX.find(command).map(|m| &command[m.end()..]).unwrap();
        let tokens = shell_tokenize(without_sed);

        let flags: Vec<&str> = tokens
            .iter()
            .filter(|t| t.starts_with('-') && *t != "--")
            .map(|t| t.as_str())
            .collect();

        let allowed = [
            "-n",
            "--quiet",
            "--silent",
            "-E",
            "--regexp-extended",
            "-r",
            "-z",
            "--zero-terminated",
            "--posix",
        ];
        if !validate_flags_against_allowlist(&flags, &allowed) {
            return false;
        }

        // -n must be present
        let has_n = flags.iter().any(|f| {
            *f == "-n"
                || *f == "--quiet"
                || *f == "--silent"
                || (f.starts_with('-') && !f.starts_with("--") && f.contains('n'))
        });
        if !has_n {
            return false;
        }

        if expressions.is_empty() {
            return false;
        }

        // All expressions must be print commands
        for expr in expressions {
            for cmd in expr.split(';') {
                if !Self::is_print_command(cmd.trim()) {
                    return false;
                }
            }
        }

        true
    }

    /// Check if a single command is a valid print command.
    fn is_print_command(cmd: &str) -> bool {
        if cmd.is_empty() {
            return false;
        }
        RE_PRINT_CMD.is_match(cmd)
    }

    /// Check if this is a substitution command (Pattern 2).
    fn is_substitution_command(
        command: &str,
        expressions: &[String],
        has_file_arguments: bool,
        allow_file_writes: bool,
    ) -> bool {
        if !allow_file_writes && has_file_arguments {
            return false;
        }

        if !RE_SED_PREFIX.is_match(command) {
            return false;
        }

        let without_sed = RE_SED_PREFIX.find(command).map(|m| &command[m.end()..]).unwrap();
        let tokens = shell_tokenize(without_sed);

        let flags: Vec<&str> = tokens
            .iter()
            .filter(|t| t.starts_with('-') && *t != "--")
            .map(|t| t.as_str())
            .collect();

        let mut allowed_flags = vec!["-E", "--regexp-extended", "-r", "--posix"];
        if allow_file_writes {
            allowed_flags.push("-i");
            allowed_flags.push("--in-place");
        }

        if !validate_flags_against_allowlist(&flags, &allowed_flags) {
            return false;
        }

        if expressions.len() != 1 {
            return false;
        }

        let expr = expressions[0].trim();
        if !expr.starts_with('s') {
            return false;
        }

        // Parse substitution: s/pattern/replacement/flags — only / delimiter.
        let rest = match RE_SUBST_CMD.captures(expr) {
            Some(cap) => cap.get(1).unwrap().as_str().to_string(),
            None => return false,
        };

        // Count unescaped / delimiters
        let mut delimiter_count = 0;
        let mut last_delimiter_pos: Option<usize> = None;
        let mut i = 0;
        let chars: Vec<char> = rest.chars().collect();
        while i < chars.len() {
            if chars[i] == '\\' {
                i += 2;
                continue;
            }
            if chars[i] == '/' {
                delimiter_count += 1;
                last_delimiter_pos = Some(i);
            }
            i += 1;
        }

        if delimiter_count != 2 {
            return false;
        }

        let last_pos = last_delimiter_pos.unwrap();
        let expr_flags = &rest[last_pos + 1..];

        if !RE_SUBST_FLAGS.is_match(expr_flags) {
            return false;
        }

        true
    }

    /// Check if a sed command has file arguments.
    fn has_file_args(command: &str) -> bool {
        if !RE_SED_PREFIX.is_match(command) {
            return false;
        }

        let without_sed = RE_SED_PREFIX.find(command).map(|m| &command[m.end()..]).unwrap();
        let tokens = shell_tokenize(without_sed);

        let mut arg_count = 0;
        let mut has_e_flag = false;
        let mut i = 0;

        while i < tokens.len() {
            let arg = &tokens[i];

            if (arg == "-e" || arg == "--expression") && i + 1 < tokens.len() {
                has_e_flag = true;
                i += 2;
                continue;
            }

            if arg.starts_with("--expression=") || arg.starts_with("-e=") {
                has_e_flag = true;
                i += 1;
                continue;
            }

            if arg.starts_with('-') {
                i += 1;
                continue;
            }

            arg_count += 1;

            if has_e_flag {
                return true;
            }

            if arg_count > 1 {
                return true;
            }

            i += 1;
        }

        false
    }

    /// Extract sed expressions from command.
    fn extract_expressions(command: &str) -> Option<Vec<String>> {
        if !RE_SED_PREFIX.is_match(command) {
            return Some(Vec::new());
        }

        let without_sed = RE_SED_PREFIX.find(command).map(|m| &command[m.end()..]).unwrap();

        // Reject dangerous flag combinations
        if RE_DANGER_FLAGS.is_match(without_sed) {
            return None;
        }

        let tokens = shell_tokenize(without_sed);
        let mut expressions = Vec::new();
        let mut found_e_flag = false;
        let mut found_expression = false;
        let mut i = 0;

        while i < tokens.len() {
            let arg = &tokens[i];

            if (arg == "-e" || arg == "--expression") && i + 1 < tokens.len() {
                found_e_flag = true;
                expressions.push(tokens[i + 1].clone());
                i += 2;
                continue;
            }

            if let Some(expr) = arg.strip_prefix("--expression=") {
                found_e_flag = true;
                expressions.push(expr.to_string());
                i += 1;
                continue;
            }

            if let Some(expr) = arg.strip_prefix("-e=") {
                found_e_flag = true;
                expressions.push(expr.to_string());
                i += 1;
                continue;
            }

            if arg.starts_with('-') {
                i += 1;
                continue;
            }

            if !found_e_flag && !found_expression {
                expressions.push(arg.clone());
                found_expression = true;
                i += 1;
                continue;
            }

            break;
        }

        Some(expressions)
    }

    /// Check if a sed expression contains dangerous operations (denylist).
    #[allow(clippy::invalid_regex)] // Backreferences (\1) are valid sed patterns, not regex bugs
    fn contains_dangerous_operations(expression: &str) -> bool {
        let cmd = expression.trim();
        if cmd.is_empty() {
            return false;
        }

        // Reject non-ASCII characters
        if !cmd.is_ascii() {
            return true;
        }

        // Reject curly braces
        if cmd.contains('{') || cmd.contains('}') {
            return true;
        }

        // Reject newlines
        if cmd.contains('\n') {
            return true;
        }

        // Reject comments (# not immediately after s command)
        if let Some(hash_idx) = cmd.find('#') {
            if hash_idx == 0 || cmd.as_bytes().get(hash_idx - 1) != Some(&b's') {
                return true;
            }
        }

        // Reject negation operator
        if cmd.starts_with('!') {
            return true;
        }
        if RE_NEGATION.is_match(cmd) {
            return true;
        }

        // Reject tilde in GNU step address format
        if RE_TILDE.is_match(cmd) {
            return true;
        }

        // Reject comma at start
        if cmd.starts_with(',') {
            return true;
        }

        // Reject comma followed by +/-
        if RE_COMMA_OFFSET.is_match(cmd) {
            return true;
        }

        // Reject backslash tricks
        if RE_BS_TRICKS.is_match(cmd) {
            return true;
        }

        // Reject escaped slashes followed by w/W
        if RE_ESCAPED_SLASH.is_match(cmd) {
            return true;
        }

        // Reject slash followed by non-slash chars then whitespace then dangerous commands
        if RE_SLASH_WS.is_match(cmd) {
            return true;
        }

        // Reject malformed substitution commands
        if cmd.starts_with("s/") && !RE_PROPER_SUBST.is_match(cmd) {
            return true;
        }

        // Paranoid: reject 's' commands ending with dangerous chars
        if cmd.len() >= 2 && cmd.starts_with('s') && cmd.ends_with(|c: char| "wWeE".contains(c)) {
            // Verify it's a proper substitution: s<delim>...<delim>...<delim>[flags]
            if !Self::is_proper_substitution_with_safe_flags(cmd) {
                return true;
            }
        }

        // Check for dangerous write commands
        let write_patterns: &[&LazyLock<Regex>] = &[
            &RE_WRITE_BARE,
            &RE_WRITE_NUM,
            &RE_WRITE_DOLLAR,
            &RE_WRITE_REGEX,
            &RE_WRITE_RANGE,
            &RE_WRITE_RANGE_DOLLAR,
        ];
        for pat in write_patterns {
            if pat.is_match(cmd) {
                return true;
            }
        }

        // Check for dangerous execute commands
        let exec_patterns: &[&LazyLock<Regex>] = &[
            &RE_EXEC_BARE,
            &RE_EXEC_NUM,
            &RE_EXEC_DOLLAR,
            &RE_EXEC_REGEX,
            &RE_EXEC_RANGE,
            &RE_EXEC_RANGE_DOLLAR,
        ];
        for pat in exec_patterns {
            if pat.is_match(cmd) {
                return true;
            }
        }

        // Check for substitution commands with dangerous flags
        if let Some(flags) = Self::extract_substitution_flags(cmd) {
            if flags.contains('w')
                || flags.contains('W')
                || flags.contains('e')
                || flags.contains('E')
            {
                return true;
            }
        }

        // Check for y (transliterate) command with dangerous operations
        if RE_Y_CMD.is_match(cmd) && RE_DANGEROUS_CHARS.is_match(cmd) {
            return true;
        }

        false
    }

    /// Parse a sed substitution command (s<delim>...<delim>...<delim>[flags])
    /// and check if all flags after the final delimiter are safe (no w/W/e/E).
    fn is_proper_substitution_with_safe_flags(cmd: &str) -> bool {
        if let Some(flags) = Self::extract_substitution_flags(cmd) {
            !flags.contains('w')
                && !flags.contains('W')
                && !flags.contains('e')
                && !flags.contains('E')
        } else {
            false // Can't parse → not proper
        }
    }

    /// Extract the flags portion from a sed s-command: `s/pat/repl/FLAGS`.
    /// Returns `None` if the command is not a valid s-command.
    fn extract_substitution_flags(cmd: &str) -> Option<&str> {
        let bytes = cmd.as_bytes();
        if bytes.len() < 2 || bytes[0] != b's' {
            return None;
        }
        let delim = bytes[1];
        if delim == b'\\' || delim == b'\n' {
            return None;
        }
        // Find three occurrences of `delim` (skipping escaped ones)
        let mut count = 0;
        let mut i = 2;
        let mut last_delim_pos = 0;
        while i < bytes.len() && count < 2 {
            if bytes[i] == b'\\' {
                i += 2; // skip escaped char
                continue;
            }
            if bytes[i] == delim {
                count += 1;
                last_delim_pos = i;
            }
            i += 1;
        }
        if count == 2 {
            Some(&cmd[last_delim_pos + 1..])
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// PathValidator
// ---------------------------------------------------------------------------

/// Validates paths in bash commands to prevent dangerous operations.
pub struct PathValidator;

/// Paths that are always dangerous to modify/remove.
const DANGEROUS_PATHS: &[&str] = &[
    "/",
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/home",
    "/lib",
    "/lib64",
    "/mnt",
    "/opt",
    "/proc",
    "/root",
    "/run",
    "/sbin",
    "/srv",
    "/sys",
    "/tmp",
    "/usr",
    "/var",
    // macOS-specific
    "/System",
    "/Library",
    "/Applications",
    "/Users",
    "/Volumes",
    "/cores",
    "/private",
];

impl PathValidator {
    /// Check if a path is dangerous to operate on (e.g., for rm/chmod/chown).
    pub fn is_dangerous_path(path: &str) -> bool {
        // Expand ~ to home dir
        let expanded = if path.starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                path.replacen('~', &home.to_string_lossy(), 1)
            } else {
                path.to_string()
            }
        } else {
            path.to_string()
        };

        // Strip quotes
        let cleaned = expanded
            .trim_start_matches('\'')
            .trim_start_matches('"')
            .trim_end_matches('\'')
            .trim_end_matches('"');

        // Normalize path
        let normalized = PathBuf::from(cleaned);
        let normalized_str = normalized.to_string_lossy();

        // Check exact match or parent of dangerous paths
        for &dp in DANGEROUS_PATHS {
            if normalized_str == dp || normalized_str == format!("{}/", dp) {
                return true;
            }
        }

        // Check "/" specifically — catches `rm -rf /`, `rm -rf //`, etc.
        let stripped = cleaned.trim_end_matches('/');
        if stripped.is_empty() {
            return true;
        }

        false
    }

    /// Check if a command operates on paths outside the allowed working directory.
    pub fn is_path_within_cwd(path: &str, cwd: &Path) -> bool {
        let expanded = if path.starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                path.replacen('~', &home.to_string_lossy(), 1)
            } else {
                return false;
            }
        } else {
            path.to_string()
        };

        let cleaned = expanded
            .trim_start_matches('\'')
            .trim_start_matches('"')
            .trim_end_matches('\'')
            .trim_end_matches('"');

        let abs_path = if Path::new(cleaned).is_absolute() {
            PathBuf::from(cleaned)
        } else {
            cwd.join(cleaned)
        };

        // Canonicalize to resolve .. and symlinks. If it fails (path doesn't
        // exist), do a best-effort normalization.
        let resolved = abs_path
            .canonicalize()
            .unwrap_or_else(|_| normalize_path(&abs_path));

        let cwd_resolved = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());

        resolved.starts_with(&cwd_resolved)
    }

    /// Validate an rm/rmdir command's paths.
    pub fn check_dangerous_removal(command: &str, args: &[String], cwd: &Path) -> SecurityVerdict {
        let paths = Self::extract_non_flag_args(args);

        for path in &paths {
            let expanded = if path.starts_with('~') {
                if let Some(home) = dirs::home_dir() {
                    path.replacen('~', &home.to_string_lossy(), 1)
                } else {
                    path.clone()
                }
            } else {
                path.clone()
            };

            let cleaned = expanded
                .trim_start_matches('\'')
                .trim_start_matches('"')
                .trim_end_matches('\'')
                .trim_end_matches('"');

            let abs = if Path::new(cleaned).is_absolute() {
                PathBuf::from(cleaned)
            } else {
                cwd.join(cleaned)
            };

            if Self::is_dangerous_path(&abs.to_string_lossy()) {
                return SecurityVerdict::Ask(format!(
                    "Dangerous {} operation detected: '{}'. This command would remove a critical system directory.",
                    command,
                    abs.display()
                ));
            }
        }

        SecurityVerdict::Allow
    }

    /// Extract non-flag arguments, respecting POSIX `--`.
    fn extract_non_flag_args(args: &[String]) -> Vec<String> {
        let mut result = Vec::new();
        let mut after_double_dash = false;
        for arg in args {
            if after_double_dash {
                result.push(arg.clone());
            } else if arg == "--" {
                after_double_dash = true;
            } else if !arg.starts_with('-') {
                result.push(arg.clone());
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Top-level validation entry point
// ---------------------------------------------------------------------------

/// Comprehensive security check for a bash command before execution.
///
/// Returns `SecurityVerdict::Allow` if the command passes all checks,
/// `SecurityVerdict::Ask(reason)` if the command requires user approval,
/// or `SecurityVerdict::Deny(reason)` if the command should be blocked.
pub fn validate_command(command: &str, cwd: &Path) -> SecurityVerdict {
    let trimmed = command.trim();

    // Empty command is safe.
    if trimmed.is_empty() {
        return SecurityVerdict::Allow;
    }

    // --- Structural safety checks (from bashSecurity.ts) ---

    // Reject commands that start with tab (incomplete fragment).
    if command.starts_with('\t') || command.starts_with(" \t") {
        return SecurityVerdict::Ask(
            "Command appears to be an incomplete fragment (starts with tab)".to_string(),
        );
    }

    // Reject commands that start with a flag (incomplete fragment).
    if trimmed.starts_with('-') {
        return SecurityVerdict::Ask(
            "Command appears to be an incomplete fragment (starts with flags)".to_string(),
        );
    }

    // Reject commands that start with an operator (continuation line).
    if RE_OPERATOR_START.is_match(command) {
        return SecurityVerdict::Ask(
            "Command appears to be a continuation line (starts with operator)".to_string(),
        );
    }

    // --- Quote-aware content extraction ---
    let unquoted = extract_unquoted_content(command);

    // Check for backticks (command substitution) in unquoted content.
    if has_unescaped_char(&unquoted, '`') {
        return SecurityVerdict::Ask(
            "Command contains backticks (`) for command substitution".to_string(),
        );
    }

    // Check for command substitution patterns.
    let substitution_patterns: &[(&LazyLock<Regex>, &str)] = &[
        (&RE_PROC_SUBST_IN, "process substitution <()"),
        (&RE_PROC_SUBST_OUT, "process substitution >()"),
        (&RE_ZSH_PROC_SUBST, "Zsh process substitution =()"),
        (&RE_CMD_SUBST, "$() command substitution"),
        (&RE_PARAM_SUBST, "${} parameter substitution"),
        (&RE_ARITH_SUBST, "$[] legacy arithmetic expansion"),
        (&RE_ZSH_PARAM_EXP, "Zsh-style parameter expansion"),
        (&RE_ZSH_GLOB_QUAL, "Zsh-style glob qualifiers"),
        (&RE_ZSH_GLOB_EXEC, "Zsh glob qualifier with command execution"),
        (&RE_ZSH_ALWAYS, "Zsh always block"),
        (&RE_PS_COMMENT, "PowerShell comment syntax"),
    ];

    for &(re, message) in substitution_patterns {
        if re.is_match(&unquoted) {
            return SecurityVerdict::Ask(format!("Command contains {}", message));
        }
    }

    // Check for redirections in fully-unquoted content.
    let fully_unquoted = extract_fully_unquoted_content(command);
    let safe_stripped = strip_safe_redirections(&fully_unquoted);

    if safe_stripped.contains('<') {
        return SecurityVerdict::Ask(
            "Command contains input redirection (<) which could read sensitive files".to_string(),
        );
    }
    if safe_stripped.contains('>') {
        return SecurityVerdict::Ask(
            "Command contains output redirection (>) which could write to arbitrary files"
                .to_string(),
        );
    }

    // Check for IFS injection.
    if RE_IFS.is_match(command) {
        return SecurityVerdict::Ask(
            "Command contains IFS variable usage which could bypass security validation"
                .to_string(),
        );
    }

    // Check for /proc/*/environ access.
    if command.contains("/proc/") && command.contains("/environ") {
        return SecurityVerdict::Ask(
            "Command accesses /proc/*/environ which could expose sensitive environment variables"
                .to_string(),
        );
    }

    // Zsh dangerous commands.
    let zsh_dangerous: HashSet<&str> = [
        "zmodload", "emulate", "sysopen", "sysread", "syswrite", "sysseek", "zpty", "ztcp",
        "zsocket", "zf_rm", "zf_mv", "zf_ln", "zf_chmod", "zf_chown", "zf_mkdir", "zf_rmdir",
        "zf_chgrp",
    ]
    .into_iter()
    .collect();

    let segments = split_command(trimmed);
    for segment in &segments {
        let base = extract_base_command(segment);
        if zsh_dangerous.contains(base.as_str()) {
            return SecurityVerdict::Ask(format!(
                "Command contains dangerous Zsh command: {}",
                base
            ));
        }
    }

    // --- Per-segment path and rm safety checks ---
    for segment in &segments {
        let seg_trimmed = segment.trim();
        let base = extract_base_command(seg_trimmed);
        let tokens = shell_tokenize(seg_trimmed);
        let args: Vec<String> = if tokens.len() > 1 {
            tokens[1..].to_vec()
        } else {
            Vec::new()
        };

        // Check dangerous rm/rmdir paths.
        if base == "rm" || base == "rmdir" {
            let verdict = PathValidator::check_dangerous_removal(&base, &args, cwd);
            if let SecurityVerdict::Ask(_) = &verdict {
                return verdict;
            }
        }
    }

    // --- Sed constraint check ---
    for segment in &segments {
        let seg_trimmed = segment.trim();
        let base = extract_base_command(seg_trimmed);
        if base == "sed" && !SedValidator::is_allowed_by_allowlist(seg_trimmed, false) {
            return SecurityVerdict::Ask(
                "sed command requires approval (contains potentially dangerous operations)"
                    .to_string(),
            );
        }
    }

    SecurityVerdict::Allow
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

/// Validate combined flags against an allowlist.
fn validate_flags_against_allowlist(flags: &[&str], allowed: &[&str]) -> bool {
    for flag in flags {
        if flag.starts_with('-') && !flag.starts_with("--") && flag.len() > 2 {
            // Combined flags: check each character.
            for ch in flag[1..].chars() {
                let single = format!("-{}", ch);
                if !allowed.contains(&single.as_str()) {
                    return false;
                }
            }
        } else if !allowed.contains(flag) {
            return false;
        }
    }
    true
}

/// Split a command on unquoted operators (&&, ||, ;, |).
pub fn split_command(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if escaped {
            current.push(c);
            escaped = false;
            i += 1;
            continue;
        }

        if c == '\\' && !in_single_quote {
            escaped = true;
            current.push(c);
            i += 1;
            continue;
        }

        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(c);
            i += 1;
            continue;
        }

        if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(c);
            i += 1;
            continue;
        }

        if in_single_quote || in_double_quote {
            current.push(c);
            i += 1;
            continue;
        }

        // Check for two-char operators.
        if i + 1 < chars.len() {
            let two = format!("{}{}", c, chars[i + 1]);
            if two == "&&" || two == "||" {
                if !current.trim().is_empty() {
                    segments.push(current.trim().to_string());
                }
                current = String::new();
                i += 2;
                continue;
            }
        }

        if c == ';' || c == '|' {
            if !current.trim().is_empty() {
                segments.push(current.trim().to_string());
            }
            current = String::new();
            i += 1;
            continue;
        }

        current.push(c);
        i += 1;
    }

    if !current.trim().is_empty() {
        segments.push(current.trim().to_string());
    }

    segments
}

/// Extract the base command (first word) from a command string.
fn extract_base_command(command: &str) -> String {
    let trimmed = command.trim();
    // Skip env var assignments (VAR=val).
    for token in trimmed.split_whitespace() {
        if token.contains('=') && !token.starts_with('-') && !token.starts_with('/') {
            let before_eq = token.split('=').next().unwrap_or("");
            if before_eq
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !before_eq.is_empty()
                && before_eq
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_alphabetic() || c == '_')
                    .unwrap_or(false)
            {
                continue;
            }
        }
        return token.to_string();
    }
    String::new()
}

/// Simple shell tokenizer that handles single and double quotes.
/// Does not handle all shell syntax — just enough for flag/arg splitting.
fn shell_tokenize(command: &str) -> Vec<String> {
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
            // Don't push the backslash for quote removal.
            continue;
        }

        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            continue;
        }

        if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
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

/// Extract content outside single quotes (but preserving double-quoted content).
fn extract_unquoted_content(command: &str) -> String {
    let mut result = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    for c in command.chars() {
        if escaped {
            escaped = false;
            if !in_single_quote {
                result.push(c);
            }
            continue;
        }

        if c == '\\' && !in_single_quote {
            escaped = true;
            if !in_single_quote {
                result.push(c);
            }
            continue;
        }

        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            continue;
        }

        if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            continue;
        }

        if !in_single_quote {
            result.push(c);
        }
    }

    result
}

/// Extract content outside both single AND double quotes.
fn extract_fully_unquoted_content(command: &str) -> String {
    let mut result = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    for c in command.chars() {
        if escaped {
            escaped = false;
            if !in_single_quote && !in_double_quote {
                result.push(c);
            }
            continue;
        }

        if c == '\\' && !in_single_quote {
            escaped = true;
            if !in_single_quote && !in_double_quote {
                result.push(c);
            }
            continue;
        }

        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            continue;
        }

        if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            continue;
        }

        if !in_single_quote && !in_double_quote {
            result.push(c);
        }
    }

    result
}

/// Strip safe redirections (2>&1, N>/dev/null, </dev/null).
fn strip_safe_redirections(content: &str) -> String {
    let s = RE_REDIR_2_TO_1.replace_all(content, " ").to_string();
    let s = RE_REDIR_DEV_NULL.replace_all(&s, "").to_string();
    RE_REDIR_IN_DEV_NULL.replace_all(&s, "").to_string()
}

/// Check for unescaped occurrences of a single character.
fn has_unescaped_char(content: &str, ch: char) -> bool {
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 2;
            continue;
        }
        if chars[i] == ch {
            return true;
        }
        i += 1;
    }
    false
}

/// Normalize a path by resolving `.` and `..` components without touching the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if !components.is_empty() {
                    components.pop();
                }
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_destructive_detection() {
        let detector = DestructiveCommandDetector::new();

        assert!(detector.detect("git reset --hard").is_some());
        assert!(detector.detect("git push --force").is_some());
        assert!(detector.detect("git push -f origin main").is_some());
        assert!(detector.detect("git clean -fd").is_some());
        assert!(detector.detect("git checkout .").is_some());
        assert!(detector.detect("git branch -D feature").is_some());
        assert!(detector.detect("rm -rf /").is_some());
        assert!(detector.detect("kubectl delete pod foo").is_some());
        assert!(detector.detect("terraform destroy").is_some());
        assert!(detector.detect("git commit --amend").is_some());
        assert!(detector.detect("git push --no-verify").is_some());

        // Safe commands
        assert!(detector.detect("git status").is_none());
        assert!(detector.detect("git diff").is_none());
        assert!(detector.detect("ls -la").is_none());
        assert!(detector.detect("echo hello").is_none());
    }

    #[test]
    fn test_command_classification() {
        let classifier = CommandClassifier::new();

        assert_eq!(
            classifier.classify("git reset --hard"),
            CommandClassification::Destructive
        );
        assert_eq!(
            classifier.classify("echo hello"),
            CommandClassification::ReadOnly
        );
        assert_eq!(
            classifier.classify("grep -r pattern ."),
            CommandClassification::ReadOnly
        );
        assert_eq!(
            classifier.classify("git diff --stat"),
            CommandClassification::ReadOnly
        );
        // Write commands that aren't destructive
        assert_eq!(
            classifier.classify("touch newfile.txt"),
            CommandClassification::Write
        );
    }

    #[test]
    fn test_read_only_validator() {
        let validator = ReadOnlyValidator::new();

        assert!(validator.is_read_only("echo hello"));
        assert!(validator.is_read_only("grep -rn pattern ."));
        assert!(validator.is_read_only("git diff --stat"));
        assert!(validator.is_read_only("git log --oneline -10"));
        assert!(validator.is_read_only("git status -s"));
        assert!(validator.is_read_only("rg pattern src/"));
        assert!(validator.is_read_only("fd -e rs"));
        assert!(validator.is_read_only("sort -n file.txt"));
        assert!(validator.is_read_only("file --mime-type test.txt"));
        assert!(validator.is_read_only("ps -ef"));
        assert!(validator.is_read_only("hostname --fqdn"));

        // Not read-only
        assert!(!validator.is_read_only("rm file.txt"));
        assert!(!validator.is_read_only("touch newfile.txt"));
        assert!(!validator.is_read_only("curl http://example.com"));
        assert!(!validator.is_read_only("wget http://example.com"));
    }

    #[test]
    fn test_sed_validator() {
        // Allowed patterns
        assert!(SedValidator::is_allowed_by_allowlist("sed -n '5p'", false));
        assert!(SedValidator::is_allowed_by_allowlist(
            "sed -n '1,10p'",
            false
        ));
        assert!(SedValidator::is_allowed_by_allowlist(
            "sed 's/foo/bar/'",
            false
        ));
        assert!(SedValidator::is_allowed_by_allowlist(
            "sed 's/foo/bar/g'",
            false
        ));

        // Dangerous patterns
        assert!(!SedValidator::is_allowed_by_allowlist(
            "sed 'w /tmp/output'",
            false
        ));
        assert!(!SedValidator::is_allowed_by_allowlist("sed 'e ls'", false));
        assert!(!SedValidator::is_allowed_by_allowlist(
            "sed 's/foo/bar/we'",
            false
        ));
    }

    #[test]
    fn test_dangerous_paths() {
        assert!(PathValidator::is_dangerous_path("/"));
        assert!(PathValidator::is_dangerous_path("/usr"));
        assert!(PathValidator::is_dangerous_path("/etc"));
        assert!(PathValidator::is_dangerous_path("/System"));
        assert!(PathValidator::is_dangerous_path("/bin"));

        assert!(!PathValidator::is_dangerous_path("/home/user/project"));
        assert!(!PathValidator::is_dangerous_path("/tmp/myfile.txt"));
    }

    #[test]
    fn test_split_command() {
        assert_eq!(
            split_command("echo hello && echo world"),
            vec!["echo hello", "echo world"]
        );
        assert_eq!(split_command("ls | grep foo"), vec!["ls", "grep foo"]);
        assert_eq!(
            split_command("echo 'hello && world'"),
            vec!["echo 'hello && world'"]
        );
    }

    #[test]
    fn test_validate_command_structural() {
        let cwd = PathBuf::from("/tmp/test");

        // Empty command is fine
        assert_eq!(validate_command("", &cwd), SecurityVerdict::Allow);

        // Starts with tab
        assert!(matches!(
            validate_command("\tls", &cwd),
            SecurityVerdict::Ask(_)
        ));

        // Starts with flag
        assert!(matches!(
            validate_command("-rf /", &cwd),
            SecurityVerdict::Ask(_)
        ));

        // Starts with operator
        assert!(matches!(
            validate_command("&& echo", &cwd),
            SecurityVerdict::Ask(_)
        ));

        // Command substitution
        assert!(matches!(
            validate_command("echo $(cat /etc/passwd)", &cwd),
            SecurityVerdict::Ask(_)
        ));

        // IFS injection
        assert!(matches!(
            validate_command("echo $IFS", &cwd),
            SecurityVerdict::Ask(_)
        ));
    }

    #[test]
    fn test_validate_command_zsh_dangerous() {
        let cwd = PathBuf::from("/tmp/test");
        assert!(matches!(
            validate_command("zmodload zsh/system", &cwd),
            SecurityVerdict::Ask(_)
        ));
        assert!(matches!(
            validate_command("syswrite something", &cwd),
            SecurityVerdict::Ask(_)
        ));
    }

    #[test]
    fn test_shell_tokenize() {
        assert_eq!(shell_tokenize("echo hello"), vec!["echo", "hello"]);
        assert_eq!(
            shell_tokenize("echo 'hello world'"),
            vec!["echo", "hello world"]
        );
        assert_eq!(
            shell_tokenize("git diff --stat"),
            vec!["git", "diff", "--stat"]
        );
    }

    #[test]
    fn test_extract_base_command() {
        assert_eq!(extract_base_command("git diff --stat"), "git");
        assert_eq!(extract_base_command("NODE_ENV=prod npm run build"), "npm");
        assert_eq!(extract_base_command("  ls -la"), "ls");
    }

    #[test]
    fn test_dangerous_removal_check() {
        let cwd = PathBuf::from("/home/user/project");
        let args: Vec<String> = vec!["-rf".to_string(), "/".to_string()];
        assert!(matches!(
            PathValidator::check_dangerous_removal("rm", &args, &cwd),
            SecurityVerdict::Ask(_)
        ));

        let safe_args: Vec<String> = vec!["some_file.txt".to_string()];
        assert_eq!(
            PathValidator::check_dangerous_removal("rm", &safe_args, &cwd),
            SecurityVerdict::Allow
        );
    }
}
