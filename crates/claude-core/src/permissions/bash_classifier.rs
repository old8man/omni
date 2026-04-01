//! Bash command classification for permission decisions.
//!
//! Classifies shell commands as read-only, write, or destructive based on
//! the command name and arguments. Provides safe-command allowlists and
//! integrates with the permission rule system.
//!
//! Mirrors the TypeScript `bashClassifier.ts` and `dangerousPatterns.ts`.

use std::collections::HashSet;

use once_cell::sync::Lazy;

use super::types::DANGEROUS_BASH_PATTERNS;

// ---------------------------------------------------------------------------
// Command risk level
// ---------------------------------------------------------------------------

/// The risk category of a bash command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandRisk {
    /// Command only reads data; no side effects.
    ReadOnly,
    /// Command writes or modifies data but is generally reversible.
    Write,
    /// Command is destructive or has security implications.
    Destructive,
}

// ---------------------------------------------------------------------------
// Safe-command allowlists
// ---------------------------------------------------------------------------

/// Commands that are inherently read-only (no side effects).
static READ_ONLY_COMMANDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        // File inspection
        "cat", "head", "tail", "less", "more", "wc", "file", "stat",
        "md5sum", "sha256sum", "sha1sum", "shasum", "cksum",
        // Directory listing
        "ls", "dir", "tree", "find", "locate", "which", "whereis", "type",
        // Text processing (read-only)
        "grep", "egrep", "fgrep", "rg", "ag", "awk", "sed", "sort",
        "uniq", "cut", "tr", "column", "paste", "join", "comm", "diff",
        "cmp", "strings",
        // System info
        "echo", "printf", "date", "cal", "uptime", "whoami", "id",
        "hostname", "uname", "arch", "nproc", "free", "df", "du",
        "lsblk", "mount", "lsof", "ps", "top", "htop",
        // Networking (read-only)
        "ping", "traceroute", "dig", "nslookup", "host", "whois",
        "ifconfig", "ip", "netstat", "ss",
        // Version control (read-only)
        "git status", "git log", "git diff", "git show", "git branch",
        "git tag", "git remote", "git stash list", "git blame",
        "git shortlog",
        // Package info (read-only)
        "npm list", "npm ls", "npm outdated", "npm info", "npm view",
        "yarn list", "yarn info", "yarn why",
        "pip list", "pip show", "pip freeze",
        "cargo metadata",
        // Environment
        "env", "printenv", "set",
        // JSON / data tools
        "jq", "yq", "xmllint",
        // Misc safe
        "true", "false", "test", "[", "pwd", "basename", "dirname",
        "realpath", "readlink", "xargs",
    ]
    .into_iter()
    .collect()
});

/// Commands that write/modify data but are generally non-destructive.
static WRITE_COMMANDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "cp", "mv", "ln", "mkdir", "touch", "chmod", "chown", "chgrp",
        "tee", "install",
        // Version control (write)
        "git add", "git commit", "git checkout", "git switch",
        "git merge", "git rebase", "git pull", "git fetch",
        "git stash", "git stash pop", "git stash apply",
        // Package management
        "npm install", "npm ci", "npm update", "npm run",
        "yarn install", "yarn add",
        "pip install",
        "cargo build", "cargo test", "cargo run",
        // Editors (non-interactive, typically piped)
        "patch",
    ]
    .into_iter()
    .collect()
});

