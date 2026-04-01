//! Context compaction strategies for managing conversation length.
//!
//! Provides four strategies:
//! - **Auto**: Send older messages to the API for summarization.
//! - **Micro**: Truncate large tool results inline without an API call.
//! - **Snip**: Drop oldest messages, keeping only recent ones with a boundary marker.
//! - **Reactive**: Two-phase — try micro first, then auto if still over budget.
//!
//! Additional capabilities:
//! - **Message grouping**: Groups related messages (tool use + result pairs) by API round.
//! - **Auto-compaction thresholds**: Token-based warning/error/blocking limits.
//! - **Compaction boundary markers**: Insert markers showing where compaction occurred.
//! - **Post-compact cleanup**: Reset caches and tracking state after compaction.
//! - **Compact warning suppression**: Suppress stale warnings after successful compaction.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use anyhow::Result;
use serde_json::Value;

use crate::api::client::ApiClient;
use crate::services::tool_use_summary::{self, ToolInfo};
use crate::utils::context;

// ── Constants ───────────────────────────────────────────────────────────────

/// Reserve this many tokens for output during compaction.
/// Based on p99.99 of compact summary output being 17,387 tokens.
const MAX_OUTPUT_TOKENS_FOR_SUMMARY: usize = 20_000;

/// Buffer tokens below the effective context window for auto-compact threshold.
const AUTOCOMPACT_BUFFER_TOKENS: usize = 13_000;

/// Buffer tokens for the warning threshold (below effective context).
pub const WARNING_THRESHOLD_BUFFER_TOKENS: usize = 20_000;

/// Buffer tokens for the error threshold (below effective context).
pub const ERROR_THRESHOLD_BUFFER_TOKENS: usize = 20_000;

/// Buffer tokens for the manual compact blocking limit.
pub const MANUAL_COMPACT_BUFFER_TOKENS: usize = 3_000;

/// Stop trying auto-compact after this many consecutive failures.
const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES: u32 = 3;

/// Maximum number of prompt-too-long retries during compaction.
const MAX_PTL_RETRIES: usize = 3;

/// Maximum retries for streaming compaction API calls.
const MAX_COMPACT_STREAMING_RETRIES: usize = 2;

/// Maximum number of files to restore post-compact.
pub const POST_COMPACT_MAX_FILES_TO_RESTORE: usize = 5;

/// Token budget for post-compact file restoration.
pub const POST_COMPACT_TOKEN_BUDGET: usize = 50_000;

/// Maximum tokens per file for post-compact restoration.
pub const POST_COMPACT_MAX_TOKENS_PER_FILE: usize = 5_000;

/// Approximate token size for image/document blocks.
const IMAGE_MAX_TOKEN_SIZE: usize = 2_000;

/// Marker text used when clearing old tool results (time-based microcompact).
pub const TIME_BASED_MC_CLEARED_MESSAGE: &str = "[Old tool result content cleared]";

/// Marker text for prompt-too-long retry truncation.
const PTL_RETRY_MARKER: &str = "[earlier conversation truncated for compaction retry]";

// ── Configuration ────────────────────────────────────────────────────────────

/// Which compaction strategy to use.
#[derive(Clone, Debug, Default)]
pub enum CompactionStrategy {
    /// Summarize older messages via an API call.
    #[default]
    Auto,
    /// Truncate large tool results inline (no API call).
    Micro,
    /// Drop oldest messages, keep only recent ones.
    Snip,
    /// Two-phase: micro first, then auto if still over budget.
    Reactive,
}

/// Tuning knobs for compaction behaviour.
#[derive(Clone, Debug)]
pub struct CompactionConfig {
    /// Maximum context tokens before compaction triggers.
    pub max_context_tokens: usize,
    /// Target token count after compaction.
    pub target_after_compact: usize,
    /// Micro-compact: truncate tool results larger than this.
    pub micro_truncate_threshold: usize,
    /// Snip-compact: number of recent message groups to keep.
    pub snip_keep_recent: usize,
    /// Token budget reserved for the summary output.
    pub summary_output_reserve: usize,
    /// Auto-compact: number of recent message groups to preserve.
    pub auto_keep_recent: usize,
    /// Whether auto-compact is enabled.
    pub auto_compact_enabled: bool,
    /// Micro-compact: number of recent tool results to keep.
    pub micro_keep_recent: usize,
}

impl CompactionConfig {
    /// Create a compaction config derived from the model's context window.
    ///
    /// Uses [`crate::utils::context::get_context_window_for_model`] to
    /// determine the correct thresholds. The auto-compact threshold is set
    /// to 80% of the context window (matching
    /// [`crate::utils::context::get_auto_compact_threshold`]).
    pub fn for_model(model: &str) -> Self {
        let window = context::get_context_window_for_model(model) as usize;
        let threshold = context::get_auto_compact_threshold(model) as usize;
        Self {
            max_context_tokens: threshold,
            target_after_compact: window * 40 / 100, // 40% of window
            summary_output_reserve: context::COMPACT_MAX_OUTPUT_TOKENS as usize,
            ..Default::default()
        }
    }
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 180_000,
            target_after_compact: 80_000,
            micro_truncate_threshold: 8_000,
            snip_keep_recent: 4,
            summary_output_reserve: 20_000,
            auto_keep_recent: 4,
            auto_compact_enabled: true,
            micro_keep_recent: 10,
        }
    }
}

// ── Result type ──────────────────────────────────────────────────────────────

/// Outcome of a compaction operation.
#[derive(Clone, Debug)]
pub struct CompactionResult {
    /// The compacted messages array.
    pub messages: Vec<Value>,
    /// Human-readable summary of what was compacted.
    pub summary: String,
    /// Estimated token count before compaction.
    pub pre_compact_tokens: usize,
    /// Estimated token count after compaction.
    pub post_compact_tokens: usize,
    /// Whether a compact boundary marker was inserted.
    pub has_boundary_marker: bool,
    /// The trigger that caused this compaction.
    pub trigger: CompactionTrigger,
}

/// What triggered the compaction.
#[derive(Clone, Debug, Default)]
pub enum CompactionTrigger {
    #[default]
    Manual,
    Auto,
    Reactive,
    PromptTooLong,
}

// ── Auto-compact tracking state ─────────────────────────────────────────────

/// Tracks auto-compact state across turns in the query loop.
#[derive(Clone, Debug)]
pub struct AutoCompactTrackingState {
    /// Whether compaction occurred on the current chain.
    pub compacted: bool,
    /// Turns since last compaction.
    pub turn_counter: u32,
    /// Unique ID for the current turn.
    pub turn_id: String,
    /// Consecutive auto-compact failures. Reset on success.
    pub consecutive_failures: u32,
}

impl Default for AutoCompactTrackingState {
    fn default() -> Self {
        Self {
            compacted: false,
            turn_counter: 0,
            turn_id: String::new(),
            consecutive_failures: 0,
        }
    }
}

// ── Token warning state ─────────────────────────────────────────────────────

/// Token usage warning thresholds for the context window.
#[derive(Clone, Debug)]
pub struct TokenWarningState {
    /// Percentage of context window remaining (0-100).
    pub percent_left: u8,
    /// Whether token usage is above the warning threshold.
    pub is_above_warning_threshold: bool,
    /// Whether token usage is above the error threshold.
    pub is_above_error_threshold: bool,
    /// Whether auto-compact should trigger.
    pub is_above_auto_compact_threshold: bool,
    /// Whether the user is at the blocking limit (can't send more).
    pub is_at_blocking_limit: bool,
}

/// Calculate token warning state for the given usage and model.
pub fn calculate_token_warning_state(token_usage: usize, model: &str) -> TokenWarningState {
    let effective_window = get_effective_context_window_size(model);
    let auto_compact_threshold = get_auto_compact_threshold(model);
    let threshold = if is_auto_compact_enabled() {
        auto_compact_threshold
    } else {
        effective_window
    };

    let percent_left = if token_usage >= threshold {
        0
    } else {
        ((threshold - token_usage) as f64 / threshold as f64 * 100.0).round() as u8
    };

    let warning_threshold = threshold.saturating_sub(WARNING_THRESHOLD_BUFFER_TOKENS);
    let error_threshold = threshold.saturating_sub(ERROR_THRESHOLD_BUFFER_TOKENS);
    let blocking_limit = effective_window.saturating_sub(MANUAL_COMPACT_BUFFER_TOKENS);

    TokenWarningState {
        percent_left,
        is_above_warning_threshold: token_usage >= warning_threshold,
        is_above_error_threshold: token_usage >= error_threshold,
        is_above_auto_compact_threshold: is_auto_compact_enabled()
            && token_usage >= auto_compact_threshold,
        is_at_blocking_limit: token_usage >= blocking_limit,
    }
}

/// Get the effective context window size minus reserved output tokens.
pub fn get_effective_context_window_size(model: &str) -> usize {
    let reserved = MAX_OUTPUT_TOKENS_FOR_SUMMARY.min(
        context::COMPACT_MAX_OUTPUT_TOKENS as usize,
    );
    let window = context::get_context_window_for_model(model) as usize;
    window.saturating_sub(reserved)
}

/// Calculate the auto-compact threshold for a model.
pub fn get_auto_compact_threshold(model: &str) -> usize {
    let effective_window = get_effective_context_window_size(model);
    effective_window.saturating_sub(AUTOCOMPACT_BUFFER_TOKENS)
}

