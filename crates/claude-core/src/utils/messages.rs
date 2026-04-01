//! Message creation helpers, queries, manipulation, and synthetic messages.

use crate::types::content::ContentBlock;
use crate::types::message::{
    ApiMessage, AssistantMessage, Message, Role, StopReason, SystemMessage, UserMessage,
};
use crate::types::usage::Usage;
use chrono::Utc;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const SYNTHETIC_MODEL: &str = "<synthetic>";

pub const INTERRUPT_MESSAGE: &str = "[Request interrupted by user]";
pub const INTERRUPT_MESSAGE_FOR_TOOL_USE: &str =
    "[Request interrupted by user for tool use]";
pub const CANCEL_MESSAGE: &str =
    "The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed.";
pub const REJECT_MESSAGE: &str =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
pub const NO_RESPONSE_REQUESTED: &str = "No response requested.";
pub const SYNTHETIC_TOOL_RESULT_PLACEHOLDER: &str =
    "[Tool result missing due to internal error]";

/// Set of content strings that mark a message as synthetic (not from the API).
const SYNTHETIC_CONTENT: &[&str] = &[
    INTERRUPT_MESSAGE,
    INTERRUPT_MESSAGE_FOR_TOOL_USE,
    CANCEL_MESSAGE,
    REJECT_MESSAGE,
    NO_RESPONSE_REQUESTED,
];

/// Check if a text string matches any of the known synthetic content markers.
pub fn is_synthetic_text(text: &str) -> bool {
    SYNTHETIC_CONTENT.contains(&text)
}

// ---------------------------------------------------------------------------
// Message creation helpers
// ---------------------------------------------------------------------------

/// Create a user message with text content.
pub fn create_user_message(text: &str) -> Message {
    Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: Utc::now(),
    })
}

/// Create a user message with arbitrary content blocks.
pub fn create_user_message_with_content(content: Vec<ContentBlock>) -> Message {
    Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        content,
        timestamp: Utc::now(),
    })
}

/// Create a synthetic assistant message with text content.
pub fn create_assistant_message(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        uuid: Uuid::new_v4(),
        message: ApiMessage {
            id: Uuid::new_v4().to_string(),
            model: SYNTHETIC_MODEL.to_string(),
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
        },
        request_id: None,
        timestamp: Utc::now(),
    })
}

/// Create a synthetic assistant message with full content blocks.
pub fn create_assistant_message_with_content(content: Vec<ContentBlock>) -> Message {
    Message::Assistant(AssistantMessage {
        uuid: Uuid::new_v4(),
        message: ApiMessage {
            id: Uuid::new_v4().to_string(),
            model: SYNTHETIC_MODEL.to_string(),
            role: Role::Assistant,
            content,
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
        },
        request_id: None,
        timestamp: Utc::now(),
    })
}

/// Create a system message (compact boundary).
pub fn create_system_message(summary: &str) -> Message {
    Message::System(SystemMessage::CompactBoundary {
        summary: summary.to_string(),
    })
}

/// Create a tool result user message for a given tool_use_id.
pub fn create_tool_result_message(
    tool_use_id: &str,
    content: Vec<ContentBlock>,
    is_error: bool,
) -> Message {
    Message::User(UserMessage {
        uuid: Uuid::new_v4(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content,
            is_error: if is_error { Some(true) } else { None },
        }],
        timestamp: Utc::now(),
    })
}

// ---------------------------------------------------------------------------
// Message queries
// ---------------------------------------------------------------------------

/// Returns `true` if the message is an assistant message containing at least one tool_use block.
pub fn is_tool_use_message(message: &Message) -> bool {
    match message {
        Message::Assistant(asst) => asst
            .message
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
        _ => false,
    }
}

/// Returns `true` if the message is a user message containing at least one tool_result block.
pub fn is_tool_result_message(message: &Message) -> bool {
    match message {
        Message::User(user) => user
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. })),
        _ => false,
    }
}

