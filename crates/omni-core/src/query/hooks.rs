use anyhow::Result;
use async_trait::async_trait;

use crate::types::content::ContentBlock;

use super::tracking::QueryTracking;

/// Context provided to post-sampling and stop hooks.
#[derive(Clone, Debug)]
pub struct HookContext {
    /// Full message history (as raw JSON values for flexibility).
    pub messages: Vec<serde_json::Value>,
    /// System prompt text.
    pub system_prompt_text: String,
    /// Query source identifier (e.g. "repl_main_thread", "sdk").
    pub query_source: String,
    /// Agent ID, if this is a subagent query.
    pub agent_id: Option<String>,
    /// Query chain tracking (chain_id + depth) for conversation continuity.
    pub tracking: QueryTracking,
}

// ── Post-sampling hooks ────────────────────────────────────────────────────

/// A hook executed after each model response (before tool execution).
///
/// Post-sampling hooks are fire-and-forget: they observe the response but
/// cannot block the query loop. Errors are logged and swallowed.
#[async_trait]
pub trait PostSamplingHook: Send + Sync {
    /// Called with the content blocks from the model's response.
    async fn on_response(
        &self,
        content: &[ContentBlock],
        context: &HookContext,
    ) -> Result<()>;
}

/// Registry of post-sampling hooks.
#[derive(Default)]
pub struct PostSamplingHookRegistry {
    hooks: Vec<Box<dyn PostSamplingHook>>,
}

impl PostSamplingHookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register(&mut self, hook: Box<dyn PostSamplingHook>) {
        self.hooks.push(hook);
    }

    /// Execute all registered hooks. Errors are logged, never propagated.
    pub async fn execute(&self, content: &[ContentBlock], context: &HookContext) {
        for hook in &self.hooks {
            if let Err(e) = hook.on_response(content, context).await {
                tracing::warn!("post-sampling hook failed: {e}");
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}

// ── Stop hooks ─────────────────────────────────────────────────────────────

/// Result from a single stop hook evaluation.
#[derive(Debug)]
pub enum StopHookAction {
    /// Allow the query to stop normally.
    Allow,
    /// Inject a blocking error message to force the model to continue.
    Block {
        /// The error message injected as a user turn.
        error_message: String,
    },
    /// Prevent the query from continuing entirely (hard stop).
    PreventContinuation {
        reason: String,
    },
}

/// A hook evaluated when the model returns `end_turn` with no tool_use.
///
/// Stop hooks can force continuation by returning `Block` with an error
/// message. Used for task completion checking, teammate status, etc.
#[async_trait]
pub trait StopHook: Send + Sync {
    /// Evaluate whether the query should actually stop.
    async fn on_stop(
        &self,
        context: &HookContext,
        stop_hook_active: bool,
    ) -> Result<StopHookAction>;
}

/// Result of evaluating all stop hooks.
#[derive(Debug, Default)]
pub struct StopHookResult {
    /// Blocking error messages to inject as user turns.
    pub blocking_errors: Vec<String>,
    /// If true, the query must terminate immediately.
    pub prevent_continuation: bool,
}

/// Registry of stop hooks.
#[derive(Default)]
pub struct StopHookRegistry {
    hooks: Vec<Box<dyn StopHook>>,
}

impl StopHookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register(&mut self, hook: Box<dyn StopHook>) {
        self.hooks.push(hook);
    }

    /// Evaluate all stop hooks and collect their decisions.
    pub async fn evaluate(
        &self,
        context: &HookContext,
        stop_hook_active: bool,
    ) -> StopHookResult {
        let mut result = StopHookResult::default();

        for hook in &self.hooks {
            match hook.on_stop(context, stop_hook_active).await {
                Ok(StopHookAction::Allow) => {}
                Ok(StopHookAction::Block { error_message }) => {
                    result.blocking_errors.push(error_message);
                }
                Ok(StopHookAction::PreventContinuation { reason }) => {
                    tracing::info!("stop hook prevented continuation: {reason}");
                    result.prevent_continuation = true;
                }
                Err(e) => {
                    tracing::warn!("stop hook error: {e}");
                }
            }
        }

        result
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}
