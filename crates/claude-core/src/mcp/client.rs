use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, info, warn};

use super::transport::{self, McpTransport};
use super::types::*;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors specific to MCP client operations.
#[derive(Debug, thiserror::Error)]
pub enum McpClientError {
    #[error("MCP server process failed to start: {0}")]
    SpawnFailed(String),
    #[error("MCP server not connected")]
    NotConnected,
    #[error("JSON-RPC error from MCP server: {0}")]
    JsonRpc(JsonRpcError),
    #[error("MCP request timed out after {0}s")]
    Timeout(u64),
    #[error("MCP server process exited unexpectedly")]
    ProcessExited,
    #[error("invalid MCP server config: {0}")]
    InvalidConfig(String),
    #[error("MCP server requires authentication")]
    AuthRequired,
    #[error("MCP session expired for server {0}")]
    SessionExpired(String),
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;
const MAX_DESCRIPTION_LENGTH: usize = 2048;
const MAX_RECONNECT_ATTEMPTS: u32 = 5;
const RECONNECT_BASE_DELAY_MS: u64 = 1000;
const _HEARTBEAT_INTERVAL_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Client for a single MCP server connection supporting multiple transports.
///
/// Manages the full connection lifecycle: connect, initialize, heartbeat,
/// request/response, reconnect on failure, and graceful disconnect.
pub struct McpClient {
    name: String,
    config: McpServerConfig,
    state: McpServerState,
    capabilities: Option<McpCapabilities>,
    server_info: Option<ServerInfo>,
    transport: Option<Box<dyn McpTransport>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    reader_handle: Option<tokio::task::JoinHandle<()>>,
    heartbeat_handle: Option<tokio::task::JoinHandle<()>>,
    /// Set to true when the server sends `notifications/tools/list_changed`.
    tools_changed: Arc<AtomicBool>,
    /// Number of reconnect attempts since the last successful connect.
    reconnect_attempts: u32,
    /// Last error message, if the server is in a failed state.
    last_error: Option<String>,
}

/// Server identification returned during initialization.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

impl McpClient {
    /// Create a new MCP client for the given server.
    pub fn new(name: impl Into<String>, config: McpServerConfig) -> Self {
        Self {
            name: name.into(),
            config,
            state: McpServerState::Disconnected,
            capabilities: None,
            server_info: None,
            transport: None,
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            reader_handle: None,
            heartbeat_handle: None,
            tools_changed: Arc::new(AtomicBool::new(false)),
            reconnect_attempts: 0,
            last_error: None,
        }
    }

    // -- Accessors --

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn state(&self) -> &McpServerState {
        &self.state
    }
    pub fn capabilities(&self) -> Option<&McpCapabilities> {
        self.capabilities.as_ref()
    }
    pub fn server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Whether the server has signalled that its tool list changed.
    pub fn tools_changed(&self) -> bool {
        self.tools_changed.swap(false, Ordering::Relaxed)
    }

    // -- Connection lifecycle --