// ── Compact warning suppression ─────────────────────────────────────────────

/// Global flag to suppress compact warnings after successful compaction.
/// We suppress immediately after compaction since we don't have accurate
/// token counts until the next API response.
static COMPACT_WARNING_SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// Global counter for consecutive auto-compact failures (circuit breaker).
static AUTO_COMPACT_FAILURE_COUNT: AtomicU32 = AtomicU32::new(0);

/// Suppress the compact warning. Call after successful compaction.
pub fn suppress_compact_warning() {
    COMPACT_WARNING_SUPPRESSED.store(true, Ordering::Relaxed);
}

/// Clear the compact warning suppression. Called at start of new compact attempt.
pub fn clear_compact_warning_suppression() {
    COMPACT_WARNING_SUPPRESSED.store(false, Ordering::Relaxed);
}

/// Check whether the compact warning is currently suppressed.
pub fn is_compact_warning_suppressed() -> bool {
    COMPACT_WARNING_SUPPRESSED.load(Ordering::Relaxed)
}

/// Check whether auto-compact is enabled (respects environment variables).
pub fn is_auto_compact_enabled() -> bool {
    if std::env::var("DISABLE_COMPACT").as_deref() == Ok("1") {
        return false;
    }
    if std::env::var("DISABLE_AUTO_COMPACT").as_deref() == Ok("1") {
        return false;
    }
    true
}

/// Record a consecutive auto-compact failure. Returns the new failure count.
pub fn record_auto_compact_failure() -> u32 {
    let count = AUTO_COMPACT_FAILURE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if count >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES {
        tracing::warn!(
            "auto-compact circuit breaker tripped after {} consecutive failures",
            count
        );
    }
    count
}

/// Reset the auto-compact failure count (call after successful compaction).
pub fn reset_auto_compact_failures() {
    AUTO_COMPACT_FAILURE_COUNT.store(0, Ordering::Relaxed);
}

/// Check whether the auto-compact circuit breaker has tripped.
pub fn is_auto_compact_circuit_broken() -> bool {
    AUTO_COMPACT_FAILURE_COUNT.load(Ordering::Relaxed) >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES
}

// ── Token estimation ─────────────────────────────────────────────────────────

/// Estimate token count for a JSON value.
///
/// Delegates to [`crate::services::token_estimation::rough_token_count_estimation`]
/// for the underlying calculation, keeping this function as the convenience
/// wrapper that the compaction code already calls everywhere.
pub fn estimate_tokens(value: &Value) -> usize {
    estimate_tokens_for_text(&value.to_string())
}

/// Estimate tokens for a text string.
///
/// Uses the canonical rough estimator from `services::token_estimation`.
pub fn estimate_tokens_for_text(text: &str) -> usize {
    crate::services::token_estimation::rough_token_count_estimation(text, 4)
}

/// Estimate tokens for file content using file-type-aware bytes-per-token ratio.
///
/// JSON/XML files use 2 bytes per token; code/text files use 4.
pub fn estimate_tokens_for_file(content: &str, extension: &str) -> usize {
    crate::services::token_estimation::rough_token_count_for_file_type(content, extension)
}

/// Estimate token count for a slice of messages, with a conservative 4/3 padding.
///
/// Walks every content block in each message, handling text, tool_result,
/// tool_use, thinking, redacted_thinking, image, and document blocks.
/// Matches the TS `estimateMessageTokens`.
pub fn estimate_message_tokens(messages: &[Value]) -> usize {
    let mut total: usize = 0;

    for msg in messages {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }

        let content = match msg.get("content").and_then(|c| c.as_array()) {
            Some(c) => c,
            None => continue,
        };

        for block in content {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        total += estimate_tokens_for_text(text);
                    }
                }
                "tool_result" => {
                    total += calculate_tool_result_tokens(block);
                }
                "image" | "document" => {
                    total += IMAGE_MAX_TOKEN_SIZE;
                }
                "thinking" => {
                    if let Some(text) = block.get("thinking").and_then(|t| t.as_str()) {
                        total += estimate_tokens_for_text(text);
                    }
                }
                "redacted_thinking" => {
                    if let Some(data) = block.get("data").and_then(|t| t.as_str()) {
                        total += estimate_tokens_for_text(data);
                    }
                }
                "tool_use" => {
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let input = block
                        .get("input")
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "{}".to_string());
                    total += estimate_tokens_for_text(&format!("{name}{input}"));
                }
                _ => {
                    // server_tool_use, web_search_tool_result, etc.
                    total += estimate_tokens_for_text(&block.to_string());
                }
            }
        }
    }

    // Pad estimate by 4/3 to be conservative
    (total as f64 * 4.0 / 3.0).ceil() as usize
}

/// Calculate token count for a tool_result content block.
fn calculate_tool_result_tokens(block: &Value) -> usize {
    let content = match block.get("content") {
        Some(c) => c,
        None => return 0,
    };

    if let Some(text) = content.as_str() {
        return estimate_tokens_for_text(text);
    }

    if let Some(arr) = content.as_array() {
        return arr
            .iter()
            .map(|item| {
                let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match item_type {
                    "text" => {
                        let text = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        estimate_tokens_for_text(text)
                    }
                    "image" | "document" => IMAGE_MAX_TOKEN_SIZE,
                    _ => 0,
                }
            })
            .sum();
    }

    0
}

/// Check whether the current message set should trigger auto-compaction.
pub fn should_auto_compact(messages: &[Value], config: &CompactionConfig) -> bool {
    if !config.auto_compact_enabled || !is_auto_compact_enabled() {
        return false;
    }
    if is_auto_compact_circuit_broken() {
        return false;
    }
    let total = estimate_message_tokens(messages);
    total > config.max_context_tokens
}

// ── Message grouping ────────────────────────────────────────────────────────

/// Groups messages at API-round boundaries: one group per API round-trip.
///
/// A boundary fires when a NEW assistant response begins (different message id
/// from the prior assistant). For well-formed conversations this is an API-safe
/// split point — the API contract requires every tool_use to be resolved before
/// the next assistant turn, so pairing validity falls out of the assistant-id
/// boundary.
///
/// This matches the TS `groupMessagesByApiRound` from `grouping.ts`.
pub fn group_messages_by_api_round(messages: &[Value]) -> Vec<Vec<Value>> {
    let mut groups: Vec<Vec<Value>> = Vec::new();
    let mut current: Vec<Value> = Vec::new();
    let mut last_assistant_id: Option<String> = None;

    for msg in messages {
        let is_assistant = msg.get("role").and_then(|r| r.as_str()) == Some("assistant");
        let msg_id = msg.get("id").and_then(|id| id.as_str()).map(String::from);

        if is_assistant && msg_id != last_assistant_id && !current.is_empty() {
            groups.push(std::mem::take(&mut current));
            current.push(msg.clone());
        } else {
            current.push(msg.clone());
        }

        if is_assistant {
            last_assistant_id = msg_id;
        }
    }

    if !current.is_empty() {
        groups.push(current);
    }

    groups
}

/// Estimate the total token count for a group of messages.
fn estimate_group_tokens(group: &[Value]) -> usize {
    group.iter().map(estimate_tokens).sum()
}

// ── Compactable tool tracking ───────────────────────────────────────────────

/// Tools whose results can be safely truncated during micro-compaction.
/// Other tools (e.g. Agent, SendMessage) keep their results to preserve context.
const COMPACTABLE_TOOLS: &[&str] = &[
    "Read",
    "Bash",
    "Grep",
    "Glob",
    "WebSearch",
    "WebFetch",
    "Edit",
    "Write",
];

/// Walk messages and collect tool_use IDs whose tool name is in
/// COMPACTABLE_TOOLS, in encounter order. Shared by both microcompact paths.
pub fn collect_compactable_tool_ids(messages: &[Value]) -> Vec<String> {
    let mut ids = Vec::new();
    for msg in messages {
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    if let (Some(id), Some(name)) = (
                        block.get("id").and_then(|v| v.as_str()),
                        block.get("name").and_then(|v| v.as_str()),
                    ) {
                        if COMPACTABLE_TOOLS.contains(&name) {
                            ids.push(id.to_string());
                        }
                    }
                }
            }
        }
    }
    ids
}

/// Build a map of tool_use_id -> tool_name from assistant messages.
fn build_tool_name_map(messages: &[Value]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for msg in messages {
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    if let (Some(id), Some(name)) = (
                        block.get("id").and_then(|v| v.as_str()),
                        block.get("name").and_then(|v| v.as_str()),
                    ) {
                        map.insert(id.to_string(), name.to_string());
                    }
                }
            }
        }
    }
    map
}

// ── Strip images from messages ──────────────────────────────────────────────

