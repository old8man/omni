//! Bridge message handling and type guards.
//!
//! Provides transport-layer helpers for parsing, routing, and deduplicating
//! messages between the bridge and backend. Includes type guards for
//! SDK messages, control requests/responses, and a bounded UUID set for
//! echo deduplication.

use serde_json::Value;

// ---------------------------------------------------------------------------
// Type guards
// ---------------------------------------------------------------------------

/// Check if a parsed JSON value is a valid SDK message (has a string `type` field).
pub fn is_sdk_message(value: &Value) -> bool {
    value.is_object() && value.get("type").and_then(|t| t.as_str()).is_some()
}

/// Check if a parsed JSON value is a control_response message.
pub fn is_sdk_control_response(value: &Value) -> bool {
    value.get("type").and_then(|t| t.as_str()) == Some("control_response")
        && value.get("response").is_some()
}

/// Check if a parsed JSON value is a control_request message.
pub fn is_sdk_control_request(value: &Value) -> bool {
    value.get("type").and_then(|t| t.as_str()) == Some("control_request")
        && value.get("request_id").is_some()
        && value.get("request").is_some()
}

/// Check if a parsed JSON value is a control_cancel_request message.
pub fn is_sdk_control_cancel_request(value: &Value) -> bool {
    value.get("type").and_then(|t| t.as_str()) == Some("control_cancel_request")
        && value.get("request_id").is_some()
}

/// Determine whether a message is eligible for bridge forwarding.
///
/// The server only wants user/assistant turns and slash-command system events;
/// progress updates, tool results, and other internal chatter is filtered out.
pub fn is_eligible_bridge_message(msg: &Value) -> bool {
    let msg_type = match msg.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return false,
    };

    // Virtual messages are display-only
    if (msg_type == "user" || msg_type == "assistant")
        && msg.get("isVirtual").and_then(|v| v.as_bool()) == Some(true)
    {
        return false;
    }

    matches!(msg_type, "user" | "assistant")
        || (msg_type == "system"
            && msg.get("subtype").and_then(|s| s.as_str()) == Some("local_command"))
}

/// Extract the UUID from an SDK message, if present.
pub fn extract_message_uuid(msg: &Value) -> Option<&str> {
    msg.get("uuid").and_then(|u| u.as_str())
}

// ---------------------------------------------------------------------------
// BoundedUuidSet — FIFO ring buffer for echo deduplication
// ---------------------------------------------------------------------------

/// A FIFO-bounded set backed by a circular buffer.
///
/// Evicts the oldest entry when capacity is reached, keeping memory usage
/// constant at O(capacity). Used for deduplicating echoed messages on the
/// bridge WebSocket.
pub struct BoundedUuidSet {
    ring: Vec<Option<String>>,
    set: std::collections::HashSet<String>,
    write_idx: usize,
    capacity: usize,
}

impl BoundedUuidSet {
    /// Create a new bounded UUID set with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            ring: vec![None; capacity],
            set: std::collections::HashSet::with_capacity(capacity),
            write_idx: 0,
            capacity,
        }
    }

    /// Add a UUID to the set, evicting the oldest entry if at capacity.
    pub fn add(&mut self, uuid: String) {
        if self.set.contains(&uuid) {
            return;
        }
        // Evict the entry at the current write position
        if let Some(evicted) = self.ring[self.write_idx].take() {
            self.set.remove(&evicted);
        }
        self.ring[self.write_idx] = Some(uuid.clone());
        self.set.insert(uuid);
        self.write_idx = (self.write_idx + 1) % self.capacity;
    }

    /// Check if a UUID is in the set.
    pub fn contains(&self, uuid: &str) -> bool {
        self.set.contains(uuid)
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.set.clear();
        self.ring.fill(None);
        self.write_idx = 0;
    }
}

// ---------------------------------------------------------------------------
// Ingress message routing
// ---------------------------------------------------------------------------

/// Classification of a parsed ingress message.
#[derive(Debug)]
pub enum IngressMessage<'a> {
    /// A control response from the server (permission decision ack, etc.).
    ControlResponse(&'a Value),
    /// A control request from the server (initialize, interrupt, set_model, etc.).
    ControlRequest(&'a Value),
    /// A control cancel request from the server.
    ControlCancelRequest(&'a Value),
    /// A standard SDK message (user, assistant, result, etc.).
    SdkMessage(&'a Value),
    /// Could not be classified — ignore.
    Unknown,
}

/// Classify a parsed JSON message from the ingress WebSocket.
pub fn classify_ingress_message(value: &Value) -> IngressMessage<'_> {
    if is_sdk_control_response(value) {
        return IngressMessage::ControlResponse(value);
    }
    if is_sdk_control_request(value) {
        return IngressMessage::ControlRequest(value);
    }
    if is_sdk_control_cancel_request(value) {
        return IngressMessage::ControlCancelRequest(value);
    }
    if is_sdk_message(value) {
        return IngressMessage::SdkMessage(value);
    }
    IngressMessage::Unknown
}

/// Build a minimal `control_response` for acknowledging a control request.
pub fn make_control_response_success(request_id: &str, response_payload: Value) -> Value {
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": response_payload,
        }
    })
}