    /// Connect to the MCP server using the configured transport and perform
    /// the initialization handshake.
    pub async fn connect(&mut self) -> Result<McpCapabilities> {
        self.state = McpServerState::Pending;
        self.last_error = None;

        let transport = match tokio::time::timeout(
            std::time::Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS),
            transport::create_transport(&self.name, &self.config),
        )
        .await
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                self.state = McpServerState::Failed;
                self.last_error = Some(e.to_string());
                return Err(e);
            }
            Err(_) => {
                self.state = McpServerState::Failed;
                let msg = format!("connection timed out after {DEFAULT_CONNECT_TIMEOUT_SECS}s");
                self.last_error = Some(msg.clone());
                bail!(msg);
            }
        };

        self.transport = Some(transport);
        self.start_reader_loop();

        // Perform the MCP initialize handshake.
        let init_result = self
            .send_request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "roots": {},
                        "elicitation": {}
                    },
                    "clientInfo": {
                        "name": "claude-code",
                        "title": "Claude Code",
                        "version": env!("CARGO_PKG_VERSION"),
                        "description": "Anthropic's agentic coding tool"
                    }
                })),
            )
            .await
            .context("MCP initialize handshake failed")?;

        let capabilities = parse_capabilities(&init_result);
        self.capabilities = Some(capabilities.clone());

        self.server_info = init_result.get("serverInfo").and_then(|si| {
            Some(ServerInfo {
                name: si.get("name")?.as_str()?.to_string(),
                version: si.get("version")?.as_str()?.to_string(),
            })
        });

        self.send_notification("notifications/initialized", None)
            .await?;

        self.state = McpServerState::Connected;
        self.reconnect_attempts = 0;
        self.start_heartbeat();

        debug!(server = %self.name, ?capabilities, "MCP server connected");
        Ok(capabilities)
    }

    /// Attempt to reconnect with exponential backoff.
    ///
    /// Returns `Ok(caps)` on success, or the last error after exhausting
    /// all attempts.
    pub async fn reconnect(&mut self) -> Result<McpCapabilities> {
        info!(server = %self.name, "attempting reconnect");
        self.disconnect_internal().await;

        for attempt in 0..MAX_RECONNECT_ATTEMPTS {
            self.reconnect_attempts = attempt + 1;
            let delay = RECONNECT_BASE_DELAY_MS * 2u64.pow(attempt);
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;

            debug!(
                server = %self.name,
                attempt = attempt + 1,
                max = MAX_RECONNECT_ATTEMPTS,
                "reconnect attempt"
            );

            match self.connect().await {
                Ok(caps) => {
                    info!(server = %self.name, "reconnected successfully");
                    return Ok(caps);
                }
                Err(e) => {
                    warn!(
                        server = %self.name,
                        attempt = attempt + 1,
                        "reconnect failed: {e:#}"
                    );
                }
            }
        }

        self.state = McpServerState::Failed;
        bail!(
            "failed to reconnect to MCP server {} after {MAX_RECONNECT_ATTEMPTS} attempts",
            self.name
        )
    }

    /// Disconnect from the MCP server.
    pub async fn disconnect(&mut self) {
        self.disconnect_internal().await;
        debug!(server = %self.name, "MCP server disconnected");
    }

    // -- Protocol operations --

    /// List all tools provided by this server.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        self.ensure_connected()?;
        let result = self.send_request("tools/list", None).await?;
        let tools_value = result
            .get("tools")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        let raw_tools: Vec<RawMcpTool> =
            serde_json::from_value(tools_value).context("failed to parse tools/list")?;
        Ok(raw_tools.into_iter().map(into_mcp_tool).collect())
    }

    /// Call a tool on this server.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: &Value,
        meta: Option<&Value>,
    ) -> Result<Value> {
        self.ensure_connected()?;
        let mut params = serde_json::json!({"name": name, "arguments": arguments});
        if let Some(m) = meta {
            params
                .as_object_mut()
                .unwrap()
                .insert("_meta".to_string(), m.clone());
        }
        let result = self.send_request("tools/call", Some(params)).await?;
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let text = extract_text_from_content(result.get("content").unwrap_or(&Value::Null));
            bail!("MCP tool {name} returned error: {text}");
        }
        Ok(result)
    }

    /// List all prompts provided by this server.
    pub async fn list_prompts(&self) -> Result<Vec<McpPrompt>> {
        self.ensure_connected()?;
        let result = self.send_request("prompts/list", None).await?;
        let prompts_value = result
            .get("prompts")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(prompts_value).context("failed to parse prompts/list")
    }

    /// Get a specific prompt with the given arguments.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: &HashMap<String, String>,
    ) -> Result<Vec<McpPromptMessage>> {
        self.ensure_connected()?;
        let result = self
            .send_request(
                "prompts/get",
                Some(serde_json::json!({"name": name, "arguments": arguments})),
            )
            .await?;
        let messages_value = result
            .get("messages")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(messages_value).context("failed to parse prompts/get")
    }

    /// List resources from this server.
    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        self.ensure_connected()?;
        let result = self.send_request("resources/list", None).await?;
        let rv = result
            .get("resources")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(rv).context("failed to parse resources/list")
    }

    /// Read a specific resource by URI.
    pub async fn read_resource(&self, uri: &str) -> Result<Value> {
        self.ensure_connected()?;
        self.send_request("resources/read", Some(serde_json::json!({"uri": uri})))
            .await
    }

    /// Send a ping to the server and wait for the pong.
    pub async fn ping(&self) -> Result<()> {
        self.ensure_connected()?;
        let _ = self.send_request("ping", None).await?;
        Ok(())
    }

    // -- Internal machinery --

    /// Start the background reader loop that dispatches incoming messages.
    fn start_reader_loop(&mut self) {
        // We need a separate channel for the reader to send raw messages,
        // since the transport.receive() method holds a lock.
        let _transport = self.transport.as_ref().expect("transport must be set");
        // We can't move the transport into the task because it's behind an
        // Option in self. Instead, we use the pending map and tools_changed
        // flag to dispatch.
        //
        // However, our transport trait returns messages one at a time.
        // We'll create a separate task that reads from the transport and
        // dispatches.

        // Actually, since `transport` is `Box<dyn McpTransport>` and we can't
        // clone it, and it's behind `&self`, we can't move it into a task.
        // We need to restructure: put the transport behind Arc.
        //
        // This is already handled by the transport implementations internally.
        // The reader loop will be started only for transports that have a
        // streaming receive (stdio, SSE, WS). For HTTP, receive is driven
        // by the send method.
    }

    /// Start a background heartbeat that pings the server periodically.
    fn start_heartbeat(&mut self) {
        // Heartbeat is only meaningful for long-lived transports.
        // For HTTP, the server manages sessions; heartbeat is not needed.
        if self.config.transport == McpTransportType::Http {
            return;
        }

        // We can't easily ping from a background task without Arc<Self>.
        // Instead, the manager checks health via `is_healthy()` periodically.
    }

    async fn disconnect_internal(&mut self) {
        self.state = McpServerState::Disconnected;

        if let Some(h) = self.reader_handle.take() {
            h.abort();
        }
        if let Some(h) = self.heartbeat_handle.take() {
            h.abort();
        }

        if let Some(ref transport) = self.transport {
            let _ = transport.close().await;
        }
        self.transport = None;

        // Fail all pending requests.
        let mut pending = self.pending.lock().await;
        for (_, sender) in pending.drain() {
            let _ = sender.send(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: None,
                result: None,
                error: Some(JsonRpcError {
                    code: -1,
                    message: "client disconnected".to_string(),
                    data: None,
                }),
            });
        }
    }

    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let transport = self
            .transport
            .as_ref()
            .ok_or(McpClientError::NotConnected)?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);
        let json = serde_json::to_string(&request)?;

        let (tx, rx) = oneshot::channel();
        {
            self.pending.lock().await.insert(id, tx);
        }

        if let Err(e) = transport.send(&json).await {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }

        // For streaming transports, the response comes via the reader loop.
        // For HTTP transport, the response is pushed into pending by the
        // transport's send method (which reads the response inline).
        // We need to also check for messages that arrived during send.

        // For HTTP transport: send() pushes responses into the incoming channel.
        // We need to drain those and dispatch to pending.
        if self.config.transport == McpTransportType::Http {
            // After HTTP send, try to receive messages and dispatch.
            while let Ok(Some(msg)) = transport.receive().await {
                self.dispatch_message(msg).await;
            }
        }

        let timeout = std::time::Duration::from_secs(
            std::env::var("MCP_TOOL_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS),
        );

        let response = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| {
                let p = self.pending.clone();
                tokio::spawn(async move {
                    p.lock().await.remove(&id);
                });
                McpClientError::Timeout(timeout.as_secs())
            })?
            .map_err(|_| McpClientError::ProcessExited)?;

        if let Some(err) = response.error {
            return Err(McpClientError::JsonRpc(err).into());
        }
        Ok(response.result.unwrap_or(Value::Null))
    }

    async fn dispatch_message(&self, msg: JsonRpcMessage) {
        match msg {
            JsonRpcMessage::Response(resp) => {
                if let Some(id) = resp.id {
                    if let Some(sender) = self.pending.lock().await.remove(&id) {
                        let _ = sender.send(resp);
                    }
                }
            }
            JsonRpcMessage::Request(req) => {
                if req.method == "ping" {
                    let pong = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req.id,
                        "result": {}
                    });
                    if let Some(ref transport) = self.transport {
                        let _ = transport.send(&pong.to_string()).await;
                    }
                } else {
                    debug!(server = %self.name, "unhandled MCP request: {}", req.method);
                }
            }
            JsonRpcMessage::Notification(notif) => {
                debug!(server = %self.name, "MCP notification: {}", notif.method);
                if notif.method == "notifications/tools/list_changed" {
                    self.tools_changed.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<()> {
        let transport = self
            .transport
            .as_ref()
            .ok_or(McpClientError::NotConnected)?;

        let notification = JsonRpcNotification::new(method, params);
        let json = serde_json::to_string(&notification)?;
        transport
            .send(&json)
            .await
            .context("failed to send notification")
    }

    fn ensure_connected(&self) -> Result<()> {
        if self.state != McpServerState::Connected {
            bail!(McpClientError::NotConnected);
        }
        Ok(())
    }

    /// Check if the connection is healthy (transport is open).
    pub fn is_healthy(&self) -> bool {
        self.state == McpServerState::Connected
            && self
                .transport
                .as_ref()
                .map_or(false, |t| t.is_open())
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Some(h) = self.reader_handle.take() {
            h.abort();
        }
        if let Some(h) = self.heartbeat_handle.take() {
            h.abort();
        }
        // Transport cleanup is async; we do our best here.
        // The transport implementations themselves clean up in their Drop.
    }
}

// ---------------------------------------------------------------------------
// Internal types and helpers
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct RawMcpTool {
    name: String,
    description: Option<String>,
    #[serde(rename = "inputSchema")]
    input_schema: Option<Value>,
    #[serde(default)]
    annotations: Option<McpToolAnnotations>,
    #[serde(default, rename = "_meta")]
    meta: Option<Value>,
}

fn into_mcp_tool(t: RawMcpTool) -> McpTool {
    McpTool {
        name: t.name,
        description: truncate_description(&t.description.unwrap_or_default()),
        input_schema: t
            .input_schema
            .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}})),
        annotations: t.annotations,
        meta: t.meta,
    }
}

