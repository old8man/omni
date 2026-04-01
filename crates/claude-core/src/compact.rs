//! Context compaction strategies for managing conversation length.
//!
//! Provides four strategies:
//! - **Auto**: Send older messages to the API for summarization.
//! - **Micro**: Truncate large tool results inline without an API call.
//! - **Snip**: Drop oldest messages, keeping only recent ones.
//! - **Reactive**: Two-phase — try micro first, then auto if still over budget.

use anyhow::Result;
use serde_json::Value;

use crate::api::client::ApiClient;
use crate::services::tool_use_summary::{self, ToolInfo};
use crate::utils::context;

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
    /// Snip-compact: number of recent messages to keep.
    pub snip_keep_recent: usize,
    /// Token budget reserved for the summary output.
    pub summary_output_reserve: usize,
    /// Auto-compact: number of recent messages to preserve.
    pub auto_keep_recent: usize,
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

/// Check whether the current message set should trigger auto-compaction.
pub fn should_auto_compact(messages: &[Value], config: &CompactionConfig) -> bool {
    let total: usize = messages.iter().map(estimate_tokens).sum();
    total > config.max_context_tokens
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
    match strategy {
        CompactionStrategy::Auto => auto_compact(messages, config, api_client, system_prompt).await,
        CompactionStrategy::Micro => Ok(micro_compact(messages, config)),
        CompactionStrategy::Snip => Ok(snip_compact(messages, config)),
        CompactionStrategy::Reactive => {
            reactive_compact(messages, config, api_client, system_prompt).await
        }
    }
}

// ── Auto compact ─────────────────────────────────────────────────────────────

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

async fn auto_compact(
    messages: &[Value],
    config: &CompactionConfig,
    api_client: Option<&ApiClient>,
    _system_prompt: &str,
) -> Result<CompactionResult> {
    let pre_tokens: usize = messages.iter().map(estimate_tokens).sum();

    let keep_count = config.auto_keep_recent.min(messages.len());
    let split_at = messages.len() - keep_count;
    let older = &messages[..split_at];
    let recent = &messages[split_at..];

    let api_client =
        api_client.ok_or_else(|| anyhow::anyhow!("auto-compact requires an API client"))?;

    // Use the exact structured prompt from the original TS implementation
    let compact_prompt = get_compact_prompt(None);

    // Extract tool use info from older messages for the summary annotation
    let tool_infos = extract_tool_infos_from_messages(older);
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
            "text": format!("{compact_prompt}{tool_summary_annotation}\n\nMessages to summarize:\n{}", serde_json::to_string_pretty(older)?)
        }]
    })];

    let response = api_client.send_request(&summary_messages, &[], &[]).await?;

    let response_text = extract_text_from_response(&response);
    let summary_text = format_compact_summary(&response_text);

    // Build the user summary message matching the TS format
    let user_summary = get_compact_user_summary_message(
        &response_text,
        true, // suppress follow-up questions
        None, // transcript path (set by caller if available)
        !recent.is_empty(),
    );

    // Build replacement messages: summary as a user message, then recent messages
    let mut compacted = Vec::with_capacity(2 + recent.len());
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
    compacted.extend_from_slice(recent);

    let post_tokens: usize = compacted.iter().map(estimate_tokens).sum();

    Ok(CompactionResult {
        messages: compacted,
        summary: summary_text,
        pre_compact_tokens: pre_tokens,
        post_compact_tokens: post_tokens,
    })
}

// ── Micro compact ────────────────────────────────────────────────────────────

/// Run micro-compaction: truncate large tool results and merge adjacent thinking blocks.
///
/// This is also used as a per-turn pre-pass (Q5) to keep context lean.
pub fn micro_compact(messages: &[Value], config: &CompactionConfig) -> CompactionResult {
    let pre_tokens: usize = messages.iter().map(estimate_tokens).sum();

    // Build a map of tool_use_id -> tool_name from assistant messages so we
    // can selectively truncate only COMPACTABLE_TOOLS results.
    let mut tool_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for msg in messages {
        if msg.get("role").and_then(|r| r.as_str()) == Some("assistant") {
            if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
                for block in content {
                    if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        if let (Some(id), Some(name)) = (
                            block.get("id").and_then(|v| v.as_str()),
                            block.get("name").and_then(|v| v.as_str()),
                        ) {
                            tool_names.insert(id.to_string(), name.to_string());
                        }
                    }
                }
            }
        }
    }

    let compacted: Vec<Value> = messages
        .iter()
        .map(|msg| truncate_tool_results(msg, config.micro_truncate_threshold, &tool_names))
        .collect();

    // Merge adjacent thinking blocks
    let compacted = merge_reasoning_blocks(compacted);
    let post_tokens: usize = compacted.iter().map(estimate_tokens).sum();

    CompactionResult {
        messages: compacted,
        summary: format!(
            "Micro-compacted: {pre_tokens} -> {post_tokens} tokens (truncated large tool results)"
        ),
        pre_compact_tokens: pre_tokens,
        post_compact_tokens: post_tokens,
    }
}

