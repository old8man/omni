/// Background memory consolidation ("auto-dream"). Fires the dream prompt as
/// a forked subagent when a time-gate passes AND enough sessions have accumulated
/// since the last consolidation.
///
/// Gate order (cheapest first):
///   1. Time: hours since lastConsolidatedAt >= min_hours (one stat)
///   2. Scan throttle: don't re-scan sessions within 10 minutes
///   3. Sessions: transcript count with mtime > lastConsolidatedAt >= min_sessions
///   4. Lock: no other process mid-consolidation
///
/// State is closure-scoped inside [`AutoDreamState`] (tests create a fresh
/// instance per test for isolation).
///
/// Port of `services/autoDream/autoDream.ts`, `consolidationPrompt.ts`,
/// `consolidationLock.ts`, and `config.ts`.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

// ── Configuration ──────────────────────────────────────────────────────────

/// Scheduling thresholds for auto-dream consolidation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutoDreamConfig {
    /// Minimum hours since last consolidation before triggering.
    pub min_hours: f64,
    /// Minimum number of sessions (excluding current) since last consolidation.
    pub min_sessions: usize,
    /// Whether auto-dream is enabled (user or feature-flag override).
    pub enabled: bool,
}

impl Default for AutoDreamConfig {
    fn default() -> Self {
        Self {
            min_hours: 24.0,
            min_sessions: 5,
            enabled: false,
        }
    }
}

// ── Scan throttle ──────────────────────────────────────────────────────────

/// Don't re-scan sessions more often than this when the time-gate passes
/// but the session-gate doesn't.
const SESSION_SCAN_INTERVAL: Duration = Duration::from_secs(10 * 60);

// ── Consolidation lock ─────────────────────────────────────────────────────

/// Lock file name (lives inside the memory dir).
const LOCK_FILE: &str = ".consolidate-lock";

/// If a lock holder has been running longer than this, consider it stale
/// even if the PID is alive (PID reuse guard).
const HOLDER_STALE: Duration = Duration::from_secs(60 * 60);

/// Get the path to the consolidation lock file.
pub fn lock_path(memory_dir: &Path) -> PathBuf {
    memory_dir.join(LOCK_FILE)
}

/// Read the last consolidation timestamp (mtime of the lock file).
/// Returns 0 (UNIX epoch) if the file doesn't exist.
pub async fn read_last_consolidated_at(memory_dir: &Path) -> Result<u64> {
    let path = lock_path(memory_dir);
    match tokio::fs::metadata(&path).await {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .context("reading lock file mtime")?
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            Ok(mtime)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e).context("stat consolidation lock"),
    }
}

/// Try to acquire the consolidation lock.
///
/// Writes the current PID to the lock file. Returns `Ok(Some(prior_mtime_ms))`
/// on success (the mtime before acquisition, for rollback), or `Ok(None)` if
/// another process holds the lock.
pub async fn try_acquire_consolidation_lock(memory_dir: &Path) -> Result<Option<u64>> {
    let path = lock_path(memory_dir);

    let (mtime_ms, holder_pid) = match tokio::fs::metadata(&path).await {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .unwrap_or(UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let pid: Option<u32> = tokio::fs::read_to_string(&path)
                .await
                .ok()
                .and_then(|s| s.trim().parse().ok());

            (Some(mtime), pid)
        }
        Err(_) => (None, None),
    };

    // Check if a live process holds the lock
    if let Some(mt) = mtime_ms {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let age = Duration::from_millis(now_ms.saturating_sub(mt));

        if age < HOLDER_STALE {
            if let Some(pid) = holder_pid {
                if is_process_running(pid) {
                    debug!(
                        pid,
                        age_secs = age.as_secs(),
                        "[autoDream] lock held by live process"
                    );
                    return Ok(None);
                }
            }
            // Dead PID or unparseable body: reclaim.
        }
    }

    // Create memory dir if needed
    tokio::fs::create_dir_all(memory_dir).await.ok();

    let pid = std::process::id();
    tokio::fs::write(&path, pid.to_string())
        .await
        .context("writing consolidation lock")?;

    // Verify we won the race
    let verify = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    if verify.trim().parse::<u32>().ok() != Some(pid) {
        return Ok(None); // Lost the race
    }

    Ok(Some(mtime_ms.unwrap_or(0)))
}

