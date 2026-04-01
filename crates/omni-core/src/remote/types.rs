//! Types for remote session management.
//!
//! Defines the configuration, callback interfaces, and message types used
//! by the remote session manager and its WebSocket transport.

use serde::{Deserialize, Serialize};

/// WebSocket reconnection delay in milliseconds.
pub const RECONNECT_DELAY_MS: u64 = 2_000;

/// Maximum reconnection attempts before giving up.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Ping interval to keep WebSocket alive, in milliseconds.
pub const PING_INTERVAL_MS: u64 = 30_000;

/// Maximum retries for session-not-found (4001) close codes.
pub const MAX_SESSION_NOT_FOUND_RETRIES: u32 = 3;

/// WebSocket close codes that indicate permanent rejection.
pub const PERMANENT_CLOSE_CODES: &[u16] = &[4003]; // unauthorized

/// WebSocket connection state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketState {
    /// Attempting to establish a connection.
    Connecting,
    /// Connection is open and authenticated.
    Connected,
    /// Connection has been closed.
    Closed,
}

/// Configuration for a remote session connection.
#[derive(Clone, Debug)]
pub struct RemoteSessionConfig {
    /// The session ID to connect to.
    pub session_id: String,
    /// Closure returning a fresh OAuth access token.
    pub access_token: String,
    /// Organization UUID for the API.
    pub org_uuid: String,
    /// Whether the session was created with an initial prompt.
    pub has_initial_prompt: bool,
    /// When true, this client is a pure viewer (no interrupts, no title updates).
    pub viewer_only: bool,
    /// API base URL.
    pub api_base_url: String,
}

/// Configuration for a direct WebSocket connection.
#[derive(Clone, Debug)]
pub struct DirectConnectConfig {
    /// HTTP server URL.
    pub server_url: String,
    /// Session identifier.
    pub session_id: String,
    /// WebSocket URL to connect to.
    pub ws_url: String,
    /// Optional authentication token.
    pub auth_token: Option<String>,
}

/// Permission response sent back to the remote server.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "behavior")]
pub enum RemotePermissionResponse {
    /// Allow the tool use with potentially updated input.
    #[serde(rename = "allow")]
    Allow {
        /// Updated tool input (may be unchanged from original).
        #[serde(rename = "updatedInput")]
        updated_input: serde_json::Value,
    },
    /// Deny the tool use with a reason.
    #[serde(rename = "deny")]
    Deny {
        /// Human-readable denial reason.
        message: String,
    },
}

/// Message content that can be sent to a remote session.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RemoteMessageContent {
    /// Plain text content.
    Text(String),
    /// Structured content blocks (same as API content format).
    Blocks(Vec<serde_json::Value>),
}

/// An SDK message received from a remote session.
///
/// This is a thin wrapper around the raw JSON, since SDK messages have
/// many variants and we defer parsing to the message adapter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SdkMessage {
    /// The message type discriminant (e.g. "assistant", "user", "result").
    #[serde(rename = "type")]
    pub msg_type: String,
    /// The full raw JSON for downstream processing.
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

/// A control request from the server (e.g. permission prompt).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlRequest {
    /// Unique request identifier.
    pub request_id: String,
    /// The inner request payload.
    pub request: ControlRequestInner,
}

/// Inner payload of a [`ControlRequest`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlRequestInner {
    /// Request subtype (e.g. "can_use_tool", "initialize", "interrupt").
    pub subtype: String,
    /// Tool name for `can_use_tool` requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool use ID for `can_use_tool` requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Tool input for `can_use_tool` requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    /// All other fields.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// A control cancel request from the server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlCancelRequest {
    /// Request ID being cancelled.
    pub request_id: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_response_allow_serialization() {
        let resp = RemotePermissionResponse::Allow {
            updated_input: serde_json::json!({"path": "/tmp/test"}),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["behavior"], "allow");
        assert_eq!(json["updatedInput"]["path"], "/tmp/test");
    }

    #[test]
    fn test_permission_response_deny_serialization() {
        let resp = RemotePermissionResponse::Deny {
            message: "Not allowed".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["behavior"], "deny");
        assert_eq!(json["message"], "Not allowed");
    }

    #[test]
    fn test_remote_message_content_text() {
        let content = RemoteMessageContent::Text("hello".to_string());
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, "hello");
    }

    #[test]
    fn test_websocket_state_transitions() {
        // Verify state enum variants are distinct
        assert_ne!(WebSocketState::Connecting, WebSocketState::Connected);
        assert_ne!(WebSocketState::Connected, WebSocketState::Closed);
    }
}
