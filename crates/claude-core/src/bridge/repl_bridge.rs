//! REPL bridge abstraction.
//!
//! Provides the in-process bridge for the REPL mode, where the bridge runs
//! within the same process as the Claude Code REPL. Handles:
//! - Bridge initialization and handle management
//! - Bidirectional message routing between the REPL and the backend
//! - Session lifecycle (create, archive, reconnect)
//! - Message flushing (sending conversation history to the server)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::messaging::{is_eligible_bridge_message, BoundedUuidSet};
use super::types::BridgeState;

/// Bridge connection state for the REPL bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplBridgeState {
    /// Not connected to the backend.
    Disconnected,
    /// Connecting to the backend.
    Connecting,
    /// Connected and forwarding messages.
    Connected,
    /// Connection failed.
    Failed,
    /// Shutting down.
    ShuttingDown,
}

impl From<ReplBridgeState> for BridgeState {
    fn from(state: ReplBridgeState) -> Self {
        match state {
            ReplBridgeState::Disconnected => BridgeState::Disconnected,
            ReplBridgeState::Connecting => BridgeState::Connecting,
            ReplBridgeState::Connected => BridgeState::Connected,
            ReplBridgeState::Failed => BridgeState::Disconnected,
            ReplBridgeState::ShuttingDown => BridgeState::ShuttingDown,
        }
    }
}

/// Callbacks for REPL bridge events.
pub trait ReplBridgeCallbacks: Send + Sync {
    /// Called when an inbound user message arrives from the server.
    fn on_inbound_message(&self, msg: &Value);

    /// Called when a permission response arrives from the server.
    fn on_permission_response(&self, response: &Value);

    /// Called when an interrupt signal arrives from the server.
    fn on_interrupt(&self);

    /// Called when a set_model control request arrives.
    fn on_set_model(&self, model: Option<&str>);

    /// Called when a set_max_thinking_tokens control request arrives.
    fn on_set_max_thinking_tokens(&self, max_tokens: Option<i64>);

    /// Called when a set_permission_mode control request arrives.
    fn on_set_permission_mode(&self, mode: &str) -> std::result::Result<(), String>;

    /// Called on bridge state transitions.
    fn on_state_change(&self, state: ReplBridgeState, detail: Option<&str>);
}

/// Handle to a running REPL bridge.
///
/// Provides methods for the REPL to interact with the bridge:
/// writing SDK messages, managing the lifecycle, and querying state.
pub struct ReplBridgeHandle {
    /// The bridge session ID on the server.
    pub bridge_session_id: String,
    /// The environment ID (if env-based bridge).
    pub environment_id: Option<String>,
    /// Whether the bridge is currently connected.
    connected: Arc<AtomicBool>,
    /// Cancellation token for shutdown.
    cancel: CancellationToken,
    /// Outbound message queue.
    outbound_tx: tokio::sync::mpsc::UnboundedSender<Value>,
    /// Echo deduplication set.
    echo_uuids: Arc<Mutex<BoundedUuidSet>>,
    /// Previously flushed UUIDs (to avoid duplicates across reconnects).
    flushed_uuids: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Whether the initial history has been flushed.
    initial_flush_done: Arc<AtomicBool>,
}

impl ReplBridgeHandle {
    /// Create a new REPL bridge handle.
    pub fn new(
        bridge_session_id: String,
        environment_id: Option<String>,
        cancel: CancellationToken,
        outbound_tx: tokio::sync::mpsc::UnboundedSender<Value>,
    ) -> Self {
        Self {
            bridge_session_id,
            environment_id,
            connected: Arc::new(AtomicBool::new(false)),
            cancel,
            outbound_tx,
            echo_uuids: Arc::new(Mutex::new(BoundedUuidSet::new(1000))),
            flushed_uuids: Arc::new(Mutex::new(std::collections::HashSet::new())),
            initial_flush_done: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if the bridge is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Set the connected state.
    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::SeqCst);
    }