/// Strip image and document blocks from user messages before sending for compaction.
///
/// Images are not needed for generating a conversation summary and can
/// cause the compaction API call itself to hit the prompt-too-long limit.
/// Replaces image blocks with a text marker so the summary still notes
/// that an image was shared.
pub fn strip_images_from_messages(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .map(|msg| {
            if msg.get("role").and_then(|r| r.as_str()) != Some("user") {
                return msg.clone();
            }

            let content = match msg.get("content").and_then(|c| c.as_array()) {
                Some(c) => c,
                None => return msg.clone(),
            };

            let mut has_media = false;
            let new_content: Vec<Value> = content
                .iter()
                .flat_map(|block| {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match block_type {
                        "image" => {
                            has_media = true;
                            vec![serde_json::json!({"type": "text", "text": "[image]"})]
                        }
                        "document" => {
                            has_media = true;
                            vec![serde_json::json!({"type": "text", "text": "[document]"})]
                        }
                        "tool_result" => {
                            if let Some(inner) = block.get("content").and_then(|c| c.as_array()) {
                                let mut tool_has_media = false;
                                let new_inner: Vec<Value> = inner
                                    .iter()
                                    .map(|item| {
                                        let item_type = item
                                            .get("type")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("");
                                        match item_type {
                                            "image" => {
                                                tool_has_media = true;
                                                serde_json::json!({"type": "text", "text": "[image]"})
                                            }
                                            "document" => {
                                                tool_has_media = true;
                                                serde_json::json!({"type": "text", "text": "[document]"})
                                            }
                                            _ => item.clone(),
                                        }
                                    })
                                    .collect();
                                if tool_has_media {
                                    has_media = true;
                                    let mut new_block = block.clone();
                                    new_block["content"] = Value::Array(new_inner);
                                    vec![new_block]
                                } else {
                                    vec![block.clone()]
                                }
                            } else {
                                vec![block.clone()]
                            }
                        }
                        _ => vec![block.clone()],
                    }
                })
                .collect();

            if !has_media {
                return msg.clone();
            }

            let mut new_msg = msg.clone();
            new_msg["content"] = Value::Array(new_content);
            new_msg
        })
        .collect()
}

// ── Compact prompt (ported from original TS) ────────────────────────────────

/// Aggressive no-tools preamble. With maxTurns: 1 a denied tool call means
/// no text output, so put this FIRST and make it explicit about rejection.
const NO_TOOLS_PREAMBLE: &str = r#"CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.

- Do NOT use Read, Bash, Grep, Glob, Edit, Write, or ANY other tool.
- You already have all the context you need in the conversation above.
- Tool calls will be REJECTED and will waste your only turn — you will fail the task.
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.

"#;

const DETAILED_ANALYSIS_INSTRUCTION_BASE: &str = r#"Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Chronologically analyze each message and section of the conversation. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly."#;

const DETAILED_ANALYSIS_INSTRUCTION_PARTIAL: &str = r#"Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Analyze the recent messages chronologically. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly."#;

const NO_TOOLS_TRAILER: &str = "\n\nREMINDER: Do NOT call any tools. Respond with plain text only \
    — an <analysis> block followed by a <summary> block. \
    Tool calls will be rejected and you will fail the task.";

/// Direction for partial compaction: summarize messages "from" the split point
/// onwards, or "up_to" the split point.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PartialCompactDirection {
    /// Summarize the recent messages after the retained prefix.
    #[default]
    From,
    /// Summarize the prefix that will be replaced (cache-sharing path).
    UpTo,
}

/// Build the full (base) compact prompt, matching the original TS `getCompactPrompt`.
pub fn get_compact_prompt(custom_instructions: Option<&str>) -> String {
    let base = format!(
        r#"Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions.
This summary should be thorough in capturing technical details, code patterns, and architectural decisions that would be essential for continuing development work without losing context.

{DETAILED_ANALYSIS_INSTRUCTION_BASE}

Your summary should include the following sections:

1. Primary Request and Intent: Capture all of the user's explicit requests and intents in detail
2. Key Technical Concepts: List all important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Pay special attention to the most recent messages and include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List all errors that you ran into, and how you fixed them. Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages that are not tool results. These are critical for understanding the users' feedback and changing intent.
7. Pending Tasks: Outline any pending tasks that you have explicitly been asked to work on.
8. Current Work: Describe in detail precisely what was being worked on immediately before this summary request, paying special attention to the most recent messages from both user and assistant. Include file names and code snippets where applicable.
9. Optional Next Step: List the next step that you will take that is related to the most recent work you were doing. IMPORTANT: ensure that this step is DIRECTLY in line with the user's most recent explicit requests, and the task you were working on immediately before this summary request. If your last task was concluded, then only list next steps if they are explicitly in line with the users request. Do not start on tangential requests or really old requests that were already completed without confirming with the user first.
                       If there is a next step, include direct quotes from the most recent conversation showing exactly what task you were working on and where you left off. This should be verbatim to ensure there's no drift in task interpretation.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]
   - [...]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Summary of the changes made to this file, if any]
      - [Important Code Snippet]
   - [File Name 2]
      - [Important Code Snippet]
   - [...]

4. Errors and fixes:
    - [Detailed description of error 1]:
      - [How you fixed the error]
      - [User feedback on the error if any]
    - [...]

5. Problem Solving:
   [Description of solved problems and ongoing troubleshooting]

6. All user messages:
    - [Detailed non tool use user message]
    - [...]

7. Pending Tasks:
   - [Task 1]
   - [Task 2]
   - [...]

8. Current Work:
   [Precise description of current work]

9. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>

Please provide your summary based on the conversation so far, following this structure and ensuring precision and thoroughness in your response.

There may be additional summarization instructions provided in the included context. If so, remember to follow these instructions when creating the above summary. Examples of instructions include:
<example>
## Compact Instructions
When summarizing the conversation focus on typescript code changes and also remember the mistakes you made and how you fixed them.
</example>

<example>
# Summary instructions
When you are using compact - please focus on test output and code changes. Include file reads verbatim.
</example>
"#
    );

    let mut prompt = format!("{NO_TOOLS_PREAMBLE}{base}");

    if let Some(instructions) = custom_instructions {
        let trimmed = instructions.trim();
        if !trimmed.is_empty() {
            prompt.push_str(&format!("\n\nAdditional Instructions:\n{trimmed}"));
        }
    }

    prompt.push_str(NO_TOOLS_TRAILER);
    prompt
}

/// Build the partial compact prompt, matching the original TS `getPartialCompactPrompt`.
pub fn get_partial_compact_prompt(
    custom_instructions: Option<&str>,
    direction: &PartialCompactDirection,
) -> String {
    let template = match direction {
        PartialCompactDirection::UpTo => format!(
            r#"Your task is to create a detailed summary of this conversation. This summary will be placed at the start of a continuing session; newer messages that build on this context will follow after your summary (you do not see them here). Summarize thoroughly so that someone reading only your summary and then the newer messages can fully understand what happened and continue the work.

{DETAILED_ANALYSIS_INSTRUCTION_BASE}

Your summary should include the following sections:

1. Primary Request and Intent: Capture the user's explicit requests and intents in detail
2. Key Technical Concepts: List important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List errors encountered and how they were fixed.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages that are not tool results.
7. Pending Tasks: Outline any pending tasks.
8. Work Completed: Describe what was accomplished by the end of this portion.
9. Context for Continuing Work: Summarize any context, decisions, or state that would be needed to understand and continue the work in subsequent messages.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Important Code Snippet]

4. Errors and fixes:
    - [Error description]:
      - [How you fixed it]

5. Problem Solving:
   [Description]

6. All user messages:
    - [Detailed non tool use user message]

7. Pending Tasks:
   - [Task 1]

8. Work Completed:
   [Description of what was accomplished]

9. Context for Continuing Work:
   [Key context, decisions, or state needed to continue the work]

</summary>
</example>

Please provide your summary following this structure, ensuring precision and thoroughness in your response.
"#
        ),
        PartialCompactDirection::From => format!(
            r#"Your task is to create a detailed summary of the RECENT portion of the conversation — the messages that follow earlier retained context. The earlier messages are being kept intact and do NOT need to be summarized. Focus your summary on what was discussed, learned, and accomplished in the recent messages only.

{DETAILED_ANALYSIS_INSTRUCTION_PARTIAL}

Your summary should include the following sections:

1. Primary Request and Intent: Capture the user's explicit requests and intents from the recent messages
2. Key Technical Concepts: List important technical concepts, technologies, and frameworks discussed recently.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List errors encountered and how they were fixed.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages from the recent portion that are not tool results.
7. Pending Tasks: Outline any pending tasks from the recent messages.
8. Current Work: Describe precisely what was being worked on immediately before this summary request.
9. Optional Next Step: List the next step related to the most recent work. Include direct quotes from the most recent conversation.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Important Code Snippet]

4. Errors and fixes:
    - [Error description]:
      - [How you fixed it]

5. Problem Solving:
   [Description]

6. All user messages:
    - [Detailed non tool use user message]

7. Pending Tasks:
   - [Task 1]

8. Current Work:
   [Precise description of current work]

9. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>

Please provide your summary based on the RECENT messages only (after the retained earlier context), following this structure and ensuring precision and thoroughness in your response.
"#
        ),
    };

    let mut prompt = format!("{NO_TOOLS_PREAMBLE}{template}");

    if let Some(instructions) = custom_instructions {
        let trimmed = instructions.trim();
        if !trimmed.is_empty() {
            prompt.push_str(&format!("\n\nAdditional Instructions:\n{trimmed}"));
        }
    }

    prompt.push_str(NO_TOOLS_TRAILER);
    prompt
}

