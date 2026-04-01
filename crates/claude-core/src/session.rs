//! Session persistence, prompt history, and session management.
//!
//! Sessions are stored as JSONL files at project-scoped paths:
//! `~/.claude/projects/{sanitized_cwd}/{session_id}.jsonl`
//!
//! Each line in the JSONL file is a timestamped entry containing either a
//! message, metadata update, session event, file history snapshot, content
//! replacement, or context-collapse commit. This append-only format
//! matches the original TypeScript implementation.
//!
//! # Entry types
//!
//! - `message` — user/assistant/attachment/system transcript messages
//! - `metadata` — session metadata (model, project_root, cost, usage)
//! - `summary` — compacted conversation summaries
//! - `custom-title` — user-set session title
//! - `ai-title` — auto-generated session title
//! - `last-prompt` — last user prompt (for session listing)
//! - `tag` — session tag/label
//! - `file-history-snapshot` — file state before/after tool execution
//! - `content-replacement` — verbose tool results replaced with summaries
//! - `context-collapse-commit` — context collapse commit entry
//! - `context-collapse-snapshot` — context collapse staged queue snapshot
//!
//! History is a separate append-only JSONL file at `~/.claude/history.jsonl`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read as _, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::usage::Usage;

// ── Session types ────────────────────────────────────────────────────────────

/// Cumulative token usage across a session's lifetime.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CumulativeUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl CumulativeUsage {
    /// Accumulate a single API response's usage into the running total.
    pub fn add(&mut self, usage: &Usage) {
        self.input_tokens += usage.input_tokens;
        self.output_tokens += usage.output_tokens;
        self.cache_creation_input_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
        self.cache_read_input_tokens += usage.cache_read_input_tokens.unwrap_or(0);
    }
}

// ── Entry types ─────────────────────────────────────────────────────────────

/// A single entry in the session JSONL transcript.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionEntry {
    /// Entry type: "message", "metadata", "summary", etc.
    #[serde(rename = "type")]
    pub entry_type: String,
    /// Timestamp of this entry.
    pub timestamp: DateTime<Utc>,
    /// The data payload (message JSON, metadata fields, etc.).
    #[serde(flatten)]
    pub data: serde_json::Value,
}

/// Transcript message types that participate in the conversation chain.
pub fn is_transcript_message(entry_type: &str) -> bool {
    matches!(entry_type, "user" | "assistant" | "attachment" | "system" | "message")
}

/// Whether a message type participates in the parentUuid chain.
/// Progress messages are ephemeral and do not participate.
pub fn is_chain_participant(entry_type: &str) -> bool {
    entry_type != "progress"
}

// ── Message grouping ────────────────────────────────────────────────────────

/// A group of related messages that should not be split during operations.
///
/// Groups tool_use + tool_result pairs so they are never separated during
/// compaction, truncation, or display operations.
#[derive(Clone, Debug)]
pub struct MessageGroup {
    /// The messages in this group.
    pub messages: Vec<serde_json::Value>,
    /// Index of the first message in the original array.
    pub start_index: usize,
    /// Whether this group contains a tool_use/tool_result pair.
    pub is_tool_pair: bool,
}

/// Group messages so tool_use + tool_result pairs are never split.
///
/// The TypeScript original ensures that tool_use blocks (from assistant messages)
/// are always paired with their corresponding tool_result (in the next user message).
/// This function groups adjacent messages into atomic units.
pub fn group_messages(messages: &[serde_json::Value]) -> Vec<MessageGroup> {
    let mut groups: Vec<MessageGroup> = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        let msg = &messages[i];
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");

        // Check if this assistant message contains tool_use blocks
        if role == "assistant" && message_has_tool_use(msg) {
            // Look ahead for the matching tool_result in the next user message
            if i + 1 < messages.len() {
                let next = &messages[i + 1];
                let next_role = next.get("role").and_then(|v| v.as_str()).unwrap_or("");
                if next_role == "user" && message_has_tool_result(next) {
                    groups.push(MessageGroup {
                        messages: vec![msg.clone(), next.clone()],
                        start_index: i,
                        is_tool_pair: true,
                    });
                    i += 2;
                    continue;
                }
            }
        }

        groups.push(MessageGroup {
            messages: vec![msg.clone()],
            start_index: i,
            is_tool_pair: false,
        });
        i += 1;
    }

    groups
}

/// Check if a message contains tool_use content blocks.
fn message_has_tool_use(msg: &serde_json::Value) -> bool {
    content_blocks(msg).any(|block| {
        block.get("type").and_then(|v| v.as_str()) == Some("tool_use")
    })
}

/// Check if a message contains tool_result content blocks.
fn message_has_tool_result(msg: &serde_json::Value) -> bool {
    content_blocks(msg).any(|block| {
        block.get("type").and_then(|v| v.as_str()) == Some("tool_result")
    })
}

/// Iterate over content blocks in a message.
fn content_blocks(msg: &serde_json::Value) -> impl Iterator<Item = &serde_json::Value> {
    msg.get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
}

// ── File history snapshots ──────────────────────────────────────────────────

/// A snapshot of file state before/after a tool execution.
///
/// Tracks file content at specific points in time so that undo operations
/// can restore previous state. Each snapshot is associated with a message
/// UUID (the tool_use message that triggered the change).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileHistorySnapshot {
    /// Map of file path to file state at this snapshot point.
    pub files: HashMap<String, FileState>,
}

/// The state of a single file at a point in time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileState {
    /// File content, or None if the file did not exist.
    pub content: Option<String>,
    /// File modification time (milliseconds since epoch).
    #[serde(default)]
    pub mtime_ms: Option<u64>,
    /// Whether this state represents the file before or after the tool execution.
    pub phase: FileStatePhase,
}

/// Whether a file state is before or after a tool execution.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileStatePhase {
    Before,
    After,
}

/// A file history snapshot entry in the session transcript.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileHistorySnapshotEntry {
    /// The UUID of the message (tool_use) this snapshot is associated with.
    pub message_id: String,
    /// The file state snapshot.
    pub snapshot: FileHistorySnapshot,
    /// Whether this is an update to a previous snapshot (vs initial).
    #[serde(default)]
    pub is_snapshot_update: bool,
}

// ── Content replacement ─────────────────────────────────────────────────────

