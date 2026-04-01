use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Configuration for the KAIROS assistant mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantConfig {
    /// Whether assistant mode is enabled (set by --assistant flag).
    pub enabled: bool,
    /// Whether to run in brief-only output mode.
    pub brief_only: bool,
    /// Interval between tick prompts sent to the engine.
    pub tick_interval: Duration,
    /// Directory for daily log files.
    pub daily_log_dir: PathBuf,
    /// Whether push notifications are enabled.
    pub push_notifications: bool,
    /// Whether GitHub webhook integration is enabled.
    pub github_webhooks: bool,
}

impl Default for AssistantConfig {
    fn default() -> Self {
        let daily_log_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(crate::config::paths::OMNI_DIR_NAME)
            .join("assistant")
            .join("logs");
        Self {
            enabled: false,
            brief_only: true,
            tick_interval: Duration::from_secs(30),
            daily_log_dir,
            push_notifications: false,
            github_webhooks: false,
        }
    }
}

/// Runtime state for the KAIROS assistant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssistantState {
    /// Assistant mode is inactive.
    #[default]
    Inactive,
    /// Assistant mode is active with full capabilities.
    Active,
    /// Assistant mode is active but restricted to brief-only output.
    BriefOnly,
    /// Assistant mode is paused (tick scheduler suspended).
    Paused,
}

impl AssistantState {
    /// Whether the assistant is in any active state (not paused or inactive).
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active | Self::BriefOnly)
    }

    /// Whether the assistant is in brief-only mode.
    pub fn is_brief_only(&self) -> bool {
        matches!(self, Self::BriefOnly)
    }

    /// Whether the assistant is paused.
    pub fn is_paused(&self) -> bool {
        matches!(self, Self::Paused)
    }
}
