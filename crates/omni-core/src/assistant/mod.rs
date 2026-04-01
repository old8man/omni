/// KAIROS always-on assistant mode.
///
/// This module provides the core infrastructure for the KAIROS assistant:
/// periodic tick scheduling, brief-mode output, push notifications,
/// dream (memory consolidation), and append-only daily logging.
pub mod brief;
pub mod daily_log;
pub mod dream;
pub mod mode;
pub mod notifications;
pub mod tick;
pub mod types;

pub use brief::{format_brief_output, is_brief_mode, BriefMessage, BriefStatus};
pub use daily_log::{DailyLog, LogCategory, LogEntry};
pub use dream::{DreamConfig, DreamMode, DreamResult};
pub use mode::{
    activate_kairos, deactivate_assistant, get_assistant_system_prompt_addendum, is_assistant_mode,
};
pub use notifications::{NotificationSender, PrSubscription};
pub use tick::TickScheduler;
pub use types::{AssistantConfig, AssistantState};
