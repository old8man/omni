use anyhow::Result;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::accumulator::ContentBlockAccumulator;
use crate::api::client::{ApiClient, StreamResponse, ToolDefinition};
use crate::api::normalize::{add_cache_markers, normalize_messages};
use crate::api::retry::RetryPolicy;
use crate::api::sse::{self, ContentDelta, SseEvent};
use crate::compact::{
    compact_messages, micro_compact, should_auto_compact, CompactionConfig, CompactionStrategy,
};
use crate::services::token_estimation;
use crate::session::SessionManager;
use crate::state::AppStateStore;
use crate::types::content::ContentBlock;
use crate::types::events::StreamEvent;
use crate::types::message::StopReason;
use crate::utils::context as ctx_utils;

use super::hooks::{HookContext, PostSamplingHookRegistry, StopHookRegistry};
use super::state::{QueryState, TransitionReason};
use super::token_budget::{self, BudgetTracker, TokenBudgetDecision};
use super::tool_executor::{PendingTool, StreamingToolExecutor, ToolCallFn};
use super::tracking::QueryTracking;

const MAX_OUTPUT_TOKENS_RECOVERY_LIMIT: u32 = 3;
const ESCALATED_MAX_TOKENS: u32 = ctx_utils::ESCALATED_MAX_TOKENS as u32;

/// Maximum consecutive auto-compact failures before disabling.
const AUTO_COMPACT_FAILURE_LIMIT: u32 = 3;

pub struct QueryEngine {
    api_client: ApiClient,
    messages: Vec<serde_json::Value>,
    system_prompt: Vec<ContentBlock>,
    tool_schemas: Vec<ToolDefinition>,
    state: QueryState,
    cancel: CancellationToken,
    // Recovery state
    max_output_tokens_override: Option<u32>,
    recovery_count: u32,
    turn_count: u32,
    max_turns: Option<u32>,
    // Session persistence
    session_manager: Option<SessionManager>,
    session_id: Option<String>,
    session_created_at: Option<DateTime<Utc>>,
    // Compaction
    compaction_config: CompactionConfig,
    compaction_strategy: CompactionStrategy,
    auto_compact_failures: u32,
    /// Retry policy for transient API errors.
    retry_policy: RetryPolicy,
    // ── Token budget ───────────────────────────────────────────────────────
    /// Configured token budget for this turn (e.g. from "+500k" in user message).
    token_budget: Option<u64>,
    /// Tracks cumulative output tokens for budget decisions.
    cumulative_output_tokens: u64,
    /// Per-turn budget tracker (continuation count, diminishing returns, etc.).
    budget_tracker: Option<BudgetTracker>,
    // ── Hooks ──────────────────────────────────────────────────────────────
    /// Post-sampling hooks: fire-and-forget after each model response.
    post_sampling_hooks: PostSamplingHookRegistry,
    /// Stop hooks: evaluated when model returns end_turn with no tool_use.
    stop_hooks: StopHookRegistry,
    /// Whether a stop hook forced continuation on the previous turn.
    stop_hook_active: bool,
    // ── Streaming tool execution ───────────────────────────────────────────
    /// Factory for spawning tool execution tasks. When set, tool_use blocks
    /// are dispatched as soon as their content_block_stop arrives (overlapping
    /// execution with model generation).
    tool_call_fn: Option<ToolCallFn>,
    // ── Query tracking ─────────────────────────────────────────────────────
    /// Chain ID + depth for conversation tracking. Persists across the entire conversation.
    tracking: QueryTracking,
    /// Agent ID for subagent queries (None for main thread).
    agent_id: Option<String>,
    /// Query source identifier.
    query_source: String,
    /// Shared application state for cost/usage/turn tracking.
    app_state: Option<AppStateStore>,
}