/// Build an error `control_response` for unrecognized or failed control requests.
pub fn make_control_response_error(request_id: &str, error: &str) -> Value {
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request_id,
            "error": error,
        }
    })
}

/// Build a minimal `SDKResultSuccess` message for session archival.
pub fn make_result_message(session_id: &str) -> Value {
    serde_json::json!({
        "type": "result",
        "subtype": "success",
        "duration_ms": 0,
        "duration_api_ms": 0,
        "is_error": false,
        "num_turns": 0,
        "result": "",
        "stop_reason": null,
        "total_cost_usd": 0,
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
        },
        "model_usage": {},
        "permission_denials": [],
        "session_id": session_id,
        "uuid": uuid::Uuid::new_v4().to_string(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_is_sdk_message() {
        assert!(is_sdk_message(&json!({"type": "assistant"})));
        assert!(is_sdk_message(&json!({"type": "user", "content": "hi"})));
        assert!(!is_sdk_message(&json!({"no_type": true})));
        assert!(!is_sdk_message(&json!(42)));
    }

    #[test]
    fn test_is_sdk_control_response() {
        let msg = json!({"type": "control_response", "response": {"subtype": "success"}});
        assert!(is_sdk_control_response(&msg));
        assert!(!is_sdk_control_response(
            &json!({"type": "control_response"})
        ));
    }

    #[test]
    fn test_is_sdk_control_request() {
        let msg = json!({
            "type": "control_request",
            "request_id": "req-1",
            "request": {"subtype": "initialize"}
        });
        assert!(is_sdk_control_request(&msg));
        assert!(!is_sdk_control_request(&json!({"type": "control_request"})));
    }

    #[test]
    fn test_is_eligible_bridge_message() {
        assert!(is_eligible_bridge_message(&json!({"type": "user"})));
        assert!(is_eligible_bridge_message(&json!({"type": "assistant"})));
        assert!(is_eligible_bridge_message(
            &json!({"type": "system", "subtype": "local_command"})
        ));
        assert!(!is_eligible_bridge_message(
            &json!({"type": "system", "subtype": "informational"})
        ));
        // Virtual messages should be filtered
        assert!(!is_eligible_bridge_message(
            &json!({"type": "user", "isVirtual": true})
        ));
    }

    #[test]
    fn test_bounded_uuid_set() {
        let mut set = BoundedUuidSet::new(3);
        set.add("a".to_string());
        set.add("b".to_string());
        set.add("c".to_string());
        assert!(set.contains("a"));
        assert!(set.contains("b"));
        assert!(set.contains("c"));

        // Adding a 4th should evict "a"
        set.add("d".to_string());
        assert!(!set.contains("a"));
        assert!(set.contains("b"));
        assert!(set.contains("d"));
    }

    #[test]
    fn test_bounded_uuid_set_dedup() {
        let mut set = BoundedUuidSet::new(3);
        set.add("a".to_string());
        set.add("a".to_string()); // Should not advance write pointer
        set.add("b".to_string());
        set.add("c".to_string());
        // "a" should still be present since dedup prevented pointer advance
        assert!(set.contains("a"));
    }

    #[test]
    fn test_classify_ingress_message() {
        let ctrl_resp = json!({"type": "control_response", "response": {}});
        assert!(matches!(
            classify_ingress_message(&ctrl_resp),
            IngressMessage::ControlResponse(_)
        ));

        let ctrl_req = json!({"type": "control_request", "request_id": "r1", "request": {}});
        assert!(matches!(
            classify_ingress_message(&ctrl_req),
            IngressMessage::ControlRequest(_)
        ));

        let sdk_msg = json!({"type": "assistant"});
        assert!(matches!(
            classify_ingress_message(&sdk_msg),
            IngressMessage::SdkMessage(_)
        ));

        let unknown = json!(42);
        assert!(matches!(
            classify_ingress_message(&unknown),
            IngressMessage::Unknown
        ));
    }

    #[test]
    fn test_make_control_response_success() {
        let resp = make_control_response_success("req-1", json!({"ok": true}));
        assert_eq!(resp["response"]["subtype"], "success");
        assert_eq!(resp["response"]["request_id"], "req-1");
    }

    #[test]
    fn test_make_control_response_error() {
        let resp = make_control_response_error("req-1", "something broke");
        assert_eq!(resp["response"]["subtype"], "error");
        assert_eq!(resp["response"]["error"], "something broke");
    }
}
