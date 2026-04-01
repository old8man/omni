//! Bridge protocol types for the environments API.
//!
//! These types model the wire format used between the Claude Code bridge
//! (running locally) and the Anthropic backend. The bridge registers an
//! environment, polls for work (sessions / healthchecks), acknowledges
//! work items, and sends heartbeats.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Default per-session timeout (24 hours).
pub const DEFAULT_SESSION_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

/// Reusable login guidance appended to bridge auth errors.
pub const BRIDGE_LOGIN_INSTRUCTION: &str =
    "Remote Control is only available with claude.ai subscriptions. \
     Please use `/login` to sign in with your claude.ai account.";

/// Full error printed when `claude remote-control` is run without auth.
pub const BRIDGE_LOGIN_ERROR: &str = "Error: You must be logged in to use Remote Control.\n\n\
     Remote Control is only available with claude.ai subscriptions. \
     Please use `/login` to sign in with your claude.ai account.";

/// Shown when the user disconnects Remote Control.
pub const REMOTE_CONTROL_DISCONNECTED_MSG: &str = "Remote Control disconnected.";

// ---------------------------------------------------------------------------
// Protocol types for the environments API
// ---------------------------------------------------------------------------

/// Data payload within a [`WorkResponse`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkData {
    /// Either `"session"` or `"healthcheck"`.
    #[serde(rename = "type")]
    pub work_type: String,
    /// Work item identifier (session id for session work).
    pub id: String,
}

/// A work item returned by `GET /v1/environments/{id}/work/poll`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkResponse {
    /// Server-assigned work item id.
    pub id: String,
    /// Always `"work"`.
    #[serde(rename = "type")]
    pub response_type: String,
    /// The environment this work belongs to.
    pub environment_id: String,
    /// Current state of the work item on the server.
    pub state: String,
    /// Embedded session/healthcheck data.
    pub data: WorkData,
    /// Base64url-encoded JSON containing session secrets.
    pub secret: String,
    /// Server timestamp of work creation.
    pub created_at: String,
}

/// Decoded contents of [`WorkResponse::secret`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkSecret {
    /// Schema version.
    pub version: u32,
    /// JWT for authenticating to the session ingress.
    pub session_ingress_token: String,
    /// API base URL for this session.
    pub api_base_url: String,
    /// Source repositories to clone.
    #[serde(default)]
    pub sources: Vec<WorkSecretSource>,
    /// Authentication tokens.
    #[serde(default)]
    pub auth: Vec<WorkSecretAuth>,
    /// Extra CLI arguments forwarded to the spawned session.
    #[serde(default)]
    pub claude_code_args: Option<HashMap<String, String>>,
    /// MCP server configuration for the session.
    #[serde(default)]
    pub mcp_config: Option<serde_json::Value>,
    /// Extra environment variables to set.
    #[serde(default)]
    pub environment_variables: Option<HashMap<String, String>>,
    /// Server-driven CCR v2 selector.
    #[serde(default)]
    pub use_code_sessions: Option<bool>,
}

/// A source repository entry inside [`WorkSecret`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkSecretSource {
    /// Source type, e.g. `"git_repository"`.
    #[serde(rename = "type")]
    pub source_type: String,
    /// Git metadata when `source_type == "git_repository"`.
    #[serde(default)]
    pub git_info: Option<GitInfo>,
}

/// Git repository metadata within a work secret source.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitInfo {
    /// Hosting provider, e.g. `"github"`.
    #[serde(rename = "type")]
    pub provider_type: String,
    /// Repository slug, e.g. `"owner/repo"`.
    pub repo: String,
    /// Branch or ref.
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    /// Temporary token for cloning.
    pub token: Option<String>,
}

/// An authentication entry inside [`WorkSecret`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkSecretAuth {
    /// Auth type, e.g. `"bearer"`.
    #[serde(rename = "type")]
    pub auth_type: String,
    /// The token value.
    pub token: String,
}

/// Terminal status for a completed session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDoneStatus {
    /// Session completed successfully.
    Completed,
    /// Session failed with an error.
    Failed,
    /// Session was interrupted by the user.
    Interrupted,
}

/// Activity type for session status reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionActivityType {
    /// A tool invocation started.
    ToolStart,
    /// Text output produced.
    Text,
    /// Final result delivered.
    Result,
    /// An error occurred.
    Error,
}

/// A single activity entry for session status display.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionActivity {
    /// What kind of activity this represents.
    #[serde(rename = "type")]
    pub activity_type: SessionActivityType,
    /// Human-readable description, e.g. "Editing src/foo.rs".
    pub summary: String,
    /// Unix timestamp in milliseconds.
    pub timestamp: u64,
}

/// How `claude remote-control` chooses session working directories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpawnMode {
    /// One session in cwd, bridge tears down when it ends.
    SingleSession,
    /// Persistent server, every session gets an isolated git worktree.
    Worktree,
    /// Persistent server, every session shares cwd.
    SameDir,
}

/// Well-known `worker_type` values produced by this codebase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeWorkerType {
    /// Standard Claude Code bridge worker.
    ClaudeCode,
    /// Claude Code assistant-mode worker.
    ClaudeCodeAssistant,
}

