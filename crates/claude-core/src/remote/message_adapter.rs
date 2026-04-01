//! SDK message adapter.
//!
//! Converts SDK-format messages received from CCR (via WebSocket) into
//! internal message types suitable for display in the REPL. The CCR
//! backend sends messages in the SDK wire format; this adapter bridges
//! to the local representation.

use chrono::Utc;
use serde_json::Value;

/// Result of converting an SDK message.
#[derive(Clone, Debug)]
pub enum ConvertedMessage {
    /// A displayable message (assistant, system, etc.).
    Message(DisplayMessage),
    /// A streaming event (text delta, content block start, etc.).
    StreamEvent(Value),
    /// Message was intentionally ignored (echoes, noise, etc.).
    Ignored,
}

/// A display-ready message produced by the adapter.
#[derive(Clone, Debug)]
pub struct DisplayMessage {
    /// Message type for rendering.
    pub msg_type: DisplayMessageType,
    /// The inner message data.
    pub content: Value,
    /// Optional UUID for deduplication.
    pub uuid: Option<String>,
    /// ISO 8601 timestamp.
    pub timestamp: String,
}

/// Classification of display messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayMessageType {
    /// An assistant response (text, tool use, etc.).
    Assistant,
    /// A user message (when converting historical events).
    User,
    /// A system/informational message.
    System,
    /// A compact boundary marker.
    CompactBoundary,
}

/// Options controlling how messages are converted.
#[derive(Clone, Debug, Default)]
pub struct ConvertOptions {
    /// Convert user messages containing tool_result content blocks.
    /// Used by direct connect mode where tool results come from the
    /// remote server and need to be rendered locally.
    pub convert_tool_results: bool,
    /// Convert user text messages for display (historical events).
    pub convert_user_text_messages: bool,
}

/// Convert an SDK message to the internal display format.
///
/// Returns [`ConvertedMessage::Ignored`] for message types that should
/// not be rendered (echoes, acks, rate limits, etc.).
pub fn convert_sdk_message(msg: &Value, opts: &ConvertOptions) -> ConvertedMessage {
    let msg_type = match msg.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return ConvertedMessage::Ignored,
    };

    let now = Utc::now().to_rfc3339();
    let uuid = msg
        .get("uuid")
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    match msg_type {
        "assistant" => ConvertedMessage::Message(DisplayMessage {
            msg_type: DisplayMessageType::Assistant,
            content: msg.clone(),
            uuid,
            timestamp: now,
        }),

        "user" => convert_user_message(msg, opts, uuid, &now),

        "stream_event" => {
            let event = msg.get("event").cloned().unwrap_or(Value::Null);
            ConvertedMessage::StreamEvent(event)
        }

        "result" => {
            let subtype = msg.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
            // Only show error results — success is noise in multi-turn sessions
            if subtype != "success" {
                let errors = msg
                    .get("errors")
                    .and_then(|e| e.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_else(|| "Unknown error".to_string());
                ConvertedMessage::Message(DisplayMessage {
                    msg_type: DisplayMessageType::System,
                    content: serde_json::json!({
                        "type": "system",
                        "subtype": "informational",
                        "content": errors,
                        "level": "warning",
                    }),
                    uuid,
                    timestamp: now,
                })
            } else {
                ConvertedMessage::Ignored
            }
        }

        "system" => convert_system_message(msg, uuid, &now),

        "tool_progress" => {
            let tool_name = msg
                .get("tool_name")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown");
            let elapsed = msg
                .get("elapsed_time_seconds")
                .and_then(|e| e.as_f64())
                .unwrap_or(0.0);
            ConvertedMessage::Message(DisplayMessage {
                msg_type: DisplayMessageType::System,
                content: serde_json::json!({
                    "type": "system",
                    "subtype": "informational",
                    "content": format!("Tool {tool_name} running for {elapsed:.0}s..."),
                    "level": "info",
                }),
                uuid,
                timestamp: now,
            })
        }

        // Auth status, tool use summaries, rate limit events are SDK-only
        "auth_status" | "tool_use_summary" | "rate_limit_event" => ConvertedMessage::Ignored,

        _ => {
            tracing::debug!("sdkMessageAdapter: unknown message type: {msg_type}");
            ConvertedMessage::Ignored
        }
    }
}

/// Check if an SDK message indicates the session has ended.
pub fn is_session_end_message(msg: &Value) -> bool {
    msg.get("type").and_then(|t| t.as_str()) == Some("result")
}

/// Check if an SDK result message indicates success.
pub fn is_success_result(msg: &Value) -> bool {
    msg.get("type").and_then(|t| t.as_str()) == Some("result")
        && msg.get("subtype").and_then(|s| s.as_str()) == Some("success")
}

/// Extract the result text from a successful SDK result message.
pub fn get_result_text(msg: &Value) -> Option<String> {
    if is_success_result(msg) {
        msg.get("result")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}

/// Convert a user SDK message based on options.
fn convert_user_message(
    msg: &Value,
    opts: &ConvertOptions,
    uuid: Option<String>,
    timestamp: &str,
) -> ConvertedMessage {
    let content = msg.get("message").and_then(|m| m.get("content"));

    // Check if it's a tool result message
    let is_tool_result = content
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
        })
        .unwrap_or(false);

    if opts.convert_tool_results && is_tool_result {
        return ConvertedMessage::Message(DisplayMessage {
            msg_type: DisplayMessageType::User,
            content: msg.clone(),
            uuid,
            timestamp: timestamp.to_string(),
        });
    }

    if opts.convert_user_text_messages && !is_tool_result && content.is_some() {
        return ConvertedMessage::Message(DisplayMessage {
            msg_type: DisplayMessageType::User,
            content: msg.clone(),
            uuid,
            timestamp: timestamp.to_string(),
        });
    }

    ConvertedMessage::Ignored
}