/// A record for replacing verbose tool results with summaries.
///
/// During compaction, large tool_result content is replaced with a compact
/// summary to save context window space. The original content is preserved
/// as a content-replacement entry in the JSONL so it can be restored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentReplacementRecord {
    /// UUID of the message containing the tool_result.
    pub message_uuid: String,
    /// Index of the content block within the message.
    pub content_block_index: usize,
    /// The replacement summary text.
    pub replacement_text: String,
    /// Hash of the original content (for verification).
    #[serde(default)]
    pub original_hash: Option<String>,
}

/// A content-replacement entry in the session JSONL.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentReplacementEntry {
    pub session_id: String,
    pub replacements: Vec<ContentReplacementRecord>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

// ── Context collapse ────────────────────────────────────────────────────────

/// A context-collapse commit entry in the transcript.
///
/// One entry per commit, in commit order. On resume these are collected
/// into an ordered array to rebuild the commit log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextCollapseCommit {
    pub collapse_id: String,
    pub summary_uuid: String,
    pub summary_content: String,
    pub summary: String,
    pub first_archived_uuid: String,
    pub last_archived_uuid: String,
}

/// A context-collapse snapshot entry in the transcript.
///
/// Snapshots the staged queue and spawn state. Written after each
/// context-agent spawn resolves. Last-wins on restore.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextCollapseSnapshot {
    pub staged: Vec<StagedRange>,
    pub armed: bool,
    pub last_spawn_tokens: u64,
}

/// A staged range in a context-collapse snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StagedRange {
    pub start_uuid: String,
    pub end_uuid: String,
    pub summary: String,
    pub risk: f64,
    pub staged_at: u64,
}

// ── Session ─────────────────────────────────────────────────────────────────

/// A persisted conversation session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub messages: Vec<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub total_cost: f64,
    #[serde(default)]
    pub cumulative_usage: CumulativeUsage,
    /// Custom user-assigned title for the session.
    #[serde(default)]
    pub custom_title: Option<String>,
    /// AI-generated title for the session.
    #[serde(default)]
    pub ai_title: Option<String>,
    /// User-assigned tag/label.
    #[serde(default)]
    pub tag: Option<String>,
    /// Last user prompt (for display in session lists).
    #[serde(default)]
    pub last_prompt: Option<String>,
    /// File history snapshots keyed by message UUID.
    #[serde(default)]
    pub file_history: HashMap<String, FileHistorySnapshotEntry>,
    /// Content replacements keyed by session ID.
    #[serde(default)]
    pub content_replacements: Vec<ContentReplacementEntry>,
    /// Context collapse commits in order.
    #[serde(default)]
    pub context_collapse_commits: Vec<ContextCollapseCommit>,
    /// Latest context collapse snapshot.
    #[serde(default)]
    pub context_collapse_snapshot: Option<ContextCollapseSnapshot>,
    /// UUIDs of all messages in this session (for deduplication).
    #[serde(skip)]
    pub message_uuids: HashSet<String>,
    /// Git branch at last write.
    #[serde(default)]
    pub git_branch: Option<String>,
}

/// Lightweight summary for listing sessions without loading messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub project_root: Option<String>,
    pub model: Option<String>,
    pub message_count: usize,
    pub first_prompt: Option<String>,
    /// Custom user-assigned title (from `custom-title` entries).
    #[serde(default)]
    pub custom_title: Option<String>,
    /// AI-generated title (from `ai-title` entries).
    #[serde(default)]
    pub ai_title: Option<String>,
    /// Session tag/label.
    #[serde(default)]
    pub tag: Option<String>,
    /// Last user prompt (from `last-prompt` entries).
    #[serde(default)]
    pub last_prompt: Option<String>,
}

impl SessionSummary {
    /// Return the best available display title for this session.
    pub fn display_title(&self) -> Option<&str> {
        self.custom_title
            .as_deref()
            .or(self.ai_title.as_deref())
            .or(self.last_prompt.as_deref())
            .or(self.first_prompt.as_deref())
    }
}

// ── Session manager ──────────────────────────────────────────────────────────

/// Size of the tail-read buffer for partial transcript reads (64 KB).
const LITE_READ_BUF_SIZE: usize = 64 * 1024;

/// Maximum transcript file size for full reads (50 MB).
const MAX_TRANSCRIPT_READ_BYTES: u64 = 50 * 1024 * 1024;

/// Manages session files on disk using project-scoped JSONL format.
///
/// Session transcripts are stored at:
/// `~/.claude/projects/{sanitized_cwd}/{session_id}.jsonl`
pub struct SessionManager {
    /// The project directory (already sanitized).
    project_dir: PathBuf,
    /// Currently active session ID for this manager.
    active_session_id: Option<String>,
}

impl SessionManager {
    /// Create a manager for the given project directory.
    ///
    /// The project directory is typically `~/.claude/projects/{sanitized_cwd}`.
    pub fn new(project_dir: PathBuf) -> Self {
        Self {
            project_dir,
            active_session_id: None,
        }
    }

    /// Compute the project-scoped session directory for a given working directory.
    ///
    /// Path: `~/.claude-omni/projects/{sanitized_cwd}`
    pub fn project_dir_for_cwd(cwd: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let sanitized = sanitize_path(cwd);
        home.join(crate::config::paths::OMNI_DIR_NAME).join("projects").join(sanitized)
    }

