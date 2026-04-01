use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Result of a dream (memory consolidation) cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamResult {
    /// Number of memory files created or updated.
    pub memories_extracted: usize,
    /// Patterns or recurring themes noted across sessions.
    pub patterns_noted: Vec<String>,
    /// Files that were touched (created, updated, or pruned).
    pub files_touched: Vec<PathBuf>,
    /// Brief summary of what changed.
    pub summary: String,
}

/// Configuration for auto-dream scheduling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamConfig {
    /// Minimum hours between dream cycles.
    pub min_hours: f64,
    /// Minimum sessions since last consolidation before dreaming.
    pub min_sessions: usize,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            min_hours: 24.0,
            min_sessions: 5,
        }
    }
}

/// Memory consolidation engine. Runs as a background process during idle
/// periods to distill daily logs and session transcripts into durable,
/// well-organized memory files.
pub struct DreamMode {
    /// Root directory for memory files.
    memory_root: PathBuf,
    /// Directory containing session transcripts.
    transcript_dir: PathBuf,
    /// Configuration for dream scheduling.
    config: DreamConfig,
}

impl DreamMode {
    /// Create a new dream mode instance.
    pub fn new(
        memory_root: impl Into<PathBuf>,
        transcript_dir: impl Into<PathBuf>,
        config: DreamConfig,
    ) -> Self {
        Self {
            memory_root: memory_root.into(),
            transcript_dir: transcript_dir.into(),
            config,
        }
    }

    /// Get the dream configuration.
    pub fn config(&self) -> &DreamConfig {
        &self.config
    }

    /// Build the consolidation prompt that drives the dream cycle.
    ///
    /// This prompt instructs the agent to orient, gather recent signal,
    /// consolidate memories, and prune the index.
    pub fn build_consolidation_prompt(&self, session_ids: &[String]) -> String {
        let memory_root = self.memory_root.display();
        let transcript_dir = self.transcript_dir.display();
        let session_list = session_ids
            .iter()
            .map(|id| format!("- {id}"))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"# Dream: Memory Consolidation

You are performing a dream — a reflective pass over your memory files. Synthesize what you've learned recently into durable, well-organized memories so that future sessions can orient quickly.

Memory directory: `{memory_root}`
This directory already exists — write to it directly with the Write tool (do not run mkdir or check for its existence).

Session transcripts: `{transcript_dir}` (large JSONL files — grep narrowly, don't read whole files)

---

## Phase 1 — Orient

- `ls` the memory directory to see what already exists
- Read `MEMORY.md` to understand the current index
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

Update `MEMORY.md` so it stays under 200 lines AND under ~25KB. It's an **index**, not a dump — each entry should be one line under ~150 characters: `- [Title](file.md) — one-line hook`. Never write memory content directly into it.

- Remove pointers to memories that are now stale, wrong, or superseded
- Demote verbose entries: if an index line is over ~200 chars, it's carrying content that belongs in the topic file — shorten the line, move the detail
- Add pointers to newly important memories
- Resolve contradictions — if two files disagree, fix the wrong one

---

Return a brief summary of what you consolidated, updated, or pruned. If nothing changed (memories are already tight), say so.

## Additional context

**Tool constraints for this run:** Bash is restricted to read-only commands (`ls`, `find`, `grep`, `cat`, `stat`, `wc`, `head`, `tail`, and similar). Anything that writes, redirects to a file, or modifies state will be denied. Plan your exploration with this in mind — no need to probe.

Sessions since last consolidation ({session_count}):
{session_list}"#,
            session_count = session_ids.len(),
        )
    }

    /// Run a dream cycle, consolidating recent session history into memory.
    ///
    /// This is the main entry point for dream mode. It checks time and session
    /// gates, acquires a lock, and runs the consolidation prompt as a forked
    /// agent. The caller provides the list of session IDs to review.
    pub async fn run_dream_cycle(&self, session_ids: &[String]) -> Result<DreamResult> {
        info!(
            sessions = session_ids.len(),
            memory_root = %self.memory_root.display(),
            "starting dream cycle"
        );

        let prompt = self.build_consolidation_prompt(session_ids);
        debug!(prompt_len = prompt.len(), "built consolidation prompt");

        // The actual agent execution is handled by the caller (QueryEngine).
        // This method builds the prompt and returns a result structure.
        // The caller is responsible for:
        // 1. Running the prompt through a forked agent
        // 2. Collecting the files touched
        // 3. Populating the DreamResult

        // Return a placeholder that the caller fills in after agent execution.
        Ok(DreamResult {
            memories_extracted: 0,
            patterns_noted: vec![],
            files_touched: vec![],
            summary: prompt,
        })
    }