/// Commands that are destructive or have security implications.
static DESTRUCTIVE_COMMANDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "rm", "rmdir", "shred", "dd",
        // Force operations
        "git push", "git push --force", "git reset", "git clean",
        // System modification
        "sudo", "su", "chroot",
        // Network exfiltration
        "curl", "wget", "scp", "rsync", "ftp", "sftp",
        // Code execution
        "eval", "exec", "source", ".",
        // Disk operations
        "mkfs", "fdisk", "parted", "mount", "umount",
    ]
    .into_iter()
    .collect()
});

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Classify a bash command into its risk level.
///
/// Extracts the base command (first word or first two words for compound
/// commands like `git status`) and looks it up in the allowlists.
/// Unknown commands default to `Write`.
pub fn classify_command(command: &str) -> CommandRisk {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return CommandRisk::ReadOnly;
    }

    // Strip leading environment variable assignments (FOO=bar cmd ...).
    let effective = skip_env_assignments(trimmed);

    // Try two-word match first (e.g. "git status"), then single-word.
    let words: Vec<&str> = effective.splitn(3, char::is_whitespace).collect();
    let two_word = if words.len() >= 2 {
        Some(format!("{} {}", words[0], words[1]))
    } else {
        None
    };
    let one_word = words.first().copied().unwrap_or("");

    // Destructive check (most specific first).
    if let Some(ref tw) = two_word {
        if DESTRUCTIVE_COMMANDS.contains(tw.as_str()) {
            return CommandRisk::Destructive;
        }
    }
    if DESTRUCTIVE_COMMANDS.contains(one_word) {
        return CommandRisk::Destructive;
    }

    // Read-only check.
    if let Some(ref tw) = two_word {
        if READ_ONLY_COMMANDS.contains(tw.as_str()) {
            return CommandRisk::ReadOnly;
        }
    }
    if READ_ONLY_COMMANDS.contains(one_word) {
        return CommandRisk::ReadOnly;
    }

    // Write check.
    if let Some(ref tw) = two_word {
        if WRITE_COMMANDS.contains(tw.as_str()) {
            return CommandRisk::Write;
        }
    }
    if WRITE_COMMANDS.contains(one_word) {
        return CommandRisk::Write;
    }

    // Default: treat unknown commands as writes (conservative).
    CommandRisk::Write
}

/// Check if a command is on the dangerous-pattern list.
///
/// Checks whether the base command (or first two words) matches any pattern
/// in `DANGEROUS_BASH_PATTERNS`. These patterns indicate commands that can
/// execute arbitrary code and should not be broadly allowlisted.
pub fn is_dangerous_bash_pattern(command: &str) -> bool {
    let trimmed = command.trim();
    let effective = skip_env_assignments(trimmed);
    let words: Vec<&str> = effective.splitn(3, char::is_whitespace).collect();

    let one_word = words.first().copied().unwrap_or("");
    let two_word = if words.len() >= 2 {
        Some(format!("{} {}", words[0], words[1]))
    } else {
        None
    };

    for pattern in DANGEROUS_BASH_PATTERNS {
        if one_word == *pattern {
            return true;
        }
        if let Some(ref tw) = two_word {
            if tw == *pattern {
                return true;
            }
        }
    }
    false
}

