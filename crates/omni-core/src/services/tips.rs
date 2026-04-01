use std::collections::HashMap;
use std::sync::Mutex;

use tracing::debug;

// ── Tip ─────────────────────────────────────────────────────────────────────

/// A tip that can be displayed to the user.
#[derive(Clone, Debug)]
pub struct Tip {
    /// Unique identifier for the tip.
    pub id: String,
    /// The tip content to display.
    pub content: String,
    /// Category for grouping/filtering.
    pub category: TipCategory,
    /// Display priority (lower = more important).
    pub priority: u32,
    /// Minimum sessions between re-displays of this tip.
    pub cooldown_sessions: u32,
}

/// Category of a tip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TipCategory {
    Workflow,
    Feature,
    Keyboard,
    Integration,
    General,
}

// ── Tip History ─────────────────────────────────────────────────────────────

/// Tracks when tips were last shown to avoid redundant display.
pub struct TipHistory {
    /// Maps tip ID to the session number when it was last shown.
    shown: Mutex<HashMap<String, u64>>,
    /// Current session number.
    current_session: u64,
}

impl TipHistory {
    pub fn new(current_session: u64) -> Self {
        Self {
            shown: Mutex::new(HashMap::new()),
            current_session,
        }
    }

    /// Load tip history from a previously persisted map (e.g., from global config).
    pub fn from_persisted(current_session: u64, history: HashMap<String, u64>) -> Self {
        Self {
            shown: Mutex::new(history),
            current_session,
        }
    }

    /// Record that a tip was shown in the current session.
    pub fn record_shown(&self, tip_id: &str) {
        self.shown
            .lock()
            .unwrap()
            .insert(tip_id.to_string(), self.current_session);
        debug!(tip_id, session = self.current_session, "recorded tip shown");
    }

    /// Get the number of sessions since a tip was last shown.
    /// Returns `u64::MAX` if the tip has never been shown.
    pub fn sessions_since_last_shown(&self, tip_id: &str) -> u64 {
        let shown = self.shown.lock().unwrap();
        match shown.get(tip_id) {
            Some(&last) => self.current_session.saturating_sub(last),
            None => u64::MAX,
        }
    }

    /// Export the history for persistence.
    pub fn to_persisted(&self) -> HashMap<String, u64> {
        self.shown.lock().unwrap().clone()
    }
}

// ── Tip Registry ────────────────────────────────────────────────────────────

/// Stores the full set of available tips.
pub struct TipRegistry {
    tips: Vec<Tip>,
}

impl TipRegistry {
    pub fn new() -> Self {
        Self {
            tips: Vec::new(),
        }
    }

    /// Register a new tip.
    pub fn register(&mut self, tip: Tip) {
        self.tips.push(tip);
    }

    /// Register multiple tips at once.
    pub fn register_all(&mut self, tips: impl IntoIterator<Item = Tip>) {
        self.tips.extend(tips);
    }

    /// Get all registered tips.
    pub fn all(&self) -> &[Tip] {
        &self.tips
    }

    /// Filter tips to those that have cooled down enough according to the
    /// provided history.
    pub fn available_tips(&self, history: &TipHistory) -> Vec<&Tip> {
        self.tips
            .iter()
            .filter(|tip| {
                history.sessions_since_last_shown(&tip.id) >= tip.cooldown_sessions as u64
            })
            .collect()
    }
}

impl Default for TipRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tip Scheduler ───────────────────────────────────────────────────────────

/// Determines which tip to show based on history and relevance.
pub struct TipScheduler {
    registry: TipRegistry,
    history: TipHistory,
}

impl TipScheduler {
    pub fn new(registry: TipRegistry, history: TipHistory) -> Self {
        Self { registry, history }
    }

    /// Select the tip that has not been shown for the longest time.
    ///
    /// Returns `None` if no tips are available (all on cooldown or registry
    /// is empty).
    pub fn select_tip(&self) -> Option<&Tip> {
        let available = self.registry.available_tips(&self.history);
        if available.is_empty() {
            return None;
        }

        // Pick the one with the longest gap since last shown
        available
            .into_iter()
            .max_by_key(|tip| self.history.sessions_since_last_shown(&tip.id))
    }

    /// Select a tip and record it as shown.
    pub fn show_tip(&self) -> Option<&Tip> {
        let tip = self.select_tip()?;
        self.history.record_shown(&tip.id);
        Some(tip)
    }

    /// Access the underlying history.
    pub fn history(&self) -> &TipHistory {
        &self.history
    }

    /// Access the underlying registry.
    pub fn registry(&self) -> &TipRegistry {
        &self.registry
    }
}

