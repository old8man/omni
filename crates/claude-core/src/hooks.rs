use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

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
    fn from_str(s: &str) -> Result<Self> {
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

/// A single hook command configuration.
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
}

/// A matcher groups hooks for an event+pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookMatcher {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hooks: Vec<HookCommand>,
}

/// Source where a hook config was loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookSource {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    SessionHook,
    PluginHook,
    BuiltinHook,
}

/// A fully resolved hook config with its source.
#[derive(Debug, Clone)]
pub struct IndividualHookConfig {
    pub event: HookEvent,
    pub config: HookCommand,
    pub matcher: Option<String>,
    pub source: HookSource,
}

/// Result of executing a hook.
#[derive(Debug, Clone)]
pub struct HookResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub outcome: HookOutcome,
}

/// Outcome of a hook execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Success,
    Error,
    Cancelled,
}

/// Registry of hooks organized by event.
pub struct HookRegistry {
    hooks: HashMap<HookEvent, Vec<IndividualHookConfig>>,
}

impl HookRegistry {
    /// Create a new, empty hook registry.
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    /// Register a hook for the given event.
    pub fn register(&mut self, hook: IndividualHookConfig) {
        self.hooks.entry(hook.event).or_default().push(hook);
    }

    /// Get all hooks for a specific event.
    pub fn get_hooks(&self, event: HookEvent) -> &[IndividualHookConfig] {
        self.hooks.get(&event).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all registered hooks.
    pub fn all_hooks(&self) -> Vec<&IndividualHookConfig> {
        self.hooks.values().flat_map(|v| v.iter()).collect()
    }

    /// Remove all hooks from a specific source.
    pub fn remove_by_source(&mut self, source: &HookSource) {
        for hooks in self.hooks.values_mut() {
            hooks.retain(|h| &h.source != source);
        }
    }

    /// Clear all hooks.
    pub fn clear(&mut self) {
        self.hooks.clear();
    }

    /// Check if any hooks are registered for the given event.
    pub fn has_hooks(&self, event: HookEvent) -> bool {
        self.hooks.get(&event).is_some_and(|v| !v.is_empty())
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Load hooks from a settings structure.
pub fn load_hooks_from_settings(
    hooks_settings: &HashMap<String, Vec<HookMatcher>>,
    source: HookSource,
) -> Vec<IndividualHookConfig> {
    let mut result = Vec::new();
    for (event_name, matchers) in hooks_settings {
        let event = match event_name.parse::<HookEvent>() {
            Ok(e) => e,
            Err(err) => {
                warn!("ignoring unknown hook event {event_name:?}: {err}");
                continue;
            }
        };
        for matcher in matchers {
            for hook_cmd in &matcher.hooks {
                result.push(IndividualHookConfig {
                    event,
                    config: hook_cmd.clone(),
                    matcher: matcher.matcher.clone(),
                    source: source.clone(),
                });
            }
        }
    }
    result
}

/// Execute a shell command hook.
pub async fn execute_hook(config: &HookCommand, input_json: &Value) -> Result<HookResult> {
    match config {
        HookCommand::Command {
            command,
            shell,
            timeout,
            ..
        } => execute_shell_hook(command, shell, input_json, *timeout).await,
        HookCommand::Http { url, timeout, .. } => {
            execute_http_hook(url, input_json, *timeout).await
        }
        HookCommand::Prompt { .. } | HookCommand::Agent { .. } => Ok(HookResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
            outcome: HookOutcome::Success,
        }),
    }
}

async fn execute_shell_hook(
    command: &str,
    shell: &str,
    input_json: &Value,
    timeout_ms: Option<u64>,
) -> Result<HookResult> {
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(60_000));
    let input = serde_json::to_string(input_json)?;
    debug!(command, shell, "executing shell hook");

    let mut child = tokio::process::Command::new(shell)
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn hook via {shell}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(input.as_bytes()).await;
    }

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let wait_result = tokio::time::timeout(timeout, async {
        let stdout_task = tokio::spawn(async move {
            if let Some(stdout) = stdout_handle {
                use tokio::io::AsyncReadExt;
                let mut buf = String::new();
                let _ = tokio::io::BufReader::new(stdout)
                    .read_to_string(&mut buf)
                    .await;
                buf
            } else {
                String::new()
            }
        });
        let stderr_task = tokio::spawn(async move {
            if let Some(stderr) = stderr_handle {
                use tokio::io::AsyncReadExt;
                let mut buf = String::new();
                let _ = tokio::io::BufReader::new(stderr)
                    .read_to_string(&mut buf)
                    .await;
                buf
            } else {
                String::new()
            }
        });
        let status = child.wait().await?;
        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        Ok::<_, anyhow::Error>((status, stdout, stderr))
    })
    .await;

    match wait_result {
        Ok(Ok((status, stdout, stderr))) => {
            let exit_code = status.code();
            let outcome = if exit_code == Some(0) {
                HookOutcome::Success
            } else {
                HookOutcome::Error
            };
            Ok(HookResult {
                stdout,
                stderr,
                exit_code,
                outcome,
            })
        }
        Ok(Err(e)) => anyhow::bail!("hook process error: {e}"),
        Err(_) => Ok(HookResult {
            stdout: String::new(),
            stderr: format!("hook timed out after {}ms", timeout.as_millis()),
            exit_code: None,
            outcome: HookOutcome::Cancelled,
        }),
    }
}

async fn execute_http_hook(
    url: &str,
    input_json: &Value,
    timeout_ms: Option<u64>,
) -> Result<HookResult> {
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(60_000));
    let client = reqwest::Client::builder().timeout(timeout).build()?;
    let response = client
        .post(url)
        .json(input_json)
        .send()
        .await
        .with_context(|| format!("HTTP hook to {url} failed"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let outcome = if status.is_success() {
        HookOutcome::Success
    } else {
        HookOutcome::Error
    };
    Ok(HookResult {
        stdout: body,
        stderr: String::new(),
        exit_code: Some(status.as_u16() as i32),
        outcome,
    })
}

/// Check if a matcher pattern matches a given value (supports `*` wildcards).
pub fn matcher_matches(pattern: &str, value: &str) -> bool {
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
    fn test_matcher_matches() {
        assert!(matcher_matches("*", "anything"));
        assert!(matcher_matches("Bash", "Bash"));
        assert!(!matcher_matches("Bash", "Read"));
        assert!(matcher_matches("Bash*", "BashTool"));
        assert!(matcher_matches("mcp__*__read", "mcp__server__read"));
    }
}