/// Build the user summary message that replaces the compacted portion,
/// matching TS `getCompactUserSummaryMessage`.
pub fn get_compact_user_summary_message(
    summary: &str,
    suppress_follow_up_questions: bool,
    transcript_path: Option<&str>,
    recent_messages_preserved: bool,
) -> String {
    let formatted = format_compact_summary(summary);

    let mut base = format!(
        "This session is being continued from a previous conversation that ran out of context. \
         The summary below covers the earlier portion of the conversation.\n\n{formatted}"
    );

    if let Some(path) = transcript_path {
        base.push_str(&format!(
            "\n\nIf you need specific details from before compaction (like exact code snippets, \
             error messages, or content you generated), read the full transcript at: {path}"
        ));
    }

    if recent_messages_preserved {
        base.push_str("\n\nRecent messages are preserved verbatim.");
    }

    if suppress_follow_up_questions {
        base.push_str(
            "\nContinue the conversation from where it left off without asking the user any \
             further questions. Resume directly — do not acknowledge the summary, do not recap \
             what was happening, do not preface with \"I'll continue\" or similar. Pick up the \
             last task as if the break never happened.",
        );
    }

    base
}

/// Merge user-supplied custom instructions with hook-provided instructions.
/// User instructions come first; hook instructions are appended.
/// Empty strings normalize to None.
pub fn merge_hook_instructions(
    user_instructions: Option<&str>,
    hook_instructions: Option<&str>,
) -> Option<String> {
    match (user_instructions.filter(|s| !s.is_empty()), hook_instructions.filter(|s| !s.is_empty())) {
        (None, None) => None,
        (Some(u), None) => Some(u.to_string()),
        (None, Some(h)) => Some(h.to_string()),
        (Some(u), Some(h)) => Some(format!("{u}\n\n{h}")),
    }
}

// ── Main entry point ─────────────────────────────────────────────────────────

/// Compact the given messages using the specified strategy.
///
/// For strategies that need an API call (`Auto`, `Reactive`), an `ApiClient`
/// and `system_prompt` must be provided.
pub async fn compact_messages(
    messages: &[Value],
    strategy: &CompactionStrategy,
    config: &CompactionConfig,
    api_client: Option<&ApiClient>,
    system_prompt: &str,
) -> Result<CompactionResult> {
    // Clear warning suppression at the start of a new compact attempt.
    clear_compact_warning_suppression();

    let result = match strategy {
        CompactionStrategy::Auto => auto_compact(messages, config, api_client, system_prompt).await,
        CompactionStrategy::Micro => Ok(micro_compact(messages, config)),
        CompactionStrategy::Snip => Ok(snip_compact(messages, config)),
        CompactionStrategy::Reactive => {
            reactive_compact(messages, config, api_client, system_prompt).await
        }
    };

    // On success, suppress the compact warning and reset failure count.
    if result.is_ok() {
        suppress_compact_warning();
        reset_auto_compact_failures();
    }

    result
}

/// Auto-compact if the context exceeds the threshold.
///
/// Returns `Ok(Some(result))` if compaction was performed, `Ok(None)` if not needed.
/// Implements the circuit breaker pattern: stops retrying after
/// `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES` consecutive failures.
pub async fn auto_compact_if_needed(
    messages: &[Value],
    config: &CompactionConfig,
    api_client: Option<&ApiClient>,
    system_prompt: &str,
    tracking: &mut AutoCompactTrackingState,
) -> Result<Option<CompactionResult>> {
    if std::env::var("DISABLE_COMPACT").as_deref() == Ok("1") {
        return Ok(None);
    }

    // Circuit breaker: stop retrying after N consecutive failures.
    if tracking.consecutive_failures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES {
        return Ok(None);
    }

    if !should_auto_compact(messages, config) {
        return Ok(None);
    }

    match compact_messages(
        messages,
        &CompactionStrategy::Auto,
        config,
        api_client,
        system_prompt,
    )
    .await
    {
        Ok(result) => {
            tracking.compacted = true;
            tracking.consecutive_failures = 0;
            run_post_compact_cleanup();
            Ok(Some(result))
        }
        Err(e) => {
            tracking.consecutive_failures += 1;
            if tracking.consecutive_failures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES {
                tracing::warn!(
                    "auto-compact circuit breaker tripped after {} consecutive failures — \
                     skipping future attempts this session",
                    tracking.consecutive_failures
                );
            }
            Err(e)
        }
    }
}

// ── Auto compact ─────────────────────────────────────────────────────────────

async fn auto_compact(
    messages: &[Value],
    config: &CompactionConfig,
    api_client: Option<&ApiClient>,
    _system_prompt: &str,
) -> Result<CompactionResult> {
    let pre_tokens = estimate_message_tokens(messages);

    if messages.is_empty() {
        anyhow::bail!("Not enough messages to compact.");
    }

    // Use API-round grouping to find a safe split point that preserves
    // tool_use/tool_result pairs.
    let groups = group_messages_by_api_round(messages);
    let keep_groups = config.auto_keep_recent.min(groups.len());
    let split_at = groups.len().saturating_sub(keep_groups);

    let older: Vec<Value> = groups[..split_at].iter().flatten().cloned().collect();
    let recent: Vec<Value> = groups[split_at..].iter().flatten().cloned().collect();

    if older.is_empty() {
        anyhow::bail!("Not enough messages to compact.");
    }

    let api_client =
        api_client.ok_or_else(|| anyhow::anyhow!("auto-compact requires an API client"))?;

    // Strip images before sending to the summarizer — they waste tokens
    // and can cause the compaction call itself to hit prompt-too-long.
    let stripped_older = strip_images_from_messages(&older);

    // Use the exact structured prompt from the original TS implementation
    let compact_prompt = get_compact_prompt(None);

    // Extract tool use info from older messages for the summary annotation
    let tool_infos = extract_tool_infos_from_messages(&older);
    let tool_summary_annotation = if !tool_infos.is_empty() {
        tool_use_summary::generate_simple_summary(&tool_infos)
            .map(|s| format!("\n\nTool activity summary: {s}"))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let summary_messages = vec![serde_json::json!({
        "role": "user",
        "content": [{
            "type": "text",
            "text": format!("{compact_prompt}{tool_summary_annotation}\n\nMessages to summarize:\n{}", serde_json::to_string_pretty(&stripped_older)?)
        }]
    })];

    // Retry loop for prompt-too-long errors during summarization.
    let mut messages_to_summarize = summary_messages;
    let mut ptl_attempts = 0;
    let response;

    loop {
        match api_client
            .send_request(&messages_to_summarize, &[], &[])
            .await
        {
            Ok(resp) => {
                let text = extract_text_from_response(&resp);
                if is_prompt_too_long_response(&text) && ptl_attempts < MAX_PTL_RETRIES {
                    ptl_attempts += 1;
                    // Truncate the oldest 20% of the content and retry
                    if let Some(truncated) =
                        truncate_head_for_ptl_retry(&stripped_older, ptl_attempts)
                    {
                        messages_to_summarize = vec![serde_json::json!({
                            "role": "user",
                            "content": [{
                                "type": "text",
                                "text": format!("{compact_prompt}{tool_summary_annotation}\n\nMessages to summarize:\n{}", serde_json::to_string_pretty(&truncated)?)
                            }]
                        })];
                        continue;
                    }
                }
                response = resp;
                break;
            }
            Err(e) => {
                return Err(e);
            }
        }
    }

    let response_text = extract_text_from_response(&response);
    if response_text.is_empty() {
        anyhow::bail!("Failed to generate conversation summary - response did not contain valid text content");
    }

    let summary_text = format_compact_summary(&response_text);

    // Build the user summary message matching the TS format
    let user_summary = get_compact_user_summary_message(
        &response_text,
        true, // suppress follow-up questions
        None, // transcript path (set by caller if available)
        !recent.is_empty(),
    );

    // Build replacement messages: boundary marker, summary, then recent messages
    let mut compacted = Vec::with_capacity(3 + recent.len());

    // Insert compact boundary marker
    let boundary = create_compact_boundary_message("auto", pre_tokens);
    compacted.push(boundary);

    compacted.push(serde_json::json!({
        "role": "user",
        "content": [{
            "type": "text",
            "text": user_summary,
        }]
    }));
    // Need an assistant ack so the conversation alternates properly
    compacted.push(serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "Understood. I have the context from the summary. How can I help?"}]
    }));
    compacted.extend(recent);

    let post_tokens = estimate_message_tokens(&compacted);

    Ok(CompactionResult {
        messages: compacted,
        summary: summary_text,
        pre_compact_tokens: pre_tokens,
        post_compact_tokens: post_tokens,
        has_boundary_marker: true,
        trigger: CompactionTrigger::Auto,
    })
}

// ── Micro compact ────────────────────────────────────────────────────────────

