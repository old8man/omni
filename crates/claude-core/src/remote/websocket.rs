//! WebSocket client for remote session subscriptions.
//!
//! Connects to `wss://.../v1/sessions/ws/{id}/subscribe` to receive
//! real-time SDK messages from a CCR session. Supports automatic
//! reconnection with bounded retries and keepalive pings.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;

use super::types::{
    MAX_RECONNECT_ATTEMPTS, MAX_SESSION_NOT_FOUND_RETRIES, PERMANENT_CLOSE_CODES, PING_INTERVAL_MS,
    RECONNECT_DELAY_MS,
};

/// Callbacks for WebSocket lifecycle events.
pub trait WebSocketCallbacks: Send + Sync {
    /// Called when an SDK message is received.
    fn on_message(&self, message: Value);
    /// Called when the connection is established.
    fn on_connected(&self);
    /// Called when the connection is permanently closed.
    fn on_close(&self);
    /// Called when a transient close triggers a reconnect attempt.
    fn on_reconnecting(&self);
    /// Called on error.
    fn on_error(&self, error: &str);
}

/// WebSocket client for CCR session subscriptions.
///
/// Protocol:
/// 1. Connect to `wss://api.anthropic.com/v1/sessions/ws/{sessionId}/subscribe?organization_uuid=...`
/// 2. Authenticate via Authorization header
/// 3. Receive SDK message stream
pub struct SessionsWebSocket {
    session_id: String,
    org_uuid: String,
    access_token: String,
    api_base_url: String,
    callbacks: Arc<dyn WebSocketCallbacks>,
    connected: Arc<AtomicBool>,
    send_tx: Option<mpsc::UnboundedSender<Value>>,
    shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
}

impl SessionsWebSocket {
    /// Create a new sessions WebSocket client.
    pub fn new(
        session_id: String,
        org_uuid: String,
        access_token: String,
        api_base_url: String,
        callbacks: Arc<dyn WebSocketCallbacks>,
    ) -> Self {
        Self {
            session_id,
            org_uuid,
            access_token,
            api_base_url,
            callbacks,
            connected: Arc::new(AtomicBool::new(false)),
            send_tx: None,
            shutdown_tx: None,
        }
    }

    /// Connect to the sessions WebSocket endpoint.
    pub async fn connect(&mut self) -> Result<()> {
        let ws_base = self.api_base_url.replace("https://", "wss://");
        let url = format!(
            "{ws_base}/v1/sessions/ws/{}/subscribe?organization_uuid={}",
            self.session_id, self.org_uuid
        );

        tracing::debug!("SessionsWebSocket: connecting to {url}");

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let (send_tx, send_rx) = mpsc::unbounded_channel::<Value>();

        self.shutdown_tx = Some(shutdown_tx);
        self.send_tx = Some(send_tx);

        let connected = Arc::clone(&self.connected);
        let callbacks = Arc::clone(&self.callbacks);
        let access_token = self.access_token.clone();

        let params = WebSocketLoopParams {
            url,
            access_token,
            connected,
            callbacks,
        };
        tokio::spawn(async move {
            run_websocket_loop(params, send_rx, shutdown_rx).await;
        });

        Ok(())
    }

    /// Send a JSON message through the WebSocket.
    pub fn send_json(&self, value: &Value) {
        if let Some(tx) = &self.send_tx {
            if tx.send(value.clone()).is_err() {
                tracing::warn!("SessionsWebSocket: send channel closed");
            }
        }
    }

    /// Check if the WebSocket is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Close the WebSocket connection.
    pub fn close(self) {
        if let Some(tx) = self.shutdown_tx {
            let _ = tx.send(true);
        }
    }
}

/// Parameters for the WebSocket event loop.
struct WebSocketLoopParams {
    url: String,
    access_token: String,
    connected: Arc<AtomicBool>,
    callbacks: Arc<dyn WebSocketCallbacks>,
}

