pub mod auto_updater;
pub mod diagnostics;
pub mod lsp_service;
pub mod notifications;
/// System services: sleep prevention, auto-updates, release notes, diagnostics.
pub mod prevent_sleep;
pub mod prompt_suggestion;
pub mod release_notes;
pub mod session_memory;
pub mod tips;
pub mod token_estimation;
pub mod tool_use_summary;

pub use auto_updater::{check_for_updates, UpdateInfo, UpdateStatus};
pub use diagnostics::DiagnosticTracker;
pub use lsp_service::{
    DiagnosticFile, DiagnosticRegistry, LspDiagnostic, LspManager, LspServerConfig,
    LspServerInstance, LspServerState,
};
pub use notifications::{
    detect_terminal, send_notification, NotificationChannel, NotificationOptions,
};
pub use prevent_sleep::{allow_sleep, is_sleep_prevented, prevent_sleep};
pub use prompt_suggestion::{
    should_filter_suggestion, PromptSuggestion, PromptVariant, SuppressReason, SUGGESTION_PROMPT,
};
pub use release_notes::{load_release_notes, ReleaseNote};
pub use session_memory::{
    load_session_memories, persist_memories, session_memory_dir, session_memory_path,
    should_extract_memory, setup_session_memory_file, Memory, MemoryCategory, SessionMemoryConfig,
    SessionMemoryState,
};
pub use tips::{
    default_tips, Tip, TipCategory, TipHistory, TipRegistry, TipScheduler,
};
pub use token_estimation::{
    bytes_per_token_for_file_type, estimate_content_block_tokens, estimate_message_tokens,
    estimate_messages_tokens, rough_token_count_estimation, rough_token_count_for_file_type,
    token_count_with_estimation, token_count_with_estimation_for_messages,
};
pub use tool_use_summary::{
    build_summary_prompt, generate_simple_summary, ToolInfo, TOOL_USE_SUMMARY_SYSTEM_PROMPT,
};
