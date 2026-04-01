/// Periodic background summarization for coordinator mode sub-agents.
///
/// Forks the sub-agent's conversation every ~30s to generate a 1-2 sentence
/// progress summary. The summary is stored per-agent for UI display.
///
/// Port of `services/AgentSummary/agentSummary.ts`.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::{debug, warn};

// ── Constants ──────────────────────────────────────────────────────────────

/// Interval between summary generation attempts.
const SUMMARY_INTERVAL: Duration = Duration::from_secs(30);

/// Minimum number of messages required before generating a summary.
const MIN_MESSAGES_FOR_SUMMARY: usize = 3;

// ── Summary prompt ─────────────────────────────────────────────────────────

/// Build the prompt sent to the forked agent to generate a summary.
pub fn build_summary_prompt(previous_summary: Option<&str>) -> String {
    let prev_line = match previous_summary {
        Some(s) => format!("\nPrevious: \"{}\" — say something NEW.\n", s),
        None => String::new(),
    };

    format!(
        "Describe your most recent action in 3-5 words using present tense (-ing). \
         Name the file or function, not the branch. Do not use tools.\n\
         {}\n\
         Good: \"Reading runAgent.ts\"\n\
         Good: \"Fixing null check in validate.ts\"\n\
         Good: \"Running auth module tests\"\n\
         Good: \"Adding retry logic to fetchUser\"\n\n\
         Bad (past tense): \"Analyzed the branch diff\"\n\
         Bad (too vague): \"Investigating the issue\"\n\
         Bad (too long): \"Reviewing full branch diff and AgentTool.tsx integration\"\n\
         Bad (branch name): \"Analyzed adam/background-summary branch diff\"",
        prev_line
    )
}

// ── Types ──────────────────────────────────────────────────────────────────

/// Unique identifier for a sub-agent.
pub type AgentId = String;

/// A snapshot of one agent's latest summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSummaryEntry {
    /// The short progress text (3-5 words, present tense).
    pub text: String,
    /// When this summary was generated (epoch millis).
    pub generated_at_ms: u64,
}

/// Aggregate state holding every active agent's most recent summary.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentSummaryState {
    /// Map from agent/task ID to its latest summary.
    pub summaries: HashMap<String, AgentSummaryEntry>,
}

impl AgentSummaryState {
    pub fn new() -> Self {
        Self {
            summaries: HashMap::new(),
        }
    }

    /// Record a new summary for the given task.
    pub fn update(&mut self, task_id: &str, text: String) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.summaries.insert(
            task_id.to_string(),
            AgentSummaryEntry {
                text,
                generated_at_ms: now,
            },
        );
    }

    /// Remove a completed/stopped agent's summary.
    pub fn remove(&mut self, task_id: &str) {
        self.summaries.remove(task_id);
    }

    /// Get the latest summary for a given task, if any.
    pub fn get(&self, task_id: &str) -> Option<&AgentSummaryEntry> {
        self.summaries.get(task_id)
    }
}

// ── Summarization handle ───────────────────────────────────────────────────

/// Handle returned by [`start_agent_summarization`]. Call `stop()` to cancel
/// the background timer and abort any in-flight summary generation.
pub struct AgentSummarizationHandle {
    stopped: Arc<AtomicBool>,
    /// Send a signal to stop the background loop.
    _stop_tx: watch::Sender<bool>,
}

impl AgentSummarizationHandle {
    /// Stop the periodic summarization. Idempotent.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self._stop_tx.send(true);
    }
}

/// Callback type for updating app state with a new summary.
pub type UpdateSummaryFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Callback type that retrieves the current message count for an agent.
/// Returns the number of messages currently in the agent's transcript.
pub type GetMessageCountFn = Arc<dyn Fn(&str) -> usize + Send + Sync>;

/// Callback type that generates a summary given an agent id and prompt.
/// Returns `Ok(Some(text))` on success, `Ok(None)` if skipped, `Err` on failure.
pub type GenerateSummaryFn =
    Arc<dyn Fn(String, String) -> tokio::task::JoinHandle<anyhow::Result<Option<String>>> + Send + Sync>;