/// Collect all tool_use IDs from an assistant message.
pub fn get_tool_use_ids(message: &Message) -> Vec<String> {
    match message {
        Message::Assistant(asst) => asst
            .message
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Extract concatenated text content from a message.
pub fn get_text_content(message: &Message) -> String {
    let blocks = match message {
        Message::User(u) => &u.content,
        Message::Assistant(a) => &a.message.content,
        Message::System(_) => return String::new(),
    };
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Returns the last assistant message from a slice, if any.
pub fn get_last_assistant_message(messages: &[Message]) -> Option<&AssistantMessage> {
    messages.iter().rev().find_map(|m| match m {
        Message::Assistant(a) => Some(a),
        _ => None,
    })
}

/// Returns `true` if the last assistant turn contains tool calls.
pub fn has_tool_calls_in_last_assistant_turn(messages: &[Message]) -> bool {
    for msg in messages.iter().rev() {
        if let Message::Assistant(a) = msg {
            return a
                .message
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Synthetic message detection
// ---------------------------------------------------------------------------

/// Returns `true` if the message is synthetic (locally generated, not from the API).
pub fn is_synthetic_message(message: &Message) -> bool {
    match message {
        Message::Assistant(asst) => {
            if asst.message.model == SYNTHETIC_MODEL {
                return true;
            }
            if let Some(ContentBlock::Text { text }) = asst.message.content.first() {
                return SYNTHETIC_CONTENT.contains(&text.as_str());
            }
            false
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Message manipulation
// ---------------------------------------------------------------------------

/// Merge adjacent assistant messages that have the same API response id.
///
/// When parallel tool calls are streamed, each content block becomes a separate
/// AssistantMessage record sharing the same `message.id`. This function coalesces
/// them back into a single message.
pub fn merge_adjacent_messages(messages: Vec<Message>) -> Vec<Message> {
    let mut result: Vec<Message> = Vec::with_capacity(messages.len());

    for msg in messages {
        let should_merge = if let (Some(Message::Assistant(prev)), Message::Assistant(curr)) =
            (result.last(), &msg)
        {
            prev.message.id == curr.message.id
        } else {
            false
        };

        if should_merge {
            if let Some(Message::Assistant(prev)) = result.last_mut() {
                if let Message::Assistant(curr) = msg {
                    prev.message.content.extend(curr.message.content);
                    // Keep the higher usage (last chunk typically has final usage).
                    if curr.message.usage.input_tokens > 0 || curr.message.usage.output_tokens > 0 {
                        prev.message.usage = curr.message.usage;
                    }
                }
            }
        } else {
            result.push(msg);
        }
    }

    result
}

/// Remove messages whose content is empty.
pub fn filter_empty_messages(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .filter(|msg| match msg {
            Message::User(u) => !u.content.is_empty(),
            Message::Assistant(a) => !a.message.content.is_empty(),
            Message::System(_) => true,
        })
        .collect()
}

/// Strip thinking/redacted_thinking blocks from all assistant messages.
pub fn strip_thinking_blocks(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .map(|msg| match msg {
            Message::Assistant(mut a) => {
                a.message.content.retain(|b| {
                    !matches!(
                        b,
                        ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
                    )
                });
                Message::Assistant(a)
            }
            other => other,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Synthetic / interruption messages
// ---------------------------------------------------------------------------

/// Create a synthetic tool_result for a tool_use that has no matching result.
pub fn create_synthetic_tool_result(tool_use_id: &str) -> Message {
    create_tool_result_message(
        tool_use_id,
        vec![ContentBlock::Text {
            text: SYNTHETIC_TOOL_RESULT_PLACEHOLDER.to_string(),
        }],
        true,
    )
}

/// Create an interruption message to signal the user cancelled a request.
pub fn create_interruption_message() -> Message {
    create_assistant_message(INTERRUPT_MESSAGE)
}

/// Create an interruption message specifically for tool use interruptions.
pub fn create_tool_use_interruption_message() -> Message {
    create_assistant_message(INTERRUPT_MESSAGE_FOR_TOOL_USE)
}

// ---------------------------------------------------------------------------
// Tool use summaries
// ---------------------------------------------------------------------------

/// Generate a concise summary string for tool use blocks in an assistant message.
///
/// Produces output like: "Used tools: Bash, FileRead, FileEdit"
pub fn generate_tool_use_summary(message: &Message) -> Option<String> {
    let tool_names: Vec<&str> = match message {
        Message::Assistant(a) => a
            .message
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect(),
        _ => return None,
    };

    if tool_names.is_empty() {
        return None;
    }

    Some(format!("Used tools: {}", tool_names.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_user_message() {
        let msg = create_user_message("hello");
        assert!(matches!(&msg, Message::User(u) if get_text_content(&msg) == "hello" && !u.content.is_empty()));
    }

    #[test]
    fn test_is_tool_use_message() {
        let msg = create_assistant_message_with_content(vec![ContentBlock::ToolUse {
            id: "tu_1".into(),
            name: "Bash".into(),
            input: serde_json::json!({}),
        }]);
        assert!(is_tool_use_message(&msg));
        assert!(!is_tool_use_message(&create_user_message("hi")));
    }

    #[test]
    fn test_get_tool_use_ids() {
        let msg = create_assistant_message_with_content(vec![
            ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "A".into(),
                input: serde_json::json!({}),
            },
            ContentBlock::ToolUse {
                id: "tu_2".into(),
                name: "B".into(),
                input: serde_json::json!({}),
            },
        ]);
        assert_eq!(get_tool_use_ids(&msg), vec!["tu_1", "tu_2"]);
    }

    #[test]
    fn test_strip_thinking_blocks() {
        let msgs = vec![create_assistant_message_with_content(vec![
            ContentBlock::Thinking {
                thinking: "hmm".into(),
                signature: "sig".into(),
            },
            ContentBlock::Text {
                text: "answer".into(),
            },
        ])];
        let stripped = strip_thinking_blocks(msgs);
        if let Message::Assistant(a) = &stripped[0] {
            assert_eq!(a.message.content.len(), 1);
            assert!(matches!(&a.message.content[0], ContentBlock::Text { text } if text == "answer"));
        } else {
            panic!("expected assistant");
        }
    }

    #[test]
    fn test_filter_empty_messages() {
        let msgs = vec![
            create_user_message("hello"),
            create_user_message_with_content(vec![]),
            create_assistant_message("world"),
        ];
        let filtered = filter_empty_messages(msgs);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_is_synthetic_message() {
        let msg = create_interruption_message();
        assert!(is_synthetic_message(&msg));
        let msg = create_user_message("hi");
        assert!(!is_synthetic_message(&msg));
    }

    #[test]
    fn test_generate_tool_use_summary() {
        let msg = create_assistant_message_with_content(vec![
            ContentBlock::ToolUse {
                id: "1".into(),
                name: "Bash".into(),
                input: serde_json::json!({}),
            },
            ContentBlock::Text {
                text: "done".into(),
            },
            ContentBlock::ToolUse {
                id: "2".into(),
                name: "FileRead".into(),
                input: serde_json::json!({}),
            },
        ]);
        assert_eq!(
            generate_tool_use_summary(&msg),
            Some("Used tools: Bash, FileRead".to_string())
        );
    }
}
