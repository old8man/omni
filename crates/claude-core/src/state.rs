//! Application state: the central, shared data structure for a Claude session.
//!
//! `AppState` is the Rust equivalent of the TypeScript `AppState` + global
//! bootstrap `State`. It owns all session-scoped data: settings, cost tracking,
//! session persistence, hook/skill/plugin registries, MCP connections, and
//! per-turn metrics.
//!
//! The state is wrapped in `AppStateStore` (an `Arc<RwLock<AppState>>`) so it
//! can be shared across the query engine, TUI, and background tasks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::config::settings::Settings;
use crate::cost_tracker::CostTracker;
use crate::hooks::HookRegistry;
use crate::mcp::McpManager;
use crate::permissions::types::PermissionMode;
use crate::plugins::PluginRegistry;
use crate::session::SessionManager;
use crate::skills::SkillRegistry;
use crate::tasks::TaskManager;
use crate::utils::file_history::FileHistoryState;

// ── Sub-state types ─────────────────────────────────────────────────────────

/// Notification state.
#[derive(Clone, Debug, Default)]
pub struct NotificationState {
    pub current: Option<Notification>,
    pub queue: Vec<Notification>,
}

/// A user-facing notification.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notification {
    pub message: String,
    #[serde(default)]
    pub level: NotificationLevel,
    #[serde(default)]
    pub source: Option<String>,
}

/// Notification severity level.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationLevel {
    #[default]
    Info,
    Warning,
    Error,
}

/// MCP sub-state: connections, tools, and resources.
#[derive(Clone, Debug, Default)]
pub struct McpState {
    pub tool_count: usize,
    pub command_count: usize,
    pub resource_count: usize,
    pub plugin_reconnect_key: u64,
}

/// Plugin sub-state.
#[derive(Clone, Debug, Default)]
pub struct PluginState {
    pub enabled_count: usize,
    pub disabled_count: usize,
    pub error_count: usize,
    pub needs_refresh: bool,
}

/// Teammate context for swarm mode.
#[derive(Clone, Debug)]
pub struct TeamContext {
    pub team_name: String,
    pub team_file_path: String,
    pub lead_agent_id: String,
    pub self_agent_id: Option<String>,
    pub self_agent_name: Option<String>,
    pub is_leader: bool,
    pub teammates: HashMap<String, TeammateInfo>,
}

/// Information about a single teammate.
#[derive(Clone, Debug)]
pub struct TeammateInfo {
    pub name: String,
    pub agent_type: Option<String>,
    pub color: Option<String>,
    pub cwd: String,
    pub spawned_at: u64,
}

/// Inbox message from teammates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxMessage {
    pub id: String,
    pub from: String,
    pub text: String,
    pub timestamp: String,
    pub status: InboxMessageStatus,
    pub color: Option<String>,
    pub summary: Option<String>,
}

/// Status of an inbox message.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InboxMessageStatus {
    #[default]
    Pending,
    Processing,
    Processed,
}

/// Per-model usage accumulator (matches TS `ModelUsage` in bootstrap/state.ts).
#[derive(Clone, Debug, Default)]
pub struct ModelUsageState {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
}

/// Expanded view state for the TUI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExpandedView {
    #[default]
    None,
    Tasks,
    Teammates,
}

// ── AppState ────────────────────────────────────────────────────────────────

/// Central application state for a Claude session.
///
/// This combines the TypeScript `AppState` (reactive UI state) and
/// `bootstrap/state.ts` (global singleton) into a single struct.
pub struct AppState {
    // ── Identity ─────────────────────────────────────────────────────────
    /// Unique session identifier (UUID).
    pub session_id: String,
    /// Parent session ID for lineage tracking (e.g. plan mode -> implementation).
    pub parent_session_id: Option<String>,

    // ── Paths ────────────────────────────────────────────────────────────
    /// Stable project root — set once at startup, not updated mid-session.
    pub project_root: PathBuf,
    /// Original working directory at startup.
    pub original_cwd: PathBuf,
    /// Current working directory (may change during session via CwdChanged hooks).
    pub cwd: PathBuf,

