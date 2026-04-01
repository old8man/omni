//! Token counting, estimation, and usage aggregation utilities.

use crate::types::message::{AssistantMessage, Message};
use crate::types::usage::Usage;
use crate::utils::messages::{is_synthetic_message, SYNTHETIC_MODEL};

// ---------------------------------------------------------------------------
// Extract token counts from API usage
// ---------------------------------------------------------------------------

/// Get the `Usage` from a message if it's a non-synthetic assistant message.
pub fn get_token_usage(message: &Message) -> Option<&Usage> {
    match message {
        Message::Assistant(asst) => {
            if asst.message.model == SYNTHETIC_MODEL {
                return None;
            }
            if is_synthetic_message(message) {
                return None;
            }
            Some(&asst.message.usage)
        }
        _ => None,
    }
}

/// Total context window tokens from usage data:
/// `input_tokens + cache_creation + cache_read + output_tokens`.
pub fn get_token_count_from_usage(usage: &Usage) -> u64 {
    usage.input_tokens
        + usage.cache_creation_input_tokens.unwrap_or(0)
        + usage.cache_read_input_tokens.unwrap_or(0)
        + usage.output_tokens
}

/// Token count from the last API response in the message list.
pub fn token_count_from_last_api_response(messages: &[Message]) -> u64 {
    for msg in messages.iter().rev() {
        if let Some(usage) = get_token_usage(msg) {
            return get_token_count_from_usage(usage);
        }
    }
    0
}

/// Output-only token count from the last API response.
///
/// WARNING: Do NOT use this for threshold comparisons (autocompact, etc.).
/// Use `token_count_with_estimation` instead.
pub fn message_token_count_from_last_api_response(messages: &[Message]) -> u64 {
    for msg in messages.iter().rev() {
        if let Some(usage) = get_token_usage(msg) {
            return usage.output_tokens;
        }
    }
    0
}

/// Get current usage breakdown from the most recent API response.
pub fn get_current_usage(messages: &[Message]) -> Option<CurrentUsage> {
    for msg in messages.iter().rev() {
        if let Some(usage) = get_token_usage(msg) {
            return Some(CurrentUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
                cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0),
            });
        }
    }
    None
}

/// Structured current usage breakdown.
#[derive(Clone, Debug)]
pub struct CurrentUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