/// Core WebSocket event loop with reconnection logic.
async fn run_websocket_loop(
    params: WebSocketLoopParams,
    mut send_rx: mpsc::UnboundedReceiver<Value>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let WebSocketLoopParams {
        url,
        access_token,
        connected,
        callbacks,
    } = params;
    let mut reconnect_attempts: u32 = 0;
    #[allow(unused_assignments)]
    let mut session_not_found_retries: u32 = 0;

    loop {
        // Check for shutdown
        if *shutdown_rx.borrow() {
            connected.store(false, Ordering::Relaxed);
            return;
        }

        // Attempt connection
        let connect_result = attempt_connection(&url, &access_token).await;

        let (mut ws_write, mut ws_read) = match connect_result {
            Ok(stream) => {
                tracing::debug!("SessionsWebSocket: connected, authenticated via headers");
                connected.store(true, Ordering::Relaxed);
                reconnect_attempts = 0;
                session_not_found_retries = 0;
                callbacks.on_connected();
                stream.split()
            }
            Err(e) => {
                callbacks.on_error(&format!("WebSocket connection failed: {e}"));
                if reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
                    callbacks.on_close();
                    return;
                }
                reconnect_attempts += 1;
                callbacks.on_reconnecting();
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(RECONNECT_DELAY_MS)) => {}
                    _ = shutdown_rx.changed() => return,
                }
                continue;
            }
        };

        let ping_interval = Duration::from_millis(PING_INTERVAL_MS);
        let mut ping_timer = tokio::time::interval(ping_interval);
        ping_timer.tick().await; // consume first immediate tick

        let mut close_code: Option<u16> = None;

        // Message processing loop
        loop {
            tokio::select! {
                // Incoming messages from server
                msg = ws_read.next() => {
                    match msg {
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                            match serde_json::from_str::<Value>(&text) {
                                Ok(value) => {
                                    if value.is_object() && value.get("type").and_then(|t| t.as_str()).is_some() {
                                        callbacks.on_message(value);
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!("SessionsWebSocket: failed to parse message: {e}");
                                }
                            }
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => {
                            close_code = frame.as_ref().map(|f| f.code.into());
                            tracing::debug!("SessionsWebSocket: close frame received, code={close_code:?}");
                            break;
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(_))) => {
                            tracing::trace!("SessionsWebSocket: pong received");
                        }
                        Some(Ok(_)) => {} // Binary, Frame — ignore
                        Some(Err(e)) => {
                            callbacks.on_error(&format!("WebSocket read error: {e}"));
                            break;
                        }
                        None => {
                            tracing::debug!("SessionsWebSocket: stream ended");
                            break;
                        }
                    }
                }

                // Outgoing messages from application
                Some(value) = send_rx.recv() => {
                    let text = serde_json::to_string(&value).unwrap_or_default();
                    if let Err(e) = ws_write.send(tokio_tungstenite::tungstenite::Message::Text(text)).await {
                        callbacks.on_error(&format!("WebSocket send error: {e}"));
                        break;
                    }
                }

                // Periodic ping
                _ = ping_timer.tick() => {
                    if let Err(e) = ws_write.send(tokio_tungstenite::tungstenite::Message::Ping(vec![])).await {
                        tracing::debug!("SessionsWebSocket: ping failed: {e}");
                        break;
                    }
                }

                // Shutdown signal
                _ = shutdown_rx.changed() => {
                    let _ = ws_write.send(tokio_tungstenite::tungstenite::Message::Close(None)).await;
                    connected.store(false, Ordering::Relaxed);
                    return;
                }
            }
        }

        // Connection lost — decide whether to reconnect
        connected.store(false, Ordering::Relaxed);

        let code = close_code.unwrap_or(0);

        // Permanent close codes
        if PERMANENT_CLOSE_CODES.contains(&code) {
            tracing::debug!("SessionsWebSocket: permanent close code {code}, not reconnecting");
            callbacks.on_close();
            return;
        }

        // Session not found (4001) — limited retries for compaction transients
        if code == 4001 {
            session_not_found_retries += 1;
            if session_not_found_retries > MAX_SESSION_NOT_FOUND_RETRIES {
                tracing::debug!(
                    "SessionsWebSocket: 4001 retry budget exhausted ({}), not reconnecting",
                    MAX_SESSION_NOT_FOUND_RETRIES
                );
                callbacks.on_close();
                return;
            }
            let delay = RECONNECT_DELAY_MS * u64::from(session_not_found_retries);
            callbacks.on_reconnecting();
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                _ = shutdown_rx.changed() => return,
            }
            continue;
        }

        // General reconnect
        if reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
            tracing::debug!("SessionsWebSocket: reconnect attempts exhausted, closing");
            callbacks.on_close();
            return;
        }
        reconnect_attempts += 1;
        callbacks.on_reconnecting();
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(RECONNECT_DELAY_MS)) => {}
            _ = shutdown_rx.changed() => return,
        }
    }
}

/// Attempt a single WebSocket connection.
async fn attempt_connection(
    url: &str,
    access_token: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    use tokio_tungstenite::tungstenite::http::Request;

    let request = Request::builder()
        .uri(url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("anthropic-version", "2023-06-01")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Host", extract_host(url))
        .body(())
        .context("failed to build WebSocket request")?;

    let (stream, _response) = tokio_tungstenite::connect_async(request)
        .await
        .context("WebSocket connection failed")?;

    Ok(stream)
}

/// Extract the host from a URL string.
fn extract_host(url: &str) -> String {
    url.replace("wss://", "")
        .replace("ws://", "")
        .split('/')
        .next()
        .unwrap_or("api.anthropic.com")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_host() {
        assert_eq!(
            extract_host("wss://api.anthropic.com/v1/sessions/ws/123/subscribe"),
            "api.anthropic.com"
        );
        assert_eq!(
            extract_host("ws://localhost:8080/v1/test"),
            "localhost:8080"
        );
    }
}