/// Run micro-compaction: truncate large tool results and merge adjacent thinking blocks.
///
/// This is also used as a per-turn pre-pass to keep context lean.
/// Matches the TS microcompactMessages — walks messages and content-clears
/// old compactable tool results, keeping only the most recent N.
pub fn micro_compact(messages: &[Value], config: &CompactionConfig) -> CompactionResult {
    let pre_tokens = estimate_message_tokens(messages);

    let tool_names = build_tool_name_map(messages);

    // Collect compactable tool IDs in encounter order
    let compactable_ids = collect_compactable_tool_ids(messages);

    // Keep only the most recent N tool results, clear the rest
    let keep_recent = config.micro_keep_recent.max(1);
    let keep_set: HashSet<String> = compactable_ids
        .iter()
        .rev()
        .take(keep_recent)
        .cloned()
        .collect();
    let clear_set: HashSet<String> = compactable_ids
        .iter()
        .filter(|id| !keep_set.contains(*id))
        .cloned()
        .collect();

    let mut tokens_saved: usize = 0;

    let compacted: Vec<Value> = messages
        .iter()
        .map(|msg| {
            let mut msg = msg.clone();
            if msg.get("role").and_then(|r| r.as_str()) != Some("user") {
                // Also truncate large tool results based on threshold
                return truncate_tool_results(&msg, config.micro_truncate_threshold, &tool_names);
            }

            if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
                let mut touched = false;
                for block in content.iter_mut() {
                    if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                        continue;
                    }
                    let tool_use_id = block
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Clear results for tools in the clear set
                    if clear_set.contains(&tool_use_id) {
                        let current_content = block.get("content");
                        let already_cleared = current_content
                            .and_then(|c| c.as_str())
                            .map(|s| s == TIME_BASED_MC_CLEARED_MESSAGE)
                            .unwrap_or(false);

                        if !already_cleared {
                            let block_tokens = calculate_tool_result_tokens(block);
                            tokens_saved += block_tokens;
                            touched = true;

                            // Generate a summary label if we know the tool name
                            let label = tool_names
                                .get(&tool_use_id)
                                .and_then(|name| {
                                    let input = block
                                        .get("input")
                                        .cloned()
                                        .unwrap_or(serde_json::json!({}));
                                    tool_use_summary::generate_simple_summary(&[ToolInfo {
                                        name: name.clone(),
                                        input,
                                        output: serde_json::json!("[truncated]"),
                                    }])
                                })
                                .unwrap_or_else(|| TIME_BASED_MC_CLEARED_MESSAGE.to_string());

                            block["content"] = serde_json::json!([{
                                "type": "text",
                                "text": label
                            }]);
                        }
                        continue;
                    }

                    // For kept results, still apply the threshold-based truncation
                    let _ = touched; // avoid unused warning
                }

                if !touched {
                    // Apply threshold-based truncation for non-cleared results
                    return truncate_tool_results(&msg, config.micro_truncate_threshold, &tool_names);
                }
            }

            msg
        })
        .collect();

    // Merge adjacent thinking blocks
    let compacted = merge_reasoning_blocks(compacted);
    let post_tokens = estimate_message_tokens(&compacted);

    CompactionResult {
        messages: compacted,
        summary: format!(
            "Micro-compacted: {pre_tokens} -> {post_tokens} tokens \
             (cleared {} tool results, ~{tokens_saved} tokens saved)",
            clear_set.len()
        ),
        pre_compact_tokens: pre_tokens,
        post_compact_tokens: post_tokens,
        has_boundary_marker: false,
        trigger: CompactionTrigger::Manual,
    }
}

/// Truncate tool result content blocks that exceed the threshold.
/// Only truncates results from tools in `COMPACTABLE_TOOLS`.
fn truncate_tool_results(
    msg: &Value,
    threshold: usize,
    tool_names: &HashMap<String, String>,
) -> Value {
    let mut msg = msg.clone();
    if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
        for block in content.iter_mut() {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                // Only truncate results from compactable tools
                let tool_use_id = block
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let is_compactable = tool_names
                    .get(&tool_use_id)
                    .map(|name| COMPACTABLE_TOOLS.contains(&name.as_str()))
                    .unwrap_or(true); // If we can't find the tool name, default to compactable

                if !is_compactable {
                    continue;
                }

                // Read these before taking a mutable borrow on "content"
                let tool_name = tool_names
                    .get(&tool_use_id)
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());
                let input = block
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));

                if let Some(inner) = block.get_mut("content") {
                    let text = inner.to_string();
                    let tokens = estimate_tokens_for_text(&text);
                    if tokens > threshold {
                        let summary_label = tool_use_summary::generate_simple_summary(&[
                            ToolInfo {
                                name: tool_name,
                                input,
                                output: serde_json::json!("[truncated]"),
                            },
                        ])
                        .unwrap_or_else(|| TIME_BASED_MC_CLEARED_MESSAGE.to_string());

                        *inner = serde_json::json!([{
                            "type": "text",
                            "text": summary_label
                        }]);
                    }
                }
            }
        }
    }
    msg
}

// ── Snip compact ─────────────────────────────────────────────────────────────

/// Snip compaction: drop oldest message groups, keeping only the most recent N groups.
///
/// Uses API-round grouping to avoid splitting tool_use/tool_result pairs.
/// Inserts a boundary marker showing where messages were snipped.
fn snip_compact(messages: &[Value], config: &CompactionConfig) -> CompactionResult {
    let pre_tokens = estimate_message_tokens(messages);

    let groups = group_messages_by_api_round(messages);
    let keep_groups = config.snip_keep_recent.min(groups.len());
    let dropped_groups = groups.len().saturating_sub(keep_groups);

    // Count actual messages being dropped for the marker
    let dropped_message_count: usize = groups[..dropped_groups]
        .iter()
        .map(|g| g.len())
        .sum();

    // Build the compacted messages: boundary marker + kept messages
    let kept: Vec<Value> = groups[dropped_groups..].iter().flatten().cloned().collect();
    let mut compacted = Vec::with_capacity(1 + kept.len());

    // Insert snip boundary marker if we actually dropped messages
    if dropped_message_count > 0 {
        let boundary = create_snip_boundary_message(dropped_message_count, pre_tokens);
        compacted.push(boundary);

        // If the kept messages start with an assistant message, prepend a
        // synthetic user marker so the conversation alternates properly.
        if kept
            .first()
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            == Some("assistant")
        {
            compacted.push(serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": format!("... snipped {dropped_message_count} earlier messages ...")}]
            }));
        }
    }

    compacted.extend(kept);

    let post_tokens = estimate_message_tokens(&compacted);

    CompactionResult {
        messages: compacted,
        summary: format!(
            "Snip-compacted: dropped {} message groups ({} messages), kept {}",
            dropped_groups,
            dropped_message_count,
            keep_groups,
        ),
        pre_compact_tokens: pre_tokens,
        post_compact_tokens: post_tokens,
        has_boundary_marker: dropped_message_count > 0,
        trigger: CompactionTrigger::Manual,
    }
}

// ── Reactive compact ─────────────────────────────────────────────────────────

/// Reactive compaction: triggered when the context window approaches limits.
///
/// Two-phase approach:
/// 1. Try micro-compact first (cheap, no API call).
/// 2. If still over budget, run auto-compact on the micro-compacted messages.
///
/// Uses token counting to determine the optimal split point for the auto phase,
/// preserving as many recent messages as possible while staying under the target.
async fn reactive_compact(
    messages: &[Value],
    config: &CompactionConfig,
    api_client: Option<&ApiClient>,
    system_prompt: &str,
) -> Result<CompactionResult> {
    let original_pre_tokens = estimate_message_tokens(messages);

    // Phase 1: try micro-compact (cheap, no API call)
    let micro_result = micro_compact(messages, config);

    // If micro brought us under budget, we're done
    if micro_result.post_compact_tokens <= config.target_after_compact {
        return Ok(CompactionResult {
            summary: format!(
                "Reactive (micro phase sufficient): {}",
                micro_result.summary
            ),
            trigger: CompactionTrigger::Reactive,
            ..micro_result
        });
    }

    // Phase 2: use API-round grouping to find the optimal split point.
    // Peel groups from the oldest end until we free enough tokens.
    let groups = group_messages_by_api_round(&micro_result.messages);

    if groups.len() < 2 {
        // Only one group — micro was all we could do without an API call.
        // Fall through to auto-compact with all messages.
        let auto_result = auto_compact(
            &micro_result.messages,
            config,
            api_client,
            system_prompt,
        )
        .await?;

        return Ok(CompactionResult {
            summary: format!(
                "Reactive (both phases): micro {} -> {} tokens, then auto {} -> {} tokens",
                original_pre_tokens,
                micro_result.post_compact_tokens,
                auto_result.pre_compact_tokens,
                auto_result.post_compact_tokens,
            ),
            pre_compact_tokens: original_pre_tokens,
            trigger: CompactionTrigger::Reactive,
            ..auto_result
        });
    }

    // Find the split point where recent groups fit within the target budget.
    let target = config.target_after_compact;
    let mut keep_start = groups.len();
    let mut running_tokens = 0usize;

    for (i, group) in groups.iter().enumerate().rev() {
        let group_tokens = estimate_group_tokens(group);
        if running_tokens + group_tokens > target {
            break;
        }
        running_tokens += group_tokens;
        keep_start = i;
    }

    // Ensure we keep at least one group
    keep_start = keep_start.min(groups.len() - 1);

    let older: Vec<Value> = groups[..keep_start].iter().flatten().cloned().collect();
    let recent: Vec<Value> = groups[keep_start..].iter().flatten().cloned().collect();

    if older.is_empty() {
        // Nothing to summarize — the recent messages alone are over budget.
        // Run auto-compact on everything.
        let auto_result = auto_compact(
            &micro_result.messages,
            config,
            api_client,
            system_prompt,
        )
        .await?;

        return Ok(CompactionResult {
            summary: format!(
                "Reactive (both phases, full): micro {} -> {} tokens, then auto {} -> {} tokens",
                original_pre_tokens,
                micro_result.post_compact_tokens,
                auto_result.pre_compact_tokens,
                auto_result.post_compact_tokens,
            ),
            pre_compact_tokens: original_pre_tokens,
            trigger: CompactionTrigger::Reactive,
            ..auto_result
        });
    }

    // Summarize the older messages via the API
    let api_client =
        api_client.ok_or_else(|| anyhow::anyhow!("reactive-compact requires an API client"))?;

    let stripped_older = strip_images_from_messages(&older);
    let compact_prompt = get_compact_prompt(None);

    let tool_infos = extract_tool_infos_from_messages(&older);
    let tool_summary_annotation = if !tool_infos.is_empty() {
        tool_use_summary::generate_simple_summary(&tool_infos)
            .map(|s| format!("\n\nTool activity summary: {s}"))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let summary_messages = vec![serde_json::json!({
        "role": "user",
        "content": [{
            "type": "text",
            "text": format!("{compact_prompt}{tool_summary_annotation}\n\nMessages to summarize:\n{}", serde_json::to_string_pretty(&stripped_older)?)
        }]
    })];

    let response = api_client.send_request(&summary_messages, &[], &[]).await?;
    let response_text = extract_text_from_response(&response);

    if response_text.is_empty() {
        anyhow::bail!("Failed to generate conversation summary during reactive compact");
    }

    let _summary_text = format_compact_summary(&response_text);
    let user_summary = get_compact_user_summary_message(
        &response_text,
        true,
        None,
        !recent.is_empty(),
    );

    // Build: boundary + summary + ack + recent
    let mut compacted = Vec::with_capacity(3 + recent.len());
    let boundary = create_compact_boundary_message("reactive", original_pre_tokens);
    compacted.push(boundary);

    compacted.push(serde_json::json!({
        "role": "user",
        "content": [{"type": "text", "text": user_summary}]
    }));
    compacted.push(serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "Understood. I have the context from the summary. How can I help?"}]
    }));
    compacted.extend(recent);

    let post_tokens = estimate_message_tokens(&compacted);

    Ok(CompactionResult {
        messages: compacted,
        summary: format!(
            "Reactive (both phases): micro {} -> {} tokens, then summarized older ({} groups), \
             final {} tokens",
            original_pre_tokens,
            micro_result.post_compact_tokens,
            keep_start,
            post_tokens,
        ),
        pre_compact_tokens: original_pre_tokens,
        post_compact_tokens: post_tokens,
        has_boundary_marker: true,
        trigger: CompactionTrigger::Reactive,
    })
}