/// Truncate tool result content blocks that exceed the threshold.
/// Only truncates results from tools in `COMPACTABLE_TOOLS`.
fn truncate_tool_results(
    msg: &Value,
    threshold: usize,
    tool_names: &std::collections::HashMap<String, String>,
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
                        .unwrap_or_else(|| "[Old tool result content cleared]".to_string());

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

fn snip_compact(messages: &[Value], config: &CompactionConfig) -> CompactionResult {
    let pre_tokens: usize = messages.iter().map(estimate_tokens).sum();

    let keep_count = config.snip_keep_recent.min(messages.len());
    let compacted: Vec<Value> = messages[messages.len() - keep_count..].to_vec();

    let post_tokens: usize = compacted.iter().map(estimate_tokens).sum();

    CompactionResult {
        messages: compacted,
        summary: format!(
            "Snip-compacted: dropped {} oldest messages, kept {keep_count}",
            messages.len() - keep_count
        ),
        pre_compact_tokens: pre_tokens,
        post_compact_tokens: post_tokens,
    }
}

// ── Reactive compact ─────────────────────────────────────────────────────────

async fn reactive_compact(
    messages: &[Value],
    config: &CompactionConfig,
    api_client: Option<&ApiClient>,
    system_prompt: &str,
) -> Result<CompactionResult> {
    // Phase 1: try micro-compact (cheap, no API call)
    let micro_result = micro_compact(messages, config);

    // If micro brought us under budget, we're done
    if micro_result.post_compact_tokens <= config.target_after_compact {
        return Ok(CompactionResult {
            summary: format!(
                "Reactive (micro phase sufficient): {}",
                micro_result.summary
            ),
            ..micro_result
        });
    }

    // Phase 2: auto-compact the micro-compacted messages
    let auto_result =
        auto_compact(&micro_result.messages, config, api_client, system_prompt).await?;

    Ok(CompactionResult {
        summary: format!(
            "Reactive (both phases): micro {} -> {} tokens, then auto {} -> {} tokens",
            micro_result.pre_compact_tokens,
            micro_result.post_compact_tokens,
            auto_result.pre_compact_tokens,
            auto_result.post_compact_tokens,
        ),
        pre_compact_tokens: micro_result.pre_compact_tokens,
        ..auto_result
    })
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
                    let prev = merged.last_mut().unwrap();
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
            *msg.get_mut("content").unwrap() = Value::Array(merged);
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
    let mut tool_map: std::collections::HashMap<String, ToolInfo> =
        std::collections::HashMap::new();

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

    #[test]
    fn test_estimate_tokens() {
        let text = "Hello, world!"; // 13 chars
        let tokens = estimate_tokens_for_text(text);
        assert!(tokens > 0);
        assert!(tokens < 20); // Sanity check
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
    fn test_snip_compact() {
        let config = CompactionConfig {
            snip_keep_recent: 2,
            ..Default::default()
        };
        let messages = vec![
            make_user_msg("first"),
            make_assistant_msg("second"),
            make_user_msg("third"),
            make_assistant_msg("fourth"),
        ];
        let result = snip_compact(&messages, &config);
        assert_eq!(result.messages.len(), 2);
        // Should keep the last 2
        assert!(result.messages[0].to_string().contains("third"));
        assert!(result.messages[1].to_string().contains("fourth"));
    }

    #[test]
    fn test_micro_compact_truncates_large_results() {
        let config = CompactionConfig {
            micro_truncate_threshold: 10,
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
            ..Default::default()
        };
        let messages = vec![make_tool_result_msg("small result")];
        let result = micro_compact(&messages, &config);
        let text = result.messages[0].to_string();
        assert!(text.contains("small result"));
        assert!(!text.contains("cleared"));
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
}
