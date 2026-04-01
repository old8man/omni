//! Session persistence and prompt history.
//!
//! Sessions are stored as JSONL files at project-scoped paths:
//! `~/.claude/projects/{sanitized_cwd}/{session_id}.jsonl`
//!
//! Each line in the JSONL file is a timestamped entry containing either a
//! message, metadata update, or session event. This append-only format
//! matches the original TypeScript implementation.
//!
//! History is a separate append-only JSONL file at `~/.claude/history.jsonl`.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

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
}

// ── Session manager ──────────────────────────────────────────────────────────

/// Manages session files on disk using project-scoped JSONL format.
///
/// Session transcripts are stored at:
/// `~/.claude/projects/{sanitized_cwd}/{session_id}.jsonl`
pub struct SessionManager {
    /// The project directory (already sanitized).
    project_dir: PathBuf,
}

impl SessionManager {
    /// Create a manager for the given project directory.
    ///
    /// The project directory is typically `~/.claude/projects/{sanitized_cwd}`.
    pub fn new(project_dir: PathBuf) -> Self {
        Self { project_dir }
    }

    /// Compute the project-scoped session directory for a given working directory.
    ///
    /// Path: `~/.claude/projects/{sanitized_cwd}`
    pub fn project_dir_for_cwd(cwd: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let sanitized = sanitize_path(cwd);
        home.join(".claude").join("projects").join(sanitized)
    }

    /// Default sessions directory (legacy): `~/.claude/sessions`.
    pub fn default_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".claude")
            .join("sessions")
    }

    /// Get the JSONL transcript path for a session.
    pub fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.project_dir.join(format!("{session_id}.jsonl"))
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
        let data = fs::read_to_string(path)
            .with_context(|| format!("read session: {}", path.display()))?;

        let mut messages = Vec::new();
        let mut created_at = None;
        let mut updated_at = Utc::now();
        let mut project_root = None;
        let mut model = None;
        let mut total_cost = 0.0;
        let mut cumulative_usage = CumulativeUsage::default();

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
                    "message" => {
                        // The data field IS the message (role, content, etc.)
                        messages.push(entry.data);
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
        })
    }

    /// List all sessions as lightweight summaries, newest first.
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
                if let Ok(session) = self.load_session(&id) {
                    summaries.push(SessionSummary {
                        id: session.id,
                        created_at: session.created_at,
                        updated_at: session.updated_at,
                        project_root: session.project_root,
                        model: session.model,
                        message_count: session.messages.len(),
                        first_prompt: extract_first_user_prompt(&session.messages),
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

/// Default history file path: `~/.claude/history.jsonl`.
fn history_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
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
        if trimmed.is_empty() {
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
        assert!(dir_str.contains(".claude/projects/"));
        assert!(dir_str.contains("Users-test-project"));
    }
}
