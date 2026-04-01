pub mod agent_summary;
pub mod auto_dream;
pub mod auto_updater;
pub mod away_summary;
pub mod diagnostics;
pub mod extract_memories;
pub mod lsp_service;
pub mod magic_docs;
pub mod notifications;
/// System services: sleep prevention, auto-updates, release notes, diagnostics.
pub mod prevent_sleep;
pub mod prompt_suggestion;
pub mod release_notes;
pub mod session_memory;
pub mod settings_sync;
pub mod team_memory_sync;
pub mod tips;
pub mod token_estimation;
pub mod tool_use_summary;

pub use agent_summary::{
    build_summary_prompt as build_agent_summary_prompt, AgentId, AgentSummaryEntry,
    AgentSummaryState, AgentSummarizationHandle, start_agent_summarization,
};
pub use auto_dream::{
    build_consolidation_prompt, build_dream_extra_context, AutoDreamConfig, AutoDreamState,
    list_sessions_touched_since, lock_path, read_last_consolidated_at, record_consolidation,
    rollback_consolidation_lock, try_acquire_consolidation_lock,
};
pub use auto_updater::{check_for_updates, UpdateInfo, UpdateStatus};
pub use away_summary::{
    build_away_summary_prompt, generate_away_summary, prepare_away_summary_context,
    AwaySummaryError, AwaySummaryResult,
};
pub use diagnostics::DiagnosticTracker;
pub use extract_memories::{
    build_extract_auto_only_prompt, build_extract_combined_prompt,
    check_auto_mem_tool_permission, count_model_visible_messages_since,
    default_auto_mem_path, extract_written_paths, has_memory_writes_since,
    is_auto_mem_path, is_model_visible_message, AutoMemToolDecision,
    ExtractedMemory, ExtractMemoriesState, MemoryType, ENTRYPOINT_NAME,
};
pub use lsp_service::{
    DiagnosticFile, DiagnosticRegistry, LspDiagnostic, LspManager, LspServerConfig,
    LspServerInstance, LspServerState,
};
pub use magic_docs::{
    build_magic_docs_update_prompt, check_magic_doc_tool_permission,
    detect_magic_doc_header, load_magic_docs_prompt, on_file_read,
    MagicDocHeader, MagicDocInfo, MagicDocRegistry,
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
pub use settings_sync::{
    apply_remote_entries_to_local, build_entries_from_local_files,
    compute_changed_entries, compute_checksum, fetch_with_retries,
    get_retry_delay, try_read_file_for_sync, write_file_for_sync,
    ApplyResult, SettingsSyncConfig, SettingsSyncFetchResult,
    SettingsSyncUploadResult, SyncKeys, UserSyncContent, UserSyncData,
};
pub use team_memory_sync::{
    check_team_mem_secrets, compute_delta, hash_content, is_permanent_failure,
    read_local_team_memory, scan_for_secrets, validate_team_mem_key,
    write_team_memory_entries, SecretMatch, SkippedSecretFile, SyncErrorType,
    SyncState, TeamMemoryContent, TeamMemoryData, TeamMemoryHashesResult,
    TeamMemorySyncFetchResult, TeamMemorySyncPullResult, TeamMemorySyncPushResult,
    TeamMemoryWatcher,
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
