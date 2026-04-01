use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use crate::types::message::Message;

// ── Configuration ───────────────────────────────────────────────────────────

/// Thresholds that govern when session memory extraction fires.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMemoryConfig {
    /// Minimum context-window tokens before first extraction.
    pub minimum_message_tokens_to_init: usize,
    /// Minimum context-window growth (tokens) between updates.
    pub minimum_tokens_between_update: usize,
    /// Number of tool calls between updates.
    pub tool_calls_between_updates: usize,
}

impl Default for SessionMemoryConfig {
    fn default() -> Self {
        Self {
            minimum_message_tokens_to_init: 10_000,
            minimum_tokens_between_update: 5_000,
            tool_calls_between_updates: 3,
        }
    }
}

// ── Memory categories ───────────────────────────────────────────────────────

/// Categories of information captured in session memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryCategory {
    CodePattern,
    UserPreference,
    ProjectFact,
    Decision,
}

/// A single extracted memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memory {
    pub content: String,
    pub category: MemoryCategory,
    pub timestamp: DateTime<Utc>,
}

// ── State ───────────────────────────────────────────────────────────────────

/// Tracks the mutable runtime state of session memory.
pub struct SessionMemoryState {
    config: Mutex<SessionMemoryConfig>,
    initialized: AtomicBool,
    tokens_at_last_extraction: AtomicU64,
    extraction_in_progress: AtomicBool,
    extraction_started_at: Mutex<Option<Instant>>,
    last_summarized_message_id: Mutex<Option<Uuid>>,
    last_memory_message_uuid: Mutex<Option<Uuid>>,
}

impl SessionMemoryState {
    pub fn new() -> Self {
        Self {
            config: Mutex::new(SessionMemoryConfig::default()),
            initialized: AtomicBool::new(false),
            tokens_at_last_extraction: AtomicU64::new(0),
            extraction_in_progress: AtomicBool::new(false),
            extraction_started_at: Mutex::new(None),
            last_summarized_message_id: Mutex::new(None),
            last_memory_message_uuid: Mutex::new(None),
        }
    }

