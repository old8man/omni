use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Permission modes
// ---------------------------------------------------------------------------

/// Top-level permission mode governing the session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Normal interactive mode - read-only auto-allowed, writes prompt.
    #[default]
    Default,
    /// All operations auto-allowed (bypass all checks).
    Bypass,
    /// Every tool call requires interactive approval.
    InteractiveOnly,
    /// Auto-mode: AI classifier decides instead of prompting.
    Auto,
    /// Accept file edits in working directory without prompting.
    AcceptEdits,
    /// Never prompt; convert asks to denials (headless / CI).
    DontAsk,
    /// Plan-mode: can enter auto within a plan session.
    Plan,
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => write!(f, "Default"),
            Self::Bypass => write!(f, "Bypass Permissions"),
            Self::InteractiveOnly => write!(f, "Interactive Only"),
            Self::Auto => write!(f, "Auto"),
            Self::AcceptEdits => write!(f, "Accept Edits"),
            Self::DontAsk => write!(f, "Don't Ask"),
            Self::Plan => write!(f, "Plan"),
        }
    }
}

// ---------------------------------------------------------------------------
// Permission decisions
// ---------------------------------------------------------------------------

/// The behaviour bucket a permission check resolves to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

/// Why a particular decision was reached.
#[derive(Clone, Debug)]
pub enum PermissionDecisionReason {
    /// Matched an explicit rule.
    Rule {
        rule: PermissionRule,
    },
    /// AI classifier made the call.
    Classifier {
        classifier: String,
        reason: String,
    },
    /// A hook decided.
    Hook {
        hook_name: String,
        reason: Option<String>,
    },
    /// Permission mode itself dictates.
    Mode {
        mode: PermissionMode,
    },
    /// Safety check (e.g. dangerous patterns).
    SafetyCheck {
        reason: String,
        classifier_approvable: bool,
    },
    /// Composite: multiple sub-command results.
    SubcommandResults {
        reasons: Vec<(String, SubcommandResult)>,
    },
    /// Async/headless agent context.
    AsyncAgent {
        reason: String,
    },
    /// Working directory constraint.
    WorkingDir {
        reason: String,
    },
    Other {
        reason: String,
    },
}

/// Result for an individual sub-command inside a compound shell invocation.
#[derive(Clone, Debug)]
pub struct SubcommandResult {
    pub behavior: PermissionBehavior,
    pub reason: Option<String>,
}

/// Final permission decision returned to the caller.
#[derive(Clone, Debug)]
pub struct PermissionDecision {
    pub behavior: PermissionBehavior,
    pub message: Option<String>,
    pub reason: Option<PermissionDecisionReason>,
    pub updated_input: Option<serde_json::Value>,
}

impl PermissionDecision {
    pub fn allow() -> Self {
        Self {
            behavior: PermissionBehavior::Allow,
            message: None,
            reason: None,
            updated_input: None,
        }
    }

    pub fn deny(message: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Deny,
            message: Some(message.into()),
            reason: None,
            updated_input: None,
        }
    }

    pub fn ask(message: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Ask,
            message: Some(message.into()),
            reason: None,
            updated_input: None,
        }
    }

    pub fn with_reason(mut self, reason: PermissionDecisionReason) -> Self {
        self.reason = Some(reason);
        self
    }
}

// ---------------------------------------------------------------------------
// Permission rules
// ---------------------------------------------------------------------------

/// Where a permission rule was loaded from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionRuleSource {
    /// Project-level settings (.claude/settings.json).
    ProjectSettings,
    /// User-level settings (~/.claude/settings.json).
    UserSettings,
    /// Enterprise / managed settings.
    EnterpriseSettings,
    /// CLI argument (--allowedTools, etc.).
    CliArg,
    /// Interactive command during session.
    Command,
    /// Session-scoped (ephemeral).
    Session,
}

/// The parsed value of a permission rule: tool name + optional content constraint.
///
/// Examples:
///   - `Bash`             -> toolName="Bash", ruleContent=None
///   - `Bash(npm install)` -> toolName="Bash", ruleContent=Some("npm install")
///   - `Bash(git *)       -> toolName="Bash", ruleContent=Some("git *")
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRuleValue {
    pub tool_name: String,
    pub rule_content: Option<String>,
}