    /// Write SDK messages to the outbound queue for forwarding to the server.
    ///
    /// Filters messages through eligibility checks and deduplication before
    /// queuing them for transmission.
    pub fn write_sdk_messages(&self, messages: &[Value]) -> Result<()> {
        for msg in messages {
            if !is_eligible_bridge_message(msg) {
                continue;
            }

            // Track the UUID for echo deduplication
            if let Some(uuid) = msg.get("uuid").and_then(|u| u.as_str()) {
                let mut echo_set = self.echo_uuids.lock().unwrap();
                echo_set.add(uuid.to_string());

                // Track in flushed UUIDs
                let mut flushed = self.flushed_uuids.lock().unwrap();
                flushed.insert(uuid.to_string());
            }

            self.outbound_tx
                .send(msg.clone())
                .map_err(|_| anyhow::anyhow!("outbound channel closed"))?;
        }
        Ok(())
    }

    /// Check if a message UUID is an echo (was sent by us).
    pub fn is_echo(&self, uuid: &str) -> bool {
        let echo_set = self.echo_uuids.lock().unwrap();
        echo_set.contains(uuid)
    }

    /// Check if a UUID was previously flushed.
    pub fn was_flushed(&self, uuid: &str) -> bool {
        let flushed = self.flushed_uuids.lock().unwrap();
        flushed.contains(uuid)
    }

    /// Mark the initial history flush as complete.
    pub fn mark_initial_flush_done(&self) {
        self.initial_flush_done.store(true, Ordering::SeqCst);
    }

    /// Check if the initial history flush has completed.
    pub fn is_initial_flush_done(&self) -> bool {
        self.initial_flush_done.load(Ordering::SeqCst)
    }

    /// Tear down the bridge (cancel the event loop).
    pub fn teardown(&self) {
        self.cancel.cancel();
    }

    /// Clear the echo deduplication set (used on reconnect).
    pub fn clear_echo_set(&self) {
        let mut echo_set = self.echo_uuids.lock().unwrap();
        echo_set.clear();
    }
}

/// Global pointer to the active REPL bridge handle.
///
/// Set when init completes; cleared on teardown. Allows callers outside the
/// bridge lifecycle (tools, slash commands) to invoke handle methods.
static GLOBAL_HANDLE: Mutex<Option<Arc<ReplBridgeHandle>>> = Mutex::new(None);

/// Set the global REPL bridge handle.
pub fn set_repl_bridge_handle(handle: Option<Arc<ReplBridgeHandle>>) {
    let mut global = GLOBAL_HANDLE.lock().unwrap();
    *global = handle;
}

/// Get a reference to the global REPL bridge handle.
pub fn get_repl_bridge_handle() -> Option<Arc<ReplBridgeHandle>> {
    let global = GLOBAL_HANDLE.lock().unwrap();
    global.clone()
}

/// Get this bridge's session ID in the compat format, if connected.
pub fn get_self_bridge_compat_id() -> Option<String> {
    let handle = get_repl_bridge_handle()?;
    Some(super::work_secret::to_compat_session_id(
        &handle.bridge_session_id,
    ))
}

/// Parameters for initializing the REPL bridge core.
pub struct BridgeCoreParams {
    /// Working directory.
    pub dir: String,
    /// Machine name for display.
    pub machine_name: String,
    /// Git branch.
    pub branch: String,
    /// Git remote URL.
    pub git_repo_url: Option<String>,
    /// Session title.
    pub title: String,
    /// API base URL.
    pub base_url: String,
    /// Session ingress URL.
    pub session_ingress_url: String,
    /// Worker type identifier.
    pub worker_type: String,
    /// Whether to keep the bridge alive across session boundaries.
    pub perpetual: bool,
}