    pub fn config(&self) -> SessionMemoryConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn set_config(&self, config: SessionMemoryConfig) {
        *self.config.lock().unwrap() = config;
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Relaxed)
    }

    pub fn mark_initialized(&self) {
        self.initialized.store(true, Ordering::Relaxed);
    }

    pub fn mark_extraction_started(&self) {
        self.extraction_in_progress.store(true, Ordering::Relaxed);
        *self.extraction_started_at.lock().unwrap() = Some(Instant::now());
    }

    pub fn mark_extraction_completed(&self) {
        self.extraction_in_progress.store(false, Ordering::Relaxed);
        *self.extraction_started_at.lock().unwrap() = None;
    }

    pub fn record_extraction_token_count(&self, count: u64) {
        self.tokens_at_last_extraction.store(count, Ordering::Relaxed);
    }

    pub fn last_summarized_message_id(&self) -> Option<Uuid> {
        *self.last_summarized_message_id.lock().unwrap()
    }

    pub fn set_last_summarized_message_id(&self, id: Option<Uuid>) {
        *self.last_summarized_message_id.lock().unwrap() = id;
    }

    /// Check if the initialization threshold has been met.
    pub fn has_met_init_threshold(&self, current_tokens: u64) -> bool {
        let config = self.config.lock().unwrap();
        current_tokens >= config.minimum_message_tokens_to_init as u64
    }

    /// Check if the update threshold has been met (context growth since last
    /// extraction).
    pub fn has_met_update_threshold(&self, current_tokens: u64) -> bool {
        let config = self.config.lock().unwrap();
        let last = self.tokens_at_last_extraction.load(Ordering::Relaxed);
        current_tokens.saturating_sub(last) >= config.minimum_tokens_between_update as u64
    }

    /// Wait for an in-progress extraction to finish (with timeout).
    pub async fn wait_for_extraction(&self) {
        const WAIT_TIMEOUT: Duration = Duration::from_secs(15);
        const STALE_THRESHOLD: Duration = Duration::from_secs(60);

        let start = Instant::now();
        while self.extraction_in_progress.load(Ordering::Relaxed) {
            let started_at = *self.extraction_started_at.lock().unwrap();
            if let Some(s) = started_at {
                if s.elapsed() > STALE_THRESHOLD {
                    debug!("extraction is stale, not waiting");
                    return;
                }
            }
            if start.elapsed() > WAIT_TIMEOUT {
                debug!("timed out waiting for extraction");
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Reset all state (useful for tests).
    pub fn reset(&self) {
        *self.config.lock().unwrap() = SessionMemoryConfig::default();
        self.initialized.store(false, Ordering::Relaxed);
        self.tokens_at_last_extraction.store(0, Ordering::Relaxed);
        self.extraction_in_progress.store(false, Ordering::Relaxed);
        *self.extraction_started_at.lock().unwrap() = None;
        *self.last_summarized_message_id.lock().unwrap() = None;
        *self.last_memory_message_uuid.lock().unwrap() = None;
    }
}

impl Default for SessionMemoryState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Threshold logic ─────────────────────────────────────────────────────────

/// Count tool calls in messages since the given UUID (exclusive).
/// If `since_uuid` is `None`, counts all tool calls.
fn count_tool_calls_since(messages: &[Message], since_uuid: Option<Uuid>) -> usize {
    let mut found_start = since_uuid.is_none();
    let mut count = 0;

    for msg in messages {
        if !found_start {
            if let Message::User(u) = msg {
                if u.uuid == since_uuid.unwrap() {
                    found_start = true;
                }
            } else if let Message::Assistant(a) = msg {
                if a.uuid == since_uuid.unwrap() {
                    found_start = true;
                }
            }
            continue;
        }

        if let Message::Assistant(a) = msg {
            count += a
                .message
                .content
                .iter()
                .filter(|b| matches!(b, crate::types::content::ContentBlock::ToolUse { .. }))
                .count();
        }
    }

    count
}

/// Check whether last assistant turn contains any tool calls.
fn has_tool_calls_in_last_assistant_turn(messages: &[Message]) -> bool {
    for msg in messages.iter().rev() {
        match msg {
            Message::Assistant(a) => {
                return a
                    .message
                    .content
                    .iter()
                    .any(|b| matches!(b, crate::types::content::ContentBlock::ToolUse { .. }));
            }
            Message::User(_) => return false,
            _ => continue,
        }
    }
    false
}

/// Determine whether session memory extraction should run now.
pub fn should_extract_memory(
    messages: &[Message],
    current_token_count: u64,
    state: &SessionMemoryState,
) -> bool {
    // Check initialization threshold
    if !state.is_initialized() {
        if !state.has_met_init_threshold(current_token_count) {
            return false;
        }
        state.mark_initialized();
    }

    let has_met_token_threshold = state.has_met_update_threshold(current_token_count);

    let last_uuid = *state.last_memory_message_uuid.lock().unwrap();
    let tool_calls_since = count_tool_calls_since(messages, last_uuid);
    let has_met_tool_call_threshold =
        tool_calls_since >= state.config().tool_calls_between_updates;

    let has_tool_calls_in_last_turn = has_tool_calls_in_last_assistant_turn(messages);

    let should_extract =
        has_met_token_threshold && (has_met_tool_call_threshold || !has_tool_calls_in_last_turn);

    if should_extract {
        if let Some(last) = messages.last() {
            let uuid = match last {
                Message::User(u) => u.uuid,
                Message::Assistant(a) => a.uuid,
                Message::System(_) => return true,
            };
            *state.last_memory_message_uuid.lock().unwrap() = Some(uuid);
        }
    }

    should_extract
}

// ── File I/O ────────────────────────────────────────────────────────────────

/// Default template written when the session memory file is first created.
const SESSION_MEMORY_TEMPLATE: &str = "# Session Memory\n\n\
## Key Decisions\n\n\
## Code Patterns\n\n\
## User Preferences\n\n\
## Project Facts\n";

/// Get the directory where session memory files are stored.
pub fn session_memory_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-code")
        .join("session-memory")
}

/// Get the session memory file path for a given session ID.
pub fn session_memory_path(session_id: &str) -> PathBuf {
    session_memory_dir().join(format!("{session_id}.md"))
}

/// Persist extracted memories to disk for the given session.
pub async fn persist_memories(session_id: &str, memories: &[Memory]) -> Result<()> {
    let dir = session_memory_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .context("failed to create session memory directory")?;

    let path = session_memory_path(session_id);

    let mut content = if path.exists() {
        tokio::fs::read_to_string(&path).await.unwrap_or_default()
    } else {
        SESSION_MEMORY_TEMPLATE.to_string()
    };

    for memory in memories {
        let section = match memory.category {
            MemoryCategory::CodePattern => "## Code Patterns",
            MemoryCategory::UserPreference => "## User Preferences",
            MemoryCategory::ProjectFact => "## Project Facts",
            MemoryCategory::Decision => "## Key Decisions",
        };

        // Append under the matching section
        if let Some(pos) = content.find(section) {
            let insert_at = pos + section.len();
            let entry = format!("\n- {}", memory.content);
            content.insert_str(insert_at, &entry);
        } else {
            // Section not found — append at the end
            content.push_str(&format!("\n{}\n- {}\n", section, memory.content));
        }
    }

    tokio::fs::write(&path, &content)
        .await
        .context("failed to write session memory")?;

    info!(
        session_id,
        memory_count = memories.len(),
        "persisted session memories"
    );
    Ok(())
}

/// Load session memories from disk.
pub async fn load_session_memories(session_id: &str) -> Result<Option<String>> {
    let path = session_memory_path(session_id);
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("failed to read session memory"),
    }
}

/// Set up the session memory file, creating directory and template if needed.
pub async fn setup_session_memory_file(session_id: &str) -> Result<(PathBuf, String)> {
    let dir = session_memory_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .context("creating session memory dir")?;

    // Set restrictive permissions on the directory (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(&dir, perms).ok();
    }

    let path = session_memory_path(session_id);

    if !path.exists() {
        tokio::fs::write(&path, SESSION_MEMORY_TEMPLATE)
            .await
            .context("writing session memory template")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perms).ok();
        }
    }

    let content = tokio::fs::read_to_string(&path)
        .await
        .context("reading session memory")?;

    Ok((path, content))
}
