//! Remote session manager.
//!
//! Coordinates real-time communication with a remote CCR session via
//! WebSocket subscription. Handles message routing, permission
//! request/response flows, and connection lifecycle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::json;

use super::types::{
    ControlRequest, ControlRequestInner, RemotePermissionResponse, RemoteSessionConfig,
};
use super::websocket::SessionsWebSocket;

/// Callbacks invoked by the remote session manager.
pub trait RemoteSessionCallbacks: Send + Sync {
    /// Called when an SDK message is received from the session.
    fn on_message(&self, message: serde_json::Value);

    /// Called when a permission request is received from CCR.
    fn on_permission_request(&self, request: ControlRequest);

    /// Called when the server cancels a pending permission request.
    fn on_permission_cancelled(&self, request_id: &str, tool_use_id: Option<&str>);

    /// Called when the WebSocket connection is established.
    fn on_connected(&self);

    /// Called when connection is lost and cannot be restored.
    fn on_disconnected(&self);

    /// Called when a transient drop occurs and reconnect is in progress.
    fn on_reconnecting(&self);

    /// Called on error.
    fn on_error(&self, error: &str);
}

/// Manages a remote CCR session.
///
/// Coordinates:
/// - WebSocket subscription for receiving messages from CCR
/// - Permission request/response flow
/// - Connection lifecycle
pub struct RemoteSessionManager {
    config: RemoteSessionConfig,
    callbacks: Arc<dyn RemoteSessionCallbacks>,
    websocket: Option<SessionsWebSocket>,
    pending_permission_requests: Arc<Mutex<HashMap<String, ControlRequest>>>,
}

impl RemoteSessionManager {
    /// Create a new remote session manager.
    pub fn new(config: RemoteSessionConfig, callbacks: Arc<dyn RemoteSessionCallbacks>) -> Self {
        Self {
            config,
            callbacks,
            websocket: None,
            pending_permission_requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Connect to the remote session via WebSocket.
    pub async fn connect(&mut self) -> Result<()> {
        tracing::debug!(
            "RemoteSessionManager: connecting to session {}",
            self.config.session_id
        );

        let pending = Arc::clone(&self.pending_permission_requests);
        let callbacks = Arc::clone(&self.callbacks);
        let session_id = self.config.session_id.clone();

        let ws_callbacks = WebSocketCallbackAdapter {
            pending,
            callbacks,
            session_id,
        };

        let mut ws = SessionsWebSocket::new(
            self.config.session_id.clone(),
            self.config.org_uuid.clone(),
            self.config.access_token.clone(),
            self.config.api_base_url.clone(),
            Arc::new(ws_callbacks),
        );

        ws.connect().await?;
        self.websocket = Some(ws);
        Ok(())
    }

    /// Respond to a permission request from CCR.
    pub fn respond_to_permission_request(
        &self,
        request_id: &str,
        result: RemotePermissionResponse,
    ) {
        let pending = self
            .pending_permission_requests
            .lock()
            .expect("pending_permission_requests mutex poisoned");
        if !pending.contains_key(request_id) {
            tracing::warn!(
                "RemoteSessionManager: no pending permission request with ID {request_id}"
            );
            return;
        }
        drop(pending);

        // Remove from pending
        self.pending_permission_requests
            .lock()
            .expect("pending_permission_requests mutex poisoned")
            .remove(request_id);

        // Build the control response
        let response_payload = match &result {
            RemotePermissionResponse::Allow { updated_input } => {
                json!({
                    "behavior": "allow",
                    "updatedInput": updated_input,
                })
            }
            RemotePermissionResponse::Deny { message } => {
                json!({
                    "behavior": "deny",
                    "message": message,
                })
            }
        };

        let response = json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": response_payload,
            }
        });

        if let Some(ws) = &self.websocket {
            ws.send_json(&response);
        }
    }

    /// Check if connected to the remote session.
    pub fn is_connected(&self) -> bool {
        self.websocket.as_ref().is_some_and(|ws| ws.is_connected())
    }

    /// Send an interrupt signal to cancel the current request.
    pub fn cancel_session(&self) {
        tracing::debug!("RemoteSessionManager: sending interrupt signal");
        if let Some(ws) = &self.websocket {
            let request = json!({
                "type": "control_request",
                "request_id": uuid::Uuid::new_v4().to_string(),
                "request": {
                    "subtype": "interrupt",
                }
            });
            ws.send_json(&request);
        }
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.config.session_id
    }

    /// Disconnect from the remote session.
    pub fn disconnect(&mut self) {
        tracing::debug!("RemoteSessionManager: disconnecting");
        if let Some(ws) = self.websocket.take() {
            ws.close();
        }
        self.pending_permission_requests
            .lock()
            .expect("pending_permission_requests mutex poisoned")
            .clear();
    }

    /// Force reconnect the WebSocket.
    pub async fn reconnect(&mut self) -> Result<()> {
        tracing::debug!("RemoteSessionManager: reconnecting WebSocket");
        if let Some(ws) = self.websocket.take() {
            ws.close();
        }
        self.connect().await
    }
}