/// Convert a system SDK message based on subtype.
fn convert_system_message(msg: &Value, uuid: Option<String>, timestamp: &str) -> ConvertedMessage {
    let subtype = msg.get("subtype").and_then(|s| s.as_str()).unwrap_or("");

    match subtype {
        "init" => {
            let model = msg
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            ConvertedMessage::Message(DisplayMessage {
                msg_type: DisplayMessageType::System,
                content: serde_json::json!({
                    "type": "system",
                    "subtype": "informational",
                    "content": format!("Remote session initialized (model: {model})"),
                    "level": "info",
                }),
                uuid,
                timestamp: timestamp.to_string(),
            })
        }
        "status" => {
            let status = msg.get("status").and_then(|s| s.as_str());
            match status {
                Some("compacting") => ConvertedMessage::Message(DisplayMessage {
                    msg_type: DisplayMessageType::System,
                    content: serde_json::json!({
                        "type": "system",
                        "subtype": "informational",
                        "content": "Compacting conversation...",
                        "level": "info",
                    }),
                    uuid,
                    timestamp: timestamp.to_string(),
                }),
                Some(s) => ConvertedMessage::Message(DisplayMessage {
                    msg_type: DisplayMessageType::System,
                    content: serde_json::json!({
                        "type": "system",
                        "subtype": "informational",
                        "content": format!("Status: {s}"),
                        "level": "info",
                    }),
                    uuid,
                    timestamp: timestamp.to_string(),
                }),
                None => ConvertedMessage::Ignored,
            }
        }
        "compact_boundary" => ConvertedMessage::Message(DisplayMessage {
            msg_type: DisplayMessageType::CompactBoundary,
            content: serde_json::json!({
                "type": "system",
                "subtype": "compact_boundary",
                "content": "Conversation compacted",
                "level": "info",
            }),
            uuid,
            timestamp: timestamp.to_string(),
        }),
        _ => {
            tracing::debug!("sdkMessageAdapter: ignoring system message subtype: {subtype}");
            ConvertedMessage::Ignored
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_convert_assistant_message() {
        let msg = json!({
            "type": "assistant",
            "uuid": "abc-123",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "Hello"}]}
        });
        let result = convert_sdk_message(&msg, &ConvertOptions::default());
        match result {
            ConvertedMessage::Message(dm) => {
                assert_eq!(dm.msg_type, DisplayMessageType::Assistant);
                assert_eq!(dm.uuid, Some("abc-123".to_string()));
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_convert_result_success_ignored() {
        let msg = json!({"type": "result", "subtype": "success", "result": "done"});
        let result = convert_sdk_message(&msg, &ConvertOptions::default());
        assert!(matches!(result, ConvertedMessage::Ignored));
    }

    #[test]
    fn test_convert_result_error_shown() {
        let msg = json!({"type": "result", "subtype": "error", "errors": ["bad thing"]});
        let result = convert_sdk_message(&msg, &ConvertOptions::default());
        assert!(matches!(result, ConvertedMessage::Message(_)));
    }

    #[test]
    fn test_convert_stream_event() {
        let msg = json!({"type": "stream_event", "event": {"type": "content_block_delta"}});
        let result = convert_sdk_message(&msg, &ConvertOptions::default());
        assert!(matches!(result, ConvertedMessage::StreamEvent(_)));
    }

    #[test]
    fn test_convert_system_init() {
        let msg = json!({"type": "system", "subtype": "init", "model": "claude-sonnet-4-6"});
        let result = convert_sdk_message(&msg, &ConvertOptions::default());
        match result {
            ConvertedMessage::Message(dm) => {
                assert_eq!(dm.msg_type, DisplayMessageType::System);
                let content = dm.content["content"].as_str().unwrap();
                assert!(content.contains("claude-sonnet-4-6"));
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn test_convert_user_message_ignored_by_default() {
        let msg = json!({"type": "user", "message": {"content": "hi"}});
        let result = convert_sdk_message(&msg, &ConvertOptions::default());
        assert!(matches!(result, ConvertedMessage::Ignored));
    }

    #[test]
    fn test_convert_user_text_when_enabled() {
        let msg = json!({"type": "user", "message": {"content": "hi"}});
        let opts = ConvertOptions {
            convert_user_text_messages: true,
            ..Default::default()
        };
        let result = convert_sdk_message(&msg, &opts);
        assert!(matches!(result, ConvertedMessage::Message(_)));
    }

    #[test]
    fn test_is_session_end_message() {
        assert!(is_session_end_message(&json!({"type": "result"})));
        assert!(!is_session_end_message(&json!({"type": "assistant"})));
    }

    #[test]
    fn test_get_result_text() {
        let msg = json!({"type": "result", "subtype": "success", "result": "all done"});
        assert_eq!(get_result_text(&msg), Some("all done".to_string()));

        let fail_msg = json!({"type": "result", "subtype": "error"});
        assert_eq!(get_result_text(&fail_msg), None);
    }

    #[test]
    fn test_unknown_type_ignored() {
        let msg = json!({"type": "future_type", "data": "stuff"});
        let result = convert_sdk_message(&msg, &ConvertOptions::default());
        assert!(matches!(result, ConvertedMessage::Ignored));
    }

    #[test]
    fn test_auth_status_ignored() {
        let msg = json!({"type": "auth_status"});
        let result = convert_sdk_message(&msg, &ConvertOptions::default());
        assert!(matches!(result, ConvertedMessage::Ignored));
    }
}