// ── Compact boundary markers ────────────────────────────────────────────────

/// Create a compact boundary message that marks where compaction occurred.
///
/// The boundary message is a system-type message that includes:
/// - The trigger type (auto, manual, reactive)
/// - The pre-compaction token count
/// - A timestamp
pub fn create_compact_boundary_message(trigger: &str, pre_compact_tokens: usize) -> Value {
    serde_json::json!({
        "role": "system",
        "type": "compact_boundary",
        "subtype": "compact_boundary",
        "trigger": trigger,
        "pre_compact_tokens": pre_compact_tokens,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "content": [{
            "type": "text",
            "text": format!(
                "[Conversation compacted ({trigger}): ~{pre_compact_tokens} tokens summarized]"
            )
        }]
    })
}

/// Create a snip boundary message showing how many messages were removed.
fn create_snip_boundary_message(dropped_count: usize, pre_compact_tokens: usize) -> Value {
    serde_json::json!({
        "role": "system",
        "type": "compact_boundary",
        "subtype": "compact_boundary",
        "trigger": "snip",
        "pre_compact_tokens": pre_compact_tokens,
        "dropped_messages": dropped_count,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "content": [{
            "type": "text",
            "text": format!(
                "[Conversation snipped: {dropped_count} earlier messages removed]"
            )
        }]
    })
}

// ── Post-compact cleanup ────────────────────────────────────────────────────

/// Run cleanup of caches and tracking state after compaction.
///
/// Call this after both auto-compact and manual /compact to free memory
/// held by tracking structures that are invalidated by compaction.
///
/// Resets:
/// - Microcompact warning suppression state
/// - Auto-compact failure counter
pub fn run_post_compact_cleanup() {
    // Reset microcompact state so stale tool IDs don't accumulate
    clear_compact_warning_suppression();
    reset_auto_compact_failures();

    tracing::debug!("post-compact cleanup complete");
}

// ── Prompt-too-long retry helpers ───────────────────────────────────────────

/// Check if a response indicates a prompt-too-long error.
fn is_prompt_too_long_response(text: &str) -> bool {
    text.starts_with("Conversation too long")
        || text.contains("prompt is too long")
        || text.contains("prompt_too_long")
}

/// Truncate the oldest messages for a prompt-too-long retry.
///
/// Drops approximately 20% * attempt of the messages from the front.
/// Returns None if nothing can be dropped.
fn truncate_head_for_ptl_retry(messages: &[Value], attempt: usize) -> Option<Vec<Value>> {
    if messages.len() < 2 {
        return None;
    }

    // Strip our own synthetic marker from a previous retry before grouping.
    let input: &[Value] = if messages
        .first()
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        == Some(PTL_RETRY_MARKER)
    {
        &messages[1..]
    } else {
        messages
    };

    let groups = group_messages_by_api_round(input);
    if groups.len() < 2 {
        return None;
    }

    // Drop 20% per attempt, at least 1 group
    let drop_count = (groups.len() * 20 * attempt / 100).max(1).min(groups.len() - 1);

    let mut result: Vec<Value> = groups[drop_count..].iter().flatten().cloned().collect();

    // If the result starts with an assistant message, prepend a synthetic user marker
    if result
        .first()
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
        == Some("assistant")
    {
        result.insert(
            0,
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": PTL_RETRY_MARKER}],
                "isMeta": true
            }),
        );
    }

    Some(result)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Merge adjacent thinking blocks, keeping the later one's signature.
fn merge_reasoning_blocks(messages: Vec<Value>) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::with_capacity(messages.len());

    for msg in messages {
        let mut msg = msg;
        if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
            let mut merged: Vec<Value> = Vec::with_capacity(content.len());
            for block in content.drain(..) {
                let is_thinking = block.get("type").and_then(|t| t.as_str()) == Some("thinking");
                let prev_is_thinking = merged
                    .last()
                    .is_some_and(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"));
                if is_thinking && prev_is_thinking {
                    // Merge: keep current block's signature, combine thinking text
                    // Safety: prev_is_thinking is true only when merged.last() is Some
                    let Some(prev) = merged.last_mut() else { merged.push(block); continue; };
                    if let (Some(prev_text), Some(cur_text)) = (
                        prev.get("thinking")
                            .and_then(|t| t.as_str())
                            .map(String::from),
                        block.get("thinking").and_then(|t| t.as_str()),
                    ) {
                        prev["thinking"] = Value::String(format!("{prev_text}\n\n{cur_text}"));
                    }
                    // Use the later signature
                    if let Some(sig) = block.get("signature") {
                        prev["signature"] = sig.clone();
                    }
                } else {
                    merged.push(block);
                }
            }
            if let Some(content) = msg.get_mut("content") {
                *content = Value::Array(merged);
            }
        }
        result.push(msg);
    }
    result
}

