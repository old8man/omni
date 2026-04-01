use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::debug;

use super::types::{JsonRpcMessage, McpTransportType};

// ---------------------------------------------------------------------------
// Transport trait
// ---------------------------------------------------------------------------

/// Abstraction over the wire protocol used to communicate with an MCP server.
///
/// Implementations must be `Send + Sync` so they can live behind an `Arc` and
/// be shared across tasks.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a raw JSON-RPC message (request or notification) to the server.
    async fn send(&self, message: &str) -> Result<()>;

    /// Receive the next JSON-RPC message from the server.
    ///
    /// Returns `None` when the underlying transport has been closed.
    async fn receive(&self) -> Result<Option<JsonRpcMessage>>;

    /// Shut down the transport, releasing any resources.
    async fn close(&self) -> Result<()>;

    /// Returns `true` if the transport is still open.
    fn is_open(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Stdio transport
// ---------------------------------------------------------------------------

/// Transport that communicates with an MCP server process via stdin/stdout.
pub struct StdioTransport {
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    lines: Arc<Mutex<tokio::io::Lines<BufReader<tokio::process::ChildStdout>>>>,
    child: Arc<Mutex<Option<Child>>>,
    open: Arc<std::sync::atomic::AtomicBool>,
    /// Server name for log messages.
    server_name: String,
}

impl StdioTransport {
    /// Spawn a child process and wrap it in a `StdioTransport`.
    pub async fn spawn(
        server_name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn MCP server process: {command}"))?;

        let child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to capture stdin of MCP server process"))?;

        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture stdout of MCP server process"))?;

        // Drain stderr to a debug logger so it doesn't block the process.
        if let Some(stderr) = child.stderr.take() {
            let name = server_name.to_string();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!(server = %name, "MCP stderr: {}", line);
                }
            });
        }

        Ok(Self {
            stdin: Arc::new(Mutex::new(child_stdin)),
            lines: Arc::new(Mutex::new(BufReader::new(child_stdout).lines())),
            child: Arc::new(Mutex::new(Some(child))),
            open: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            server_name: server_name.to_string(),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, message: &str) -> Result<()> {
        if !self.is_open() {
            bail!("stdio transport is closed");
        }
        let mut guard = self.stdin.lock().await;
        guard
            .write_all(format!("{message}\n").as_bytes())
            .await
            .context("failed to write to MCP server stdin")?;
        guard.flush().await?;
        Ok(())
    }

    async fn receive(&self) -> Result<Option<JsonRpcMessage>> {
        let mut lines = self.lines.lock().await;
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                        Ok(msg) => return Ok(Some(msg)),
                        Err(e) => {
                            debug!(
                                server = %self.server_name,
                                "failed to parse MCP message: {e}"
                            );
                            continue;
                        }
                    }
                }
                Ok(None) => {
                    self.open
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    return Ok(None);
                }
                Err(e) => {
                    self.open
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    return Err(e.into());
                }
            }
        }
    }

    async fn close(&self) -> Result<()> {
        self.open
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// SSE transport
// ---------------------------------------------------------------------------

/// Transport that communicates with an MCP server over Server-Sent Events.
///
/// The SSE protocol uses:
/// - An HTTP GET request that returns an SSE stream for server->client messages.
/// - An HTTP POST endpoint (discovered from the SSE stream) for client->server messages.
pub struct SseTransport {
    /// The POST endpoint URL for sending messages.
    post_url: Arc<Mutex<Option<String>>>,
    /// Buffered incoming messages parsed from the SSE stream.
    incoming: Arc<Mutex<tokio::sync::mpsc::Receiver<JsonRpcMessage>>>,
    /// HTTP client.
    http: reqwest::Client,
    /// Whether the transport is still active.
    open: Arc<std::sync::atomic::AtomicBool>,
    /// Handle for the background SSE reader task.
    _reader_handle: tokio::task::JoinHandle<()>,
    /// Headers to send on POST requests.
    headers: HashMap<String, String>,
}

impl SseTransport {
    /// Open an SSE connection to the given URL.
    pub async fn connect(
        server_name: &str,
        url: &str,
        headers: HashMap<String, String>,
    ) -> Result<Self> {
        let http = reqwest::Client::new();
        let open = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let post_url: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let (tx, rx) = tokio::sync::mpsc::channel::<JsonRpcMessage>(256);

        // Build the SSE GET request.
        let mut req = http.get(url);
        req = req.header("Accept", "text/event-stream");
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let response = req.send().await.context("SSE connection failed")?;
        if !response.status().is_success() {
            bail!(
                "SSE server returned HTTP {}",
                response.status().as_u16()
            );
        }

        let post_url_c = post_url.clone();
        let open_c = open.clone();
        let name = server_name.to_string();
        let base_url = url.to_string();

        let reader_handle = tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk_result) = stream.next().await {
                if !open_c.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        debug!(server = %name, "SSE stream error: {e}");
                        break;
                    }
                };
                let text = match std::str::from_utf8(&chunk) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                buffer.push_str(text);

                // Process complete SSE events (terminated by double newline).
                while let Some(idx) = buffer.find("\n\n") {
                    let event_block = buffer[..idx].to_string();
                    buffer = buffer[idx + 2..].to_string();

                    let mut event_type = String::new();
                    let mut data_lines = Vec::new();

                    for line in event_block.lines() {
                        if let Some(rest) = line.strip_prefix("event:") {
                            event_type = rest.trim().to_string();
                        } else if let Some(rest) = line.strip_prefix("data:") {
                            data_lines.push(rest.trim().to_string());
                        }
                    }

                    if event_type == "endpoint" && !data_lines.is_empty() {
                        // The server tells us the POST URL.
                        let endpoint = &data_lines[0];
                        let resolved = if endpoint.starts_with("http://")
                            || endpoint.starts_with("https://")
                        {
                            endpoint.clone()
                        } else {
                            // Relative path — resolve against the base URL.
                            resolve_url(&base_url, endpoint)
                        };
                        *post_url_c.lock().await = Some(resolved);
                    } else if event_type == "message" && !data_lines.is_empty() {
                        let data = data_lines.join("\n");
                        match serde_json::from_str::<JsonRpcMessage>(&data) {
                            Ok(msg) => {
                                if tx.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                debug!(server = %name, "SSE bad JSON-RPC: {e}");
                            }
                        }
                    }
                }
            }
            open_c.store(false, std::sync::atomic::Ordering::Relaxed);
        });

        Ok(Self {
            post_url,
            incoming: Arc::new(Mutex::new(rx)),
            http,
            open,
            _reader_handle: reader_handle,
            headers,
        })
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send(&self, message: &str) -> Result<()> {
        if !self.is_open() {
            bail!("SSE transport is closed");
        }
        let url = self
            .post_url
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow!("SSE endpoint not yet discovered"))?;

        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json");
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.body(message.to_string()).send().await?;
        if !resp.status().is_success() {
            bail!("SSE POST returned HTTP {}", resp.status().as_u16());
        }
        Ok(())
    }

    async fn receive(&self) -> Result<Option<JsonRpcMessage>> {
        let mut rx = self.incoming.lock().await;
        match rx.recv().await {
            Some(msg) => Ok(Some(msg)),
            None => Ok(None),
        }
    }

    async fn close(&self) -> Result<()> {
        self.open
            .store(false, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// HTTP (Streamable HTTP) transport
// ---------------------------------------------------------------------------

/// Transport that communicates with an MCP server using the Streamable HTTP
/// protocol (MCP spec 2025-03-26).
///
/// Each client message is an HTTP POST that may return either:
/// - A single JSON response (Content-Type: application/json), or
/// - An SSE stream (Content-Type: text/event-stream) with one or more messages.
pub struct HttpTransport {
    url: String,
    http: reqwest::Client,
    headers: HashMap<String, String>,
    /// Session ID returned by the server.
    session_id: Arc<Mutex<Option<String>>>,
    /// Incoming messages from SSE responses.
    incoming_tx: tokio::sync::mpsc::Sender<JsonRpcMessage>,
    incoming_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<JsonRpcMessage>>>,
    open: Arc<std::sync::atomic::AtomicBool>,
}

impl HttpTransport {
    /// Create a new HTTP transport targeting the given URL.
    pub fn new(url: &str, headers: HashMap<String, String>) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel::<JsonRpcMessage>(256);
        Self {
            url: url.to_string(),
            http: reqwest::Client::new(),
            headers,
            session_id: Arc::new(Mutex::new(None)),
            incoming_tx: tx,
            incoming_rx: Arc::new(Mutex::new(rx)),
            open: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(&self, message: &str) -> Result<()> {
        if !self.is_open() {
            bail!("HTTP transport is closed");
        }

        let mut req = self
            .http
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        if let Some(ref sid) = *self.session_id.lock().await {
            req = req.header("Mcp-Session-Id", sid.as_str());
        }

        let resp = req.body(message.to_string()).send().await?;

        // Capture session id header.
        if let Some(sid) = resp.headers().get("mcp-session-id") {
            if let Ok(s) = sid.to_str() {
                *self.session_id.lock().await = Some(s.to_string());
            }
        }

        let status = resp.status();
        if !status.is_success() {
            bail!("HTTP transport POST returned {status}");
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|ct| ct.to_str().ok())
            .unwrap_or("")
            .to_string();

        if content_type.contains("text/event-stream") {
            // Parse SSE stream for multiple messages.
            let body = resp.text().await?;
            for event_block in body.split("\n\n") {
                for line in event_block.lines() {
                    if let Some(data) = line.strip_prefix("data:") {
                        let data = data.trim();
                        if let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(data) {
                            let _ = self.incoming_tx.send(msg).await;
                        }
                    }
                }
            }
        } else {
            // Single JSON response.
            let body = resp.text().await?;
            if !body.trim().is_empty() {
                if let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(&body) {
                    let _ = self.incoming_tx.send(msg).await;
                }
            }
        }

        Ok(())
    }

    async fn receive(&self) -> Result<Option<JsonRpcMessage>> {
        let mut rx = self.incoming_rx.lock().await;
        match rx.recv().await {
            Some(msg) => Ok(Some(msg)),
            None => Ok(None),
        }
    }

    async fn close(&self) -> Result<()> {
        self.open
            .store(false, std::sync::atomic::Ordering::Relaxed);

        // Send a DELETE to terminate the session if we have a session ID.
        if let Some(ref sid) = *self.session_id.lock().await {
            let mut req = self.http.delete(&self.url);
            req = req.header("Mcp-Session-Id", sid.as_str());
            for (k, v) in &self.headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let _ = req.send().await;
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// WebSocket transport
// ---------------------------------------------------------------------------

/// Transport that communicates with an MCP server over WebSockets.
pub struct WebSocketTransport {
    write: Arc<
        Mutex<
            futures_util::stream::SplitSink<
                tokio_tungstenite::WebSocketStream<
                    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
                >,
                tokio_tungstenite::tungstenite::Message,
            >,
        >,
    >,
    incoming_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<JsonRpcMessage>>>,
    open: Arc<std::sync::atomic::AtomicBool>,
    _reader_handle: tokio::task::JoinHandle<()>,
}

impl WebSocketTransport {
    /// Connect to a WebSocket MCP server at the given URL.
    pub async fn connect(
        server_name: &str,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<Self> {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite;

        let mut request = tungstenite::client::IntoClientRequest::into_client_request(url)?;
        {
            let hdrs = request.headers_mut();
            hdrs.insert(
                "Sec-WebSocket-Protocol",
                tungstenite::http::HeaderValue::from_static("mcp"),
            );
            for (k, v) in headers {
                if let (Ok(name), Ok(val)) = (
                    tungstenite::http::HeaderName::from_bytes(k.as_bytes()),
                    tungstenite::http::HeaderValue::from_str(v),
                ) {
                    hdrs.insert(name, val);
                }
            }
        }

        let (ws_stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .context("WebSocket connection failed")?;

        let (write, mut read) = futures_util::StreamExt::split(ws_stream);
        let open = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (tx, rx) = tokio::sync::mpsc::channel::<JsonRpcMessage>(256);

        let open_c = open.clone();
        let name = server_name.to_string();

        let reader_handle = tokio::spawn(async move {
            while let Some(msg_result) = read.next().await {
                if !open_c.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                match msg_result {
                    Ok(tungstenite::Message::Text(text)) => {
                        match serde_json::from_str::<JsonRpcMessage>(&text) {
                            Ok(msg) => {
                                if tx.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                debug!(server = %name, "WS bad JSON-RPC: {e}");
                            }
                        }
                    }
                    Ok(tungstenite::Message::Close(_)) => break,
                    Err(e) => {
                        debug!(server = %name, "WS error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
            open_c.store(false, std::sync::atomic::Ordering::Relaxed);
        });

        Ok(Self {
            write: Arc::new(Mutex::new(write)),
            incoming_rx: Arc::new(Mutex::new(rx)),
            open,
            _reader_handle: reader_handle,
        })
    }
}

#[async_trait]
impl McpTransport for WebSocketTransport {
    async fn send(&self, message: &str) -> Result<()> {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite;

        if !self.is_open() {
            bail!("WebSocket transport is closed");
        }
        let mut write = self.write.lock().await;
        write
            .send(tungstenite::Message::Text(message.to_string()))
            .await
            .context("failed to send WebSocket message")?;
        Ok(())
    }

    async fn receive(&self) -> Result<Option<JsonRpcMessage>> {
        let mut rx = self.incoming_rx.lock().await;
        match rx.recv().await {
            Some(msg) => Ok(Some(msg)),
            None => Ok(None),
        }
    }

    async fn close(&self) -> Result<()> {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite;

        self.open
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let mut write = self.write.lock().await;
        let _ = write
            .send(tungstenite::Message::Close(None))
            .await;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Transport factory
// ---------------------------------------------------------------------------

/// Create the appropriate transport for a given server config.
pub async fn create_transport(
    server_name: &str,
    config: &super::types::McpServerConfig,
) -> Result<Box<dyn McpTransport>> {
    match config.transport {
        McpTransportType::Stdio => {
            let command = config
                .command
                .as_deref()
                .ok_or_else(|| anyhow!("stdio transport requires a command"))?;
            let transport =
                StdioTransport::spawn(server_name, command, &config.args, &config.env).await?;
            Ok(Box::new(transport))
        }
        McpTransportType::Sse | McpTransportType::SseIde => {
            let url = config
                .url
                .as_deref()
                .ok_or_else(|| anyhow!("SSE transport requires a url"))?;
            let transport =
                SseTransport::connect(server_name, url, config.headers.clone()).await?;
            Ok(Box::new(transport))
        }
        McpTransportType::Http => {
            let url = config
                .url
                .as_deref()
                .ok_or_else(|| anyhow!("HTTP transport requires a url"))?;
            let transport = HttpTransport::new(url, config.headers.clone());
            Ok(Box::new(transport))
        }
        McpTransportType::Ws => {
            let url = config
                .url
                .as_deref()
                .ok_or_else(|| anyhow!("WebSocket transport requires a url"))?;
            let transport =
                WebSocketTransport::connect(server_name, url, &config.headers).await?;
            Ok(Box::new(transport))
        }
        McpTransportType::Sdk => {
            bail!("SDK transport is handled out-of-band; cannot create a generic transport for it")
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Resolve a possibly-relative endpoint URL against a base URL.
fn resolve_url(base: &str, endpoint: &str) -> String {
    if let Ok(base_url) = url::Url::parse(base) {
        if let Ok(resolved) = base_url.join(endpoint) {
            return resolved.to_string();
        }
    }
    // Fallback: just concatenate (unlikely path).
    format!("{base}{endpoint}")
}
