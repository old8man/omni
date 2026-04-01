use crate::types::content::ContentBlock;
use crate::types::message::Message;

// ── Rough estimation ────────────────────────────────────────────────────────

/// Estimate token count from raw text using a bytes-per-token ratio.
///
/// The default ratio of 4 bytes per token is a reasonable approximation for
/// English prose and code. JSON is denser (many single-character tokens like
/// `{`, `}`, `:`, `,`, `"`) so callers can pass a lower ratio.
pub fn rough_token_count_estimation(content: &str, bytes_per_token: usize) -> usize {
    if content.is_empty() {
        return 0;
    }
    if bytes_per_token == 4 {
        // Delegate to the canonical ~4 chars/token estimator.
        return crate::utils::tokens::estimate_tokens_for_string(content) as usize;
    }
    content.len().div_ceil(bytes_per_token)
}

/// Returns a bytes-per-token ratio appropriate for a given file extension.
pub fn bytes_per_token_for_file_type(ext: &str) -> usize {
    match ext {
        "json" | "jsonl" | "jsonc" => 2,
        _ => 4,
    }
}

/// Rough token estimate for file content, using a file-type-aware ratio.
pub fn rough_token_count_for_file_type(content: &str, file_extension: &str) -> usize {
    rough_token_count_estimation(content, bytes_per_token_for_file_type(file_extension))
}

// ── Content block estimation ────────────────────────────────────────────────

/// Estimate the token count for a single content block.
pub fn estimate_content_block_tokens(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text { text } => rough_token_count_estimation(text, 4),
        ContentBlock::ToolUse { name, input, .. } => {
            let input_str = serde_json::to_string(input).unwrap_or_default();
            rough_token_count_estimation(&format!("{}{}", name, input_str), 4)
        }
        ContentBlock::ToolResult { content, .. } => {
            content.iter().map(estimate_content_block_tokens).sum()
        }
        ContentBlock::Thinking { thinking, .. } => rough_token_count_estimation(thinking, 4),
        ContentBlock::RedactedThinking { data } => rough_token_count_estimation(data, 4),
        // Images and documents: use a conservative fixed estimate.
        // Actual image tokens = (width * height) / 750, capped at ~5333.
        // We use 2000 to match the TS implementation's conservative approach.
        ContentBlock::Image { .. } | ContentBlock::Document { .. } => 2000,
    }
}

// ── Message-level estimation ────────────────────────────────────────────────

/// Estimate the token count for a single message.
pub fn estimate_message_tokens(message: &Message) -> usize {
    match message {
        Message::User(u) => u
            .content
            .iter()
            .map(estimate_content_block_tokens)
            .sum(),
        Message::Assistant(a) => a
            .message
            .content
            .iter()
            .map(estimate_content_block_tokens)
            .sum(),
        Message::System(_) => {
            // System messages are typically small; a flat estimate suffices.
            50
        }
    }
}

/// Estimate the total token count across a slice of messages.
pub fn estimate_messages_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// Compute a token count for a message, preferring the API-reported count when
/// available and falling back to rough estimation otherwise.
pub fn token_count_with_estimation(message: &Message) -> usize {
    match message {
        Message::Assistant(a) => {
            let usage = &a.message.usage;
            let api_count = usage.input_tokens as usize
                + usage.output_tokens as usize
                + usage.cache_read_input_tokens.unwrap_or(0) as usize
                + usage.cache_creation_input_tokens.unwrap_or(0) as usize;
            if api_count > 0 {
                api_count
            } else {
                estimate_message_tokens(message)
            }
        }
        _ => estimate_message_tokens(message),
    }
}

/// Compute aggregate token count across messages, using API counts where
/// available.
pub fn token_count_with_estimation_for_messages(messages: &[Message]) -> usize {
    messages.iter().map(token_count_with_estimation).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rough_estimation() {
        assert_eq!(rough_token_count_estimation("", 4), 0);
        assert_eq!(rough_token_count_estimation("hello world", 4), 3); // 11/4 rounded up
        assert_eq!(rough_token_count_estimation("abcd", 4), 1);
        assert_eq!(rough_token_count_estimation("abcde", 4), 2);
    }

    #[test]
    fn test_bytes_per_token_for_file_type() {
        assert_eq!(bytes_per_token_for_file_type("json"), 2);
        assert_eq!(bytes_per_token_for_file_type("rs"), 4);
        assert_eq!(bytes_per_token_for_file_type("ts"), 4);
    }

    #[test]
    fn test_content_block_text() {
        let block = ContentBlock::Text {
            text: "hello world".to_string(),
        };
        assert_eq!(estimate_content_block_tokens(&block), 3);
    }

    #[test]
    fn test_content_block_image() {
        let block = ContentBlock::Image {
            source: crate::types::content::ImageSource {
                source_type: "base64".into(),
                media_type: "image/png".into(),
                data: "abc".into(),
            },
        };
        assert_eq!(estimate_content_block_tokens(&block), 2000);
    }
}