/// Adapts WebSocket events to RemoteSessionCallbacks + permission tracking.
struct WebSocketCallbackAdapter {
    pending: Arc<Mutex<HashMap<String, ControlRequest>>>,
    callbacks: Arc<dyn RemoteSessionCallbacks>,
    session_id: String,
}

impl super::websocket::WebSocketCallbacks for WebSocketCallbackAdapter {
    fn on_message(&self, message: serde_json::Value) {
        let msg_type = message.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match msg_type {
            "control_request" => {
                self.handle_control_request(&message);
            }
            "control_cancel_request" => {
                if let Some(request_id) = message.get("request_id").and_then(|r| r.as_str()) {
                    let tool_use_id = {
                        let pending = self
                            .pending
                            .lock()
                            .expect("pending_permission_requests mutex poisoned");
                        pending
                            .get(request_id)
                            .and_then(|r| r.request.tool_use_id.clone())
                    };
                    self.pending
                        .lock()
                        .expect("pending_permission_requests mutex poisoned")
                        .remove(request_id);
                    self.callbacks
                        .on_permission_cancelled(request_id, tool_use_id.as_deref());
                }
            }
            "control_response" => {
                tracing::debug!(session_id = %self.session_id, "RemoteSessionManager: received control_response");
            }
            _ => {
                // Forward SDK messages
                self.callbacks.on_message(message);
            }
        }
    }

    fn on_connected(&self) {
        self.callbacks.on_connected();
    }

    fn on_close(&self) {
        self.callbacks.on_disconnected();
    }

    fn on_reconnecting(&self) {
        self.callbacks.on_reconnecting();
    }

    fn on_error(&self, error: &str) {
        self.callbacks.on_error(error);
    }
}

impl WebSocketCallbackAdapter {
    fn handle_control_request(&self, message: &serde_json::Value) {
        let request_id = match message.get("request_id").and_then(|r| r.as_str()) {
            Some(id) => id.to_string(),
            None => return,
        };

        let request_inner = match message.get("request") {
            Some(r) => r,
            None => return,
        };

        let subtype = request_inner
            .get("subtype")
            .and_then(|s| s.as_str())
            .unwrap_or("");

        if subtype == "can_use_tool" {
            let control_request = ControlRequest {
                request_id: request_id.clone(),
                request: ControlRequestInner {
                    subtype: subtype.to_string(),
                    tool_name: request_inner
                        .get("tool_name")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string()),
                    tool_use_id: request_inner
                        .get("tool_use_id")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string()),
                    input: request_inner.get("input").cloned(),
                    extra: request_inner.clone(),
                },
            };

            self.pending
                .lock()
                .expect("pending_permission_requests mutex poisoned")
                .insert(request_id, control_request.clone());
            self.callbacks.on_permission_request(control_request);
        } else {
            tracing::debug!("RemoteSessionManager: unsupported control request subtype: {subtype}");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCallbacks {
        messages: Mutex<Vec<serde_json::Value>>,
    }

    impl TestCallbacks {
        fn new() -> Self {
            Self {
                messages: Mutex::new(Vec::new()),
            }
        }
    }

    impl RemoteSessionCallbacks for TestCallbacks {
        fn on_message(&self, message: serde_json::Value) {
            self.messages
                .lock()
                .expect("test mutex poisoned")
                .push(message);
        }
        fn on_permission_request(&self, _: ControlRequest) {}
        fn on_permission_cancelled(&self, _: &str, _: Option<&str>) {}
        fn on_connected(&self) {}
        fn on_disconnected(&self) {}
        fn on_reconnecting(&self) {}
        fn on_error(&self, _: &str) {}
    }

    #[test]
    fn test_remote_session_manager_creation() {
        let config = RemoteSessionConfig {
            session_id: "test-123".to_string(),
            access_token: "tok".to_string(),
            org_uuid: "org-1".to_string(),
            has_initial_prompt: false,
            viewer_only: false,
            api_base_url: "https://api.anthropic.com".to_string(),
        };
        let callbacks = Arc::new(TestCallbacks::new());
        let mgr = RemoteSessionManager::new(config, callbacks);
        assert_eq!(mgr.session_id(), "test-123");
        assert!(!mgr.is_connected());
    }
}