    /// Default sessions directory (legacy): `~/.claude-omni/sessions`.
    pub fn default_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(crate::config::paths::OMNI_DIR_NAME)
            .join("sessions")
    }

    /// Get the JSONL transcript path for a session.
    pub fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.project_dir.join(format!("{session_id}.jsonl"))
    }

    /// Get the active session ID, if set.
    pub fn active_session_id(&self) -> Option<&str> {
        self.active_session_id.as_deref()
    }

    /// Switch the active session. Returns the previous session ID.
    pub fn switch_session(&mut self, session_id: String) -> Option<String> {
        self.active_session_id.replace(session_id)
    }

    /// Create a new session with a fresh UUID.
    pub fn create_session(&self) -> Result<Session> {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4().to_string(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            project_root: None,
            model: None,
            total_cost: 0.0,
            cumulative_usage: CumulativeUsage::default(),
            custom_title: None,
            ai_title: None,
            tag: None,
            last_prompt: None,
            file_history: HashMap::new(),
            content_replacements: Vec::new(),
            context_collapse_commits: Vec::new(),
            context_collapse_snapshot: None,
            message_uuids: HashSet::new(),
            git_branch: None,
        };
        // Write initial metadata entry
        self.append_entry(
            &session.id,
            &SessionEntry {
                entry_type: "metadata".to_string(),
                timestamp: now,
                data: serde_json::json!({
                    "session_id": session.id,
                    "created_at": session.created_at,
                }),
            },
        )?;
        Ok(session)
    }

    /// Persist a session to disk by writing all messages as JSONL entries.
    ///
    /// This overwrites the transcript file with the full session state.
    /// For incremental appends, use `append_message` instead.
    pub fn save_session(&self, session: &Session) -> Result<()> {
        fs::create_dir_all(&self.project_dir)
            .with_context(|| format!("create project dir: {}", self.project_dir.display()))?;

        let path = self.transcript_path(&session.id);
        let mut file = fs::File::create(&path)
            .with_context(|| format!("create session file: {}", path.display()))?;

        // Write metadata entry first
        let meta = SessionEntry {
            entry_type: "metadata".to_string(),
            timestamp: session.updated_at,
            data: serde_json::json!({
                "session_id": session.id,
                "created_at": session.created_at,
                "model": session.model,
                "project_root": session.project_root,
                "total_cost": session.total_cost,
                "cumulative_usage": session.cumulative_usage,
            }),
        };
        writeln!(file, "{}", serde_json::to_string(&meta)?)?;

        // Write each message as a JSONL entry
        for msg in &session.messages {
            let entry = SessionEntry {
                entry_type: "message".to_string(),
                timestamp: session.updated_at,
                data: msg.clone(),
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }

        // Write file history snapshots
        for (msg_id, snapshot) in &session.file_history {
            let entry = SessionEntry {
                entry_type: "file-history-snapshot".to_string(),
                timestamp: session.updated_at,
                data: serde_json::json!({
                    "messageId": msg_id,
                    "snapshot": snapshot.snapshot,
                    "isSnapshotUpdate": snapshot.is_snapshot_update,
                }),
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }

        // Write content replacements
        for replacement in &session.content_replacements {
            let entry = SessionEntry {
                entry_type: "content-replacement".to_string(),
                timestamp: session.updated_at,
                data: serde_json::to_value(replacement)?,
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }

        // Write context collapse commits
        for commit in &session.context_collapse_commits {
            let entry = SessionEntry {
                entry_type: "context-collapse-commit".to_string(),
                timestamp: session.updated_at,
                data: serde_json::to_value(commit)?,
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }

        // Write context collapse snapshot (last one wins)
        if let Some(snapshot) = &session.context_collapse_snapshot {
            let entry = SessionEntry {
                entry_type: "context-collapse-snapshot".to_string(),
                timestamp: session.updated_at,
                data: serde_json::to_value(snapshot)?,
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }

        // Re-append session metadata at the end for tail reads
        self.write_session_metadata_entries(&mut file, session)?;

        Ok(())
    }

    /// Write session metadata entries (title, tag, etc.) to a file.
    fn write_session_metadata_entries(
        &self,
        file: &mut fs::File,
        session: &Session,
    ) -> Result<()> {
        let now = session.updated_at;
        if let Some(title) = &session.custom_title {
            let entry = SessionEntry {
                entry_type: "custom-title".to_string(),
                timestamp: now,
                data: serde_json::json!({
                    "customTitle": title,
                    "sessionId": session.id,
                }),
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }
        if let Some(title) = &session.ai_title {
            let entry = SessionEntry {
                entry_type: "ai-title".to_string(),
                timestamp: now,
                data: serde_json::json!({
                    "aiTitle": title,
                    "sessionId": session.id,
                }),
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }
        if let Some(tag) = &session.tag {
            let entry = SessionEntry {
                entry_type: "tag".to_string(),
                timestamp: now,
                data: serde_json::json!({
                    "tag": tag,
                    "sessionId": session.id,
                }),
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }
        if let Some(prompt) = &session.last_prompt {
            let entry = SessionEntry {
                entry_type: "last-prompt".to_string(),
                timestamp: now,
                data: serde_json::json!({
                    "lastPrompt": prompt,
                    "sessionId": session.id,
                }),
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }
        Ok(())
    }

    /// Append a single entry to the session transcript.
    pub fn append_entry(&self, session_id: &str, entry: &SessionEntry) -> Result<()> {
        fs::create_dir_all(&self.project_dir)
            .with_context(|| format!("create project dir: {}", self.project_dir.display()))?;

        let path = self.transcript_path(session_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open session file: {}", path.display()))?;

        let line = serde_json::to_string(entry)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// Append a message to the session transcript.
    pub fn append_message(&self, session_id: &str, message: &serde_json::Value) -> Result<()> {
        self.append_entry(
            session_id,
            &SessionEntry {
                entry_type: "message".to_string(),
                timestamp: Utc::now(),
                data: message.clone(),
            },
        )
    }

    /// Append a file history snapshot entry.
    pub fn append_file_history_snapshot(
        &self,
        session_id: &str,
        message_id: &str,
        snapshot: &FileHistorySnapshot,
        is_update: bool,
    ) -> Result<()> {
        self.append_entry(
            session_id,
            &SessionEntry {
                entry_type: "file-history-snapshot".to_string(),
                timestamp: Utc::now(),
                data: serde_json::json!({
                    "messageId": message_id,
                    "snapshot": snapshot,
                    "isSnapshotUpdate": is_update,
                }),
            },
        )
    }

    /// Append a content-replacement entry.
    pub fn append_content_replacement(
        &self,
        session_id: &str,
        replacement: &ContentReplacementEntry,
    ) -> Result<()> {
        self.append_entry(
            session_id,
            &SessionEntry {
                entry_type: "content-replacement".to_string(),
                timestamp: Utc::now(),
                data: serde_json::to_value(replacement)?,
            },
        )
    }

    /// Append a context-collapse commit entry.
    pub fn append_context_collapse_commit(
        &self,
        session_id: &str,
        commit: &ContextCollapseCommit,
    ) -> Result<()> {
        self.append_entry(
            session_id,
            &SessionEntry {
                entry_type: "context-collapse-commit".to_string(),
                timestamp: Utc::now(),
                data: serde_json::to_value(commit)?,
            },
        )
    }

    /// Append a context-collapse snapshot entry.
    pub fn append_context_collapse_snapshot(
        &self,
        session_id: &str,
        snapshot: &ContextCollapseSnapshot,
    ) -> Result<()> {
        self.append_entry(
            session_id,
            &SessionEntry {
                entry_type: "context-collapse-snapshot".to_string(),
                timestamp: Utc::now(),
                data: serde_json::to_value(snapshot)?,
            },
        )
    }

    /// Append a custom title entry.
    pub fn set_custom_title(&self, session_id: &str, title: &str) -> Result<()> {
        self.append_entry(
            session_id,
            &SessionEntry {
                entry_type: "custom-title".to_string(),
                timestamp: Utc::now(),
                data: serde_json::json!({
                    "customTitle": title,
                    "sessionId": session_id,
                }),
            },
        )
    }

    /// Append a tag entry.
    pub fn set_tag(&self, session_id: &str, tag: &str) -> Result<()> {
        self.append_entry(
            session_id,
            &SessionEntry {
                entry_type: "tag".to_string(),
                timestamp: Utc::now(),
                data: serde_json::json!({
                    "tag": tag,
                    "sessionId": session_id,
                }),
            },
        )
    }

    /// Load a session by ID from its JSONL transcript.
    pub fn load_session(&self, id: &str) -> Result<Session> {
        let path = self.transcript_path(id);

        // Try JSONL format first
        if path.exists() {
            return self.load_session_jsonl(id, &path);
        }

        // Fall back to legacy JSON format in default_dir
        let legacy_path = Self::default_dir().join(format!("{id}.json"));
        if legacy_path.exists() {
            let data = fs::read_to_string(&legacy_path)
                .with_context(|| format!("read session: {}", legacy_path.display()))?;
            let session: Session = serde_json::from_str(&data)?;
            return Ok(session);
        }

        anyhow::bail!("session not found: {id}")
    }

    /// Load a session from its JSONL transcript file.
    fn load_session_jsonl(&self, id: &str, path: &PathBuf) -> Result<Session> {
        let file_size = fs::metadata(path)
            .with_context(|| format!("stat session: {}", path.display()))?
            .len();

        if file_size > MAX_TRANSCRIPT_READ_BYTES {
            anyhow::bail!(
                "session file too large ({} bytes, limit {})",
                file_size,
                MAX_TRANSCRIPT_READ_BYTES
            );
        }

        let data = fs::read_to_string(path)
            .with_context(|| format!("read session: {}", path.display()))?;

        self.parse_session_entries(id, &data)
    }

    /// Parse session entries from JSONL content.
    fn parse_session_entries(&self, id: &str, data: &str) -> Result<Session> {
        let mut messages = Vec::new();
        let mut created_at = None;
        let mut updated_at = Utc::now();
        let mut project_root = None;
        let mut model = None;
        let mut total_cost = 0.0;
        let mut cumulative_usage = CumulativeUsage::default();
        let mut custom_title = None;
        let mut ai_title = None;
        let mut tag = None;
        let mut last_prompt = None;
        let mut file_history = HashMap::new();
        let mut content_replacements = Vec::new();
        let mut context_collapse_commits = Vec::new();
        let mut context_collapse_snapshot = None;
        let mut message_uuids = HashSet::new();
        let mut git_branch = None;

        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<SessionEntry>(line) {
                updated_at = entry.timestamp;
                match entry.entry_type.as_str() {
                    "metadata" => {
                        if let Some(ts) = entry.data.get("created_at").and_then(|v| v.as_str()) {
                            created_at = Some(ts.parse::<DateTime<Utc>>().unwrap_or(updated_at));
                        }
                        if let Some(m) = entry.data.get("model").and_then(|v| v.as_str()) {
                            model = Some(m.to_string());
                        }
                        if let Some(p) = entry.data.get("project_root").and_then(|v| v.as_str()) {
                            project_root = Some(p.to_string());
                        }
                        if let Some(c) = entry.data.get("total_cost").and_then(|v| v.as_f64()) {
                            total_cost = c;
                        }
                        if let Some(cu) = entry.data.get("cumulative_usage") {
                            if let Ok(parsed) = serde_json::from_value(cu.clone()) {
                                cumulative_usage = parsed;
                            }
                        }
                    }
                    "message" | "user" | "assistant" | "attachment" | "system" => {
                        if let Some(uuid) = entry.data.get("uuid").and_then(|v| v.as_str()) {
                            message_uuids.insert(uuid.to_string());
                        }
                        if let Some(branch) =
                            entry.data.get("gitBranch").and_then(|v| v.as_str())
                        {
                            git_branch = Some(branch.to_string());
                        }
                        messages.push(entry.data);
                    }
                    "custom-title" => {
                        if let Some(t) = entry.data.get("customTitle").and_then(|v| v.as_str()) {
                            custom_title = Some(t.to_string());
                        }
                    }
                    "ai-title" => {
                        if let Some(t) = entry.data.get("aiTitle").and_then(|v| v.as_str()) {
                            ai_title = Some(t.to_string());
                        }
                    }
                    "tag" => {
                        if let Some(t) = entry.data.get("tag").and_then(|v| v.as_str()) {
                            tag = Some(t.to_string());
                        }
                    }
                    "last-prompt" => {
                        if let Some(p) = entry.data.get("lastPrompt").and_then(|v| v.as_str()) {
                            last_prompt = Some(p.to_string());
                        }
                    }
                    "file-history-snapshot" => {
                        if let Some(msg_id) =
                            entry.data.get("messageId").and_then(|v| v.as_str())
                        {
                            if let Ok(snapshot) = serde_json::from_value::<FileHistorySnapshot>(
                                entry
                                    .data
                                    .get("snapshot")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null),
                            ) {
                                let is_update = entry
                                    .data
                                    .get("isSnapshotUpdate")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                file_history.insert(
                                    msg_id.to_string(),
                                    FileHistorySnapshotEntry {
                                        message_id: msg_id.to_string(),
                                        snapshot,
                                        is_snapshot_update: is_update,
                                    },
                                );
                            }
                        }
                    }
                    "content-replacement" => {
                        if let Ok(cr) =
                            serde_json::from_value::<ContentReplacementEntry>(entry.data)
                        {
                            content_replacements.push(cr);
                        }
                    }
                    "context-collapse-commit" | "marble-origami-commit" => {
                        if let Ok(cc) =
                            serde_json::from_value::<ContextCollapseCommit>(entry.data)
                        {
                            context_collapse_commits.push(cc);
                        }
                    }
                    "context-collapse-snapshot" | "marble-origami-snapshot" => {
                        if let Ok(cs) =
                            serde_json::from_value::<ContextCollapseSnapshot>(entry.data)
                        {
                            context_collapse_snapshot = Some(cs);
                        }
                    }
                    _ => {} // ignore unknown entry types
                }
            }
        }

        Ok(Session {
            id: id.to_string(),
            messages,
            created_at: created_at.unwrap_or(updated_at),
            updated_at,
            project_root,
            model,
            total_cost,
            cumulative_usage,
            custom_title,
            ai_title,
            tag,
            last_prompt,
            file_history,
            content_replacements,
            context_collapse_commits,
            context_collapse_snapshot,
            message_uuids,
            git_branch,
        })
    }

    /// Read the tail of a session file for lightweight metadata extraction.
    ///
    /// This reads only the last `LITE_READ_BUF_SIZE` bytes of the file,
    /// avoiding full-file reads for large sessions. Used by session listing
    /// to extract titles, tags, and last prompts without loading all messages.
    pub fn read_lite_metadata(&self, session_id: &str) -> Result<LiteMetadata> {
        let path = self.transcript_path(session_id);
        let tail = read_file_tail_sync(&path, LITE_READ_BUF_SIZE)?;
        Ok(extract_lite_metadata(&tail))
    }

    /// Read the first N and last N lines of a session for efficient display.
    ///
    /// Returns (head_lines, tail_lines) where head is the first `head_count`
    /// entries and tail is the last `tail_count` entries. Entries in the middle
    /// are skipped, avoiding full file reads for large sessions.
    pub fn read_head_and_tail(
        &self,
        session_id: &str,
        head_count: usize,
        tail_count: usize,
    ) -> Result<(Vec<SessionEntry>, Vec<SessionEntry>)> {
        let path = self.transcript_path(session_id);
        let data = fs::read_to_string(&path)
            .with_context(|| format!("read session: {}", path.display()))?;

        let entries: Vec<SessionEntry> = data
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();

        let total = entries.len();
        if total <= head_count + tail_count {
            let mid = total.min(head_count);
            return Ok((entries[..mid].to_vec(), entries[mid..].to_vec()));
        }

        let head = entries[..head_count].to_vec();
        let tail = entries[total - tail_count..].to_vec();
        Ok((head, tail))
    }

    /// Resume a session by loading it and restoring its state.
    ///
    /// This is the complete session resume flow:
    /// 1. Load the session from disk
    /// 2. Rebuild message UUID set for deduplication
    /// 3. Restore file history, content replacements, context collapse state
    /// 4. Set this as the active session
    pub fn resume_session(&mut self, session_id: &str) -> Result<Session> {
        let session = self.load_session(session_id)?;
        self.active_session_id = Some(session.id.clone());
        Ok(session)
    }

    /// List all sessions as lightweight summaries, newest first.
    ///
    /// Uses tail reads to extract metadata efficiently without loading
    /// full message arrays.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let dir = &self.project_dir;
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut summaries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "jsonl") {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    continue;
                }
                // Try lite metadata first for efficiency
                if let Ok(lite) = self.read_lite_metadata(&id) {
                    if let Ok(session) = self.load_session(&id) {
                        summaries.push(SessionSummary {
                            id: session.id,
                            created_at: session.created_at,
                            updated_at: session.updated_at,
                            project_root: session.project_root,
                            model: session.model,
                            message_count: session.messages.len(),
                            first_prompt: extract_first_user_prompt(&session.messages),
                            custom_title: lite.custom_title.or(session.custom_title),
                            ai_title: lite.ai_title.or(session.ai_title),
                            tag: lite.tag.or(session.tag),
                            last_prompt: lite.last_prompt.or(session.last_prompt),
                        });
                    }
                } else if let Ok(session) = self.load_session(&id) {
                    summaries.push(SessionSummary {
                        id: session.id,
                        created_at: session.created_at,
                        updated_at: session.updated_at,
                        project_root: session.project_root,
                        model: session.model,
                        message_count: session.messages.len(),
                        first_prompt: extract_first_user_prompt(&session.messages),
                        custom_title: session.custom_title,
                        ai_title: session.ai_title,
                        tag: session.tag,
                        last_prompt: session.last_prompt,
                    });
                }
            }
            // Also check legacy .json files
            else if path.extension().is_some_and(|e| e == "json") {
                if let Ok(data) = fs::read_to_string(&path) {
                    if let Ok(session) = serde_json::from_str::<Session>(&data) {
                        summaries.push(SessionSummary {
                            id: session.id,
                            created_at: session.created_at,
                            updated_at: session.updated_at,
                            project_root: session.project_root,
                            model: session.model,
                            message_count: session.messages.len(),
                            first_prompt: extract_first_user_prompt(&session.messages),
                            custom_title: session.custom_title,
                            ai_title: session.ai_title,
                            tag: session.tag,
                            last_prompt: session.last_prompt,
                        });
                    }
                }
            }
        }
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(summaries)
    }

    /// Delete a session by ID.
    pub fn delete_session(&self, id: &str) -> Result<()> {
        // Try JSONL first
        let jsonl_path = self.transcript_path(id);
        if jsonl_path.exists() {
            fs::remove_file(&jsonl_path)?;
            return Ok(());
        }
        // Try legacy JSON
        let json_path = self.project_dir.join(format!("{id}.json"));
        if json_path.exists() {
            fs::remove_file(&json_path)?;
        }
        Ok(())
    }

    /// Load the most recently updated session.
    pub fn get_latest_session(&self) -> Result<Option<Session>> {
        let summaries = self.list_sessions()?;
        match summaries.first() {
            Some(s) => Ok(Some(self.load_session(&s.id)?)),
            None => Ok(None),
        }
    }

    /// Check if a session exists on disk.
    pub fn session_exists(&self, session_id: &str) -> bool {
        self.transcript_path(session_id).exists()
    }

    /// Remove a message from the transcript by UUID.
    ///
    /// Reads the tail of the file first (the target is almost always the most
    /// recently appended entry). Falls back to a full rewrite for entries
    /// deeper in the file.
    pub fn remove_message_by_uuid(&self, session_id: &str, target_uuid: &str) -> Result<()> {
        let path = self.transcript_path(session_id);
        if !path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("read session for removal: {}", path.display()))?;

        let needle = format!("\"uuid\":\"{}\"", target_uuid);
        let filtered: Vec<&str> = content
            .lines()
            .filter(|line| !line.contains(&needle))
            .collect();

        fs::write(&path, filtered.join("\n") + "\n")?;
        Ok(())
    }

    /// List all sessions across all project directories.
    ///
    /// Scans all project directories under `~/.claude-omni/projects/` and returns
    /// summaries for all sessions found.
    pub fn list_all_sessions() -> Result<Vec<SessionSummary>> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let projects_dir = home.join(crate::config::paths::OMNI_DIR_NAME).join("projects");
        if !projects_dir.exists() {
            return Ok(Vec::new());
        }

        let mut all_summaries = Vec::new();
        for entry in fs::read_dir(&projects_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let mgr = SessionManager::new(entry.path());
                if let Ok(summaries) = mgr.list_sessions() {
                    all_summaries.extend(summaries);
                }
            }
        }
        all_summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(all_summaries)
    }
}

// ── Lite metadata ───────────────────────────────────────────────────────────

/// Lightweight metadata extracted from the tail of a session file.
#[derive(Clone, Debug, Default)]
pub struct LiteMetadata {
    pub custom_title: Option<String>,
    pub ai_title: Option<String>,
    pub tag: Option<String>,
    pub last_prompt: Option<String>,
}

/// Read the tail of a file synchronously.
fn read_file_tail_sync(path: &Path, buf_size: usize) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    let file_size = metadata.len();

    if file_size == 0 {
        return Ok(String::new());
    }

    let read_size = (file_size as usize).min(buf_size);
    let seek_pos = file_size - read_size as u64;

    file.seek(SeekFrom::Start(seek_pos))?;
    let mut buf = vec![0u8; read_size];
    file.read_exact(&mut buf)?;

    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// Extract lite metadata from a tail string.
fn extract_lite_metadata(tail: &str) -> LiteMetadata {
    let mut result = LiteMetadata::default();

    // Scan lines in reverse (last occurrence wins)
    for line in tail.lines().rev() {
        if result.custom_title.is_some()
            && result.ai_title.is_some()
            && result.tag.is_some()
            && result.last_prompt.is_some()
        {
            break;
        }

        if result.custom_title.is_none() && line.contains("\"type\":\"custom-title\"") {
            result.custom_title = extract_json_string_field(line, "customTitle");
        }
        if result.ai_title.is_none() && line.contains("\"type\":\"ai-title\"") {
            result.ai_title = extract_json_string_field(line, "aiTitle");
        }
        if result.tag.is_none() && line.contains("\"type\":\"tag\"") {
            result.tag = extract_json_string_field(line, "tag");
        }
        if result.last_prompt.is_none() && line.contains("\"type\":\"last-prompt\"") {
            result.last_prompt = extract_json_string_field(line, "lastPrompt");
        }
    }

    result
}

/// Extract a string field from a JSON line without full parsing.
///
/// Looks for `"field":"value"` pattern. This is a fast path that avoids
/// serde_json parsing for the common case.
fn extract_json_string_field(line: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", field);
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    // Find the closing quote, handling escaped quotes
    let mut end = 0;
    let mut escaped = false;
    for ch in rest.chars() {
        if escaped {
            escaped = false;
            end += ch.len_utf8();
            continue;
        }
        if ch == '\\' {
            escaped = true;
            end += 1;
            continue;
        }
        if ch == '"' {
            break;
        }
        end += ch.len_utf8();
    }
    Some(rest[..end].to_string())
}

// ── Path sanitization ───────────────────────────────────────────────────────

/// Sanitize a filesystem path for use as a directory name.
///
/// Replaces path separators with `-` and removes special characters,
/// matching the behavior of the original TS `sanitizePath`.
fn sanitize_path(path: &str) -> String {
    let mapped: String = path
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' => c,
            _ => '-',
        })
        .collect();
    // Collapse consecutive dashes and trim leading/trailing dashes
    let mut result = String::with_capacity(mapped.len());
    let mut prev_dash = false;
    for c in mapped.chars() {
        if c == '-' {
            if !prev_dash {
                result.push('-');
            }
            prev_dash = true;
        } else {
            prev_dash = false;
            result.push(c);
        }
    }
    let trimmed = result.trim_matches('-');
    trimmed.to_string()
}