    // ── Settings & config ────────────────────────────────────────────────
    /// Merged settings (user + project + local + env).
    pub settings: Settings,

    // ── Model ────────────────────────────────────────────────────────────
    /// Active model name (resolved from CLI, settings, or default).
    pub model: String,
    /// Model override for the current session (via /model command).
    pub model_override: Option<String>,

    // ── Mode flags ───────────────────────────────────────────────────────
    pub verbose: bool,
    pub permission_mode: PermissionMode,
    pub plan_mode: bool,
    pub vim_mode: bool,
    pub brief_mode: bool,
    /// KAIROS assistant mode active.
    pub assistant_mode: bool,
    /// Fast mode (same model, faster output).
    pub fast_mode: bool,
    /// Whether thinking is enabled for the model.
    pub thinking_enabled: bool,
    /// Whether prompt suggestions are enabled.
    pub prompt_suggestion_enabled: bool,
    /// Whether this is a non-interactive (headless/print) session.
    pub is_non_interactive: bool,

    // ── Cost & usage tracking ────────────────────────────────────────────
    pub cost_tracker: CostTracker,
    /// Per-model usage accumulator.
    pub model_usage: HashMap<String, ModelUsageState>,

    // ── Turn metrics ─────────────────────────────────────────────────────
    pub turn_count: u64,
    pub total_api_duration_ms: f64,
    pub total_api_duration_without_retries_ms: f64,
    pub total_tool_duration_ms: f64,
    /// Per-turn metrics (reset each turn).
    pub turn_hook_duration_ms: f64,
    pub turn_tool_duration_ms: f64,
    pub turn_tool_count: u64,
    pub turn_hook_count: u64,

    // ── Session persistence ──────────────────────────────────────────────
    pub session_manager: SessionManager,

    // ── Registries ───────────────────────────────────────────────────────
    pub hook_registry: HookRegistry,
    pub skill_registry: SkillRegistry,
    pub plugin_registry: PluginRegistry,
    pub task_manager: TaskManager,

    // ── MCP ──────────────────────────────────────────────────────────────
    pub mcp_manager: Option<McpManager>,
    pub mcp_state: McpState,

    // ── File history ─────────────────────────────────────────────────────
    pub file_history: FileHistoryState,

    // ── Notifications ────────────────────────────────────────────────────
    pub notifications: NotificationState,

    // ── Plugin state ─────────────────────────────────────────────────────
    pub plugin_state: PluginState,

    // ── UI state ─────────────────────────────────────────────────────────
    pub expanded_view: ExpandedView,
    pub status_line_text: Option<String>,
    pub spinner_tip: Option<String>,
    /// Named agent (from --agent CLI flag or settings).
    pub agent_name: Option<String>,

    // ── Team / swarm ─────────────────────────────────────────────────────
    pub team_context: Option<TeamContext>,
    pub inbox: Vec<InboxMessage>,

    // ── Bridge state ─────────────────────────────────────────────────────
    pub bridge_enabled: bool,
    pub bridge_connected: bool,
    pub bridge_session_active: bool,
    pub bridge_connect_url: Option<String>,
    pub bridge_session_url: Option<String>,

    // ── Remote mode ──────────────────────────────────────────────────────
    pub remote_session_url: Option<String>,
    pub is_remote_mode: bool,

    // ── Session flags ────────────────────────────────────────────────────
    pub session_bypass_permissions: bool,
    pub session_trust_accepted: bool,
    pub session_persistence_disabled: bool,
    pub has_exited_plan_mode: bool,

    // ── Timing ───────────────────────────────────────────────────────────
    pub start_time: Instant,
    pub last_interaction_time: Instant,

    // ── Effort ───────────────────────────────────────────────────────────
    pub effort_value: Option<String>,

