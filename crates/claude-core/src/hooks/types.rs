//! Hook type definitions: events, commands, inputs, outputs, and results.
//!
//! Mirrors the TypeScript types from `types/hooks.ts` and `schemas/hooks.ts`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Hook events ───────────────────────────────────────────────────────────────

/// All supported hook events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    Notification,
    UserPromptSubmit,
    SessionStart,
    SessionEnd,
    Stop,
    StopFailure,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PostCompact,
    PermissionRequest,
    PermissionDenied,
    Setup,
    TeammateIdle,
    TaskCreated,
    TaskCompleted,
    Elicitation,
    ElicitationResult,
    ConfigChange,
    WorktreeCreate,
    WorktreeRemove,
    InstructionsLoaded,
    CwdChanged,
    FileChanged,
}

impl HookEvent {
    /// All known hook events.
    pub fn all() -> &'static [HookEvent] {
        &[
            Self::PreToolUse,
            Self::PostToolUse,
            Self::PostToolUseFailure,
            Self::Notification,
            Self::UserPromptSubmit,
            Self::SessionStart,
            Self::SessionEnd,
            Self::Stop,
            Self::StopFailure,
            Self::SubagentStart,
            Self::SubagentStop,
            Self::PreCompact,
            Self::PostCompact,
            Self::PermissionRequest,
            Self::PermissionDenied,
            Self::Setup,
            Self::TeammateIdle,
            Self::TaskCreated,
            Self::TaskCompleted,
            Self::Elicitation,
            Self::ElicitationResult,
            Self::ConfigChange,
            Self::WorktreeCreate,
            Self::WorktreeRemove,
            Self::InstructionsLoaded,
            Self::CwdChanged,
            Self::FileChanged,
        ]
    }
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::str::FromStr for HookEvent {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "PreToolUse" => Ok(Self::PreToolUse),
            "PostToolUse" => Ok(Self::PostToolUse),
            "PostToolUseFailure" => Ok(Self::PostToolUseFailure),
            "Notification" => Ok(Self::Notification),
            "UserPromptSubmit" => Ok(Self::UserPromptSubmit),
            "SessionStart" => Ok(Self::SessionStart),
            "SessionEnd" => Ok(Self::SessionEnd),
            "Stop" => Ok(Self::Stop),
            "StopFailure" => Ok(Self::StopFailure),
            "SubagentStart" => Ok(Self::SubagentStart),
            "SubagentStop" => Ok(Self::SubagentStop),
            "PreCompact" => Ok(Self::PreCompact),
            "PostCompact" => Ok(Self::PostCompact),
            "PermissionRequest" => Ok(Self::PermissionRequest),
            "PermissionDenied" => Ok(Self::PermissionDenied),
            "Setup" => Ok(Self::Setup),
            "TeammateIdle" => Ok(Self::TeammateIdle),
            "TaskCreated" => Ok(Self::TaskCreated),
            "TaskCompleted" => Ok(Self::TaskCompleted),
            "Elicitation" => Ok(Self::Elicitation),
            "ElicitationResult" => Ok(Self::ElicitationResult),
            "ConfigChange" => Ok(Self::ConfigChange),
            "WorktreeCreate" => Ok(Self::WorktreeCreate),
            "WorktreeRemove" => Ok(Self::WorktreeRemove),
            "InstructionsLoaded" => Ok(Self::InstructionsLoaded),
            "CwdChanged" => Ok(Self::CwdChanged),
            "FileChanged" => Ok(Self::FileChanged),
            _ => anyhow::bail!("unknown hook event: {s:?}"),
        }
    }
}

/// Check whether a string is a valid hook event name.
pub fn is_hook_event(value: &str) -> bool {
    value.parse::<HookEvent>().is_ok()
}

// ── Hook commands (user-defined hook configuration) ──────────────────────────