/// Flush conversation history to the server.
///
/// Converts the given messages to SDK format and sends them through the
/// bridge transport. Skips messages whose UUIDs were already flushed
/// (to prevent duplicate messages after reconnects).
pub fn prepare_flush_messages(
    messages: &[Value],
    previously_flushed: &std::collections::HashSet<String>,
) -> Vec<Value> {
    messages
        .iter()
        .filter(|msg| {
            // Only flush eligible messages
            if !is_eligible_bridge_message(msg) {
                return false;
            }
            // Skip messages already flushed in a prior session
            if let Some(uuid) = msg.get("uuid").and_then(|u| u.as_str()) {
                if previously_flushed.contains(uuid) {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// Cap the number of messages to flush to avoid overwhelming the server.
pub fn cap_flush_messages(messages: Vec<Value>, cap: usize) -> Vec<Value> {
    let len = messages.len();
    if len <= cap {
        return messages;
    }
    // Keep the last `cap` messages
    messages.into_iter().skip(len - cap).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_repl_bridge_state_conversion() {
        assert_eq!(
            BridgeState::from(ReplBridgeState::Connected),
            BridgeState::Connected
        );
        assert_eq!(
            BridgeState::from(ReplBridgeState::Disconnected),
            BridgeState::Disconnected
        );
        assert_eq!(
            BridgeState::from(ReplBridgeState::Connecting),
            BridgeState::Connecting
        );
        assert_eq!(
            BridgeState::from(ReplBridgeState::ShuttingDown),
            BridgeState::ShuttingDown
        );
        assert_eq!(
            BridgeState::from(ReplBridgeState::Failed),
            BridgeState::Disconnected
        );
    }

    #[test]
    fn test_prepare_flush_messages() {
        let messages = vec![
            json!({"type": "user", "uuid": "u1", "message": {"content": "hello"}}),
            json!({"type": "assistant", "uuid": "u2", "message": {"content": "hi"}}),
            json!({"type": "system", "subtype": "informational", "uuid": "u3"}),
        ];
        let mut flushed = std::collections::HashSet::new();
        flushed.insert("u1".to_string());

        let result = prepare_flush_messages(&messages, &flushed);
        // u1 was already flushed, system/informational is not eligible
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["uuid"], "u2");
    }

    #[test]
    fn test_cap_flush_messages() {
        let messages: Vec<Value> = (0..10)
            .map(|i| json!({"type": "user", "index": i}))
            .collect();
        let capped = cap_flush_messages(messages.clone(), 5);
        assert_eq!(capped.len(), 5);
        assert_eq!(capped[0]["index"], 5);
        assert_eq!(capped[4]["index"], 9);
    }

    #[test]
    fn test_cap_flush_messages_under_cap() {
        let messages: Vec<Value> = (0..3)
            .map(|i| json!({"type": "user", "index": i}))
            .collect();
        let capped = cap_flush_messages(messages.clone(), 5);
        assert_eq!(capped.len(), 3);
    }

    #[test]
    fn test_repl_bridge_handle_echo_dedup() {
        let cancel = CancellationToken::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = ReplBridgeHandle::new(
            "session_123".to_string(),
            Some("env_456".to_string()),
            cancel,
            tx,
        );

        // Write a message with a UUID
        let msg = json!({"type": "user", "uuid": "test-uuid-1234"});
        handle.write_sdk_messages(&[msg]).unwrap();

        // Should be detected as echo
        assert!(handle.is_echo("test-uuid-1234"));
        assert!(!handle.is_echo("other-uuid"));
    }

    #[test]
    fn test_repl_bridge_handle_lifecycle() {
        let cancel = CancellationToken::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = ReplBridgeHandle::new(
            "session_123".to_string(),
            None,
            cancel.clone(),
            tx,
        );

        assert!(!handle.is_connected());
        handle.set_connected(true);
        assert!(handle.is_connected());

        assert!(!handle.is_initial_flush_done());
        handle.mark_initial_flush_done();
        assert!(handle.is_initial_flush_done());

        handle.teardown();
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn test_global_handle() {
        // Clear any existing global handle
        set_repl_bridge_handle(None);
        assert!(get_repl_bridge_handle().is_none());
        assert!(get_self_bridge_compat_id().is_none());

        let cancel = CancellationToken::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = Arc::new(ReplBridgeHandle::new(
            "cse_abc123".to_string(),
            None,
            cancel,
            tx,
        ));
        set_repl_bridge_handle(Some(handle));

        assert!(get_repl_bridge_handle().is_some());
        assert_eq!(
            get_self_bridge_compat_id(),
            Some("session_abc123".to_string())
        );

        // Clean up
        set_repl_bridge_handle(None);
    }
}