// ── Default tips ────────────────────────────────────────────────────────────

/// Build the default set of tips matching the TS implementation.
pub fn default_tips() -> Vec<Tip> {
    vec![
        Tip {
            id: "plan-mode-for-complex-tasks".into(),
            content: "Use Plan Mode to prepare for a complex request before making changes. Press shift+tab twice to enable.".into(),
            category: TipCategory::Feature,
            priority: 5,
            cooldown_sessions: 5,
        },
        Tip {
            id: "git-worktrees".into(),
            content: "Use git worktrees to run multiple Claude sessions in parallel.".into(),
            category: TipCategory::Workflow,
            priority: 10,
            cooldown_sessions: 10,
        },
        Tip {
            id: "shift-enter".into(),
            content: "Press Shift+Enter to send a multi-line message".into(),
            category: TipCategory::Keyboard,
            priority: 10,
            cooldown_sessions: 10,
        },
        Tip {
            id: "memory-command".into(),
            content: "Use /memory to view and manage Claude memory".into(),
            category: TipCategory::Feature,
            priority: 15,
            cooldown_sessions: 15,
        },
        Tip {
            id: "custom-commands".into(),
            content: "Create skills by adding .md files to .claude/skills/ in your project or ~/.claude/skills/ for skills that work in any project".into(),
            category: TipCategory::Feature,
            priority: 15,
            cooldown_sessions: 15,
        },
        Tip {
            id: "prompt-queue".into(),
            content: "Hit Enter to queue up additional messages while Claude is working.".into(),
            category: TipCategory::Feature,
            priority: 5,
            cooldown_sessions: 5,
        },
        Tip {
            id: "double-esc".into(),
            content: "Double-tap esc to rewind the conversation to a previous point in time".into(),
            category: TipCategory::Keyboard,
            priority: 10,
            cooldown_sessions: 10,
        },
        Tip {
            id: "continue".into(),
            content: "Run claude --continue or claude --resume to resume a conversation".into(),
            category: TipCategory::Feature,
            priority: 10,
            cooldown_sessions: 10,
        },
        Tip {
            id: "permissions".into(),
            content: "Use /permissions to pre-approve and pre-deny bash, edit, and MCP tools".into(),
            category: TipCategory::Feature,
            priority: 10,
            cooldown_sessions: 10,
        },
        Tip {
            id: "drag-and-drop-images".into(),
            content: "Did you know you can drag and drop image files into your terminal?".into(),
            category: TipCategory::Feature,
            priority: 10,
            cooldown_sessions: 10,
        },
        Tip {
            id: "todo-list".into(),
            content: "Ask Claude to create a todo list when working on complex tasks to track progress and remain on track".into(),
            category: TipCategory::Workflow,
            priority: 20,
            cooldown_sessions: 20,
        },
        Tip {
            id: "shift-tab".into(),
            content: "Hit shift+tab to cycle between default mode, auto-accept edit mode, and plan mode".into(),
            category: TipCategory::Keyboard,
            priority: 10,
            cooldown_sessions: 10,
        },
        Tip {
            id: "theme-command".into(),
            content: "Use /theme to change the color theme".into(),
            category: TipCategory::Feature,
            priority: 20,
            cooldown_sessions: 20,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tip_selection_prefers_least_recently_shown() {
        let mut registry = TipRegistry::new();
        registry.register(Tip {
            id: "a".into(),
            content: "tip a".into(),
            category: TipCategory::General,
            priority: 1,
            cooldown_sessions: 0,
        });
        registry.register(Tip {
            id: "b".into(),
            content: "tip b".into(),
            category: TipCategory::General,
            priority: 1,
            cooldown_sessions: 0,
        });

        let history = TipHistory::new(10);
        history.record_shown("a"); // shown at session 10

        let scheduler = TipScheduler::new(registry, history);
        // "b" has never been shown, so it should be selected
        let tip = scheduler.select_tip().unwrap();
        assert_eq!(tip.id, "b");
    }

    #[test]
    fn test_cooldown_respected() {
        let mut registry = TipRegistry::new();
        registry.register(Tip {
            id: "a".into(),
            content: "tip a".into(),
            category: TipCategory::General,
            priority: 1,
            cooldown_sessions: 5,
        });

        let history = TipHistory::new(3);
        history.record_shown("a"); // shown at session 3, cooldown=5

        let scheduler = TipScheduler::new(registry, history);
        // Session 3, shown at 3, diff=0 < cooldown=5 → no tip
        assert!(scheduler.select_tip().is_none());
    }
}