/// A single hook command configuration, tagged by type.
///
/// Matches the TypeScript `HookCommand` discriminated union from `schemas/hooks.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HookCommand {
    #[serde(rename = "command")]
    Command {
        command: String,
        #[serde(default = "default_shell")]
        shell: String,
        #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    #[serde(rename = "prompt")]
    Prompt {
        prompt: String,
        #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    #[serde(rename = "agent")]
    Agent {
        prompt: String,
        #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    #[serde(rename = "http")]
    Http {
        url: String,
        #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
}

fn default_shell() -> String {
    "bash".to_string()
}

impl HookCommand {
    /// Get a short display string for this hook.
    pub fn display_text(&self) -> &str {
        match self {
            Self::Command { command, .. } => command,
            Self::Prompt { prompt, .. } | Self::Agent { prompt, .. } => prompt,
            Self::Http { url, .. } => url,
        }
    }

    /// Get the `if` condition for this hook, if any.
    pub fn condition(&self) -> Option<&str> {
        match self {
            Self::Command { condition, .. }
            | Self::Prompt { condition, .. }
            | Self::Agent { condition, .. }
            | Self::Http { condition, .. } => condition.as_deref(),
        }
    }

    /// Get the timeout in seconds, if any.
    pub fn timeout_secs(&self) -> Option<u64> {
        match self {
            Self::Command { timeout, .. }
            | Self::Prompt { timeout, .. }
            | Self::Agent { timeout, .. }
            | Self::Http { timeout, .. } => *timeout,
        }
    }

    /// Check equality by comparing command/prompt content, shell, and `if` condition.
    /// Timeout is intentionally excluded (same hook with different timeouts is the same hook).
    pub fn is_equal(&self, other: &HookCommand) -> bool {
        match (self, other) {
            (
                HookCommand::Command {
                    command: a,
                    shell: sa,
                    condition: ca,
                    ..
                },
                HookCommand::Command {
                    command: b,
                    shell: sb,
                    condition: cb,
                    ..
                },
            ) => a == b && sa == sb && ca == cb,
            (
                HookCommand::Prompt {
                    prompt: a,
                    condition: ca,
                    ..
                },
                HookCommand::Prompt {
                    prompt: b,
                    condition: cb,
                    ..
                },
            ) => a == b && ca == cb,
            (
                HookCommand::Agent {
                    prompt: a,
                    condition: ca,
                    ..
                },
                HookCommand::Agent {
                    prompt: b,
                    condition: cb,
                    ..
                },
            ) => a == b && ca == cb,
            (
                HookCommand::Http {
                    url: a,
                    condition: ca,
                    ..
                },
                HookCommand::Http {
                    url: b,
                    condition: cb,
                    ..
                },
            ) => a == b && ca == cb,
            _ => false,
        }
    }
}

/// A matcher groups hooks for an event + pattern.
///
/// Matches the TypeScript `HookMatcher` from `schemas/hooks.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookMatcher {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hooks: Vec<HookCommand>,
}

// ── Hook source ──────────────────────────────────────────────────────────────

/// Source where a hook config was loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookSource {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    SessionHook,
    PluginHook,
    BuiltinHook,
    PolicySettings,
}

impl HookSource {
    /// Human-readable description of the source for display.
    pub fn description(&self) -> &'static str {
        match self {
            Self::UserSettings => "User settings (~/.claude/settings.json)",
            Self::ProjectSettings => "Project settings (.claude/settings.json)",
            Self::LocalSettings => "Local settings (.claude/settings.local.json)",
            Self::SessionHook => "Session hooks (in-memory, temporary)",
            Self::PluginHook => "Plugin hooks (~/.claude/plugins/*/hooks/hooks.json)",
            Self::BuiltinHook => "Built-in hooks (registered internally by Claude Code)",
            Self::PolicySettings => "Policy settings (managed)",
        }
    }

    /// Short display string for inline use.
    pub fn inline_display(&self) -> &'static str {
        match self {
            Self::UserSettings => "User",
            Self::ProjectSettings => "Project",
            Self::LocalSettings => "Local",
            Self::SessionHook => "Session",
            Self::PluginHook => "Plugin",
            Self::BuiltinHook => "Built-in",
            Self::PolicySettings => "Policy",
        }
    }

    /// Whether this is a managed (policy) source.
    pub fn is_managed(&self) -> bool {
        matches!(self, Self::PolicySettings | Self::BuiltinHook)
    }
}

/// A fully resolved hook config with its source.
#[derive(Debug, Clone)]
pub struct IndividualHookConfig {
    pub event: HookEvent,
    pub config: HookCommand,
    pub matcher: Option<String>,
    pub source: HookSource,
    pub plugin_name: Option<String>,
}

// ── Hook input types ─────────────────────────────────────────────────────────

/// Common fields shared by all hook inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseHookInput {
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
}

