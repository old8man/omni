//! Multi-session spawn modes for the bridge.
//!
//! The standalone bridge (`claude remote-control`) supports three spawn modes:
//! - SingleSession: one session in cwd, bridge tears down when it ends
//! - Worktree: persistent server, each session gets an isolated git worktree
//! - SameDir: persistent server, all sessions share the working directory
//!
//! This module handles session spawning, git worktree lifecycle, capacity
//! tracking, and session timeout management.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::types::{SessionActivity, SessionActivityType, SessionDoneStatus, SpawnMode};

/// Maximum number of activity entries to keep per session.
const MAX_ACTIVITIES: usize = 10;

/// Maximum stderr lines to buffer per session for error diagnostics.
const MAX_STDERR_LINES: usize = 10;

/// Default maximum sessions for multi-session modes.
pub const DEFAULT_MAX_SESSIONS: u32 = 32;

/// Map tool names to human-readable verbs for the status display.
fn tool_verb(name: &str) -> &str {
    match name {
        "Read" | "FileReadTool" => "Reading",
        "Write" | "FileWriteTool" => "Writing",
        "Edit" | "MultiEdit" | "FileEditTool" => "Editing",
        "Bash" | "BashTool" => "Running",
        "Glob" | "Grep" | "GlobTool" | "GrepTool" => "Searching",
        "WebFetch" => "Fetching",
        "WebSearch" => "Searching",
        "Task" => "Running task",
        "NotebookEditTool" => "Editing notebook",
        "LSP" => "LSP",
        other => other,
    }
}