/// Configuration for a bridge instance.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// Working directory for sessions.
    pub dir: String,
    /// Human-readable machine identifier.
    pub machine_name: String,
    /// Current git branch.
    pub branch: String,
    /// Git remote URL, if available.
    pub git_repo_url: Option<String>,
    /// Maximum concurrent sessions.
    pub max_sessions: u32,
    /// How sessions choose working directories.
    pub spawn_mode: SpawnMode,
    /// Enable verbose logging.
    pub verbose: bool,
    /// Enable sandbox mode for sessions.
    pub sandbox: bool,
    /// Client-generated UUID identifying this bridge instance.
    pub bridge_id: String,
    /// Sent as `metadata.worker_type` for web-side filtering.
    pub worker_type: String,
    /// Client-generated UUID for idempotent environment registration.
    pub environment_id: String,
    /// Backend-issued environment ID to reuse on re-register.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reuse_environment_id: Option<String>,
    /// API base URL the bridge polls against.
    pub api_base_url: String,
    /// Session ingress base URL for WebSocket connections.
    pub session_ingress_url: String,
    /// Debug file path passed via `--debug-file`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_file: Option<String>,
    /// Per-session timeout in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_timeout_ms: Option<u64>,
}

/// A permission response event sent back to a session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionResponseEvent {
    /// Always `"control_response"`.
    #[serde(rename = "type")]
    pub event_type: String,
    /// The response payload.
    pub response: PermissionResponsePayload,
}

/// Inner payload of a [`PermissionResponseEvent`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionResponsePayload {
    /// Always `"success"` for permission responses.
    pub subtype: String,
    /// Matches the original request ID.
    pub request_id: String,
    /// The permission decision payload.
    pub response: serde_json::Value,
}

/// Registration response from `POST /v1/environments/bridge`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvironmentRegistration {
    /// Server-assigned environment identifier.
    pub environment_id: String,
    /// Secret token for environment-level API calls.
    pub environment_secret: String,
}

/// Heartbeat response from `POST /v1/environments/{id}/work/{id}/heartbeat`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    /// Whether the server extended the work item lease.
    pub lease_extended: bool,
    /// Current state of the work item.
    pub state: String,
}

/// Bridge connection state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeState {
    /// Not connected to the backend.
    Disconnected,
    /// Connection attempt in progress.
    Connecting,
    /// Registered and polling for work.
    Connected,
    /// Shutting down gracefully.
    ShuttingDown,
}

/// Backoff configuration for retry loops.
#[derive(Clone, Debug)]
pub struct BackoffConfig {
    /// Initial delay between retries in milliseconds.
    pub initial_delay_ms: u64,
    /// Maximum delay cap in milliseconds.
    pub max_delay_ms: u64,
    /// Total elapsed time before giving up, in milliseconds.
    pub give_up_after_ms: u64,
}

impl BackoffConfig {
    /// Default connection backoff: 2s initial, 2m cap, 10m give-up.
    pub fn connection() -> Self {
        Self {
            initial_delay_ms: 2_000,
            max_delay_ms: 120_000,
            give_up_after_ms: 600_000,
        }
    }

    /// Default general backoff: 500ms initial, 30s cap, 10m give-up.
    pub fn general() -> Self {
        Self {
            initial_delay_ms: 500,
            max_delay_ms: 30_000,
            give_up_after_ms: 600_000,
        }
    }

    /// Calculate the delay for a given attempt number (exponential with cap).
    pub fn delay_for_attempt(&self, attempt: u32) -> u64 {
        let delay = self
            .initial_delay_ms
            .saturating_mul(1u64 << attempt.min(20));
        delay.min(self.max_delay_ms)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_delay_calculation() {
        let cfg = BackoffConfig::general();
        assert_eq!(cfg.delay_for_attempt(0), 500);
        assert_eq!(cfg.delay_for_attempt(1), 1000);
        assert_eq!(cfg.delay_for_attempt(2), 2000);
        // Should cap at max_delay_ms
        assert_eq!(cfg.delay_for_attempt(20), 30_000);
    }

    #[test]
    fn test_backoff_connection_defaults() {
        let cfg = BackoffConfig::connection();
        assert_eq!(cfg.initial_delay_ms, 2_000);
        assert_eq!(cfg.max_delay_ms, 120_000);
        assert_eq!(cfg.give_up_after_ms, 600_000);
    }

    #[test]
    fn test_spawn_mode_serialization() {
        let json = serde_json::to_string(&SpawnMode::SingleSession).unwrap();
        assert_eq!(json, "\"single-session\"");

        let mode: SpawnMode = serde_json::from_str("\"worktree\"").unwrap();
        assert_eq!(mode, SpawnMode::Worktree);
    }

    #[test]
    fn test_session_done_status_serialization() {
        let json = serde_json::to_string(&SessionDoneStatus::Completed).unwrap();
        assert_eq!(json, "\"completed\"");

        let status: SessionDoneStatus = serde_json::from_str("\"interrupted\"").unwrap();
        assert_eq!(status, SessionDoneStatus::Interrupted);
    }

    #[test]
    fn test_work_secret_deserialization() {
        let json = r#"{
            "version": 1,
            "session_ingress_token": "sk-ant-si-abc.def.ghi",
            "api_base_url": "https://api.anthropic.com",
            "sources": [],
            "auth": [{"type": "bearer", "token": "tok123"}]
        }"#;
        let secret: WorkSecret = serde_json::from_str(json).unwrap();
        assert_eq!(secret.version, 1);
        assert_eq!(secret.session_ingress_token, "sk-ant-si-abc.def.ghi");
        assert_eq!(secret.auth.len(), 1);
        assert_eq!(secret.auth[0].auth_type, "bearer");
    }
}