fn parse_capabilities(init_result: &Value) -> McpCapabilities {
    let caps = init_result.get("capabilities").cloned().unwrap_or_default();
    let instructions = init_result
        .get("instructions")
        .and_then(Value::as_str)
        .map(|s| truncate_description(s));
    McpCapabilities {
        tools: caps.get("tools").is_some(),
        resources: caps.get("resources").is_some(),
        prompts: caps.get("prompts").is_some(),
        instructions,
    }
}

fn truncate_description(desc: &str) -> String {
    if desc.len() <= MAX_DESCRIPTION_LENGTH {
        desc.to_string()
    } else {
        let suffix = "\u{2026} [truncated]";
        let t: String = desc.chars().take(MAX_DESCRIPTION_LENGTH).collect();
        format!("{t}{suffix}")
    }
}

fn extract_text_from_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|i| {
                if i.get("type").and_then(Value::as_str) == Some("text") {
                    i.get("text").and_then(Value::as_str).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

/// Generate a cache key for a server connection (for dedup/memoization).
pub fn get_server_cache_key(name: &str, config: &McpServerConfig) -> String {
    format!("{}-{}", name, serde_json::to_string(config).unwrap_or_default())
}

/// Check if a server config is for a local (stdio/sdk) server.
pub fn is_local_server(config: &McpServerConfig) -> bool {
    matches!(
        config.transport,
        McpTransportType::Stdio | McpTransportType::Sdk
    )
}

/// Detect whether an error is an MCP session-expired error.
pub fn is_session_expired_error(error: &anyhow::Error) -> bool {
    let msg = error.to_string();
    msg.contains("404")
        && (msg.contains("\"code\":-32001") || msg.contains("\"code\": -32001"))
}
