use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};
use tracing::debug;

use super::types::*;

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
}

const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_DESCRIPTION_LENGTH: usize = 2048;

/// Client for a single MCP server connection via stdio transport.
pub struct McpClient {
    name: String,
    config: McpServerConfig,
    state: McpServerState,
    capabilities: Option<McpCapabilities>,
    process: Option<Child>,
    stdin: Option<Arc<Mutex<tokio::process::ChildStdin>>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    reader_handle: Option<tokio::task::JoinHandle<()>>,
    /// Set to true when the server sends `notifications/tools/list_changed`.
    /// Callers should check via `tools_changed()` and re-fetch tools.
    tools_changed: Arc<AtomicBool>,
}

impl McpClient {
    /// Create a new MCP client for the given server.
    pub fn new(name: impl Into<String>, config: McpServerConfig) -> Self {
        Self {
            name: name.into(),
            config,
            state: McpServerState::Disconnected,
            capabilities: None,
            process: None,
            stdin: None,
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            reader_handle: None,
            tools_changed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The server name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Current connection state.
    pub fn state(&self) -> &McpServerState {
        &self.state
    }
    /// Capabilities reported after initialization.
    pub fn capabilities(&self) -> Option<&McpCapabilities> {
        self.capabilities.as_ref()
    }

    /// Whether the server has signalled that its tool list changed.
    /// Callers should check this and re-fetch via `list_tools()`.
    pub fn tools_changed(&self) -> bool {
        self.tools_changed.swap(false, Ordering::Relaxed)
    }

    /// Connect to the MCP server by spawning its process and performing the initialize handshake.
    pub async fn connect(&mut self) -> Result<McpCapabilities> {
        if self.config.transport != McpTransport::Stdio {
            bail!(McpClientError::InvalidConfig(format!(
                "only stdio transport supported, got {:?}",
                self.config.transport
            )));
        }
        let command = self
            .config
            .command
            .as_deref()
            .ok_or_else(|| McpClientError::InvalidConfig("stdio requires a command".into()))?;

        self.state = McpServerState::Pending;
        let mut cmd = Command::new(command);
        cmd.args(&self.config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            self.state = McpServerState::Failed;
            McpClientError::SpawnFailed(format!("{command}: {e}"))
        })?;
        let child_stdin = child.stdin.take().ok_or_else(|| {
            self.state = McpServerState::Failed;
            anyhow!("failed to capture stdin")
        })?;
        let child_stdout = child.stdout.take().ok_or_else(|| {
            self.state = McpServerState::Failed;
            anyhow!("failed to capture stdout")
        })?;

        if let Some(stderr) = child.stderr.take() {
            let name = self.name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!(server = %name, "MCP stderr: {}", line);
                }
            });
        }

        let stdin = Arc::new(Mutex::new(child_stdin));
        self.stdin = Some(stdin.clone());
        self.process = Some(child);

        let pending = self.pending.clone();
        let server_name = self.name.clone();
        let tools_changed = self.tools_changed.clone();
        let reader_stdin = self.stdin.clone();
        let reader_handle = tokio::spawn(async move {
            let mut lines = BufReader::new(child_stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonRpcMessage>(&line) {
                    Ok(JsonRpcMessage::Response(resp)) => {
                        if let Some(id) = resp.id {
                            if let Some(sender) = pending.lock().await.remove(&id) {
                                let _ = sender.send(resp);
                            }
                        }
                    }
                    Ok(JsonRpcMessage::Request(req)) => {
                        // Handle server-initiated requests (e.g. ping).
                        if req.method == "ping" {
                            let pong = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": req.id,
                                "result": {}
                            });
                            if let Some(ref stdin) = reader_stdin {
                                let mut guard = stdin.lock().await;
                                let msg = format!("{}\n", pong);
                                let _ = guard.write_all(msg.as_bytes()).await;
                                let _ = guard.flush().await;
                            }
                        } else {
                            debug!(server = %server_name, "unhandled MCP request: {}", req.method);
                        }
                    }
                    Ok(JsonRpcMessage::Notification(notif)) => {
                        debug!(server = %server_name, "MCP notification: {}", notif.method);
                        if notif.method == "notifications/tools/list_changed" {
                            tools_changed.store(true, Ordering::Relaxed);
                        }
                    }
                    Err(e) => {
                        debug!(server = %server_name, "failed to parse MCP message: {e}");
                    }
                }
            }
        });
        self.reader_handle = Some(reader_handle);

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
        self.send_notification("notifications/initialized", None)
            .await?;
        self.state = McpServerState::Connected;
        debug!(server = %self.name, ?capabilities, "MCP server connected");
        Ok(capabilities)
    }

    /// List all tools provided by this server.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        self.ensure_connected()?;
        let result = self.send_request("tools/list", None).await?;
        let tools_value = result.get("tools").cloned().unwrap_or(Value::Array(vec![]));
        let raw_tools: Vec<RawMcpTool> =
            serde_json::from_value(tools_value).context("failed to parse tools/list")?;
        Ok(raw_tools
            .into_iter()
            .map(|t| McpTool {
                name: t.name,
                description: truncate_description(&t.description.unwrap_or_default()),
                input_schema: t
                    .input_schema
                    .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}})),
                annotations: t.annotations,
                meta: t.meta,
            })
            .collect())
    }

    /// Call a tool on this server.
    ///
    /// `meta` is an optional `_meta` object sent alongside the call (e.g.
    /// `{"claudecode/toolUseId": "..."}`) per the MCP spec.
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

    /// Disconnect from the MCP server.
    pub async fn disconnect(&mut self) {
        self.state = McpServerState::Disconnected;
        if let Some(h) = self.reader_handle.take() {
            h.abort();
        }
        self.stdin = None;
        if let Some(mut child) = self.process.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
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
        debug!(server = %self.name, "MCP server disconnected");
    }

    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);
        let (tx, rx) = oneshot::channel();
        {
            self.pending.lock().await.insert(id, tx);
        }

        self.write_message(&request).await.inspect_err(|_| {
            let pending = self.pending.clone();
            tokio::spawn(async move {
                pending.lock().await.remove(&id);
            });
        })?;

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS),
            rx,
        )
        .await
        .map_err(|_| {
            let p = self.pending.clone();
            tokio::spawn(async move {
                p.lock().await.remove(&id);
            });
            McpClientError::Timeout(DEFAULT_REQUEST_TIMEOUT_SECS)
        })?
        .map_err(|_| McpClientError::ProcessExited)?;

        if let Some(err) = response.error {
            return Err(McpClientError::JsonRpc(err).into());
        }
        Ok(response.result.unwrap_or(Value::Null))
    }

    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<()> {
        let notification = JsonRpcNotification::new(method, params);
        let json = serde_json::to_string(&notification)?;
        let stdin = self.stdin.as_ref().ok_or(McpClientError::NotConnected)?;
        let mut guard = stdin.lock().await;
        guard
            .write_all(format!("{json}\n").as_bytes())
            .await
            .context("failed to write notification")?;
        guard.flush().await?;
        Ok(())
    }

    async fn write_message(&self, request: &JsonRpcRequest) -> Result<()> {
        let json = serde_json::to_string(request)?;
        let stdin = self.stdin.as_ref().ok_or(McpClientError::NotConnected)?;
        let mut guard = stdin.lock().await;
        guard
            .write_all(format!("{json}\n").as_bytes())
            .await
            .context("failed to write request")?;
        guard.flush().await?;
        Ok(())
    }

    fn ensure_connected(&self) -> Result<()> {
        if self.state != McpServerState::Connected {
            bail!(McpClientError::NotConnected);
        }
        Ok(())
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Some(h) = self.reader_handle.take() {
            h.abort();
        }
        if let Some(ref mut child) = self.process {
            let _ = child.start_kill();
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct RawMcpTool {
    name: String,
    description: Option<String>,
    #[serde(rename = "inputSchema")]
    input_schema: Option<Value>,
    /// MCP tool annotations (readOnlyHint, destructiveHint, openWorldHint, etc.).
    #[serde(default)]
    annotations: Option<McpToolAnnotations>,
    /// Vendor-extension metadata from the server.
    #[serde(default, rename = "_meta")]
    meta: Option<Value>,
}

fn parse_capabilities(init_result: &Value) -> McpCapabilities {
    let caps = init_result.get("capabilities").cloned().unwrap_or_default();
    let instructions = init_result
        .get("instructions")
        .and_then(Value::as_str)
        .map(|s| {
            // Truncate server instructions same as tool descriptions.
            truncate_description(s)
        });
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