/// Structured input passed to hooks. This is the JSON that gets written to stdin
/// or POSTed to HTTP endpoints.
///
/// Mirrors the TypeScript `HookInput` discriminated union.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum HookInput {
    PreToolUse {
        #[serde(flatten)]
        base: BaseHookInput,
        tool_name: String,
        tool_input: Value,
    },
    PostToolUse {
        #[serde(flatten)]
        base: BaseHookInput,
        tool_name: String,
        tool_input: Value,
        tool_output: Value,
    },
    PostToolUseFailure {
        #[serde(flatten)]
        base: BaseHookInput,
        tool_name: String,
        tool_input: Value,
        error: String,
    },
    Notification {
        #[serde(flatten)]
        base: BaseHookInput,
        notification_type: String,
        message: String,
    },
    UserPromptSubmit {
        #[serde(flatten)]
        base: BaseHookInput,
        user_prompt: String,
    },
    SessionStart {
        #[serde(flatten)]
        base: BaseHookInput,
        source: String,
    },
    SessionEnd {
        #[serde(flatten)]
        base: BaseHookInput,
        reason: String,
    },
    Stop {
        #[serde(flatten)]
        base: BaseHookInput,
        stop_hook_active: bool,
        assistant_message: String,
    },
    StopFailure {
        #[serde(flatten)]
        base: BaseHookInput,
        error: String,
    },
    SubagentStart {
        #[serde(flatten)]
        base: BaseHookInput,
        agent_type: String,
    },
    SubagentStop {
        #[serde(flatten)]
        base: BaseHookInput,
        agent_type: String,
    },
    PreCompact {
        #[serde(flatten)]
        base: BaseHookInput,
        trigger: String,
    },
    PostCompact {
        #[serde(flatten)]
        base: BaseHookInput,
        trigger: String,
    },
    PermissionRequest {
        #[serde(flatten)]
        base: BaseHookInput,
        tool_name: String,
        tool_input: Value,
    },
    PermissionDenied {
        #[serde(flatten)]
        base: BaseHookInput,
        tool_name: String,
        tool_input: Value,
    },
    Setup {
        #[serde(flatten)]
        base: BaseHookInput,
        trigger: String,
    },
    TeammateIdle {
        #[serde(flatten)]
        base: BaseHookInput,
    },
    TaskCreated {
        #[serde(flatten)]
        base: BaseHookInput,
        task_id: String,
    },
    TaskCompleted {
        #[serde(flatten)]
        base: BaseHookInput,
        task_id: String,
    },
    Elicitation {
        #[serde(flatten)]
        base: BaseHookInput,
        mcp_server_name: String,
    },
    ElicitationResult {
        #[serde(flatten)]
        base: BaseHookInput,
        mcp_server_name: String,
    },
    ConfigChange {
        #[serde(flatten)]
        base: BaseHookInput,
        source: String,
    },
    WorktreeCreate {
        #[serde(flatten)]
        base: BaseHookInput,
        worktree_path: String,
    },
    WorktreeRemove {
        #[serde(flatten)]
        base: BaseHookInput,
        worktree_path: String,
    },
    InstructionsLoaded {
        #[serde(flatten)]
        base: BaseHookInput,
        load_reason: String,
    },
    CwdChanged {
        #[serde(flatten)]
        base: BaseHookInput,
        old_cwd: String,
        new_cwd: String,
    },
    FileChanged {
        #[serde(flatten)]
        base: BaseHookInput,
        file_path: String,
        change_type: String,
    },
}

impl HookInput {
    /// Get the hook event name for this input.
    pub fn hook_event_name(&self) -> HookEvent {
        match self {
            Self::PreToolUse { .. } => HookEvent::PreToolUse,
            Self::PostToolUse { .. } => HookEvent::PostToolUse,
            Self::PostToolUseFailure { .. } => HookEvent::PostToolUseFailure,
            Self::Notification { .. } => HookEvent::Notification,
            Self::UserPromptSubmit { .. } => HookEvent::UserPromptSubmit,
            Self::SessionStart { .. } => HookEvent::SessionStart,
            Self::SessionEnd { .. } => HookEvent::SessionEnd,
            Self::Stop { .. } => HookEvent::Stop,
            Self::StopFailure { .. } => HookEvent::StopFailure,
            Self::SubagentStart { .. } => HookEvent::SubagentStart,
            Self::SubagentStop { .. } => HookEvent::SubagentStop,
            Self::PreCompact { .. } => HookEvent::PreCompact,
            Self::PostCompact { .. } => HookEvent::PostCompact,
            Self::PermissionRequest { .. } => HookEvent::PermissionRequest,
            Self::PermissionDenied { .. } => HookEvent::PermissionDenied,
            Self::Setup { .. } => HookEvent::Setup,
            Self::TeammateIdle { .. } => HookEvent::TeammateIdle,
            Self::TaskCreated { .. } => HookEvent::TaskCreated,
            Self::TaskCompleted { .. } => HookEvent::TaskCompleted,
            Self::Elicitation { .. } => HookEvent::Elicitation,
            Self::ElicitationResult { .. } => HookEvent::ElicitationResult,
            Self::ConfigChange { .. } => HookEvent::ConfigChange,
            Self::WorktreeCreate { .. } => HookEvent::WorktreeCreate,
            Self::WorktreeRemove { .. } => HookEvent::WorktreeRemove,
            Self::InstructionsLoaded { .. } => HookEvent::InstructionsLoaded,
            Self::CwdChanged { .. } => HookEvent::CwdChanged,
            Self::FileChanged { .. } => HookEvent::FileChanged,
        }
    }

