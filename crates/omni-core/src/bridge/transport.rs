//! Transport abstraction for the bridge.
//!
//! Covers the surface that the bridge uses for bidirectional message routing:
//! - v1: WebSocket-based (HybridTransport) -- WS reads + POST writes
//! - v2: SSE reads + CCRClient HTTP POST writes
//!
//! This module provides a unified [`BridgeTransport`] trait and concrete
//! implementations for in-process and stdio transports, plus a transport
//! builder for constructing the appropriate transport based on configuration.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

/// Session state reported via the transport's `report_state` method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// Session is idle, waiting for input.
    Idle,
    /// Session is actively processing.
    Running,
    /// Session needs user action (permission prompt).
    RequiresAction,
}

/// Transport abstraction for the bridge.
///
/// Both v1 (WebSocket) and v2 (SSE + CCRClient) paths implement this trait
/// so the bridge logic can be transport-agnostic.
#[async_trait]
pub trait BridgeTransport: Send + Sync {
    /// Write a single message through the transport.
    async fn write(&self, message: &Value) -> Result<()>;

    /// Write a batch of messages through the transport.
    ///
    /// The default implementation writes messages sequentially. Implementations
    /// may override for batch optimization.
    async fn write_batch(&self, messages: &[Value]) -> Result<()> {
        for msg in messages {
            self.write(msg).await?;
        }
        Ok(())
    }

    /// Close the transport.
    fn close(&self);

    /// Check if the transport is in a connected state.
    fn is_connected(&self) -> bool;

    /// Get a human-readable label for the current transport state.
    fn state_label(&self) -> &str;

    /// Get the high-water mark of the underlying read stream's event
    /// sequence numbers. Used to resume from where the old transport left
    /// off when swapping transports.
    ///
    /// Returns 0 for v1 (Session-Ingress WS doesn't use SSE sequence numbers).
    fn last_sequence_num(&self) -> u64 {
        0
    }

    /// Count of batches dropped due to max consecutive failures.
    fn dropped_batch_count(&self) -> u64 {
        0
    }

    /// Report the session state (v2 only; v1 is a no-op).
    fn report_state(&self, _state: SessionState) {}

    /// Report external metadata (v2 only; v1 is a no-op).
    fn report_metadata(&self, _metadata: &Value) {}

    /// Report delivery status for an event (v2 only; v1 is a no-op).
    fn report_delivery(&self, _event_id: &str, _status: DeliveryStatus) {}

    /// Drain the write queue before close (v2 only; v1 resolves immediately).
    async fn flush(&self) -> Result<()> {
        Ok(())
    }
}

/// Delivery status for event tracking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryStatus {
    /// Event received by the transport.
    Received,
    /// Event is being processed.
    Processing,
    /// Event has been fully processed.
    Processed,
}

/// Callback types for transport events.
pub type OnDataFn = Arc<dyn Fn(String) + Send + Sync>;
pub type OnCloseFn = Arc<dyn Fn(Option<u16>) + Send + Sync>;
pub type OnConnectFn = Arc<dyn Fn() + Send + Sync>;

/// Configuration for creating a transport.
#[derive(Clone, Debug)]
pub struct TransportConfig {
    /// Session ID for this transport.
    pub session_id: String,
    /// API base URL or session URL.
    pub url: String,
    /// Authentication token.
    pub auth_token: String,
    /// Whether this is a v2 (CCR) transport.
    pub use_v2: bool,
    /// Initial SSE sequence number for resumption.
    pub initial_sequence_num: u64,
    /// Worker epoch (v2 only).
    pub worker_epoch: Option<i64>,
    /// Whether to skip the SSE read stream (outbound-only mode).
    pub outbound_only: bool,
}

/// In-process transport for testing and local development.
///
/// Routes messages through in-memory channels without any network IO.
pub struct InProcessTransport {
    outbound_tx: mpsc::UnboundedSender<Value>,
    connected: std::sync::atomic::AtomicBool,
}

impl InProcessTransport {
    /// Create a new in-process transport pair.
    ///
    /// Returns the transport and a receiver for outbound messages.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                outbound_tx: tx,
                connected: std::sync::atomic::AtomicBool::new(true),
            },
            rx,
        )
    }

    /// Mark this transport as disconnected.
    pub fn disconnect(&self) {
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait]
impl BridgeTransport for InProcessTransport {
    async fn write(&self, message: &Value) -> Result<()> {
        if !self.is_connected() {
            anyhow::bail!("transport is disconnected");
        }
        self.outbound_tx
            .send(message.clone())
            .map_err(|_| anyhow::anyhow!("transport channel closed"))
    }