/// A fully-qualified permission rule with source and behaviour.
#[derive(Clone, Debug)]
pub struct PermissionRule {
    pub source: PermissionRuleSource,
    pub behavior: PermissionBehavior,
    pub value: PermissionRuleValue,
}

/// Legacy Rust-side rule (kept for backward compat in evaluator).
#[derive(Clone, Debug)]
pub struct LegacyPermissionRule {
    pub tool: String,
    pub pattern: Option<String>,
    pub mode: Option<PermissionRuleMode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionRuleMode {
    Read,
    Write,
    Full,
}

// ---------------------------------------------------------------------------
// Permission context
// ---------------------------------------------------------------------------

/// All the permission-related state needed to evaluate a tool invocation.
#[derive(Clone, Debug, Default)]
pub struct ToolPermissionContext {
    pub mode: PermissionMode,
    pub working_directories: HashMap<String, PathBuf>,
    /// Rules keyed by source name -> list of raw rule strings.
    pub allow_rules: HashMap<String, Vec<String>>,
    pub deny_rules: HashMap<String, Vec<String>>,
    pub ask_rules: HashMap<String, Vec<String>>,
    /// Whether permission prompts should be suppressed (headless agents).
    pub should_avoid_permission_prompts: bool,
}

// ---------------------------------------------------------------------------
// Classifier types
// ---------------------------------------------------------------------------

/// Confidence level for classifier decisions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// Result from the AI classifier.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassifierResult {
    /// Whether the classifier thinks the command matches the rule/concern.
    pub matches: bool,
    /// Optional description of what was matched.
    pub matched_description: Option<String>,
    /// Confidence in the classification.
    pub confidence: Confidence,
    /// Human-readable reason.
    pub reason: String,
}

/// Result from the YOLO / auto-mode classifier.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YoloClassifierResult {
    pub thinking: String,
    pub should_block: bool,
    pub reason: String,
}

/// Configuration for auto-mode classifier behaviour.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AutoModeConfig {
    /// Rules the classifier should allow.
    pub allow: Vec<String>,
    /// Rules the classifier should soft-deny (ask).
    pub soft_deny: Vec<String>,
    /// Environment context hints.
    pub environment: Vec<String>,
}

// ---------------------------------------------------------------------------
// Pattern types used by the rule parser
// ---------------------------------------------------------------------------

/// Discriminated union for parsed shell permission rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellPermissionRule {
    /// Exact command match.
    Exact { command: String },
    /// Legacy `prefix:*` syntax.
    Prefix { prefix: String },
    /// Glob/wildcard pattern.
    Wildcard { pattern: String },
}

// ---------------------------------------------------------------------------
// Permission template
// ---------------------------------------------------------------------------

/// A named, reusable set of permission rules.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionTemplate {
    pub name: String,
    pub description: Option<String>,
    pub allow_rules: Vec<String>,
    pub deny_rules: Vec<String>,
    pub ask_rules: Vec<String>,
    pub auto_mode_config: Option<AutoModeConfig>,
}

// ---------------------------------------------------------------------------
// Dangerous-pattern lists (mirrors dangerousPatterns.ts)
// ---------------------------------------------------------------------------

/// Cross-platform code-execution entry points.
pub const CROSS_PLATFORM_CODE_EXEC: &[&str] = &[
    "python", "python3", "python2", "node", "deno", "tsx", "ruby", "perl",
    "php", "lua", "npx", "bunx", "npm run", "yarn run", "pnpm run",
    "bun run", "bash", "sh", "ssh",
];

/// Bash-specific dangerous patterns (superset of cross-platform).
pub const DANGEROUS_BASH_PATTERNS: &[&str] = &[
    "python", "python3", "python2", "node", "deno", "tsx", "ruby", "perl",
    "php", "lua", "npx", "bunx", "npm run", "yarn run", "pnpm run",
    "bun run", "bash", "sh", "ssh",
    // Bash-only additions
    "zsh", "fish", "eval", "exec", "env", "xargs", "sudo",
];