    /// Get the match query value for this input (used for pattern matching).
    pub fn match_query(&self) -> Option<&str> {
        match self {
            Self::PreToolUse { tool_name, .. }
            | Self::PostToolUse { tool_name, .. }
            | Self::PostToolUseFailure { tool_name, .. }
            | Self::PermissionRequest { tool_name, .. }
            | Self::PermissionDenied { tool_name, .. } => Some(tool_name.as_str()),
            Self::SessionStart { source, .. } | Self::ConfigChange { source, .. } => {
                Some(source.as_str())
            }
            Self::Setup { trigger, .. }
            | Self::PreCompact { trigger, .. }
            | Self::PostCompact { trigger, .. } => Some(trigger.as_str()),
            Self::Notification {
                notification_type, ..
            } => Some(notification_type.as_str()),
            Self::SessionEnd { reason, .. } => Some(reason.as_str()),
            Self::StopFailure { error, .. } => Some(error.as_str()),
            Self::SubagentStart { agent_type, .. }
            | Self::SubagentStop { agent_type, .. } => Some(agent_type.as_str()),
            Self::Elicitation {
                mcp_server_name, ..
            }
            | Self::ElicitationResult {
                mcp_server_name, ..
            } => Some(mcp_server_name.as_str()),
            Self::InstructionsLoaded { load_reason, .. } => Some(load_reason.as_str()),
            Self::FileChanged { file_path, .. } => {
                // Match on the basename, not full path
                std::path::Path::new(file_path)
                    .file_name()
                    .and_then(|n| n.to_str())
            }
            Self::TeammateIdle { .. }
            | Self::TaskCreated { .. }
            | Self::TaskCompleted { .. }
            | Self::Stop { .. }
            | Self::UserPromptSubmit { .. }
            | Self::CwdChanged { .. }
            | Self::WorktreeCreate { .. }
            | Self::WorktreeRemove { .. } => None,
        }
    }

    /// Access the base fields.
    pub fn base(&self) -> &BaseHookInput {
        match self {
            Self::PreToolUse { base, .. }
            | Self::PostToolUse { base, .. }
            | Self::PostToolUseFailure { base, .. }
            | Self::Notification { base, .. }
            | Self::UserPromptSubmit { base, .. }
            | Self::SessionStart { base, .. }
            | Self::SessionEnd { base, .. }
            | Self::Stop { base, .. }
            | Self::StopFailure { base, .. }
            | Self::SubagentStart { base, .. }
            | Self::SubagentStop { base, .. }
            | Self::PreCompact { base, .. }
            | Self::PostCompact { base, .. }
            | Self::PermissionRequest { base, .. }
            | Self::PermissionDenied { base, .. }
            | Self::Setup { base, .. }
            | Self::TeammateIdle { base, .. }
            | Self::TaskCreated { base, .. }
            | Self::TaskCompleted { base, .. }
            | Self::Elicitation { base, .. }
            | Self::ElicitationResult { base, .. }
            | Self::ConfigChange { base, .. }
            | Self::WorktreeCreate { base, .. }
            | Self::WorktreeRemove { base, .. }
            | Self::InstructionsLoaded { base, .. }
            | Self::CwdChanged { base, .. }
            | Self::FileChanged { base, .. } => base,
        }
    }
}

// ── Hook JSON output (parsed from hook stdout / HTTP response) ───────────────