    /// Check if enough time has passed since the last consolidation.
    pub fn should_dream(&self, last_consolidated_at: chrono::DateTime<chrono::Utc>) -> bool {
        let hours_since = (chrono::Utc::now() - last_consolidated_at).num_minutes() as f64 / 60.0;
        hours_since >= self.config.min_hours
    }

    /// Read the last consolidation timestamp from the lock file.
    pub fn read_last_consolidated_at(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let lock_path = self.memory_root.join(".dream_lock");
        if !lock_path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&lock_path)
            .with_context(|| format!("failed to read dream lock {}", lock_path.display()))?;
        let ts = content
            .trim()
            .parse::<chrono::DateTime<chrono::Utc>>()
            .with_context(|| "failed to parse dream lock timestamp")?;
        Ok(Some(ts))
    }

    /// Write the current time as the last consolidation timestamp.
    pub fn write_consolidated_at(&self) -> Result<()> {
        let lock_path = self.memory_root.join(".dream_lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let now = chrono::Utc::now().to_rfc3339();
        std::fs::write(&lock_path, now)
            .with_context(|| format!("failed to write dream lock {}", lock_path.display()))?;
        debug!(path = %lock_path.display(), "updated dream lock timestamp");
        Ok(())
    }

    /// Try to acquire the consolidation lock. Returns the prior timestamp
    /// if the lock was acquired, or None if another process holds it.
    pub fn try_acquire_lock(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let pid_lock = self.memory_root.join(".dream_pid");

        // Check if another process is mid-consolidation.
        if pid_lock.exists() {
            let pid_str = std::fs::read_to_string(&pid_lock).unwrap_or_default();
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                if is_process_alive(pid) {
                    debug!(pid, "dream lock held by another process");
                    return Ok(None);
                }
            }
            // Stale lock — remove it.
            let _ = std::fs::remove_file(&pid_lock);
        }

        let prior = self.read_last_consolidated_at()?;

        // Write our PID.
        if let Some(parent) = pid_lock.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&pid_lock, std::process::id().to_string())?;

        Ok(prior)
    }

    /// Release the consolidation lock.
    pub fn release_lock(&self) {
        let pid_lock = self.memory_root.join(".dream_pid");
        let _ = std::fs::remove_file(&pid_lock);
    }

    /// Rollback the consolidation lock to a prior timestamp (on failure).
    pub fn rollback_lock(&self, prior: Option<chrono::DateTime<chrono::Utc>>) -> Result<()> {
        self.release_lock();
        if let Some(ts) = prior {
            let lock_path = self.memory_root.join(".dream_lock");
            std::fs::write(&lock_path, ts.to_rfc3339())?;
        }
        Ok(())
    }
}

/// Check if a process with the given PID is still alive.
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // signal 0 checks if the process exists without sending a signal.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On non-unix, assume the process might still be alive.
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dream_config_default() {
        let config = DreamConfig::default();
        assert_eq!(config.min_hours, 24.0);
        assert_eq!(config.min_sessions, 5);
    }

    #[test]
    fn test_build_consolidation_prompt() {
        let dream = DreamMode::new("/tmp/memory", "/tmp/transcripts", DreamConfig::default());
        let prompt =
            dream.build_consolidation_prompt(&["session-1".to_string(), "session-2".to_string()]);
        assert!(prompt.contains("Dream: Memory Consolidation"));
        assert!(prompt.contains("/tmp/memory"));
        assert!(prompt.contains("/tmp/transcripts"));
        assert!(prompt.contains("session-1"));
        assert!(prompt.contains("session-2"));
        assert!(prompt.contains("Sessions since last consolidation (2)"));
        assert!(prompt.contains("Phase 1"));
        assert!(prompt.contains("Phase 4"));
    }

    #[test]
    fn test_should_dream() {
        let dream = DreamMode::new(
            "/tmp/memory",
            "/tmp/transcripts",
            DreamConfig {
                min_hours: 1.0,
                min_sessions: 1,
            },
        );
        let two_hours_ago = chrono::Utc::now() - chrono::Duration::hours(2);
        assert!(dream.should_dream(two_hours_ago));

        let five_minutes_ago = chrono::Utc::now() - chrono::Duration::minutes(5);
        assert!(!dream.should_dream(five_minutes_ago));
    }

    #[test]
    fn test_lock_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let dream = DreamMode::new(dir.path(), "/tmp/transcripts", DreamConfig::default());

        // No lock exists initially.
        assert!(dream.read_last_consolidated_at().unwrap().is_none());

        // Write a timestamp.
        dream.write_consolidated_at().unwrap();
        assert!(dream.read_last_consolidated_at().unwrap().is_some());

        // Acquire lock.
        let prior = dream.try_acquire_lock().unwrap();
        assert!(prior.is_some());

        // Release lock.
        dream.release_lock();
    }
}