/// Extract text content from an API response JSON.
fn extract_text_from_response(response: &Value) -> String {
    if let Some(content) = response.get("content").and_then(|c| c.as_array()) {
        content
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    block.get("text").and_then(|t| t.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    }
}

/// Format a compact summary by stripping the `<analysis>` drafting scratchpad
/// and replacing `<summary>` XML tags with readable section headers.
///
/// Matches the original TS `formatCompactSummary` exactly.
pub fn format_compact_summary(text: &str) -> String {
    let mut formatted = text.to_string();

    // Strip analysis section — it's a drafting scratchpad that improves summary
    // quality but has no informational value once the summary is written.
    formatted = strip_xml_tag(&formatted, "analysis");

    // Extract and format summary section
    if let Some(content) = extract_xml_tag_content(&formatted, "summary") {
        let replacement = format!("Summary:\n{}", content.trim());
        if let (Some(start), Some(end)) =
            (formatted.find("<summary>"), formatted.find("</summary>"))
        {
            formatted = format!(
                "{}{}{}",
                &formatted[..start],
                replacement,
                &formatted[end + "</summary>".len()..]
            );
        }
    }

    // Clean up extra whitespace between sections
    while formatted.contains("\n\n\n") {
        formatted = formatted.replace("\n\n\n", "\n\n");
    }

    formatted.trim().to_string()
}

/// Extract the text content between `<tag>` and `</tag>`.
fn extract_xml_tag_content(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text.find(&close)?;
    if start < end {
        Some(text[start..end].to_string())
    } else {
        None
    }
}

/// Remove an XML tag and its contents from text.
fn strip_xml_tag(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let (Some(start), Some(end)) = (text.find(&open), text.find(&close)) {
        let before = &text[..start];
        let after = &text[end + close.len()..];
        format!("{before}{after}")
    } else {
        text.to_string()
    }
}

/// Extract tool use information from messages for summary generation.
///
/// Scans assistant messages for `tool_use` blocks and pairs them with their
/// results (from subsequent `tool_result` blocks) to build `ToolInfo` entries.
pub(crate) fn extract_tool_infos_from_messages(messages: &[Value]) -> Vec<ToolInfo> {
    let mut tool_map: HashMap<String, ToolInfo> = HashMap::new();

    for msg in messages {
        if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
            for block in content {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match block_type {
                    "tool_use" => {
                        if let (Some(id), Some(name)) = (
                            block.get("id").and_then(|v| v.as_str()),
                            block.get("name").and_then(|v| v.as_str()),
                        ) {
                            let input = block.get("input").cloned().unwrap_or(Value::Null);
                            tool_map.insert(
                                id.to_string(),
                                ToolInfo {
                                    name: name.to_string(),
                                    input,
                                    output: Value::Null,
                                },
                            );
                        }
                    }
                    "tool_result" => {
                        if let Some(id) = block.get("tool_use_id").and_then(|v| v.as_str()) {
                            if let Some(info) = tool_map.get_mut(id) {
                                info.output = block
                                    .get("content")
                                    .cloned()
                                    .unwrap_or(Value::Null);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    tool_map.into_values().collect()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_user_msg(text: &str) -> Value {
        serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": text}]
        })
    }

    fn make_assistant_msg(text: &str) -> Value {
        serde_json::json!({
            "role": "assistant",
            "content": [{"type": "text", "text": text}]
        })
    }

    fn make_assistant_msg_with_id(text: &str, id: &str) -> Value {
        serde_json::json!({
            "role": "assistant",
            "id": id,
            "content": [{"type": "text", "text": text}]
        })
    }

    fn make_tool_use_msg(tool_use_id: &str, tool_name: &str) -> Value {
        serde_json::json!({
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": tool_use_id,
                "name": tool_name,
                "input": {"path": "/test/file.rs"}
            }]
        })
    }

    fn make_tool_result_msg(content_text: &str) -> Value {
        serde_json::json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "test_id",
                "content": [{"type": "text", "text": content_text}]
            }]
        })
    }

    fn make_tool_result_msg_with_id(content_text: &str, tool_use_id: &str) -> Value {
        serde_json::json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": [{"type": "text", "text": content_text}]
            }]
        })
    }

    #[test]
    fn test_estimate_tokens() {
        let text = "Hello, world!"; // 13 chars
        let tokens = estimate_tokens_for_text(text);
        assert!(tokens > 0);
        assert!(tokens < 20); // Sanity check
    }

    #[test]
    fn test_estimate_message_tokens_conservative() {
        let messages = vec![
            make_user_msg("Hello"),
            make_assistant_msg("World"),
        ];
        let tokens = estimate_message_tokens(&messages);
        // estimate_message_tokens walks inner text blocks and pads by 4/3.
        // It should be strictly positive for non-empty messages.
        assert!(tokens > 0, "estimate_message_tokens should be positive for non-empty messages");
        // The inner-text token sum before padding
        let inner_raw: usize = ["Hello", "World"]
            .iter()
            .map(|t| estimate_tokens_for_text(t))
            .sum();
        let padded = (inner_raw as f64 * 4.0 / 3.0).ceil() as usize;
        assert_eq!(tokens, padded, "estimate_message_tokens should equal padded inner-text estimate");
    }

    #[test]
    fn test_should_auto_compact_under_threshold() {
        let config = CompactionConfig {
            max_context_tokens: 1000,
            ..Default::default()
        };
        let messages = vec![make_user_msg("short message")];
        assert!(!should_auto_compact(&messages, &config));
    }

    #[test]
    fn test_should_auto_compact_over_threshold() {
        let config = CompactionConfig {
            max_context_tokens: 10,
            ..Default::default()
        };
        let messages = vec![
            make_user_msg(&"x".repeat(500)),
            make_assistant_msg(&"y".repeat(500)),
        ];
        assert!(should_auto_compact(&messages, &config));
    }

    #[test]
    fn test_should_auto_compact_disabled() {
        let config = CompactionConfig {
            max_context_tokens: 10,
            auto_compact_enabled: false,
            ..Default::default()
        };
        let messages = vec![
            make_user_msg(&"x".repeat(500)),
        ];
        assert!(!should_auto_compact(&messages, &config));
    }

    #[test]
    fn test_snip_compact() {
        let config = CompactionConfig {
            snip_keep_recent: 2,
            ..Default::default()
        };
        // Use assistant messages with distinct IDs so group_messages_by_api_round
        // creates separate groups at each new assistant id boundary.
        let messages = vec![
            make_user_msg("first"),
            make_assistant_msg_with_id("second", "id1"),
            make_user_msg("third"),
            make_assistant_msg_with_id("fourth", "id2"),
        ];
        let result = snip_compact(&messages, &config);
        // Should have boundary + kept messages
        assert!(result.has_boundary_marker);
        assert!(result.summary.contains("Snip-compacted"));
        // Last messages should contain "fourth"
        let all_text = result
            .messages
            .iter()
            .map(|m| m.to_string())
            .collect::<String>();
        assert!(all_text.contains("fourth"));
    }

    #[test]
    fn test_snip_compact_with_boundary_marker() {
        let config = CompactionConfig {
            snip_keep_recent: 1,
            ..Default::default()
        };
        // Use distinct assistant IDs so group_messages_by_api_round creates
        // multiple groups, allowing snip_compact to actually drop some.
        let messages = vec![
            make_user_msg("old1"),
            make_assistant_msg_with_id("old2", "id1"),
            make_user_msg("recent"),
            make_assistant_msg_with_id("recent2", "id2"),
        ];
        let result = snip_compact(&messages, &config);
        assert!(result.has_boundary_marker);
        // Check that the boundary message is present
        let first = &result.messages[0];
        let text = first.to_string();
        assert!(
            text.contains("compact_boundary") || text.contains("snipped"),
            "First message should be a boundary marker or snip indicator"
        );
    }

    #[test]
    fn test_micro_compact_truncates_large_results() {
        let config = CompactionConfig {
            micro_truncate_threshold: 10,
            micro_keep_recent: 100, // Keep all for this test
            ..Default::default()
        };
        let big_content = "x".repeat(10_000);
        let messages = vec![make_tool_result_msg(&big_content)];
        let result = micro_compact(&messages, &config);
        assert!(result.post_compact_tokens < result.pre_compact_tokens);
        let text = result.messages[0].to_string();
        // The truncated result is replaced with a summary from generate_simple_summary
        assert!(text.contains("Used tool") || text.contains("cleared"));
    }

    #[test]
    fn test_micro_compact_preserves_small_results() {
        let config = CompactionConfig {
            micro_truncate_threshold: 100_000,
            micro_keep_recent: 100,
            ..Default::default()
        };
        let messages = vec![make_tool_result_msg("small result")];
        let result = micro_compact(&messages, &config);
        let text = result.messages[0].to_string();
        assert!(text.contains("small result"));
        assert!(!text.contains("cleared"));
    }

    #[test]
    fn test_micro_compact_clears_old_tool_results() {
        let config = CompactionConfig {
            micro_truncate_threshold: 100_000,
            micro_keep_recent: 1, // Keep only the most recent
            ..Default::default()
        };
        let messages = vec![
            make_tool_use_msg("tool1", "Read"),
            make_tool_result_msg_with_id("old result content", "tool1"),
            make_tool_use_msg("tool2", "Read"),
            make_tool_result_msg_with_id("recent result content", "tool2"),
        ];
        let result = micro_compact(&messages, &config);
        let all_text: String = result.messages.iter().map(|m| m.to_string()).collect();
        // Recent should be preserved
        assert!(all_text.contains("recent result content"));
        // Old should be cleared
        assert!(!all_text.contains("old result content"));
    }

    #[test]
    fn test_merge_reasoning_blocks() {
        let messages = vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "first thought", "signature": "sig1"},
                {"type": "thinking", "thinking": "second thought", "signature": "sig2"},
                {"type": "text", "text": "answer"}
            ]
        })];
        let merged = merge_reasoning_blocks(messages);
        let content = merged[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2); // 1 merged thinking + 1 text
        assert_eq!(content[0]["signature"], "sig2"); // Later signature
        let thinking = content[0]["thinking"].as_str().unwrap();
        assert!(thinking.contains("first thought"));
        assert!(thinking.contains("second thought"));
    }

    #[test]
    fn test_format_compact_summary_with_tags() {
        let text = "<analysis>Some analysis</analysis>\n<summary>The real summary</summary>";
        let result = format_compact_summary(text);
        assert!(result.contains("Summary:"));
        assert!(result.contains("The real summary"));
        assert!(!result.contains("<analysis>"));
    }

    #[test]
    fn test_format_compact_summary_no_tags() {
        let text = "Just plain text summary";
        assert_eq!(format_compact_summary(text), "Just plain text summary");
    }

    #[test]
    fn test_extract_xml_tag_content() {
        assert_eq!(
            extract_xml_tag_content("<foo>bar</foo>", "foo"),
            Some("bar".to_string())
        );
        assert_eq!(extract_xml_tag_content("no tags here", "foo"), None);
    }

    #[test]
    fn test_strip_xml_tag() {
        let text = "before<analysis>stuff</analysis>after";
        assert_eq!(strip_xml_tag(text, "analysis"), "beforeafter");
    }

    #[test]
    fn test_compaction_config_defaults() {
        let config = CompactionConfig::default();
        assert_eq!(config.max_context_tokens, 180_000);
        assert_eq!(config.target_after_compact, 80_000);
        assert_eq!(config.auto_keep_recent, 4);
        assert!(config.auto_compact_enabled);
        assert_eq!(config.micro_keep_recent, 10);
    }

    #[test]
    fn test_snip_compact_fewer_than_keep() {
        let config = CompactionConfig {
            snip_keep_recent: 10,
            ..Default::default()
        };
        let messages = vec![make_user_msg("only one")];
        let result = snip_compact(&messages, &config);
        assert_eq!(result.messages.len(), 1);
        assert!(!result.has_boundary_marker);
    }

    #[tokio::test]
    async fn test_reactive_micro_phase_sufficient() {
        // If micro-compact alone gets us under budget, no API call needed
        let config = CompactionConfig {
            micro_truncate_threshold: 10,
            target_after_compact: 1_000_000, // Very high target
            ..Default::default()
        };
        let messages = vec![
            make_user_msg("hello"),
            make_tool_result_msg(&"x".repeat(10_000)),
        ];
        let result = reactive_compact(&messages, &config, None, "system")
            .await
            .unwrap();
        assert!(result.summary.contains("micro phase sufficient"));
    }

    #[test]
    fn test_compaction_strategy_variants() {
        // Just verify all variants can be constructed and matched
        let strategies = vec![
            CompactionStrategy::Auto,
            CompactionStrategy::Micro,
            CompactionStrategy::Snip,
            CompactionStrategy::Reactive,
        ];
        for s in &strategies {
            match s {
                CompactionStrategy::Auto => {}
                CompactionStrategy::Micro => {}
                CompactionStrategy::Snip => {}
                CompactionStrategy::Reactive => {}
            }
        }
    }

    #[test]
    fn test_compact_prompt_has_9_sections() {
        let prompt = get_compact_prompt(None);
        assert!(prompt.contains("1. Primary Request and Intent"));
        assert!(prompt.contains("2. Key Technical Concepts"));
        assert!(prompt.contains("3. Files and Code Sections"));
        assert!(prompt.contains("4. Errors and fixes"));
        assert!(prompt.contains("5. Problem Solving"));
        assert!(prompt.contains("6. All user messages"));
        assert!(prompt.contains("7. Pending Tasks"));
        assert!(prompt.contains("8. Current Work"));
        assert!(prompt.contains("9. Optional Next Step"));
        assert!(prompt.contains("CRITICAL: Respond with TEXT ONLY"));
        assert!(prompt.contains("REMINDER: Do NOT call any tools"));
    }

    #[test]
    fn test_compact_prompt_custom_instructions() {
        let prompt = get_compact_prompt(Some("Focus on test output"));
        assert!(prompt.contains("Additional Instructions:\nFocus on test output"));
    }

    #[test]
    fn test_partial_compact_prompt_from() {
        let prompt = get_partial_compact_prompt(None, &PartialCompactDirection::From);
        assert!(prompt.contains("RECENT portion"));
    }

    #[test]
    fn test_partial_compact_prompt_up_to() {
        let prompt = get_partial_compact_prompt(None, &PartialCompactDirection::UpTo);
        assert!(prompt.contains("Context for Continuing Work"));
    }

    #[test]
    fn test_compact_user_summary_message() {
        let msg = get_compact_user_summary_message(
            "<analysis>thought</analysis><summary>The summary</summary>",
            true,
            Some("/path/to/transcript"),
            true,
        );
        assert!(msg.contains("continued from a previous conversation"));
        assert!(msg.contains("Summary:"));
        assert!(msg.contains("The summary"));
        assert!(msg.contains("/path/to/transcript"));
        assert!(msg.contains("Recent messages are preserved"));
        assert!(msg.contains("Resume directly"));
    }

    #[test]
    fn test_group_messages_by_api_round() {
        let messages = vec![
            make_user_msg("q1"),
            make_assistant_msg_with_id("a1", "id1"),
            make_user_msg("q2"),
            make_assistant_msg_with_id("a2", "id2"),
        ];
        let groups = group_messages_by_api_round(&messages);
        // A new group starts each time a NEW assistant id appears.
        // group1=[user("q1")], group2=[assistant("a1",id1), user("q2")], group3=[assistant("a2",id2)]
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].len(), 1); // user
        assert_eq!(groups[1].len(), 2); // assistant (id1) + user
        assert_eq!(groups[2].len(), 1); // assistant (id2)
    }

    #[test]
    fn test_group_messages_keeps_tool_pairs() {
        let messages = vec![
            make_user_msg("q1"),
            make_assistant_msg_with_id("thinking...", "id1"),
            make_tool_use_msg("tu1", "Read"),
            make_tool_result_msg_with_id("file contents", "tu1"),
        ];
        // group1=[user("q1")], group2=[assistant(id1)],
        // group3=[tool_use(assistant,no id) + tool_result(user)]
        // tool_use has role=assistant and msg_id=None which differs from last_assistant_id=Some("id1"),
        // so it starts a new group.
        let groups = group_messages_by_api_round(&messages);
        assert_eq!(groups.len(), 3);
    }

    #[test]
    fn test_collect_compactable_tool_ids() {
        let messages = vec![
            make_tool_use_msg("tu1", "Read"),
            make_tool_use_msg("tu2", "Agent"), // Not compactable
            make_tool_use_msg("tu3", "Bash"),
        ];
        let ids = collect_compactable_tool_ids(&messages);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"tu1".to_string()));
        assert!(ids.contains(&"tu3".to_string()));
        assert!(!ids.contains(&"tu2".to_string()));
    }

    #[test]
    fn test_compact_boundary_message() {
        let boundary = create_compact_boundary_message("auto", 150_000);
        assert_eq!(
            boundary.get("type").and_then(|t| t.as_str()),
            Some("compact_boundary")
        );
        assert_eq!(
            boundary.get("trigger").and_then(|t| t.as_str()),
            Some("auto")
        );
        assert_eq!(
            boundary.get("pre_compact_tokens").and_then(|t| t.as_u64()),
            Some(150_000)
        );
    }

    #[test]
    fn test_compact_warning_suppression() {
        // Reset state
        clear_compact_warning_suppression();
        assert!(!is_compact_warning_suppressed());

        suppress_compact_warning();
        assert!(is_compact_warning_suppressed());

        clear_compact_warning_suppression();
        assert!(!is_compact_warning_suppressed());
    }

    #[test]
    fn test_token_warning_state() {
        let state = calculate_token_warning_state(0, "claude-sonnet-4-20250514");
        assert!(!state.is_above_warning_threshold);
        assert!(!state.is_above_error_threshold);
        assert!(!state.is_at_blocking_limit);
        assert!(state.percent_left > 0);
    }

    #[test]
    fn test_merge_hook_instructions() {
        assert_eq!(merge_hook_instructions(None, None), None);
        assert_eq!(
            merge_hook_instructions(Some("user"), None),
            Some("user".to_string())
        );
        assert_eq!(
            merge_hook_instructions(None, Some("hook")),
            Some("hook".to_string())
        );
        assert_eq!(
            merge_hook_instructions(Some("user"), Some("hook")),
            Some("user\n\nhook".to_string())
        );
        assert_eq!(merge_hook_instructions(Some(""), None), None);
    }

    #[test]
    fn test_strip_images_from_messages() {
        let messages = vec![
            serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "Look at this"},
                    {"type": "image", "source": {"data": "base64..."}}
                ]
            }),
            make_assistant_msg("I see it"),
        ];
        let stripped = strip_images_from_messages(&messages);
        assert_eq!(stripped.len(), 2);
        let content = stripped[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["text"], "[image]");
        // Assistant message unchanged
        assert_eq!(stripped[1], messages[1]);
    }

    #[test]
    fn test_estimate_message_tokens_handles_all_block_types() {
        let messages = vec![
            serde_json::json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "let me think..."},
                    {"type": "text", "text": "here is my answer"},
                    {"type": "tool_use", "name": "Read", "id": "tu1", "input": {"path": "/test"}}
                ]
            }),
            serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "tu1", "content": "file contents"},
                    {"type": "image", "source": {"data": "..."}}
                ]
            }),
        ];
        let tokens = estimate_message_tokens(&messages);
        assert!(tokens > 0);
    }

    #[test]
    fn test_truncate_head_for_ptl_retry() {
        // Use distinct assistant IDs so group_messages_by_api_round creates
        // multiple groups, allowing truncation to actually drop some.
        let messages = vec![
            make_user_msg("q1"),
            make_assistant_msg_with_id("a1", "id1"),
            make_user_msg("q2"),
            make_assistant_msg_with_id("a2", "id2"),
            make_user_msg("q3"),
            make_assistant_msg_with_id("a3", "id3"),
        ];
        let result = truncate_head_for_ptl_retry(&messages, 1);
        assert!(result.is_some());
        let truncated = result.unwrap();
        // At least one group is dropped; the result may include a synthetic
        // user marker if it starts with an assistant message, so we verify
        // the content excludes the first group's user message.
        let all_text: String = truncated.iter().map(|m| m.to_string()).collect();
        assert!(!all_text.contains("\"q1\""), "first group should have been dropped");
    }

    #[test]
    fn test_truncate_head_for_ptl_retry_too_few_messages() {
        let messages = vec![make_user_msg("only one")];
        assert!(truncate_head_for_ptl_retry(&messages, 1).is_none());
    }

    #[test]
    fn test_auto_compact_tracking_state_default() {
        let state = AutoCompactTrackingState::default();
        assert!(!state.compacted);
        assert_eq!(state.turn_counter, 0);
        assert_eq!(state.consecutive_failures, 0);
    }
}