// ── History ──────────────────────────────────────────────────────────────────

/// A single entry in the append-only prompt history.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub session_id: String,
    pub prompt: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// Default history file path: `~/.claude-omni/history.jsonl`.
fn history_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(crate::config::paths::OMNI_DIR_NAME)
        .join("history.jsonl")
}

/// Append a history entry to the JSONL file.
pub fn add_to_history(entry: &HistoryEntry) -> Result<()> {
    let path = history_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let line = serde_json::to_string(entry)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Load recent history entries, optionally filtered by project root.
pub fn load_history(limit: usize, project_filter: Option<&str>) -> Result<Vec<HistoryEntry>> {
    let path = history_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(&path)?;
    let mut entries: Vec<HistoryEntry> = data
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    if let Some(project) = project_filter {
        entries.retain(|e| e.project_root.as_deref() == Some(project));
    }

    // Return most recent entries
    let start = entries.len().saturating_sub(limit);
    Ok(entries[start..].to_vec())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

const MAX_PROMPT_CHARS: usize = 120;

/// Regex pattern to skip non-meaningful messages when extracting first prompt.
/// Matches XML-like tags at start (IDE context, hook output) or interrupt markers.
fn is_skip_first_prompt(text: &str) -> bool {
    let trimmed = text.trim();
    // Starts with lowercase XML-like tag
    if trimmed.starts_with('<') {
        if let Some(after_lt) = trimmed.strip_prefix('<') {
            if let Some(first_char) = after_lt.chars().next() {
                if first_char.is_ascii_lowercase() {
                    return true;
                }
            }
        }
    }
    // Interrupt marker
    trimmed.starts_with("[Request interrupted by user")
}

/// Extract the first user prompt from messages for display in session lists.
fn extract_first_user_prompt(messages: &[serde_json::Value]) -> Option<String> {
    for msg in messages {
        if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }
        let content = msg.get("content")?;
        let text = if let Some(s) = content.as_str() {
            s.to_string()
        } else if let Some(arr) = content.as_array() {
            arr.iter().find_map(|block| {
                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                    block.get("text").and_then(|v| v.as_str()).map(String::from)
                } else {
                    None
                }
            })?
        } else {
            continue;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() || is_skip_first_prompt(trimmed) {
            continue;
        }
        if trimmed.chars().count() <= MAX_PROMPT_CHARS {
            return Some(trimmed.to_string());
        }
        let truncated: String = trimmed.chars().take(MAX_PROMPT_CHARS).collect();
        return Some(format!("{truncated}..."));
    }
    None
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cumulative_usage_add() {
        let mut cu = CumulativeUsage::default();
        let u = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: Some(10),
            cache_read_input_tokens: Some(20),
            server_tool_use: None,
            speed: None,
        };
        cu.add(&u);
        assert_eq!(cu.input_tokens, 100);
        assert_eq!(cu.output_tokens, 50);
        assert_eq!(cu.cache_creation_input_tokens, 10);
        assert_eq!(cu.cache_read_input_tokens, 20);

        cu.add(&u);
        assert_eq!(cu.input_tokens, 200);
        assert_eq!(cu.output_tokens, 100);
    }

    #[test]
    fn test_session_create_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();
        let loaded = mgr.load_session(&session.id).unwrap();
        assert_eq!(loaded.id, session.id);
        assert!(loaded.messages.is_empty());
    }

    #[test]
    fn test_session_save_and_load_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());

        let mut session = mgr.create_session().unwrap();
        session.messages.push(serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "Hello"}]
        }));
        session.model = Some("opus".to_string());
        mgr.save_session(&session).unwrap();

        let loaded = mgr.load_session(&session.id).unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.model, Some("opus".to_string()));
        assert!(loaded.messages[0].to_string().contains("Hello"));
    }

    #[test]
    fn test_session_save_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());

        let s1 = mgr.create_session().unwrap();
        let s2 = mgr.create_session().unwrap();

        // Save both so they have JSONL files
        mgr.save_session(&s1).unwrap();
        mgr.save_session(&s2).unwrap();

        let summaries = mgr.list_sessions().unwrap();
        assert_eq!(summaries.len(), 2);
        // Newest first
        assert_eq!(summaries[0].id, s2.id);
        assert_eq!(summaries[1].id, s1.id);
    }

    #[test]
    fn test_session_delete() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();
        mgr.save_session(&session).unwrap();
        mgr.delete_session(&session.id).unwrap();
        assert!(mgr.load_session(&session.id).is_err());
    }

    #[test]
    fn test_append_message() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();

        let msg = serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "Hello"}]
        });
        mgr.append_message(&session.id, &msg).unwrap();

        let loaded = mgr.load_session(&session.id).unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert!(loaded.messages[0].to_string().contains("Hello"));
    }

    #[test]
    fn test_extract_first_user_prompt_simple() {
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "Hello world"}]
        })];
        assert_eq!(
            extract_first_user_prompt(&messages),
            Some("Hello world".to_string())
        );
    }

    #[test]
    fn test_extract_first_user_prompt_truncation() {
        let long_text = "x".repeat(200);
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": long_text}]
        })];
        let result = extract_first_user_prompt(&messages).unwrap();
        assert!(result.ends_with("..."));
        assert_eq!(result.chars().count(), MAX_PROMPT_CHARS + 3);
    }

    #[test]
    fn test_extract_first_user_prompt_utf8_safe() {
        let text = "日本語テスト".repeat(30);
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": text}]
        })];
        // Should not panic
        let _ = extract_first_user_prompt(&messages);
    }

    #[test]
    fn test_extract_first_user_prompt_skips_xml_tags() {
        let messages = vec![
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "<context>some context</context>"}]
            }),
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "Real prompt here"}]
            }),
        ];
        assert_eq!(
            extract_first_user_prompt(&messages),
            Some("Real prompt here".to_string())
        );
    }

    #[test]
    fn test_extract_first_user_prompt_skips_interrupts() {
        let messages = vec![
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "[Request interrupted by user]"}]
            }),
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "Real prompt"}]
            }),
        ];
        assert_eq!(
            extract_first_user_prompt(&messages),
            Some("Real prompt".to_string())
        );
    }

    #[test]
    fn test_history_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");

        let entry = HistoryEntry {
            session_id: "test-id".to_string(),
            prompt: "hello".to_string(),
            timestamp: Utc::now(),
            project_root: Some("/tmp/proj".to_string()),
            model: Some("opus".to_string()),
        };
        let line = serde_json::to_string(&entry).unwrap();
        fs::write(&path, format!("{line}\n")).unwrap();

        let data = fs::read_to_string(&path).unwrap();
        let loaded: HistoryEntry = serde_json::from_str(data.trim()).unwrap();
        assert_eq!(loaded.session_id, "test-id");
        assert_eq!(loaded.prompt, "hello");
    }

    #[test]
    fn test_sanitize_path() {
        assert_eq!(
            sanitize_path("/Users/alice/projects/my-app"),
            "Users-alice-projects-my-app"
        );
        assert_eq!(sanitize_path("C:\\Users\\bob\\code"), "C-Users-bob-code");
        assert_eq!(sanitize_path("/tmp/test"), "tmp-test");
    }

    #[test]
    fn test_project_dir_for_cwd() {
        let dir = SessionManager::project_dir_for_cwd("/Users/test/project");
        let dir_str = dir.to_string_lossy();
        assert!(dir_str.contains(".claude-omni/projects/"));
        assert!(dir_str.contains("Users-test-project"));
    }

    #[test]
    fn test_message_grouping_simple() {
        let messages = vec![
            serde_json::json!({"role": "user", "content": [{"type": "text", "text": "Hello"}]}),
            serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "Hi"}]}),
        ];
        let groups = group_messages(&messages);
        assert_eq!(groups.len(), 2);
        assert!(!groups[0].is_tool_pair);
        assert!(!groups[1].is_tool_pair);
    }

    #[test]
    fn test_message_grouping_tool_pair() {
        let messages = vec![
            serde_json::json!({"role": "user", "content": [{"type": "text", "text": "Run ls"}]}),
            serde_json::json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "bash", "input": {}}]}),
            serde_json::json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "output"}]}),
            serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "Done"}]}),
        ];
        let groups = group_messages(&messages);
        assert_eq!(groups.len(), 3);
        assert!(!groups[0].is_tool_pair);
        assert!(groups[1].is_tool_pair);
        assert_eq!(groups[1].messages.len(), 2);
        assert!(!groups[2].is_tool_pair);
    }

    #[test]
    fn test_session_custom_title() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();

        mgr.set_custom_title(&session.id, "My Session").unwrap();

        let loaded = mgr.load_session(&session.id).unwrap();
        assert_eq!(loaded.custom_title, Some("My Session".to_string()));
    }

    #[test]
    fn test_session_tag() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();

        mgr.set_tag(&session.id, "debug").unwrap();

        let loaded = mgr.load_session(&session.id).unwrap();
        assert_eq!(loaded.tag, Some("debug".to_string()));
    }

    #[test]
    fn test_file_history_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();

        let mut files = HashMap::new();
        files.insert(
            "/tmp/test.rs".to_string(),
            FileState {
                content: Some("fn main() {}".to_string()),
                mtime_ms: Some(1700000000000),
                phase: FileStatePhase::Before,
            },
        );
        let snapshot = FileHistorySnapshot { files };
        mgr.append_file_history_snapshot(&session.id, "msg-uuid-1", &snapshot, false)
            .unwrap();

        let loaded = mgr.load_session(&session.id).unwrap();
        assert!(loaded.file_history.contains_key("msg-uuid-1"));
        let entry = &loaded.file_history["msg-uuid-1"];
        assert!(!entry.is_snapshot_update);
        assert!(entry.snapshot.files.contains_key("/tmp/test.rs"));
    }

    #[test]
    fn test_content_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();

        let replacement = ContentReplacementEntry {
            session_id: session.id.clone(),
            replacements: vec![ContentReplacementRecord {
                message_uuid: "msg-1".to_string(),
                content_block_index: 0,
                replacement_text: "[file content summarized]".to_string(),
                original_hash: Some("abc123".to_string()),
            }],
            agent_id: None,
        };
        mgr.append_content_replacement(&session.id, &replacement)
            .unwrap();

        let loaded = mgr.load_session(&session.id).unwrap();
        assert_eq!(loaded.content_replacements.len(), 1);
        assert_eq!(loaded.content_replacements[0].replacements.len(), 1);
        assert_eq!(
            loaded.content_replacements[0].replacements[0].replacement_text,
            "[file content summarized]"
        );
    }

    #[test]
    fn test_session_resume() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new(dir.path().to_path_buf());

        let mut session = mgr.create_session().unwrap();
        session.messages.push(serde_json::json!({
            "role": "user",
            "uuid": "msg-1",
            "content": [{"type": "text", "text": "Hello"}]
        }));
        session.custom_title = Some("Test Session".to_string());
        mgr.save_session(&session).unwrap();

        let resumed = mgr.resume_session(&session.id).unwrap();
        assert_eq!(resumed.id, session.id);
        assert_eq!(resumed.messages.len(), 1);
        assert_eq!(resumed.custom_title, Some("Test Session".to_string()));
        assert!(resumed.message_uuids.contains("msg-1"));
        assert_eq!(mgr.active_session_id(), Some(session.id.as_str()));
    }

    #[test]
    fn test_session_exists() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();
        assert!(mgr.session_exists(&session.id));
        assert!(!mgr.session_exists("nonexistent"));
    }

    #[test]
    fn test_extract_json_string_field() {
        let line = r#"{"type":"custom-title","customTitle":"My Title","sessionId":"abc"}"#;
        assert_eq!(
            extract_json_string_field(line, "customTitle"),
            Some("My Title".to_string())
        );
        assert_eq!(
            extract_json_string_field(line, "sessionId"),
            Some("abc".to_string())
        );
        assert_eq!(extract_json_string_field(line, "missing"), None);
    }

    #[test]
    fn test_extract_json_string_field_escaped_quotes() {
        let line = r#"{"type":"custom-title","customTitle":"Say \"hello\""}"#;
        let result = extract_json_string_field(line, "customTitle");
        assert_eq!(result, Some(r#"Say \"hello\""#.to_string()));
    }

    #[test]
    fn test_session_summary_display_title() {
        let summary = SessionSummary {
            id: "test".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            project_root: None,
            model: None,
            message_count: 0,
            first_prompt: Some("first prompt".to_string()),
            custom_title: None,
            ai_title: Some("AI title".to_string()),
            tag: None,
            last_prompt: Some("last prompt".to_string()),
        };
        // ai_title takes precedence over last_prompt and first_prompt
        assert_eq!(summary.display_title(), Some("AI title"));

        let summary2 = SessionSummary {
            custom_title: Some("Custom".to_string()),
            ..summary
        };
        // custom_title takes precedence over everything
        assert_eq!(summary2.display_title(), Some("Custom"));
    }

    #[test]
    fn test_read_head_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());

        let mut session = mgr.create_session().unwrap();
        for i in 0..10 {
            session.messages.push(serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": format!("Message {i}")}]
            }));
        }
        mgr.save_session(&session).unwrap();

        let (head, tail) = mgr.read_head_and_tail(&session.id, 3, 3).unwrap();
        assert_eq!(head.len(), 3);
        assert_eq!(tail.len(), 3);
    }

    #[test]
    fn test_context_collapse() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();

        let commit = ContextCollapseCommit {
            collapse_id: "cc-1".to_string(),
            summary_uuid: "sum-1".to_string(),
            summary_content: "Summary text".to_string(),
            summary: "Short summary".to_string(),
            first_archived_uuid: "first-1".to_string(),
            last_archived_uuid: "last-1".to_string(),
        };
        mgr.append_context_collapse_commit(&session.id, &commit)
            .unwrap();

        let snapshot = ContextCollapseSnapshot {
            staged: vec![StagedRange {
                start_uuid: "s1".to_string(),
                end_uuid: "e1".to_string(),
                summary: "range summary".to_string(),
                risk: 0.3,
                staged_at: 1700000000,
            }],
            armed: true,
            last_spawn_tokens: 50000,
        };
        mgr.append_context_collapse_snapshot(&session.id, &snapshot)
            .unwrap();

        let loaded = mgr.load_session(&session.id).unwrap();
        assert_eq!(loaded.context_collapse_commits.len(), 1);
        assert_eq!(loaded.context_collapse_commits[0].collapse_id, "cc-1");
        assert!(loaded.context_collapse_snapshot.is_some());
        assert!(loaded.context_collapse_snapshot.unwrap().armed);
    }

    #[test]
    fn test_remove_message_by_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let session = mgr.create_session().unwrap();

        let msg1 = serde_json::json!({"role": "user", "uuid": "uuid-1", "content": "Hello"});
        let msg2 = serde_json::json!({"role": "assistant", "uuid": "uuid-2", "content": "Hi"});
        mgr.append_message(&session.id, &msg1).unwrap();
        mgr.append_message(&session.id, &msg2).unwrap();

        let loaded = mgr.load_session(&session.id).unwrap();
        assert_eq!(loaded.messages.len(), 2);

        mgr.remove_message_by_uuid(&session.id, "uuid-2").unwrap();

        let loaded = mgr.load_session(&session.id).unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(
            loaded.messages[0].get("uuid").unwrap().as_str().unwrap(),
            "uuid-1"
        );
    }
}