/// Hook-specific output fields keyed by event name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "hookEventName")]
pub enum HookSpecificOutput {
    PreToolUse {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "permissionDecision")]
        permission_decision: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "permissionDecisionReason")]
        permission_decision_reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "updatedInput")]
        updated_input: Option<HashMap<String, Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "additionalContext")]
        additional_context: Option<String>,
    },
    UserPromptSubmit {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "additionalContext")]
        additional_context: Option<String>,
    },
    SessionStart {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "additionalContext")]
        additional_context: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "initialUserMessage")]
        initial_user_message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "watchPaths")]
        watch_paths: Option<Vec<String>>,
    },
    Setup {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "additionalContext")]
        additional_context: Option<String>,
    },
    SubagentStart {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "additionalContext")]
        additional_context: Option<String>,
    },
    PostToolUse {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "additionalContext")]
        additional_context: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "updatedMCPToolOutput")]
        updated_mcp_tool_output: Option<Value>,
    },
    PostToolUseFailure {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "additionalContext")]
        additional_context: Option<String>,
    },
    PermissionDenied {
        #[serde(skip_serializing_if = "Option::is_none")]
        retry: Option<bool>,
    },
    Notification {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "additionalContext")]
        additional_context: Option<String>,
    },
    PermissionRequest {
        decision: PermissionRequestDecision,
    },
    Elicitation {
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<HashMap<String, Value>>,
    },
    ElicitationResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<HashMap<String, Value>>,
    },
    CwdChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "watchPaths")]
        watch_paths: Option<Vec<String>>,
    },
    FileChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "watchPaths")]
        watch_paths: Option<Vec<String>>,
    },
    WorktreeCreate {
        #[serde(rename = "worktreePath")]
        worktree_path: String,
    },
}

/// Permission request decision from a PermissionRequest hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior")]
pub enum PermissionRequestDecision {
    #[serde(rename = "allow")]
    Allow {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "updatedInput")]
        updated_input: Option<HashMap<String, Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "updatedPermissions")]
        updated_permissions: Option<Vec<Value>>,
    },
    #[serde(rename = "deny")]
    Deny {
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        interrupt: Option<bool>,
    },
}

/// Synchronous hook JSON output (the common case).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncHookJsonOutput {
    /// Whether Claude should continue after hook (default: true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#continue: Option<bool>,
    /// Hide stdout from transcript (default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppress_output: Option<bool>,
    /// Message shown when continue is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// "approve" or "block".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// Explanation for the decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Warning message shown to the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
    /// Event-specific output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook_specific_output: Option<HookSpecificOutput>,
}

/// Asynchronous hook JSON output (hook runs in background).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsyncHookJsonOutput {
    pub r#async: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub async_timeout: Option<u64>,
}

/// Hook JSON output: either sync or async.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HookJsonOutput {
    Async(AsyncHookJsonOutput),
    Sync(SyncHookJsonOutput),
}

impl HookJsonOutput {
    /// Check if this is an async response.
    pub fn is_async(&self) -> bool {
        matches!(self, Self::Async(a) if a.r#async)
    }

    /// Get the sync output, if this is a sync response.
    pub fn as_sync(&self) -> Option<&SyncHookJsonOutput> {
        match self {
            Self::Sync(s) => Some(s),
            Self::Async(_) => None,
        }
    }
}

// ── Hook results ─────────────────────────────────────────────────────────────

/// A blocking error from a hook execution.
#[derive(Debug, Clone)]
pub struct HookBlockingError {
    pub blocking_error: String,
    pub command: String,
}

/// Permission behavior set by a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
    Passthrough,
}

/// Outcome of executing a single hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    Success,
    Blocking,
    NonBlockingError,
    Cancelled,
}

impl std::fmt::Display for HookOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Blocking => write!(f, "blocking"),
            Self::NonBlockingError => write!(f, "non_blocking_error"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Result of executing a single hook.
#[derive(Debug, Clone)]
pub struct HookResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub outcome: HookOutcome,
    pub blocking_error: Option<HookBlockingError>,
    pub prevent_continuation: bool,
    pub stop_reason: Option<String>,
    pub permission_behavior: Option<PermissionBehavior>,
    pub hook_permission_decision_reason: Option<String>,
    pub additional_context: Option<String>,
    pub initial_user_message: Option<String>,
    pub updated_input: Option<HashMap<String, Value>>,
    pub updated_mcp_tool_output: Option<Value>,
    pub permission_request_result: Option<PermissionRequestDecision>,
    pub watch_paths: Option<Vec<String>>,
    pub retry: Option<bool>,
    pub system_message: Option<String>,
}