/// Start periodic summarization for a single sub-agent.
///
/// The caller provides callbacks for:
/// - `update_summary`: persist the summary text to app state
/// - `get_message_count`: return the current transcript length
/// - `generate_summary`: run the forked model call (async)
///
/// Returns a handle whose `stop()` method cancels the timer.
pub fn start_agent_summarization(
    task_id: String,
    agent_id: AgentId,
    update_summary: UpdateSummaryFn,
    get_message_count: GetMessageCountFn,
    generate_summary: GenerateSummaryFn,
) -> AgentSummarizationHandle {
    let stopped = Arc::new(AtomicBool::new(false));
    let (stop_tx, mut stop_rx) = watch::channel(false);

    let stopped_clone = stopped.clone();
    let task_id_clone = task_id.clone();

    tokio::spawn(async move {
        let mut previous_summary: Option<String> = None;

        loop {
            // Wait for interval or stop signal
            tokio::select! {
                _ = tokio::time::sleep(SUMMARY_INTERVAL) => {}
                _ = stop_rx.changed() => {
                    debug!(task_id = %task_id_clone, "[AgentSummary] stopped via signal");
                    return;
                }
            }

            if stopped_clone.load(Ordering::Acquire) {
                return;
            }

            let msg_count = get_message_count(&agent_id);
            if msg_count < MIN_MESSAGES_FOR_SUMMARY {
                debug!(
                    task_id = %task_id_clone,
                    msg_count,
                    "[AgentSummary] skipping — not enough messages"
                );
                continue;
            }

            let prompt = build_summary_prompt(previous_summary.as_deref());

            debug!(
                task_id = %task_id_clone,
                msg_count,
                "[AgentSummary] generating summary"
            );

            let handle = generate_summary(agent_id.clone(), prompt);

            match handle.await {
                Ok(Ok(Some(text))) => {
                    if stopped_clone.load(Ordering::Acquire) {
                        return;
                    }
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        debug!(
                            task_id = %task_id_clone,
                            summary = %trimmed,
                            "[AgentSummary] summary generated"
                        );
                        update_summary(&task_id_clone, &trimmed);
                        previous_summary = Some(trimmed);
                    }
                }
                Ok(Ok(None)) => {
                    debug!(task_id = %task_id_clone, "[AgentSummary] no summary produced");
                }
                Ok(Err(e)) => {
                    if !stopped_clone.load(Ordering::Acquire) {
                        warn!(task_id = %task_id_clone, error = %e, "[AgentSummary] generation failed");
                    }
                }
                Err(e) => {
                    if !stopped_clone.load(Ordering::Acquire) {
                        warn!(task_id = %task_id_clone, error = %e, "[AgentSummary] task panicked");
                    }
                }
            }
        }
    });

    AgentSummarizationHandle {
        stopped,
        _stop_tx: stop_tx,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_summary_prompt_no_previous() {
        let prompt = build_summary_prompt(None);
        assert!(prompt.contains("Describe your most recent action"));
        assert!(!prompt.contains("Previous:"));
    }

    #[test]
    fn test_build_summary_prompt_with_previous() {
        let prompt = build_summary_prompt(Some("Reading main.rs"));
        assert!(prompt.contains("Previous: \"Reading main.rs\""));
        assert!(prompt.contains("say something NEW"));
    }

    #[test]
    fn test_agent_summary_state_crud() {
        let mut state = AgentSummaryState::new();

        assert!(state.get("task-1").is_none());

        state.update("task-1", "Reading config.rs".to_string());
        let entry = state.get("task-1").unwrap();
        assert_eq!(entry.text, "Reading config.rs");
        assert!(entry.generated_at_ms > 0);

        state.update("task-1", "Fixing validation logic".to_string());
        assert_eq!(state.get("task-1").unwrap().text, "Fixing validation logic");

        state.remove("task-1");
        assert!(state.get("task-1").is_none());
    }

    #[test]
    fn test_agent_summary_state_multiple_agents() {
        let mut state = AgentSummaryState::new();

        state.update("agent-a", "Reading auth.rs".to_string());
        state.update("agent-b", "Running tests".to_string());
        state.update("agent-c", "Editing config.rs".to_string());

        assert_eq!(state.summaries.len(), 3);
        assert_eq!(state.get("agent-a").unwrap().text, "Reading auth.rs");
        assert_eq!(state.get("agent-b").unwrap().text, "Running tests");
        assert_eq!(state.get("agent-c").unwrap().text, "Editing config.rs");

        state.remove("agent-b");
        assert_eq!(state.summaries.len(), 2);
        assert!(state.get("agent-b").is_none());
    }

    #[test]
    fn test_agent_summary_state_default() {
        let state = AgentSummaryState::default();
        assert!(state.summaries.is_empty());
    }

    #[tokio::test]
    async fn test_summarization_handle_stop() {
        let stopped = Arc::new(AtomicBool::new(false));
        let (stop_tx, _stop_rx) = watch::channel(false);

        let handle = AgentSummarizationHandle {
            stopped: stopped.clone(),
            _stop_tx: stop_tx,
        };

        assert!(!stopped.load(Ordering::Acquire));
        handle.stop();
        assert!(stopped.load(Ordering::Acquire));

        // Idempotent
        handle.stop();
        assert!(stopped.load(Ordering::Acquire));
    }
}