impl QueryEngine {
    pub fn new(
        api_client: ApiClient,
        system_prompt: Vec<ContentBlock>,
        tool_schemas: Vec<ToolDefinition>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            api_client,
            messages: Vec::new(),
            system_prompt,
            tool_schemas,
            state: QueryState::Querying,
            cancel,
            max_output_tokens_override: None,
            recovery_count: 0,
            turn_count: 0,
            max_turns: None,
            session_manager: None,
            session_id: None,
            session_created_at: None,
            compaction_config: CompactionConfig::default(),
            compaction_strategy: CompactionStrategy::default(),
            auto_compact_failures: 0,
            retry_policy: RetryPolicy::default(),
            token_budget: None,
            cumulative_output_tokens: 0,
            budget_tracker: None,
            post_sampling_hooks: PostSamplingHookRegistry::new(),
            stop_hooks: StopHookRegistry::new(),
            stop_hook_active: false,
            tool_call_fn: None,
            tracking: QueryTracking::new(),
            agent_id: None,
            query_source: "repl_main_thread".to_string(),
            app_state: None,
        }
    }

    /// Configure compaction thresholds based on the model's context window.
    ///
    /// Uses [`crate::utils::context::get_context_window_for_model`] and
    /// [`crate::utils::context::get_auto_compact_threshold`] to derive the
    /// correct max_context_tokens for the model. Call this after setting the
    /// model (before the first turn).
    pub fn configure_for_model(&mut self) {
        let model = &self.api_client.config.model;
        self.compaction_config = CompactionConfig::for_model(model);
        // Set default max output tokens from model capabilities unless already overridden.
        if self.max_output_tokens_override.is_none() {
            let (default_max, _) = ctx_utils::get_max_output_tokens(model);
            self.api_client.config.max_tokens = default_max;
        }
    }

    /// Attach the shared application state for live cost/usage/turn tracking.
    pub fn set_app_state(&mut self, state: AppStateStore) {
        self.app_state = Some(state);
    }

    pub fn set_max_turns(&mut self, max: u32) {
        self.max_turns = Some(max);
    }

    /// Set the token budget for this turn (e.g. parsed from "+500k").
    pub fn set_token_budget(&mut self, budget: u64) {
        self.token_budget = Some(budget);
        self.budget_tracker = Some(BudgetTracker::new());
    }

    /// Register a post-sampling hook.
    pub fn register_post_sampling_hook(
        &mut self,
        hook: Box<dyn super::hooks::PostSamplingHook>,
    ) {
        self.post_sampling_hooks.register(hook);
    }

    /// Register a stop hook.
    pub fn register_stop_hook(&mut self, hook: Box<dyn super::hooks::StopHook>) {
        self.stop_hooks.register(hook);
    }

    /// Enable streaming tool execution by providing a tool call factory.
    pub fn set_tool_call_fn(&mut self, tool_call_fn: ToolCallFn) {
        self.tool_call_fn = Some(tool_call_fn);
    }

    /// Set the agent ID for subagent queries.
    pub fn set_agent_id(&mut self, agent_id: String) {
        self.agent_id = Some(agent_id);
    }

    /// Set the query source identifier.
    pub fn set_query_source(&mut self, source: String) {
        self.query_source = source;
    }

    /// Get the current query tracking state.
    pub fn tracking(&self) -> &QueryTracking {
        &self.tracking
    }

    pub fn state(&self) -> &QueryState {
        &self.state
    }

    pub fn messages(&self) -> &[serde_json::Value] {
        &self.messages
    }

    /// Get total input tokens from the shared AppState cost tracker.
    /// Returns 0 if no AppState is attached.
    pub fn total_input_tokens(&self) -> u64 {
        self.app_state
            .as_ref()
            .map(|s| s.read().cost_tracker.total_input_tokens())
            .unwrap_or(0)
    }

    /// Build a model-ready prompt for summarizing recent tool usage.
    ///
    /// Returns `None` if there are no tool_use blocks in the recent messages.
    /// The caller can send this prompt to a fast model (e.g., Haiku) using
    /// [`crate::services::tool_use_summary::TOOL_USE_SUMMARY_SYSTEM_PROMPT`]
    /// as the system prompt, to get a concise label like "Edited auth.rs".
    pub fn build_tool_summary_prompt(&self) -> Option<String> {
        use crate::services::tool_use_summary::build_summary_prompt;
        let infos = crate::compact::extract_tool_infos_from_messages(&self.messages);
        if infos.is_empty() {
            return None;
        }
        // Extract last assistant text for context
        let last_text = self.messages.iter().rev().find_map(|m| {
            if m.get("role")?.as_str()? == "assistant" {
                m.get("content")?.as_array()?.iter().find_map(|b| {
                    if b.get("type")?.as_str()? == "text" {
                        b.get("text")?.as_str().map(String::from)
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        });
        build_summary_prompt(&infos, last_text.as_deref())
    }

    /// Add a user message.
    ///
    /// Automatically parses token budget directives (e.g. "+500k") from the
    /// text and enables the budget tracker when found.
    pub fn add_user_message(&mut self, text: &str) {
        // Check for token budget directives in user input
        if let Some(budget) = token_budget::parse_token_budget(text) {
            tracing::info!("detected token budget: {budget} tokens");
            self.set_token_budget(budget);
        }

        self.messages.push(serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": text}]
        }));
    }

    /// Add a tool result message
    pub fn add_tool_result(&mut self, tool_use_id: &str, content: &str, is_error: bool) {
        self.messages.push(serde_json::json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": [{"type": "text", "text": content}],
                "is_error": is_error,
            }]
        }));
    }

    /// Add the raw assistant message from the API response
    pub fn add_assistant_message(&mut self, content: Vec<serde_json::Value>) {
        self.messages.push(serde_json::json!({
            "role": "assistant",
            "content": content,
        }));
    }

    /// Add a pre-built message (used when restoring a session).
    pub fn add_raw_message(&mut self, message: serde_json::Value) {
        self.messages.push(message);
    }

    /// Add synthetic error tool_result blocks for orphaned tool_use blocks.
    ///
    /// Called on cancellation (Q13) to prevent conversation corruption where
    /// tool_use blocks exist without matching tool_result responses.
    fn add_synthetic_tool_results_for_pending(&mut self, tool_use_blocks: &[ToolUseInfo]) {
        if tool_use_blocks.is_empty() {
            return;
        }
        let synthetic: Vec<serde_json::Value> = tool_use_blocks
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": t.id,
                    "content": [{"type": "text", "text": crate::utils::messages::INTERRUPT_MESSAGE_FOR_TOOL_USE}],
                    "is_error": true,
                })
            })
            .collect();
        self.messages.push(serde_json::json!({
            "role": "user",
            "content": synthetic,
        }));
    }

    /// Enable session persistence with the given manager.
    ///
    /// If `resume_id` is provided, loads that session. Otherwise creates a new one.
    pub fn enable_session_persistence(
        &mut self,
        manager: SessionManager,
        resume_id: Option<&str>,
    ) -> Result<String> {
        let session = if let Some(id) = resume_id {
            let s = manager.load_session(id)?;
            self.messages = s.messages.clone();
            self.session_created_at = Some(s.created_at);
            s
        } else {
            let s = manager.create_session()?;
            self.session_created_at = Some(s.created_at);
            s
        };
        let id = session.id.clone();
        self.session_id = Some(id.clone());
        self.session_manager = Some(manager);
        Ok(id)
    }

    /// Persist the current session state to disk.
    pub fn save_session_snapshot(&self) -> Result<()> {
        let Some(ref manager) = self.session_manager else {
            return Ok(());
        };
        let Some(ref id) = self.session_id else {
            return Ok(());
        };
        let now = Utc::now();
        let session = crate::session::Session {
            id: id.clone(),
            messages: self.messages.clone(),
            created_at: self.session_created_at.unwrap_or(now),
            updated_at: now,
            project_root: None,
            model: Some(self.api_client.config.model.clone()),
            total_cost: 0.0,
            cumulative_usage: crate::session::CumulativeUsage::default(),
        };
        manager.save_session(&session)
    }

    /// Clear all messages (used by /clear command).
    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    /// Manually trigger compaction using the configured strategy.
    ///
    /// Returns a summary string describing what was compacted.
    pub async fn compact(&mut self, event_tx: &mpsc::Sender<StreamEvent>) -> Result<String> {
        let system_text = self.system_prompt_text();

        let result = compact_messages(
            &self.messages,
            &self.compaction_strategy,
            &self.compaction_config,
            Some(&self.api_client),
            &system_text,
        )
        .await?;

        self.messages = result.messages;
        let summary = result.summary.clone();

        let _ = event_tx
            .send(StreamEvent::Compacted {
                summary: summary.clone(),
            })
            .await;

        // Auto-save after compaction
        let _ = self.save_session_snapshot();

        Ok(summary)
    }

    /// Set the compaction configuration.
    pub fn set_compaction_config(&mut self, config: CompactionConfig) {
        self.compaction_config = config;
    }

    /// Set the compaction strategy.
    pub fn set_compaction_strategy(&mut self, strategy: CompactionStrategy) {
        self.compaction_strategy = strategy;
    }

    /// Extract the system prompt as plain text.
    fn system_prompt_text(&self) -> String {
        self.system_prompt
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Run one turn of the query loop.
    /// Returns collected tool_use blocks (if any) and the stop reason.
    pub async fn run_turn(&mut self, event_tx: &mpsc::Sender<StreamEvent>) -> Result<TurnResult> {
        if self.cancel.is_cancelled() {
            self.state = QueryState::Terminal {
                stop_reason: StopReason::EndTurn,
                transition: TransitionReason::Aborted,
            };
            return Ok(TurnResult::Done(StopReason::EndTurn));
        }

        // Check max turns
        if let Some(max) = self.max_turns {
            if self.turn_count >= max {
                self.state = QueryState::Terminal {
                    stop_reason: StopReason::EndTurn,
                    transition: TransitionReason::MaxTurns,
                };
                return Ok(TurnResult::Done(StopReason::EndTurn));
            }
        }

        self.turn_count += 1;

        // ── Update shared AppState turn counter ──────────────────────────
        if let Some(ref state) = self.app_state {
            state.write().begin_turn();
            // Sync model from AppState (user may have changed it via /model)
            let s = state.read();
            let active_model = s.active_model().to_string();
            if self.api_client.config.model != active_model {
                tracing::info!("model changed via AppState: {} -> {}", self.api_client.config.model, active_model);
                self.api_client.config.model = active_model;
            }
            // Sync max_tokens from settings if configured
            if let Some(max_tokens) = s.settings.max_tokens {
                if self.max_output_tokens_override.is_none() {
                    self.api_client.config.max_tokens = max_tokens as u64;
                }
            }
            drop(s);
        }

        // ── Query tracking: advance depth each turn ────────────────────────
        self.tracking = QueryTracking::init_or_advance(Some(&self.tracking));

        self.state = QueryState::Querying;

        // ── Q5: Micro-compact as per-turn pre-pass ──────────────────────────
        {
            let pre_pass = micro_compact(&self.messages, &self.compaction_config);
            if pre_pass.post_compact_tokens < pre_pass.pre_compact_tokens {
                tracing::debug!(
                    "micro pre-pass: {} -> {} tokens",
                    pre_pass.pre_compact_tokens,
                    pre_pass.post_compact_tokens
                );
                self.messages = pre_pass.messages;
            }
        }

        // ── Auto-compact if context is too large (circuit breaker: max 3 failures) ──
        if self.auto_compact_failures < AUTO_COMPACT_FAILURE_LIMIT
            && should_auto_compact(&self.messages, &self.compaction_config)
        {
            tracing::info!("auto-compacting before API call");
            let system_text = self.system_prompt_text();

            match compact_messages(
                &self.messages,
                &self.compaction_strategy,
                &self.compaction_config,
                Some(&self.api_client),
                &system_text,
            )
            .await
            {
                Ok(result) => {
                    self.messages = result.messages;
                    self.auto_compact_failures = 0;
                    let _ = event_tx
                        .send(StreamEvent::Compacted {
                            summary: result.summary,
                        })
                        .await;
                }
                Err(e) => {
                    self.auto_compact_failures += 1;
                    tracing::warn!(
                        failures = self.auto_compact_failures,
                        "auto-compact failed: {e}"
                    );
                }
            }
        }

        // Apply max_output_tokens override if set
        if let Some(override_tokens) = self.max_output_tokens_override {
            self.api_client.config.max_tokens = override_tokens as u64;
        }

        // Send request start event
        let _ = event_tx
            .send(StreamEvent::RequestStart {
                request_id: format!("turn_{}", self.turn_count),
            })
            .await;

        // ── Pre-flight token estimation ────────────────────────────────────
        {
            let estimated = token_estimation::rough_token_count_estimation(
                &serde_json::to_string(&self.messages).unwrap_or_default(),
                4,
            );
            tracing::debug!(
                estimated_tokens = estimated,
                turn = self.turn_count,
                "pre-flight token estimate"
            );
        }

        // ── Q1: Normalize messages before building API request ──────────────
        let mut api_messages = normalize_messages(&self.messages);
        add_cache_markers(&mut api_messages);

        // ── A1: Retry loop wrapping stream_request ──────────────────────────
        let api_start = std::time::Instant::now();
        let stream_response = self.stream_with_retry(&api_messages, event_tx).await?;

        self.state = QueryState::Streaming;

        let mut tool_use_blocks: Vec<ToolUseInfo> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut assistant_content: Vec<serde_json::Value> = Vec::new();
        let mut turn_output_tokens: u64 = 0;

        // ── Streaming tool executor: start tools as their blocks complete ──
        let mut streaming_executor = self.tool_call_fn.as_ref().map(|call_fn| {
            StreamingToolExecutor::new(self.cancel.child_token(), Arc::clone(call_fn))
        });

        // ── Extract quota from streaming response headers ─────────────────
        if let StreamResponse::Streaming(ref response) = stream_response {
            let quota = ApiClient::extract_quota(response);
            if let Some(remaining) = quota.tokens_remaining {
                tracing::debug!(tokens_remaining = remaining, "rate-limit quota");
            }
            if let Some(remaining) = quota.requests_remaining {
                tracing::debug!(requests_remaining = remaining, "rate-limit quota");
            }
        }

        match stream_response {
            StreamResponse::NonStreaming(value) => {
                // 529 fallback path: parse the complete JSON response.
                (assistant_content, tool_use_blocks, stop_reason) =
                    Self::parse_non_streaming_response(&value, event_tx).await;
                // Track output tokens from the non-streaming response.
                if let Some(usage) = value.get("usage") {
                    turn_output_tokens = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                }
            }
            StreamResponse::Streaming(response) => {
                // Normal streaming path: read SSE events from the body.
                let mut byte_stream = response.bytes_stream();
                let mut line_buffer = String::new();
                let mut current_event_type: Option<String> = None;
                let mut current_data: Option<String> = None;
                let mut accumulator = ContentBlockAccumulator::new();

                while let Some(chunk) = byte_stream.next().await {
                    if self.cancel.is_cancelled() {
                        if !assistant_content.is_empty() {
                            self.add_assistant_message(assistant_content);
                        }
                        self.add_synthetic_tool_results_for_pending(&tool_use_blocks);

                        self.state = QueryState::Terminal {
                            stop_reason: StopReason::EndTurn,
                            transition: TransitionReason::Aborted,
                        };
                        return Ok(TurnResult::Done(StopReason::EndTurn));
                    }

                    let chunk = chunk?;
                    line_buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(newline_pos) = line_buffer.find('\n') {
                        let line = line_buffer[..newline_pos]
                            .trim_end_matches('\r')
                            .to_string();
                        line_buffer = line_buffer[newline_pos + 1..].to_string();

                        if let Some(rest) = line.strip_prefix("event:") {
                            current_event_type = Some(rest.trim().to_string());
                        } else if let Some(rest) = line.strip_prefix("data:") {
                            current_data = Some(rest.trim().to_string());
                        } else if line.is_empty() {
                            if let (Some(event_type), Some(data)) =
                                (current_event_type.take(), current_data.take())
                            {
                                if let Ok(event) = sse::parse_sse_event(&event_type, &data) {
                                    match event {
                                        SseEvent::ContentBlockStart { index, block } => {
                                            accumulator.on_start(index, block);
                                        }
                                        SseEvent::ContentBlockDelta { index, delta } => {
                                            match &delta {
                                                ContentDelta::TextDelta { text } => {
                                                    let _ = event_tx
                                                        .send(StreamEvent::TextDelta {
                                                            text: text.clone(),
                                                        })
                                                        .await;
                                                }
                                                ContentDelta::ThinkingDelta { thinking } => {
                                                    let _ = event_tx
                                                        .send(StreamEvent::ThinkingDelta {
                                                            text: thinking.clone(),
                                                        })
                                                        .await;
                                                }
                                                _ => {}
                                            }
                                            accumulator.on_delta(index, delta);
                                        }
                                        SseEvent::ContentBlockStop { index } => {
                                            if let Ok(block) = accumulator.on_stop(index) {
                                                match &block {
                                                    ContentBlock::ToolUse { id, name, input } => {
                                                        let _ = event_tx
                                                            .send(StreamEvent::ToolStart {
                                                                tool_use_id: id.clone(),
                                                                name: name.clone(),
                                                                input: input.clone(),
                                                            })
                                                            .await;
                                                        tool_use_blocks.push(ToolUseInfo {
                                                            id: id.clone(),
                                                            name: name.clone(),
                                                            input: input.clone(),
                                                        });
                                                        assistant_content
                                                            .push(serde_json::json!({
                                                                "type": "tool_use",
                                                                "id": id,
                                                                "name": name,
                                                                "input": input,
                                                            }));

                                                        // ── Streaming tool execution: dispatch immediately ──
                                                        if let Some(ref mut executor) =
                                                            streaming_executor
                                                        {
                                                            executor.add_tool(PendingTool {
                                                                id: id.clone(),
                                                                name: name.clone(),
                                                                input: input.clone(),
                                                                is_concurrent: true,
                                                            });
                                                        }
                                                    }
                                                    ContentBlock::Text { text } => {
                                                        assistant_content
                                                            .push(serde_json::json!({
                                                                "type": "text",
                                                                "text": text,
                                                            }));
                                                    }
                                                    ContentBlock::Thinking {
                                                        thinking,
                                                        signature,
                                                    } => {
                                                        assistant_content
                                                            .push(serde_json::json!({
                                                                "type": "thinking",
                                                                "thinking": thinking,
                                                                "signature": signature,
                                                            }));
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        SseEvent::MessageDelta {
                                            stop_reason: sr,
                                            usage,
                                        } => {
                                            if let Some(sr_str) = sr {
                                                stop_reason = match sr_str.as_str() {
                                                    "end_turn" => StopReason::EndTurn,
                                                    "tool_use" => StopReason::ToolUse,
                                                    "max_tokens" => StopReason::MaxTokens,
                                                    "stop_sequence" => StopReason::StopSequence,
                                                    _ => StopReason::EndTurn,
                                                };
                                            }
                                            if let Some(u) = usage {
                                                turn_output_tokens = u.output_tokens;
                                                // Track output usage in shared AppState
                                                if let Some(ref state) = self.app_state {
                                                    let model = state.read().active_model().to_string();
                                                    let output_usage = crate::types::usage::Usage {
                                                        input_tokens: 0,
                                                        output_tokens: u.output_tokens,
                                                        cache_creation_input_tokens: None,
                                                        cache_read_input_tokens: None,
                                                        server_tool_use: None,
                                                        speed: None,
                                                    };
                                                    state.read().cost_tracker.add_usage(&model, &output_usage);
                                                }
                                                let _ = event_tx
                                                    .send(StreamEvent::UsageUpdate(
                                                        crate::types::usage::Usage {
                                                            input_tokens: 0,
                                                            output_tokens: u.output_tokens,
                                                            cache_creation_input_tokens: None,
                                                            cache_read_input_tokens: None,
                                                            server_tool_use: None,
                                                            speed: None,
                                                        },
                                                    ))
                                                    .await;
                                            }
                                        }
                                        SseEvent::MessageStart { message } => {
                                            // Track usage in shared AppState
                                            if let Some(ref state) = self.app_state {
                                                let model = state.read().active_model().to_string();
                                                state.read().cost_tracker.add_usage(&model, &message.usage);
                                            }
                                            let _ = event_tx
                                                .send(StreamEvent::UsageUpdate(
                                                    message.usage.clone(),
                                                ))
                                                .await;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }

                    // ── Poll streaming executor for completed tools during streaming ──
                    if let Some(ref mut executor) = streaming_executor {
                        let completed = executor.poll_completed();
                        for result in completed {
                            let (data, is_error) = match result.result {
                                Ok(data) => (data.data, data.is_error),
                                Err(e) => (
                                    serde_json::Value::String(format!("Tool error: {e}")),
                                    true,
                                ),
                            };
                            let _ = event_tx
                                .send(StreamEvent::ToolResult {
                                    tool_use_id: result.id.clone(),
                                    result: crate::types::events::ToolResultData {
                                        data: data.clone(),
                                        is_error,
                                    },
                                })
                                .await;
                        }
                    }
                }
            }
        }

        // Track cumulative output tokens for token budget
        self.cumulative_output_tokens += turn_output_tokens;

        // ── Write turn metrics back to AppState ─────────────────────────────
        if let Some(ref state) = self.app_state {
            let api_duration_ms = api_start.elapsed().as_secs_f64() * 1000.0;
            let mut s = state.write();
            s.total_api_duration_ms += api_duration_ms;
        }

        // ── Post-sampling hooks: fire-and-forget after model response ──────
        if !self.post_sampling_hooks.is_empty() && !assistant_content.is_empty() {
            let content_blocks = Self::parse_content_blocks(&assistant_content);
            let hook_context = self.build_hook_context();
            self.post_sampling_hooks
                .execute(&content_blocks, &hook_context)
                .await;
        }

        // Add assistant message to history
        if !assistant_content.is_empty() {
            self.add_assistant_message(assistant_content);
        }

        // Auto-save session after each turn
        let _ = self.save_session_snapshot();

        // ── Flush streaming executor: wait for any tools still running ─────
        if let Some(ref mut executor) = streaming_executor {
            let remaining = executor.flush().await;
            for result in remaining {
                let (data, is_error) = match result.result {
                    Ok(data) => (data.data, data.is_error),
                    Err(e) => (
                        serde_json::Value::String(format!("Tool error: {e}")),
                        true,
                    ),
                };
                let _ = event_tx
                    .send(StreamEvent::ToolResult {
                        tool_use_id: result.id.clone(),
                        result: crate::types::events::ToolResultData {
                            data: data.clone(),
                            is_error,
                        },
                    })
                    .await;
            }
        }

        // Handle stop reason
        match stop_reason {
            StopReason::ToolUse if !tool_use_blocks.is_empty() => {
                self.state = QueryState::ExecutingTools;
                Ok(TurnResult::ToolUse(tool_use_blocks))
            }
            StopReason::MaxTokens => self.handle_max_tokens(event_tx).await,
            _ => {
                // ── Stop hooks: check if we should actually stop ──────────────
                if !self.stop_hooks.is_empty() {
                    let hook_context = self.build_hook_context();
                    let result = self
                        .stop_hooks
                        .evaluate(&hook_context, self.stop_hook_active)
                        .await;

                    if result.prevent_continuation {
                        self.state = QueryState::Terminal {
                            stop_reason: stop_reason.clone(),
                            transition: TransitionReason::StopHookPrevented,
                        };
                        let _ = event_tx
                            .send(StreamEvent::Done {
                                stop_reason: stop_reason.clone(),
                            })
                            .await;
                        return Ok(TurnResult::Done(stop_reason));
                    }

                    if !result.blocking_errors.is_empty() {
                        for error_msg in &result.blocking_errors {
                            self.messages.push(serde_json::json!({
                                "role": "user",
                                "content": [{"type": "text", "text": error_msg}]
                            }));
                        }
                        self.stop_hook_active = true;
                        self.state = QueryState::Querying;
                        return Ok(TurnResult::StopHookBlocking);
                    }
                }
                self.stop_hook_active = false;

                // ── Token budget: check if we should auto-continue ────────────
                if let Some(ref mut tracker) = self.budget_tracker {
                    let decision = token_budget::check_token_budget(
                        tracker,
                        self.agent_id.is_some(),
                        self.token_budget,
                        self.cumulative_output_tokens,
                    );

                    match decision {
                        TokenBudgetDecision::Continue { nudge_message, .. } => {
                            tracing::debug!(
                                "token budget continuation: {}",
                                nudge_message
                            );
                            self.messages.push(serde_json::json!({
                                "role": "user",
                                "content": [{"type": "text", "text": nudge_message}]
                            }));
                            self.state = QueryState::Querying;
                            return Ok(TurnResult::TokenBudgetContinuation);
                        }
                        TokenBudgetDecision::Stop { completion_event } => {
                            if let Some(event) = completion_event {
                                tracing::info!(
                                    continuation_count = event.continuation_count,
                                    pct = event.pct,
                                    diminishing = event.diminishing_returns,
                                    "token budget completed"
                                );
                            }
                        }
                    }
                }

                self.state = QueryState::Terminal {
                    stop_reason: stop_reason.clone(),
                    transition: TransitionReason::Completed,
                };
                let _ = event_tx
                    .send(StreamEvent::Done {
                        stop_reason: stop_reason.clone(),
                    })
                    .await;
                Ok(TurnResult::Done(stop_reason))
            }
        }
    }

    /// Send a streaming request with retry logic (A1) including retry-after
    /// header parsing (A2), 529 non-streaming fallback, and prompt-too-long
    /// reactive compaction (Q11).
    async fn stream_with_retry(
        &mut self,
        api_messages: &[serde_json::Value],
        event_tx: &mpsc::Sender<StreamEvent>,
    ) -> Result<StreamResponse> {
        // First, try a prompt-too-long pre-check by attempting the request.
        // If it fails with prompt-too-long, compact and retry once before
        // entering the main retry loop.
        let result = self
            .api_client
            .stream_request_with_retry(
                api_messages,
                &self.system_prompt,
                &self.tool_schemas,
                &self.retry_policy,
                event_tx,
            )
            .await;

        match result {
            Ok(resp) => Ok(resp),
            Err(e) => {
                let error_msg = e.to_string();

                // ── Q11: Reactive compact on prompt-too-long ────────
                if is_prompt_too_long(&error_msg) {
                    tracing::warn!("prompt too long, running reactive compact");
                    let system_text = self.system_prompt_text();
                    match compact_messages(
                        &self.messages,
                        &CompactionStrategy::Reactive,
                        &self.compaction_config,
                        Some(&self.api_client),
                        &system_text,
                    )
                    .await
                    {
                        Ok(compaction_result) => {
                            self.messages = compaction_result.messages;
                            let _ = event_tx
                                .send(StreamEvent::Compacted {
                                    summary: compaction_result.summary,
                                })
                                .await;
                            // Re-normalize after compaction and retry once
                            let mut compacted_messages = normalize_messages(&self.messages);
                            add_cache_markers(&mut compacted_messages);
                            return self
                                .api_client
                                .stream_request_with_retry(
                                    &compacted_messages,
                                    &self.system_prompt,
                                    &self.tool_schemas,
                                    &self.retry_policy,
                                    event_tx,
                                )
                                .await;
                        }
                        Err(compact_err) => {
                            tracing::error!("reactive compact failed: {compact_err}");
                            return Err(e);
                        }
                    }
                }

                Err(e)
            }
        }
    }

    /// Parse a non-streaming `/v1/messages` response into assistant content,
    /// tool-use blocks, and stop reason — used by the 529 fallback path.
    async fn parse_non_streaming_response(
        value: &serde_json::Value,
        event_tx: &mpsc::Sender<StreamEvent>,
    ) -> (Vec<serde_json::Value>, Vec<ToolUseInfo>, StopReason) {
        let mut assistant_content: Vec<serde_json::Value> = Vec::new();
        let mut tool_use_blocks: Vec<ToolUseInfo> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;

        // Parse usage from the top-level message
        if let Some(usage) = value.get("usage") {
            let u = crate::types::usage::Usage {
                input_tokens: usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                output_tokens: usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                cache_creation_input_tokens: usage
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64()),
                cache_read_input_tokens: usage
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64()),
                server_tool_use: None,
                speed: None,
            };
            let _ = event_tx.send(StreamEvent::UsageUpdate(u)).await;
        }

        // Parse stop_reason
        if let Some(sr) = value.get("stop_reason").and_then(|v| v.as_str()) {
            stop_reason = match sr {
                "end_turn" => StopReason::EndTurn,
                "tool_use" => StopReason::ToolUse,
                "max_tokens" => StopReason::MaxTokens,
                "stop_sequence" => StopReason::StopSequence,
                _ => StopReason::EndTurn,
            };
        }

        // Parse content blocks
        if let Some(content) = value.get("content").and_then(|v| v.as_array()) {
            for block in content {
                let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match block_type {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                            let _ = event_tx
                                .send(StreamEvent::TextDelta {
                                    text: text.to_string(),
                                })
                                .await;
                            assistant_content.push(serde_json::json!({
                                "type": "text",
                                "text": text,
                            }));
                        }
                    }
                    "tool_use" => {
                        let id = block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let input = block
                            .get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        let _ = event_tx
                            .send(StreamEvent::ToolStart {
                                tool_use_id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                            })
                            .await;
                        tool_use_blocks.push(ToolUseInfo {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        });
                        assistant_content.push(serde_json::json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input,
                        }));
                    }
                    "thinking" => {
                        let thinking = block
                            .get("thinking")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let signature = block
                            .get("signature")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        assistant_content.push(serde_json::json!({
                            "type": "thinking",
                            "thinking": thinking,
                            "signature": signature,
                        }));
                    }
                    _ => {}
                }
            }
        }

        (assistant_content, tool_use_blocks, stop_reason)
    }

    /// Build a HookContext from the current engine state.
    fn build_hook_context(&self) -> HookContext {
        HookContext {
            messages: self.messages.clone(),
            system_prompt_text: self.system_prompt_text(),
            query_source: self.query_source.clone(),
            agent_id: self.agent_id.clone(),
            tracking: self.tracking.clone(),
        }
    }

    /// Parse raw JSON content blocks into typed ContentBlock values for hooks.
    fn parse_content_blocks(raw: &[serde_json::Value]) -> Vec<ContentBlock> {
        raw.iter()
            .filter_map(|v| serde_json::from_value::<ContentBlock>(v.clone()).ok())
            .collect()
    }

    async fn handle_max_tokens(
        &mut self,
        event_tx: &mpsc::Sender<StreamEvent>,
    ) -> Result<TurnResult> {
        // Stage 1: One-shot escalation (8k → 64k)
        if self.max_output_tokens_override.is_none() {
            self.max_output_tokens_override = Some(ESCALATED_MAX_TOKENS);
            self.state = QueryState::RecoveringMaxTokens {
                recovery_count: self.recovery_count,
                escalated: true,
            };
            return Ok(TurnResult::ContinueRecovery);
        }

        // Stage 2: Recovery loop (up to 3)
        if self.recovery_count < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT {
            self.recovery_count += 1;
            self.messages.push(serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "Output token limit hit. Resume directly \u{2014} no apology, no recap of what you were doing. Pick up mid-thought if that is where the cut happened. Break remaining work into smaller pieces."}]
            }));
            self.state = QueryState::RecoveringMaxTokens {
                recovery_count: self.recovery_count,
                escalated: true,
            };
            return Ok(TurnResult::ContinueRecovery);
        }

        // Stage 3: Exhausted
        self.state = QueryState::Terminal {
            stop_reason: StopReason::MaxTokens,
            transition: TransitionReason::Error(
                crate::types::error::QueryError::MaxTokensExhausted {
                    recovery_count: self.recovery_count,
                },
            ),
        };
        let _ = event_tx
            .send(StreamEvent::Done {
                stop_reason: StopReason::MaxTokens,
            })
            .await;
        Ok(TurnResult::Done(StopReason::MaxTokens))
    }
}

// ── Helper functions ────────────────────────────────────────────────────────

/// Check if the error indicates a prompt-too-long condition.
fn is_prompt_too_long(error_msg: &str) -> bool {
    error_msg.contains("prompt is too long")
        || error_msg.contains("prompt_too_long")
        || error_msg.contains("context_length_exceeded")
}

#[derive(Debug)]
pub struct ToolUseInfo {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug)]
pub enum TurnResult {
    /// Query complete
    Done(StopReason),
    /// Tools need to be executed, then continue
    ToolUse(Vec<ToolUseInfo>),
    /// Max tokens recovery — caller should call run_turn again
    ContinueRecovery,
    /// Stop hook injected blocking errors — caller should call run_turn again
    StopHookBlocking,
    /// Token budget not yet exhausted — caller should call run_turn again
    TokenBudgetContinuation,
}