impl Default for HookResult {
    fn default() -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            outcome: HookOutcome::Success,
            blocking_error: None,
            prevent_continuation: false,
            stop_reason: None,
            permission_behavior: None,
            hook_permission_decision_reason: None,
            additional_context: None,
            initial_user_message: None,
            updated_input: None,
            updated_mcp_tool_output: None,
            permission_request_result: None,
            watch_paths: None,
            retry: None,
            system_message: None,
        }
    }
}

/// Aggregated result from executing all hooks for an event.
#[derive(Debug, Clone, Default)]
pub struct AggregatedHookResult {
    pub blocking_errors: Vec<HookBlockingError>,
    pub prevent_continuation: bool,
    pub stop_reason: Option<String>,
    pub hook_permission_decision_reason: Option<String>,
    pub permission_behavior: Option<PermissionBehavior>,
    pub additional_contexts: Vec<String>,
    pub initial_user_message: Option<String>,
    pub updated_input: Option<HashMap<String, Value>>,
    pub updated_mcp_tool_output: Option<Value>,
    pub permission_request_result: Option<PermissionRequestDecision>,
    pub watch_paths: Vec<String>,
    pub retry: Option<bool>,
    pub system_messages: Vec<String>,
    pub outcomes: HookOutcomeCounts,
}

/// Counts of each outcome type from a batch of hook executions.
#[derive(Debug, Clone, Default)]
pub struct HookOutcomeCounts {
    pub success: usize,
    pub blocking: usize,
    pub non_blocking_error: usize,
    pub cancelled: usize,
}

// ── Hook execution events (for SDK / UI observability) ───────────────────────

/// Events emitted during hook execution for observability.
#[derive(Debug, Clone)]
pub enum HookExecutionEvent {
    Started {
        hook_id: String,
        hook_name: String,
        hook_event: String,
    },
    Progress {
        hook_id: String,
        hook_name: String,
        hook_event: String,
        stdout: String,
        stderr: String,
        output: String,
    },
    Response {
        hook_id: String,
        hook_name: String,
        hook_event: String,
        output: String,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        outcome: String,
    },
}

/// Result from hooks executed outside the REPL loop.
#[derive(Debug, Clone)]
pub struct HookOutsideReplResult {
    pub command: String,
    pub succeeded: bool,
    pub output: String,
    pub blocked: bool,
    pub watch_paths: Vec<String>,
    pub system_message: Option<String>,
}

/// Settings for hooks at the top-level configuration.
pub type HooksSettings = HashMap<String, Vec<HookMatcher>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_event_roundtrip() {
        for event in HookEvent::all() {
            let s = event.to_string();
            let parsed: HookEvent = s.parse().unwrap();
            assert_eq!(*event, parsed);
        }
    }

    #[test]
    fn test_is_hook_event() {
        assert!(is_hook_event("PreToolUse"));
        assert!(is_hook_event("SessionStart"));
        assert!(!is_hook_event("FakeEvent"));
        assert!(!is_hook_event(""));
    }

    #[test]
    fn test_hook_command_equality() {
        let a = HookCommand::Command {
            command: "echo hi".to_string(),
            shell: "bash".to_string(),
            condition: None,
            timeout: Some(10),
        };
        let b = HookCommand::Command {
            command: "echo hi".to_string(),
            shell: "bash".to_string(),
            condition: None,
            timeout: Some(999),
        };
        assert!(a.is_equal(&b));

        let c = HookCommand::Command {
            command: "echo hi".to_string(),
            shell: "zsh".to_string(),
            condition: None,
            timeout: None,
        };
        assert!(!a.is_equal(&c));
    }

    #[test]
    fn test_hook_json_output_parse_sync() {
        let json = r#"{"continue": false, "stopReason": "test"}"#;
        let output: HookJsonOutput = serde_json::from_str(json).unwrap();
        assert!(!output.is_async());
        let sync = output.as_sync().unwrap();
        assert_eq!(sync.r#continue, Some(false));
        assert_eq!(sync.stop_reason.as_deref(), Some("test"));
    }

    #[test]
    fn test_hook_json_output_parse_async() {
        let json = r#"{"async": true, "asyncTimeout": 5000}"#;
        let output: HookJsonOutput = serde_json::from_str(json).unwrap();
        assert!(output.is_async());
    }
}