/// Rollback the lock by either deleting it (if prior mtime was 0) or
/// clearing the PID body and resetting the mtime.
pub async fn rollback_consolidation_lock(memory_dir: &Path, prior_mtime_ms: u64) -> Result<()> {
    let path = lock_path(memory_dir);

    if prior_mtime_ms == 0 {
        tokio::fs::remove_file(&path).await.ok();
        return Ok(());
    }

    // Clear PID body
    tokio::fs::write(&path, "").await.ok();

    // Reset mtime using filetime-like approach
    #[cfg(unix)]
    {
        let secs = (prior_mtime_ms / 1000) as i64;
        let nsecs = ((prior_mtime_ms % 1000) * 1_000_000) as i64;
        let times = [
            libc::timespec {
                tv_sec: secs,
                tv_nsec: nsecs,
            },
            libc::timespec {
                tv_sec: secs,
                tv_nsec: nsecs,
            },
        ];
        let c_path = std::ffi::CString::new(path.to_str().unwrap_or_default())
            .unwrap_or_default();
        unsafe {
            libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0);
        }
    }

    Ok(())
}

/// Record a consolidation timestamp (called after manual /dream).
pub async fn record_consolidation(memory_dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(memory_dir).await.ok();
    let path = lock_path(memory_dir);
    let pid = std::process::id();
    tokio::fs::write(&path, pid.to_string())
        .await
        .context("recording consolidation timestamp")?;
    Ok(())
}

/// List session IDs with mtime after `since_ms`.
pub async fn list_sessions_touched_since(
    transcript_dir: &Path,
    since_ms: u64,
) -> Result<Vec<String>> {
    let mut sessions = Vec::new();

    let mut entries = tokio::fs::read_dir(transcript_dir)
        .await
        .context("reading transcript directory")?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();

        // Only consider .jsonl files
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        // Skip agent transcripts (agent-*.jsonl)
        if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
            if name.starts_with("agent-") {
                continue;
            }

            let meta = tokio::fs::metadata(&path).await?;
            let mtime = meta
                .modified()
                .unwrap_or(UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            if mtime > since_ms {
                sessions.push(name.to_string());
            }
        }
    }

    Ok(sessions)
}

// ── Check if process is running ────────────────────────────────────────────

/// Check if a process with the given PID is currently running.
fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) checks existence without sending a signal
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On non-Unix, assume the process is dead (conservative)
        false
    }
}

// ── Consolidation prompt ───────────────────────────────────────────────────

/// Entrypoint file name for memory index.
const ENTRYPOINT_NAME: &str = "MEMORY.md";

/// Maximum lines for the entrypoint index.
const MAX_ENTRYPOINT_LINES: usize = 200;