    // ── Auth version (incremented on login/logout) ───────────────────────
    pub auth_version: u64,

    // ── Error log (in-memory for diagnostics) ────────────────────────────
    pub in_memory_error_log: Vec<ErrorLogEntry>,
}

/// An in-memory error log entry.
#[derive(Clone, Debug)]
pub struct ErrorLogEntry {
    pub error: String,
    pub timestamp: String,
}

impl AppState {
    /// Create a new AppState with the given session ID and paths.
    pub fn new(
        session_id: String,
        project_root: PathBuf,
        cwd: PathBuf,
        settings: Settings,
        model: String,
        session_manager: SessionManager,
    ) -> Self {
        let now = Instant::now();
        Self {
            session_id,
            parent_session_id: None,
            project_root,
            original_cwd: cwd.clone(),
            cwd,
            settings,
            model,
            model_override: None,
            verbose: false,
            permission_mode: PermissionMode::Default,
            plan_mode: false,
            vim_mode: false,
            brief_mode: false,
            assistant_mode: false,
            fast_mode: false,
            thinking_enabled: true,
            prompt_suggestion_enabled: false,
            is_non_interactive: false,
            cost_tracker: CostTracker::new(),
            model_usage: HashMap::new(),
            turn_count: 0,
            total_api_duration_ms: 0.0,
            total_api_duration_without_retries_ms: 0.0,
            total_tool_duration_ms: 0.0,
            turn_hook_duration_ms: 0.0,
            turn_tool_duration_ms: 0.0,
            turn_tool_count: 0,
            turn_hook_count: 0,
            session_manager,
            hook_registry: HookRegistry::new(),
            skill_registry: SkillRegistry::default(),
            plugin_registry: PluginRegistry::default(),
            task_manager: TaskManager::new(),
            mcp_manager: None,
            mcp_state: McpState::default(),
            file_history: FileHistoryState::default(),
            notifications: NotificationState::default(),
            plugin_state: PluginState::default(),
            expanded_view: ExpandedView::None,
            status_line_text: None,
            spinner_tip: None,
            agent_name: None,
            team_context: None,
            inbox: Vec::new(),
            bridge_enabled: false,
            bridge_connected: false,
            bridge_session_active: false,
            bridge_connect_url: None,
            bridge_session_url: None,
            remote_session_url: None,
            is_remote_mode: false,
            session_bypass_permissions: false,
            session_trust_accepted: false,
            session_persistence_disabled: false,
            has_exited_plan_mode: false,
            start_time: now,
            last_interaction_time: now,
            effort_value: None,
            auth_version: 0,
            in_memory_error_log: Vec::new(),
        }
    }

    // ── Accessors ────────────────────────────────────────────────────────

    /// Total cost in USD for this session.
    pub fn total_cost_usd(&self) -> f64 {
        self.cost_tracker.total_cost_usd()
    }

    /// Total input tokens across all models.
    pub fn total_input_tokens(&self) -> u64 {
        self.cost_tracker.total_input_tokens()
    }

    /// Total output tokens across all models.
    pub fn total_output_tokens(&self) -> u64 {
        self.cost_tracker.total_output_tokens()
    }

    /// Active model name (override takes precedence).
    pub fn active_model(&self) -> &str {
        self.model_override.as_deref().unwrap_or(&self.model)
    }

    /// Whether assistant (KAIROS) mode is active.
    pub fn is_assistant_mode(&self) -> bool {
        self.assistant_mode
    }

    /// Whether the session is in plan mode.
    pub fn is_plan_mode(&self) -> bool {
        self.plan_mode
    }

    /// Wall-clock duration since session start.
    pub fn session_duration(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }

    // ── Mutators ─────────────────────────────────────────────────────────

    /// Record the start of a new turn.
    pub fn begin_turn(&mut self) {
        self.turn_count += 1;
        self.turn_hook_duration_ms = 0.0;
        self.turn_tool_duration_ms = 0.0;
        self.turn_tool_count = 0;
        self.turn_hook_count = 0;
        self.last_interaction_time = Instant::now();
    }