    fn close(&self) {
        self.disconnect();
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn state_label(&self) -> &str {
        if self.is_connected() {
            "connected"
        } else {
            "closed"
        }
    }
}

/// Stdio transport for subprocess communication.
///
/// Routes messages as NDJSON over stdin/stdout of a child process.
pub struct StdioTransport {
    stdin_tx: mpsc::UnboundedSender<String>,
    connected: std::sync::atomic::AtomicBool,
}

impl StdioTransport {
    /// Create a stdio transport that writes NDJSON to the given sender.
    ///
    /// The caller is responsible for forwarding lines from the sender to
    /// the child process's stdin.
    pub fn new(stdin_tx: mpsc::UnboundedSender<String>) -> Self {
        Self {
            stdin_tx,
            connected: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Mark this transport as disconnected.
    pub fn disconnect(&self) {
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait]
impl BridgeTransport for StdioTransport {
    async fn write(&self, message: &Value) -> Result<()> {
        if !self.is_connected() {
            anyhow::bail!("transport is disconnected");
        }
        let line = serde_json::to_string(message)
            .map_err(|e| anyhow::anyhow!("failed to serialize message: {e}"))?;
        self.stdin_tx
            .send(format!("{line}\n"))
            .map_err(|_| anyhow::anyhow!("stdin channel closed"))
    }

    fn close(&self) {
        self.disconnect();
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn state_label(&self) -> &str {
        if self.is_connected() {
            "connected"
        } else {
            "closed"
        }
    }
}

/// Message received from a transport's inbound stream.
#[derive(Clone, Debug)]
pub enum TransportEvent {
    /// A data message was received.
    Data(String),
    /// The transport connection was established.
    Connected,
    /// The transport was closed.
    Closed(Option<u16>),
}

/// Builder for constructing the appropriate transport based on configuration.
pub struct TransportBuilder {
    config: TransportConfig,
}

impl TransportBuilder {
    /// Create a new transport builder with the given configuration.
    pub fn new(config: TransportConfig) -> Self {
        Self { config }
    }

    /// Build an in-process transport (for testing).
    pub fn build_in_process(&self) -> (InProcessTransport, mpsc::UnboundedReceiver<Value>) {
        InProcessTransport::new()
    }

    /// Build a stdio transport for subprocess communication.
    pub fn build_stdio(&self) -> (StdioTransport, mpsc::UnboundedSender<String>) {
        let (tx, _rx) = mpsc::unbounded_channel();
        let transport = StdioTransport::new(tx.clone());
        (transport, tx)
    }

    /// Get the transport configuration.
    pub fn config(&self) -> &TransportConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_in_process_transport_write() {
        let (transport, mut rx) = InProcessTransport::new();
        let msg = json!({"type": "user", "content": "hello"});

        transport.write(&msg).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received, msg);
    }

    #[tokio::test]
    async fn test_in_process_transport_write_batch() {
        let (transport, mut rx) = InProcessTransport::new();
        let msgs = vec![
            json!({"type": "user", "content": "hello"}),
            json!({"type": "assistant", "content": "hi"}),
        ];

        transport.write_batch(&msgs).await.unwrap();
        let m1 = rx.recv().await.unwrap();
        let m2 = rx.recv().await.unwrap();
        assert_eq!(m1, msgs[0]);
        assert_eq!(m2, msgs[1]);
    }

    #[tokio::test]
    async fn test_in_process_transport_disconnect() {
        let (transport, _rx) = InProcessTransport::new();
        assert!(transport.is_connected());
        assert_eq!(transport.state_label(), "connected");

        transport.close();
        assert!(!transport.is_connected());
        assert_eq!(transport.state_label(), "closed");

        let result = transport.write(&json!({"type": "test"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stdio_transport_write() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let transport = StdioTransport::new(tx);
        let msg = json!({"type": "user", "content": "hello"});

        transport.write(&msg).await.unwrap();
        let line = rx.recv().await.unwrap();
        assert!(line.contains("\"type\":\"user\""));
        assert!(line.ends_with('\n'));
    }

    #[tokio::test]
    async fn test_stdio_transport_disconnect() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let transport = StdioTransport::new(tx);

        transport.close();
        assert!(!transport.is_connected());

        let result = transport.write(&json!({"type": "test"})).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_transport_config() {
        let config = TransportConfig {
            session_id: "session_abc".to_string(),
            url: "https://api.example.com".to_string(),
            auth_token: "token".to_string(),
            use_v2: false,
            initial_sequence_num: 0,
            worker_epoch: None,
            outbound_only: false,
        };
        assert_eq!(config.session_id, "session_abc");
        assert!(!config.use_v2);
    }

    #[test]
    fn test_transport_builder() {
        let config = TransportConfig {
            session_id: "session_abc".to_string(),
            url: "https://api.example.com".to_string(),
            auth_token: "token".to_string(),
            use_v2: false,
            initial_sequence_num: 0,
            worker_epoch: None,
            outbound_only: false,
        };
        let builder = TransportBuilder::new(config);
        assert_eq!(builder.config().session_id, "session_abc");
    }

    #[test]
    fn test_delivery_status() {
        assert_ne!(DeliveryStatus::Received, DeliveryStatus::Processing);
        assert_ne!(DeliveryStatus::Processing, DeliveryStatus::Processed);
    }

    #[test]
    fn test_session_state() {
        assert_ne!(SessionState::Idle, SessionState::Running);
        assert_ne!(SessionState::Running, SessionState::RequiresAction);
    }
}