/// Build the consolidation prompt for the dream subagent.
pub fn build_consolidation_prompt(
    memory_root: &str,
    transcript_dir: &str,
    extra: &str,
) -> String {
    format!(
        r#"# Dream: Memory Consolidation

You are performing a dream — a reflective pass over your memory files. Synthesize what you've learned recently into durable, well-organized memories so that future sessions can orient quickly.

Memory directory: `{memory_root}`
If the directory already exists, work with its current structure. If it does not exist, create it.

Session transcripts: `{transcript_dir}` (large JSONL files — grep narrowly, don't read whole files)

---

## Phase 1 — Orient

- `ls` the memory directory to see what already exists
- Read `{entrypoint}` to understand the current index
- Skim existing topic files so you improve them rather than creating duplicates
- If `logs/` or `sessions/` subdirectories exist (assistant-mode layout), review recent entries there

## Phase 2 — Gather recent signal

Look for new information worth persisting. Sources in rough priority order:

1. **Daily logs** (`logs/YYYY/MM/YYYY-MM-DD.md`) if present — these are the append-only stream
2. **Existing memories that drifted** — facts that contradict something you see in the codebase now
3. **Transcript search** — if you need specific context (e.g., "what was the error message from yesterday's build failure?"), grep the JSONL transcripts for narrow terms:
   `grep -rn "<narrow term>" {transcript_dir}/ --include="*.jsonl" | tail -50`

Don't exhaustively read transcripts. Look only for things you already suspect matter.

## Phase 3 — Consolidate

For each thing worth remembering, write or update a memory file at the top level of the memory directory. Use the memory file format and type conventions from your system prompt's auto-memory section — it's the source of truth for what to save, how to structure it, and what NOT to save.

Focus on:
- Merging new signal into existing topic files rather than creating near-duplicates
- Converting relative dates ("yesterday", "last week") to absolute dates so they remain interpretable after time passes
- Deleting contradicted facts — if today's investigation disproves an old memory, fix it at the source

## Phase 4 — Prune and index

Update `{entrypoint}` so it stays under {max_lines} lines AND under ~25KB. It's an **index**, not a dump — each entry should be one line under ~150 characters: `- [Title](file.md) — one-line hook`. Never write memory content directly into it.

- Remove pointers to memories that are now stale, wrong, or superseded
- Demote verbose entries: if an index line is over ~200 chars, it's carrying content that belongs in the topic file — shorten the line, move the detail
- Add pointers to newly important memories
- Resolve contradictions — if two files disagree, fix the wrong one

---

Return a brief summary of what you consolidated, updated, or pruned. If nothing changed (memories are already tight), say so.{extra_section}"#,
        memory_root = memory_root,
        transcript_dir = transcript_dir,
        entrypoint = ENTRYPOINT_NAME,
        max_lines = MAX_ENTRYPOINT_LINES,
        extra_section = if extra.is_empty() {
            String::new()
        } else {
            format!("\n\n## Additional context\n\n{}", extra)
        },
    )
}

// ── Auto-dream state ───────────────────────────────────────────────────────

/// Mutable state for the auto-dream system. Create one per session.
pub struct AutoDreamState {
    /// Config for scheduling thresholds.
    config: Mutex<AutoDreamConfig>,
    /// Timestamp of the last session scan (millis since epoch).
    last_session_scan_at: Mutex<u64>,
    /// Whether a dream is currently in progress.
    in_progress: AtomicBool,
}

impl AutoDreamState {
    pub fn new(config: AutoDreamConfig) -> Self {
        Self {
            config: Mutex::new(config),
            last_session_scan_at: Mutex::new(0),
            in_progress: AtomicBool::new(false),
        }
    }

    pub fn config(&self) -> AutoDreamConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn set_config(&self, config: AutoDreamConfig) {
        *self.config.lock().unwrap() = config;
    }

    pub fn is_in_progress(&self) -> bool {
        self.in_progress.load(Ordering::Acquire)
    }

    /// Check the time gate: have enough hours passed since last consolidation?
    pub fn check_time_gate(&self, last_consolidated_at_ms: u64) -> Option<f64> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let hours_since = (now_ms - last_consolidated_at_ms) as f64 / 3_600_000.0;
        let min_hours = self.config.lock().unwrap().min_hours;
        if hours_since >= min_hours {
            Some(hours_since)
        } else {
            None
        }
    }

    /// Check the scan throttle: have we scanned too recently?
    pub fn check_scan_throttle(&self) -> bool {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut last_scan = self.last_session_scan_at.lock().unwrap();
        let since_ms = now_ms.saturating_sub(*last_scan);
        if since_ms < SESSION_SCAN_INTERVAL.as_millis() as u64 {
            debug!(
                since_secs = since_ms / 1000,
                "[autoDream] scan throttle — too recent"
            );
            return false;
        }
        *last_scan = now_ms;
        true
    }

    /// Check the session gate: have enough sessions accumulated?
    pub fn check_session_gate(
        &self,
        session_count: usize,
    ) -> bool {
        let min = self.config.lock().unwrap().min_sessions;
        if session_count < min {
            debug!(
                session_count,
                min_sessions = min,
                "[autoDream] not enough sessions"
            );
            return false;
        }
        true
    }

    /// Mark dream as in-progress. Returns false if already running.
    pub fn try_start(&self) -> bool {
        self.in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Mark dream as complete.
    pub fn finish(&self) {
        self.in_progress.store(false, Ordering::Release);
    }

    /// Reset all state (for tests).
    pub fn reset(&self) {
        *self.last_session_scan_at.lock().unwrap() = 0;
        self.in_progress.store(false, Ordering::Release);
    }
}

impl Default for AutoDreamState {
    fn default() -> Self {
        Self::new(AutoDreamConfig::default())
    }
}

/// Build the extra context string listing tool constraints and session IDs.
pub fn build_dream_extra_context(session_ids: &[String]) -> String {
    format!(
        "\n\n**Tool constraints for this run:** Bash is restricted to read-only commands \
         (`ls`, `find`, `grep`, `cat`, `stat`, `wc`, `head`, `tail`, and similar). \
         Anything that writes, redirects to a file, or modifies state will be denied. \
         Plan your exploration with this in mind — no need to probe.\n\n\
         Sessions since last consolidation ({count}):\n{list}",
        count = session_ids.len(),
        list = session_ids
            .iter()
            .map(|id| format!("- {}", id))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_auto_dream_config_defaults() {
        let config = AutoDreamConfig::default();
        assert_eq!(config.min_hours, 24.0);
        assert_eq!(config.min_sessions, 5);
        assert!(!config.enabled);
    }

    #[test]
    fn test_build_consolidation_prompt() {
        let prompt = build_consolidation_prompt(
            "/home/user/.claude/memory",
            "/home/user/.claude/projects/abc/sessions",
            "",
        );
        assert!(prompt.contains("Dream: Memory Consolidation"));
        assert!(prompt.contains("/home/user/.claude/memory"));
        assert!(prompt.contains("MEMORY.md"));
        assert!(prompt.contains("Phase 1"));
        assert!(prompt.contains("Phase 4"));
    }

    #[test]
    fn test_build_consolidation_prompt_with_extra() {
        let prompt = build_consolidation_prompt(
            "/mem",
            "/transcripts",
            "5 sessions to review",
        );
        assert!(prompt.contains("## Additional context"));
        assert!(prompt.contains("5 sessions to review"));
    }

    #[test]
    fn test_build_dream_extra_context() {
        let sessions = vec!["sess-1".to_string(), "sess-2".to_string()];
        let extra = build_dream_extra_context(&sessions);
        assert!(extra.contains("Tool constraints"));
        assert!(extra.contains("Sessions since last consolidation (2)"));
        assert!(extra.contains("- sess-1"));
        assert!(extra.contains("- sess-2"));
    }

    #[test]
    fn test_auto_dream_state_time_gate() {
        let config = AutoDreamConfig {
            min_hours: 1.0,
            min_sessions: 1,
            enabled: true,
        };
        let state = AutoDreamState::new(config);

        // Last consolidated 2 hours ago — should pass
        let two_hours_ago_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 2 * 3_600_000;
        let result = state.check_time_gate(two_hours_ago_ms);
        assert!(result.is_some());
        assert!(result.unwrap() >= 1.9);

        // Last consolidated 30 min ago — should fail
        let thirty_min_ago_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 30 * 60_000;
        assert!(state.check_time_gate(thirty_min_ago_ms).is_none());
    }

    #[test]
    fn test_auto_dream_state_session_gate() {
        let config = AutoDreamConfig {
            min_hours: 1.0,
            min_sessions: 3,
            enabled: true,
        };
        let state = AutoDreamState::new(config);

        assert!(!state.check_session_gate(2));
        assert!(state.check_session_gate(3));
        assert!(state.check_session_gate(10));
    }

    #[test]
    fn test_auto_dream_state_try_start() {
        let state = AutoDreamState::default();
        assert!(state.try_start());
        assert!(state.is_in_progress());
        assert!(!state.try_start()); // Already running
        state.finish();
        assert!(!state.is_in_progress());
        assert!(state.try_start()); // Can start again
    }

    #[tokio::test]
    async fn test_read_last_consolidated_at_no_file() {
        let dir = TempDir::new().unwrap();
        let result = read_last_consolidated_at(dir.path()).await.unwrap();
        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn test_record_and_read_consolidation() {
        let dir = TempDir::new().unwrap();
        let mem_dir = dir.path().join("memory");

        record_consolidation(&mem_dir).await.unwrap();

        let timestamp = read_last_consolidated_at(&mem_dir).await.unwrap();
        assert!(timestamp > 0);

        // Verify the lock file contains our PID
        let content = tokio::fs::read_to_string(lock_path(&mem_dir))
            .await
            .unwrap();
        assert_eq!(content.trim().parse::<u32>().unwrap(), std::process::id());
    }

    #[tokio::test]
    async fn test_try_acquire_lock_fresh() {
        let dir = TempDir::new().unwrap();
        let mem_dir = dir.path().join("memory");
        tokio::fs::create_dir_all(&mem_dir).await.unwrap();

        let result = try_acquire_consolidation_lock(&mem_dir).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 0); // No prior mtime
    }

    #[tokio::test]
    async fn test_rollback_consolidation_lock_no_prior() {
        let dir = TempDir::new().unwrap();
        let mem_dir = dir.path().join("memory");
        tokio::fs::create_dir_all(&mem_dir).await.unwrap();

        // Create a lock file
        tokio::fs::write(lock_path(&mem_dir), "12345").await.unwrap();

        // Rollback with prior_mtime 0 should delete the file
        rollback_consolidation_lock(&mem_dir, 0).await.unwrap();
        assert!(!lock_path(&mem_dir).exists());
    }

    #[tokio::test]
    async fn test_list_sessions_touched_since() {
        let dir = TempDir::new().unwrap();

        // Create some session files
        tokio::fs::write(dir.path().join("session-1.jsonl"), "{}").await.unwrap();
        tokio::fs::write(dir.path().join("session-2.jsonl"), "{}").await.unwrap();
        tokio::fs::write(dir.path().join("agent-foo.jsonl"), "{}").await.unwrap();
        tokio::fs::write(dir.path().join("notes.txt"), "text").await.unwrap();

        // All sessions should have mtime > 0
        let sessions = list_sessions_touched_since(dir.path(), 0).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&"session-1".to_string()));
        assert!(sessions.contains(&"session-2".to_string()));
        // agent-foo.jsonl should be excluded
        assert!(!sessions.contains(&"agent-foo".to_string()));
    }

    #[test]
    fn test_lock_path() {
        let mem_dir = Path::new("/home/user/.claude/memory");
        assert_eq!(
            lock_path(mem_dir),
            PathBuf::from("/home/user/.claude/memory/.consolidate-lock")
        );
    }

    #[test]
    fn test_is_process_running() {
        // Our own process should be running
        assert!(is_process_running(std::process::id()));
        // A very high PID is almost certainly not running
        assert!(!is_process_running(u32::MAX - 1));
    }
}
