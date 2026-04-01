//! Hooks system: user-defined commands executed at various lifecycle points.
//!
//! This module mirrors the TypeScript hooks implementation from `utils/hooks.ts`,
//! `utils/hooks/*.ts`, `types/hooks.ts`, and `schemas/hooks.ts`.
//!
//! # Architecture
//!
//! - **types**: Event definitions, hook commands, structured input/output, results
//! - **registry**: Storage and querying of registered hooks by event
//! - **matching**: Pattern matching for hook matchers and deduplication
//! - **execution**: Running shell commands, HTTP hooks, processing outputs
//! - **config**: Loading hooks from settings, snapshot management, validation
//! - **watcher**: File change watching for FileChanged/CwdChanged hooks
//! - **events**: Hook execution event broadcasting for observability

pub mod config;
pub mod events;
pub mod execution;
pub mod matching;
pub mod registry;
pub mod types;
pub mod watcher;

// Re-export primary types for convenience.
pub use config::{
    build_hook_registry, load_hooks_from_settings, load_hooks_into_registry,
    validate_hooks_config, HooksConfigSnapshot,
};
pub use events::HookEventEmitter;
pub use execution::{
    execute_hooks_for_event, execute_hooks_outside_repl, get_session_end_hook_timeout_ms,
};
pub use matching::{
    get_matching_hooks, get_pre_tool_hook_blocking_message, get_stop_hook_message,
    get_user_prompt_submit_hook_blocking_message, glob_matches, has_blocking_result,
    matches_pattern,
};
pub use registry::HookRegistry;
pub use types::{
    AggregatedHookResult, BaseHookInput, HookBlockingError, HookCommand, HookEvent,
    HookExecutionEvent, HookInput, HookJsonOutput, HookMatcher, HookOutcome, HookOutcomeCounts,
    HookOutsideReplResult, HookResult, HookSource, HookSpecificOutput, HooksSettings,
    IndividualHookConfig, PermissionBehavior, PermissionRequestDecision, SyncHookJsonOutput,
    is_hook_event,
};
pub use watcher::FileChangedWatcher;

// Backward-compatible re-export of the glob matcher under its old name.
pub fn matcher_matches(pattern: &str, value: &str) -> bool {
    glob_matches(pattern, value)
}