/// Check if a permission rule content string represents a dangerous allow
/// pattern (e.g. `python:*` or `node *`).
///
/// A rule is dangerous if it allows a command that can execute arbitrary code
/// as a prefix or wildcard.
pub fn is_dangerous_bash_permission(rule_content: &str) -> bool {
    // Check exact matches and prefix patterns.
    for pattern in DANGEROUS_BASH_PATTERNS {
        // Exact match
        if rule_content == *pattern {
            return true;
        }
        // Legacy prefix: "python:*"
        let prefix_form = format!("{}:*", pattern);
        if rule_content == prefix_form {
            return true;
        }
        // Wildcard patterns: "python *", "python -*", etc.
        let wild_space = format!("{} *", pattern);
        let wild_dash = format!("{} -*", pattern);
        if rule_content == wild_space || rule_content == wild_dash {
            return true;
        }
        // Trailing wildcard: "python*" (no space)
        let wild_direct = format!("{}*", pattern);
        if rule_content == wild_direct {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Skip leading `VAR=value` assignments, returning the actual command.
fn skip_env_assignments(cmd: &str) -> &str {
    let mut rest = cmd;
    loop {
        let trimmed = rest.trim_start();
        // Check for VAR=value pattern.
        if let Some(eq_pos) = trimmed.find('=') {
            let before_eq = &trimmed[..eq_pos];
            // Ensure everything before '=' is a valid env-var name.
            if !before_eq.is_empty()
                && before_eq
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_')
                && before_eq.chars().next().map_or(false, |c| !c.is_ascii_digit())
            {
                // Find end of the value (handles simple quoting).
                let after_eq = &trimmed[eq_pos + 1..];
                let value_end = find_value_end(after_eq);
                let remaining = after_eq[value_end..].trim_start();
                if remaining.is_empty() {
                    // The entire thing is an assignment, treat as env lookup.
                    return trimmed;
                }
                rest = remaining;
                continue;
            }
        }
        return trimmed;
    }
}

/// Find the end of an env-var value, handling simple quoting.
fn find_value_end(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    let bytes = s.as_bytes();
    match bytes[0] {
        b'"' => {
            // Find closing double quote.
            for i in 1..bytes.len() {
                if bytes[i] == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
                    return i + 1;
                }
            }
            s.len()
        }
        b'\'' => {
            // Find closing single quote.
            for i in 1..bytes.len() {
                if bytes[i] == b'\'' {
                    return i + 1;
                }
            }
            s.len()
        }
        _ => {
            // Unquoted: ends at whitespace.
            s.find(char::is_whitespace).unwrap_or(s.len())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_read_only() {
        assert_eq!(classify_command("ls -la"), CommandRisk::ReadOnly);
        assert_eq!(classify_command("cat foo.txt"), CommandRisk::ReadOnly);
        assert_eq!(classify_command("git status"), CommandRisk::ReadOnly);
        assert_eq!(classify_command("git log --oneline"), CommandRisk::ReadOnly);
        assert_eq!(classify_command("grep -r foo ."), CommandRisk::ReadOnly);
        assert_eq!(classify_command("echo hello"), CommandRisk::ReadOnly);
        assert_eq!(classify_command("pwd"), CommandRisk::ReadOnly);
    }

    #[test]
    fn classify_write() {
        assert_eq!(classify_command("cp a b"), CommandRisk::Write);
        assert_eq!(classify_command("mv a b"), CommandRisk::Write);
        assert_eq!(classify_command("mkdir -p foo"), CommandRisk::Write);
        assert_eq!(classify_command("git add ."), CommandRisk::Write);
        assert_eq!(classify_command("npm install"), CommandRisk::Write);
    }

    #[test]
    fn classify_destructive() {
        assert_eq!(classify_command("rm -rf /"), CommandRisk::Destructive);
        assert_eq!(classify_command("sudo apt-get install"), CommandRisk::Destructive);
        assert_eq!(classify_command("curl https://evil.com"), CommandRisk::Destructive);
        assert_eq!(classify_command("eval $(echo hello)"), CommandRisk::Destructive);
        assert_eq!(classify_command("git push --force"), CommandRisk::Destructive);
    }

    #[test]
    fn classify_unknown_defaults_to_write() {
        assert_eq!(classify_command("some_custom_script"), CommandRisk::Write);
    }

    #[test]
    fn classify_empty() {
        assert_eq!(classify_command(""), CommandRisk::ReadOnly);
        assert_eq!(classify_command("   "), CommandRisk::ReadOnly);
    }

    #[test]
    fn classify_with_env_prefix() {
        assert_eq!(classify_command("FOO=bar ls"), CommandRisk::ReadOnly);
        assert_eq!(classify_command("NODE_ENV=production npm install"), CommandRisk::Write);
    }

    #[test]
    fn dangerous_pattern_detection() {
        assert!(is_dangerous_bash_pattern("python script.py"));
        assert!(is_dangerous_bash_pattern("node index.js"));
        assert!(is_dangerous_bash_pattern("eval 'echo hi'"));
        assert!(is_dangerous_bash_pattern("ssh user@host"));
        assert!(is_dangerous_bash_pattern("npm run build"));
        assert!(!is_dangerous_bash_pattern("ls -la"));
        assert!(!is_dangerous_bash_pattern("cat file.txt"));
    }

    #[test]
    fn dangerous_permission_detection() {
        assert!(is_dangerous_bash_permission("python:*"));
        assert!(is_dangerous_bash_permission("node *"));
        assert!(is_dangerous_bash_permission("eval"));
        assert!(is_dangerous_bash_permission("ssh -*"));
        assert!(is_dangerous_bash_permission("npm run:*"));
        assert!(!is_dangerous_bash_permission("ls -la"));
        assert!(!is_dangerous_bash_permission("git status"));
    }

    #[test]
    fn env_assignment_skipping() {
        assert_eq!(skip_env_assignments("FOO=bar ls"), "ls");
        assert_eq!(skip_env_assignments("A=1 B=2 cat file"), "cat file");
        assert_eq!(skip_env_assignments("FOO=\"hello world\" ls"), "ls");
        assert_eq!(skip_env_assignments("ls -la"), "ls -la");
    }
}