    /// Update the current working directory.
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    /// Toggle plan mode.
    pub fn set_plan_mode(&mut self, enabled: bool) {
        if self.plan_mode && !enabled {
            self.has_exited_plan_mode = true;
        }
        self.plan_mode = enabled;
    }

    /// Toggle fast mode.
    pub fn set_fast_mode(&mut self, enabled: bool) {
        self.fast_mode = enabled;
    }

    /// Set the model override for this session.
    pub fn set_model_override(&mut self, model: Option<String>) {
        self.model_override = model;
    }

    /// Push a notification onto the queue.
    pub fn push_notification(&mut self, notification: Notification) {
        if self.notifications.current.is_none() {
            self.notifications.current = Some(notification);
        } else {
            self.notifications.queue.push(notification);
        }
    }

    /// Dismiss the current notification and advance the queue.
    pub fn dismiss_notification(&mut self) -> Option<Notification> {
        let dismissed = self.notifications.current.take();
        self.notifications.current = if self.notifications.queue.is_empty() {
            None
        } else {
            Some(self.notifications.queue.remove(0))
        };
        dismissed
    }

    /// Log an error for in-memory diagnostics.
    pub fn log_error(&mut self, error: String) {
        let timestamp = chrono::Utc::now().to_rfc3339();
        self.in_memory_error_log.push(ErrorLogEntry { error, timestamp });
        // Keep bounded
        if self.in_memory_error_log.len() > 100 {
            self.in_memory_error_log.remove(0);
        }
    }

    /// Increment auth version (triggers re-fetch of auth-dependent data).
    pub fn bump_auth_version(&mut self) {
        self.auth_version += 1;
    }
}

// ── AppStateStore ───────────────────────────────────────────────────────────

/// Thread-safe wrapper around `AppState`.
///
/// Provides `read()` and `write()` accessors. The `RwLock` allows concurrent
/// readers (e.g. TUI rendering) while serializing writes (e.g. cost updates).
#[derive(Clone)]
pub struct AppStateStore {
    inner: Arc<RwLock<AppState>>,
}

impl AppStateStore {
    /// Wrap an `AppState` in a thread-safe store.
    pub fn new(state: AppState) -> Self {
        Self {
            inner: Arc::new(RwLock::new(state)),
        }
    }

    /// Acquire a read lock on the state.
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, AppState> {
        self.inner.read().expect("AppState RwLock poisoned (read)")
    }

    /// Acquire a write lock on the state.
    pub fn write(&self) -> std::sync::RwLockWriteGuard<'_, AppState> {
        self.inner.write().expect("AppState RwLock poisoned (write)")
    }

    /// Try to acquire a read lock without blocking.
    pub fn try_read(&self) -> Option<std::sync::RwLockReadGuard<'_, AppState>> {
        self.inner.try_read().ok()
    }

    /// Try to acquire a write lock without blocking.
    pub fn try_write(&self) -> Option<std::sync::RwLockWriteGuard<'_, AppState>> {
        self.inner.try_write().ok()
    }

    // ── Convenience accessors ────────────────────────────────────────────

    /// Get a clone of the session ID.
    pub fn session_id(&self) -> String {
        self.read().session_id.clone()
    }

    /// Get current total cost in USD.
    pub fn total_cost_usd(&self) -> f64 {
        self.read().total_cost_usd()
    }

    /// Get current turn count.
    pub fn turn_count(&self) -> u64 {
        self.read().turn_count
    }

    /// Get the active model name.
    pub fn active_model(&self) -> String {
        self.read().active_model().to_string()
    }

    /// Whether the session is in plan mode.
    pub fn is_plan_mode(&self) -> bool {
        self.read().plan_mode
    }

    /// Whether assistant mode is active.
    pub fn is_assistant_mode(&self) -> bool {
        self.read().assistant_mode
    }
}