/// Build a short tool activity summary from a tool name and its input.
pub fn tool_summary(name: &str, input: &Value) -> String {
    let verb = tool_verb(name);
    let target = input
        .get("file_path")
        .or_else(|| input.get("filePath"))
        .or_else(|| input.get("pattern"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            input
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| s[..s.len().min(60)].to_string())
        })
        .or_else(|| {
            input
                .get("url")
                .or_else(|| input.get("query"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    if target.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} {target}")
    }
}

/// Extract session activities from a child process NDJSON line.
pub fn extract_activities(line: &str) -> Vec<SessionActivity> {
    let parsed: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let obj = match parsed.as_object() {
        Some(o) => o,
        None => return Vec::new(),
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut activities = Vec::new();

    match obj.get("type").and_then(|t| t.as_str()) {
        Some("assistant") => {
            if let Some(message) = obj.get("message").and_then(|m| m.as_object()) {
                if let Some(content) = message.get("content").and_then(|c| c.as_array()) {
                    for block in content {
                        let block = match block.as_object() {
                            Some(b) => b,
                            None => continue,
                        };
                        match block.get("type").and_then(|t| t.as_str()) {
                            Some("tool_use") => {
                                let name = block
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("Tool");
                                let input = block
                                    .get("input")
                                    .cloned()
                                    .unwrap_or(Value::Object(Default::default()));
                                let summary = tool_summary(name, &input);
                                activities.push(SessionActivity {
                                    activity_type: SessionActivityType::ToolStart,
                                    summary,
                                    timestamp: now,
                                });
                            }
                            Some("text") => {
                                let text = block
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("");
                                if !text.is_empty() {
                                    activities.push(SessionActivity {
                                        activity_type: SessionActivityType::Text,
                                        summary: text[..text.len().min(80)].to_string(),
                                        timestamp: now,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Some("result") => {
            let subtype = obj.get("subtype").and_then(|s| s.as_str());
            match subtype {
                Some("success") => {
                    activities.push(SessionActivity {
                        activity_type: SessionActivityType::Result,
                        summary: "Session completed".to_string(),
                        timestamp: now,
                    });
                }
                Some(st) => {
                    let error_summary = obj
                        .get("errors")
                        .and_then(|e| e.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("Error: {st}"));
                    activities.push(SessionActivity {
                        activity_type: SessionActivityType::Error,
                        summary: error_summary,
                        timestamp: now,
                    });
                }
                None => {}
            }
        }
        _ => {}
    }

    activities
}

/// Sanitize a session ID for use in file names.
///
/// Strips any characters that could cause path traversal or filesystem issues.
pub fn safe_filename_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Extract plain text from a user message for title derivation.
///
/// Returns the trimmed text if this looks like a real human-authored message,
/// otherwise `None`.
pub fn extract_user_message_text(msg: &Value) -> Option<String> {
    let obj = msg.as_object()?;

    // Skip tool-result user messages, synthetic messages, and replayed messages
    if obj.get("parent_tool_use_id").is_some()
        || obj.get("isSynthetic").and_then(|v| v.as_bool()) == Some(true)
        || obj.get("isReplay").and_then(|v| v.as_bool()) == Some(true)
    {
        return None;
    }

    let message = obj.get("message")?.as_object()?;
    let content = message.get("content")?;

    let text = if let Some(s) = content.as_str() {
        s.to_string()
    } else if let Some(arr) = content.as_array() {
        arr.iter()
            .find_map(|block| {
                let b = block.as_object()?;
                if b.get("type")?.as_str()? == "text" {
                    b.get("text")?.as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default()
    } else {
        return None;
    };

    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Options for spawning a session process.
#[derive(Clone, Debug)]
pub struct SessionSpawnOpts {
    /// Session ID from the work item.
    pub session_id: String,
    /// WebSocket/SSE URL for the session transport.
    pub sdk_url: String,
    /// Access token for session authentication.
    pub access_token: String,
    /// Whether to use CCR v2 transport (SSE + CCRClient).
    pub use_ccr_v2: bool,
    /// Worker epoch (required when use_ccr_v2 is true).
    pub worker_epoch: Option<i64>,
}

/// Handle to a running session process.
pub struct SessionHandle {
    /// Session ID.
    pub session_id: String,
    /// Completion future resolving to the terminal status.
    pub done: tokio::sync::oneshot::Receiver<SessionDoneStatus>,
    /// Ring buffer of recent activities.
    pub activities: Vec<SessionActivity>,
    /// Most recent activity.
    pub current_activity: Option<SessionActivity>,
    /// Session access token.
    pub access_token: String,
    /// Ring buffer of last stderr lines.
    pub last_stderr: Vec<String>,
    /// Channel for writing to the child's stdin.
    stdin_tx: Option<mpsc::UnboundedSender<String>>,
    /// Activity receiver from the child stdout parser.
    activity_rx: Option<mpsc::UnboundedReceiver<SessionActivity>>,
    /// First user message receiver. Used by callers to derive session titles.
    pub first_user_msg_rx: Option<tokio::sync::oneshot::Receiver<String>>,
    /// Handle to kill the child process.
    kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl SessionHandle {
    /// Kill the child process with SIGTERM.
    pub fn kill(&mut self) {
        if let Some(tx) = self.kill_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Write data to the child's stdin.
    pub fn write_stdin(&self, data: &str) {
        if let Some(tx) = &self.stdin_tx {
            let _ = tx.send(data.to_string());
        }
    }

    /// Update the access token for a running session.
    ///
    /// Sends the fresh token to the child process via stdin as an
    /// `update_environment_variables` message.
    pub fn update_access_token(&mut self, token: &str) {
        self.access_token = token.to_string();
        let msg = serde_json::json!({
            "type": "update_environment_variables",
            "variables": {
                "CLAUDE_CODE_SESSION_ACCESS_TOKEN": token,
            }
        });
        if let Ok(line) = serde_json::to_string(&msg) {
            self.write_stdin(&format!("{line}\n"));
        }
    }

    /// Drain pending activities from the activity channel into the internal buffer.
    pub fn drain_activities(&mut self) {
        if let Some(rx) = &mut self.activity_rx {
            while let Ok(activity) = rx.try_recv() {
                if self.activities.len() >= MAX_ACTIVITIES {
                    self.activities.remove(0);
                }
                self.current_activity = Some(activity.clone());
                self.activities.push(activity);
            }
        }
    }
}

/// Configuration for the session spawner.
#[derive(Clone, Debug)]
pub struct SpawnerConfig {
    /// Path to the executable to spawn.
    pub exec_path: String,
    /// Additional script arguments (for npm/node installs).
    pub script_args: Vec<String>,
    /// Whether to enable verbose logging.
    pub verbose: bool,
    /// Whether to enable sandbox mode.
    pub sandbox: bool,
    /// Debug file path template.
    pub debug_file: Option<String>,
    /// Permission mode for sessions.
    pub permission_mode: Option<String>,
}

/// Spawns session child processes.
pub struct SessionSpawner {
    config: SpawnerConfig,
}

impl SessionSpawner {
    /// Create a new session spawner with the given configuration.
    pub fn new(config: SpawnerConfig) -> Self {
        Self { config }
    }

    /// Spawn a new session child process in the given directory.
    pub fn spawn(
        &self,
        opts: &SessionSpawnOpts,
        dir: &Path,
    ) -> Result<SessionHandle> {
        let safe_id = safe_filename_id(&opts.session_id);

        // Resolve debug file path
        let debug_file = self.config.debug_file.as_ref().map(|df| {
            if let Some(dot) = df.rfind('.') {
                format!("{}-{}{}", &df[..dot], safe_id, &df[dot..])
            } else {
                format!("{df}-{safe_id}")
            }
        });

        let mut args = self.config.script_args.clone();
        args.extend([
            "--print".to_string(),
            "--sdk-url".to_string(),
            opts.sdk_url.clone(),
            "--session-id".to_string(),
            opts.session_id.clone(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--replay-user-messages".to_string(),
        ]);
        if self.config.verbose {
            args.push("--verbose".to_string());
        }
        if let Some(df) = &debug_file {
            args.extend(["--debug-file".to_string(), df.clone()]);
        }
        if let Some(pm) = &self.config.permission_mode {
            args.extend(["--permission-mode".to_string(), pm.clone()]);
        }

        let mut cmd = Command::new(&self.config.exec_path);
        cmd.args(&args)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("CLAUDE_CODE_ENVIRONMENT_KIND", "bridge");

        // Strip the bridge's OAuth token so the child uses the session token
        cmd.env_remove("CLAUDE_CODE_OAUTH_TOKEN");
        cmd.env("CLAUDE_CODE_SESSION_ACCESS_TOKEN", &opts.access_token);
        cmd.env("CLAUDE_CODE_POST_FOR_SESSION_INGRESS_V2", "1");

        if self.config.sandbox {
            cmd.env("CLAUDE_CODE_FORCE_SANDBOX", "1");
        }

        if opts.use_ccr_v2 {
            cmd.env("CLAUDE_CODE_USE_CCR_V2", "1");
            if let Some(epoch) = opts.worker_epoch {
                cmd.env("CLAUDE_CODE_WORKER_EPOCH", epoch.to_string());
            }
        }

        let mut child = cmd.spawn().context("failed to spawn session process")?;

        tracing::debug!(
            "[bridge:session] Spawned sessionId={} pid={:?}",
            opts.session_id,
            child.id()
        );

        // Set up channels
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
        let (activity_tx, activity_rx) = mpsc::unbounded_channel::<SessionActivity>();
        let (first_user_msg_tx, first_user_msg_rx) = tokio::sync::oneshot::channel::<String>();
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<SessionDoneStatus>();

        // Forward stdin
        let child_stdin = child.stdin.take();
        tokio::spawn(async move {
            if let Some(mut stdin) = child_stdin {
                use tokio::io::AsyncWriteExt;
                while let Some(data) = stdin_rx.recv().await {
                    if stdin.write_all(data.as_bytes()).await.is_err() {
                        break;
                    }
                }
            }
        });

        // Parse stdout NDJSON
        let child_stdout = child.stdout.take();
        let session_id_clone = opts.session_id.clone();
        let verbose = self.config.verbose;
        tokio::spawn(async move {
            if let Some(stdout) = child_stdout {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                let mut first_user_msg_tx = Some(first_user_msg_tx);

                while let Ok(Some(line)) = lines.next_line().await {
                    if verbose {
                        eprintln!("{line}");
                    }

                    let extracted = extract_activities(&line);
                    for activity in extracted {
                        let _ = activity_tx.send(activity);
                    }

                    // Detect first user message
                    if first_user_msg_tx.is_some() {
                        if let Ok(parsed) = serde_json::from_str::<Value>(&line) {
                            if parsed.get("type").and_then(|t| t.as_str()) == Some("user") {
                                if let Some(text) = extract_user_message_text(&parsed) {
                                    if let Some(tx) = first_user_msg_tx.take() {
                                        let _ = tx.send(text);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let _ = session_id_clone;
        });

        // Buffer stderr
        let child_stderr = child.stderr.take();
        let stderr_lines: Arc<tokio::sync::Mutex<Vec<String>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr_clone = stderr_lines.clone();
        tokio::spawn(async move {
            if let Some(stderr) = child_stderr {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut buf = stderr_clone.lock().await;
                    if buf.len() >= MAX_STDERR_LINES {
                        buf.remove(0);
                    }
                    buf.push(line);
                }
            }
        });

        // Wait for child exit or kill signal
        let session_id_done = opts.session_id.clone();
        tokio::spawn(async move {
            let status = tokio::select! {
                result = child.wait() => {
                    match result {
                        Ok(exit) => {
                            if exit.success() {
                                SessionDoneStatus::Completed
                            } else {
                                SessionDoneStatus::Failed
                            }
                        }
                        Err(_) => SessionDoneStatus::Failed,
                    }
                }
                _ = kill_rx => {
                    let _ = child.kill().await;
                    SessionDoneStatus::Interrupted
                }
            };
            tracing::debug!(
                "[bridge:session] sessionId={} exited status={:?}",
                session_id_done,
                status
            );
            let _ = done_tx.send(status);
        });

        Ok(SessionHandle {
            session_id: opts.session_id.clone(),
            done: done_rx,
            activities: Vec::new(),
            current_activity: None,
            access_token: opts.access_token.clone(),
            last_stderr: Vec::new(),
            stdin_tx: Some(stdin_tx),
            activity_rx: Some(activity_rx),
            first_user_msg_rx: Some(first_user_msg_rx),
            kill_tx: Some(kill_tx),
        })
    }
}

/// Git worktree information for a session.
#[derive(Clone, Debug)]
pub struct WorktreeInfo {
    /// Path to the worktree directory.
    pub worktree_path: PathBuf,
    /// Branch name used for this worktree.
    pub worktree_branch: Option<String>,
    /// Git root directory.
    pub git_root: Option<PathBuf>,
}

/// Create a git worktree for a session.
///
/// Creates a new worktree branched from the current branch with a unique
/// name based on the session ID.
pub async fn create_session_worktree(
    git_root: &Path,
    session_id: &str,
    base_branch: &str,
) -> Result<WorktreeInfo> {
    let safe_id = safe_filename_id(session_id);
    let branch_name = format!("bridge/{safe_id}");
    let worktree_path = git_root.join(".bridge-worktrees").join(&safe_id);

    // Create the worktree directory parent
    if let Some(parent) = worktree_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create worktree parent directory")?;
    }

    // Create the git worktree
    let output = Command::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            &branch_name,
            &worktree_path.to_string_lossy(),
            base_branch,
        ])
        .current_dir(git_root)
        .output()
        .await
        .context("failed to create git worktree")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree add failed: {stderr}");
    }

    tracing::debug!(
        "[bridge:worktree] Created worktree for sessionId={} at {}",
        session_id,
        worktree_path.display()
    );

    Ok(WorktreeInfo {
        worktree_path,
        worktree_branch: Some(branch_name),
        git_root: Some(git_root.to_path_buf()),
    })
}

/// Remove a git worktree and its associated branch.
pub async fn remove_session_worktree(info: &WorktreeInfo) -> Result<()> {
    let git_root = info
        .git_root
        .as_ref()
        .context("no git root for worktree removal")?;

    // Remove the worktree
    let output = Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            &info.worktree_path.to_string_lossy(),
        ])
        .current_dir(git_root)
        .output()
        .await;

    if let Err(e) = output {
        tracing::warn!(
            "[bridge:worktree] Failed to remove worktree at {}: {e}",
            info.worktree_path.display()
        );
    }

    // Delete the branch
    if let Some(branch) = &info.worktree_branch {
        let _ = Command::new("git")
            .args(["branch", "-D", branch])
            .current_dir(git_root)
            .output()
            .await;
    }

    Ok(())
}

/// Determine the session working directory based on the spawn mode.
pub async fn resolve_session_dir(
    spawn_mode: SpawnMode,
    base_dir: &Path,
    session_id: &str,
    base_branch: &str,
) -> Result<(PathBuf, Option<WorktreeInfo>)> {
    match spawn_mode {
        SpawnMode::SingleSession | SpawnMode::SameDir => Ok((base_dir.to_path_buf(), None)),
        SpawnMode::Worktree => {
            let info = create_session_worktree(base_dir, session_id, base_branch).await?;
            let path = info.worktree_path.clone();
            Ok((path, Some(info)))
        }
    }
}

/// Tracks active sessions and their capacity for multi-session bridges.
pub struct SessionCapacity {
    /// Maximum concurrent sessions.
    max_sessions: u32,
    /// Active session handles.
    active: HashMap<String, ()>,
}

impl SessionCapacity {
    /// Create a new session capacity tracker.
    pub fn new(max_sessions: u32) -> Self {
        Self {
            max_sessions,
            active: HashMap::new(),
        }
    }

    /// Check if there is available capacity for new sessions.
    pub fn has_capacity(&self) -> bool {
        (self.active.len() as u32) < self.max_sessions
    }

    /// Get the number of active sessions.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Get the maximum number of sessions.
    pub fn max_sessions(&self) -> u32 {
        self.max_sessions
    }

    /// Register a new active session. Returns `false` if at capacity.
    pub fn register(&mut self, session_id: &str) -> bool {
        if !self.has_capacity() {
            return false;
        }
        self.active.insert(session_id.to_string(), ());
        true
    }

    /// Remove a session from the active set.
    pub fn remove(&mut self, session_id: &str) {
        self.active.remove(session_id);
    }

    /// Check if a session is currently active.
    pub fn is_active(&self, session_id: &str) -> bool {
        self.active.contains_key(session_id)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_summary_with_file() {
        let input = json!({"file_path": "/src/main.rs"});
        assert_eq!(tool_summary("Read", &input), "Reading /src/main.rs");
    }

    #[test]
    fn test_tool_summary_with_command() {
        let input = json!({"command": "cargo build"});
        assert_eq!(tool_summary("Bash", &input), "Running cargo build");
    }

    #[test]
    fn test_tool_summary_no_target() {
        let input = json!({});
        assert_eq!(tool_summary("WebSearch", &input), "Searching");
    }

    #[test]
    fn test_tool_summary_unknown_tool() {
        let input = json!({"file_path": "test.py"});
        assert_eq!(tool_summary("CustomTool", &input), "CustomTool test.py");
    }

    #[test]
    fn test_safe_filename_id() {
        assert_eq!(safe_filename_id("session_abc-123"), "session_abc-123");
        assert_eq!(safe_filename_id("../../bad"), "______bad");
        assert_eq!(safe_filename_id("has space"), "has_space");
    }

    #[test]
    fn test_extract_activities_tool_use() {
        let line = json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Read",
                    "input": {"file_path": "/src/main.rs"}
                }]
            }
        });
        let activities = extract_activities(&serde_json::to_string(&line).unwrap());
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].activity_type, SessionActivityType::ToolStart);
        assert!(activities[0].summary.contains("Reading"));
    }

    #[test]
    fn test_extract_activities_text() {
        let line = json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "text",
                    "text": "Let me help you with that."
                }]
            }
        });
        let activities = extract_activities(&serde_json::to_string(&line).unwrap());
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].activity_type, SessionActivityType::Text);
    }

    #[test]
    fn test_extract_activities_result() {
        let line = json!({
            "type": "result",
            "subtype": "success"
        });
        let activities = extract_activities(&serde_json::to_string(&line).unwrap());
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].activity_type, SessionActivityType::Result);
    }

    #[test]
    fn test_extract_activities_error_result() {
        let line = json!({
            "type": "result",
            "subtype": "error",
            "errors": ["Something went wrong"]
        });
        let activities = extract_activities(&serde_json::to_string(&line).unwrap());
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].activity_type, SessionActivityType::Error);
        assert!(activities[0].summary.contains("Something went wrong"));
    }

    #[test]
    fn test_extract_activities_invalid_json() {
        let activities = extract_activities("not json");
        assert!(activities.is_empty());
    }

    #[test]
    fn test_extract_user_message_text_simple() {
        let msg = json!({
            "type": "user",
            "message": {"content": "Hello world"}
        });
        assert_eq!(
            extract_user_message_text(&msg),
            Some("Hello world".to_string())
        );
    }

    #[test]
    fn test_extract_user_message_text_blocks() {
        let msg = json!({
            "type": "user",
            "message": {
                "content": [{"type": "text", "text": "Hello world"}]
            }
        });
        assert_eq!(
            extract_user_message_text(&msg),
            Some("Hello world".to_string())
        );
    }

    #[test]
    fn test_extract_user_message_text_synthetic() {
        let msg = json!({
            "type": "user",
            "isSynthetic": true,
            "message": {"content": "Hello"}
        });
        assert_eq!(extract_user_message_text(&msg), None);
    }

    #[test]
    fn test_extract_user_message_text_tool_result() {
        let msg = json!({
            "type": "user",
            "parent_tool_use_id": "tu_123",
            "message": {"content": "Tool result"}
        });
        assert_eq!(extract_user_message_text(&msg), None);
    }

    #[test]
    fn test_session_capacity_basic() {
        let mut cap = SessionCapacity::new(2);
        assert!(cap.has_capacity());
        assert_eq!(cap.active_count(), 0);

        assert!(cap.register("session_1"));
        assert!(cap.has_capacity());
        assert_eq!(cap.active_count(), 1);

        assert!(cap.register("session_2"));
        assert!(!cap.has_capacity());
        assert_eq!(cap.active_count(), 2);

        // At capacity, register should fail
        assert!(!cap.register("session_3"));

        cap.remove("session_1");
        assert!(cap.has_capacity());
        assert_eq!(cap.active_count(), 1);
    }

    #[test]
    fn test_session_capacity_is_active() {
        let mut cap = SessionCapacity::new(4);
        cap.register("session_1");
        assert!(cap.is_active("session_1"));
        assert!(!cap.is_active("session_2"));
    }
}