/// Returns `true` if the most recent assistant message exceeds 200k total tokens.
pub fn does_most_recent_assistant_message_exceed_200k(messages: &[Message]) -> bool {
    const THRESHOLD: u64 = 200_000;
    let last_asst = messages.iter().rev().find(|m| matches!(m, Message::Assistant(_)));
    if let Some(msg) = last_asst {
        if let Some(usage) = get_token_usage(msg) {
            return get_token_count_from_usage(usage) > THRESHOLD;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Rough token estimation
// ---------------------------------------------------------------------------

/// Rough token estimate: ~4 characters per token.
pub fn estimate_tokens_for_string(s: &str) -> u64 {
    (s.len() as u64).div_ceil(4)
}

/// Rough token estimation for a slice of messages.
pub fn rough_token_count_estimation_for_messages(messages: &[Message]) -> u64 {
    crate::services::token_estimation::estimate_messages_tokens(messages) as u64
}

/// Get the character content length of an assistant message (for spinner estimation).
pub fn get_assistant_message_content_length(message: &AssistantMessage) -> usize {
    use crate::types::content::ContentBlock;
    let mut len = 0;
    for block in &message.message.content {
        match block {
            ContentBlock::Text { text } => len += text.len(),
            ContentBlock::Thinking { thinking, .. } => len += thinking.len(),
            ContentBlock::RedactedThinking { data } => len += data.len(),
            ContentBlock::ToolUse { input, .. } => len += input.to_string().len(),
            _ => {}
        }
    }
    len
}

// ---------------------------------------------------------------------------
// Context-aware token estimation
// ---------------------------------------------------------------------------

/// Get the assistant message API response id (non-synthetic).
fn get_assistant_message_id(message: &Message) -> Option<&str> {
    match message {
        Message::Assistant(asst) if asst.message.model != SYNTHETIC_MODEL => {
            Some(&asst.message.id)
        }
        _ => None,
    }
}

/// The canonical function for measuring current context window size in tokens.
///
/// Uses the last API response's token count plus rough estimates for any
/// messages added since. Handles parallel tool call message interleaving by
/// walking back to the first sibling with the same response id.
pub fn token_count_with_estimation(messages: &[Message]) -> u64 {
    let mut i = messages.len();
    while i > 0 {
        i -= 1;
        if let Some(usage) = get_token_usage(&messages[i]) {
            // Walk back past earlier sibling records split from the same API
            // response so interleaved tool_results are included.
            if let Some(response_id) = get_assistant_message_id(&messages[i]) {
                let response_id = response_id.to_string();
                let mut j = i;
                while j > 0 {
                    j -= 1;
                    let prior_id = get_assistant_message_id(&messages[j]);
                    if prior_id == Some(response_id.as_str()) {
                        i = j;
                    } else if prior_id.is_some() {
                        break;
                    }
                }
            }

            let base = get_token_count_from_usage(usage);
            let rest = if i + 1 < messages.len() {
                rough_token_count_estimation_for_messages(&messages[i + 1..])
            } else {
                0
            };
            return base + rest;
        }
    }

    // No API response found — estimate everything.
    rough_token_count_estimation_for_messages(messages)
}

// ---------------------------------------------------------------------------
// Usage aggregation
// ---------------------------------------------------------------------------

/// Aggregated usage across multiple turns.
#[derive(Clone, Debug, Default)]
pub struct AggregatedUsage {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub api_calls: u64,
}

/// Aggregate token usage from all non-synthetic assistant messages.
pub fn aggregate_usage(messages: &[Message]) -> AggregatedUsage {
    let mut agg = AggregatedUsage::default();
    // Track seen response IDs to avoid double-counting split parallel tool calls.
    let mut seen_ids = std::collections::HashSet::new();

    for msg in messages {
        if let Some(usage) = get_token_usage(msg) {
            let response_id = get_assistant_message_id(msg).unwrap_or("");
            if !response_id.is_empty() && !seen_ids.insert(response_id.to_string()) {
                continue;
            }
            agg.total_input_tokens += usage.input_tokens;
            agg.total_output_tokens += usage.output_tokens;
            agg.total_cache_creation_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
            agg.total_cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0);
            agg.api_calls += 1;
        }
    }

    agg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::messages::{create_assistant_message, create_user_message};

    #[test]
    fn test_estimate_tokens_for_string() {
        assert_eq!(estimate_tokens_for_string(""), 0);
        assert_eq!(estimate_tokens_for_string("abcd"), 1);
        assert_eq!(estimate_tokens_for_string("abcde"), 2);
        assert_eq!(estimate_tokens_for_string("abcdefgh"), 2);
    }

    #[test]
    fn test_get_token_count_from_usage() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: Some(10),
            cache_read_input_tokens: Some(20),
            ..Default::default()
        };
        assert_eq!(get_token_count_from_usage(&usage), 180);
    }

    #[test]
    fn test_token_count_with_estimation_no_api() {
        let msgs = vec![
            create_user_message("hello world test message"),
            create_assistant_message("response here"),
        ];
        let count = token_count_with_estimation(&msgs);
        // Should be pure estimation since synthetic messages have no real usage.
        assert!(count > 0);
    }

    #[test]
    fn test_aggregate_usage() {
        // Synthetic messages should be skipped.
        let msgs = vec![
            create_user_message("hi"),
            create_assistant_message("hello"),
        ];
        let agg = aggregate_usage(&msgs);
        assert_eq!(agg.api_calls, 0);
    }
}
