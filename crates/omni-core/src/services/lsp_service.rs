use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};
use url::Url;

// ── Configuration types ─────────────────────────────────────────────────────

/// LSP server configuration as loaded from plugin configs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LspServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub extension_to_language: HashMap<String, String>,
    #[serde(default)]
    pub workspace_folder: Option<String>,
    #[serde(default)]
    pub initialization_options: Option<Value>,
    #[serde(default)]
    pub max_restarts: Option<u32>,
    #[serde(default)]
    pub startup_timeout: Option<u64>,
}

/// Server lifecycle state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LspServerState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
}

// ── LSP Client ──────────────────────────────────────────────────────────────

/// Low-level JSON-RPC communication with an LSP server process via stdio.
struct LspClient {
    process: Child,
    stdin: ChildStdin,
    stdout_reader: BufReader<ChildStdout>,
    next_id: AtomicU64,
    initialized: AtomicBool,
}

impl LspClient {
    /// Spawn the LSP server process.
    fn start(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&str>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().context("failed to spawn LSP server")?;
        let stdin = child.stdin.take().context("LSP stdin not available")?;
        let stdout = child.stdout.take().context("LSP stdout not available")?;

        Ok(Self {
            process: child,
            stdin,
            stdout_reader: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
            initialized: AtomicBool::new(false),
        })
    }

    /// Send a JSON-RPC request and wait for the response.
    fn send_request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&request)?;
        self.read_response(id)
    }

    /// Send a JSON-RPC notification (no response expected).
    fn send_notification(&mut self, method: &str, params: Value) -> Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&notification)
    }

    /// Write a JSON-RPC message with the Content-Length header.
    fn write_message(&mut self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.stdin.write_all(header.as_bytes())?;
        self.stdin.write_all(body.as_bytes())?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Read the next JSON-RPC response matching the given request ID.
    ///
    /// Discards notifications and server-to-client requests that arrive before
    /// the expected response.
    fn read_response(&mut self, expected_id: u64) -> Result<Value> {
        loop {
            let msg = self.read_message()?;
            if let Some(id) = msg.get("id") {
                if id.as_u64() == Some(expected_id) {
                    if let Some(err) = msg.get("error") {
                        bail!("LSP error: {}", err);
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
            }
            // Otherwise it's a notification or mismatched id — skip
        }
    }

    /// Read one JSON-RPC message from the stdout stream.
    fn read_message(&mut self) -> Result<Value> {
        // Parse Content-Length header
        let mut content_length: usize = 0;
        let mut header_line = String::new();
        loop {
            header_line.clear();
            let bytes_read = self.stdout_reader.read_line(&mut header_line)?;
            if bytes_read == 0 {
                bail!("LSP server closed stdout");
            }
            let trimmed = header_line.trim();
            if trimmed.is_empty() {
                break; // End of headers
            }
            if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                content_length = len_str.trim().parse().context("invalid Content-Length")?;
            }
        }

        if content_length == 0 {
            bail!("LSP message with zero Content-Length");
        }

        let mut body = vec![0u8; content_length];
        std::io::Read::read_exact(&mut self.stdout_reader, &mut body)?;
        let value: Value = serde_json::from_slice(&body)?;
        Ok(value)
    }

    /// Perform the LSP initialize handshake.
    fn initialize(&mut self, workspace_folder: &str) -> Result<Value> {
        let workspace_uri = Url::from_file_path(workspace_folder)
            .map_err(|_| anyhow::anyhow!("invalid workspace path"))?
            .to_string();

        let workspace_name = Path::new(workspace_folder)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let init_params = serde_json::json!({
            "processId": std::process::id(),
            "initializationOptions": {},
            "workspaceFolders": [{
                "uri": workspace_uri,
                "name": workspace_name,
            }],
            "rootPath": workspace_folder,
            "rootUri": workspace_uri,
            "capabilities": {
                "workspace": {
                    "configuration": false,
                    "workspaceFolders": false,
                },
                "textDocument": {
                    "synchronization": {
                        "dynamicRegistration": false,
                        "willSave": false,
                        "willSaveWaitUntil": false,
                        "didSave": true,
                    },
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "tagSupport": { "valueSet": [1, 2] },
                        "codeDescriptionSupport": true,
                    },
                    "hover": {
                        "dynamicRegistration": false,
                        "contentFormat": ["markdown", "plaintext"],
                    },
                    "definition": {
                        "dynamicRegistration": false,
                        "linkSupport": true,
                    },
                    "references": { "dynamicRegistration": false },
                    "documentSymbol": {
                        "dynamicRegistration": false,
                        "hierarchicalDocumentSymbolSupport": true,
                    },
                    "callHierarchy": { "dynamicRegistration": false },
                },
                "general": {
                    "positionEncodings": ["utf-16"],
                },
            },
        });

        let result = self.send_request("initialize", init_params)?;
        self.send_notification("initialized", serde_json::json!({}))?;
        self.initialized.store(true, Ordering::Relaxed);
        debug!("LSP server initialized");
        Ok(result)
    }

    /// Graceful shutdown: send shutdown + exit, then kill.
    fn stop(&mut self) -> Result<()> {
        let _ = self.send_request("shutdown", serde_json::json!({}));
        let _ = self.send_notification("exit", serde_json::json!({}));
        let _ = self.process.kill();
        let _ = self.process.wait();
        Ok(())
    }
}

// ── LSP Server Instance ─────────────────────────────────────────────────────

/// Manages the lifecycle of a single LSP server with state tracking.
pub struct LspServerInstance {
    pub name: String,
    pub config: LspServerConfig,
    state: Mutex<LspServerState>,
    client: Mutex<Option<LspClient>>,
    restart_count: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl LspServerInstance {
    pub fn new(name: String, config: LspServerConfig) -> Self {
        Self {
            name,
            config,
            state: Mutex::new(LspServerState::Stopped),
            client: Mutex::new(None),
            restart_count: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }

    pub fn state(&self) -> LspServerState {
        *self.state.lock().unwrap()
    }

    /// Start the LSP server and perform the initialize handshake.
    pub fn start(&self) -> Result<()> {
        let current = self.state();
        if current == LspServerState::Running || current == LspServerState::Starting {
            return Ok(());
        }

        *self.state.lock().unwrap() = LspServerState::Starting;
        debug!(server = %self.name, "starting LSP server");

        let workspace = self
            .config
            .workspace_folder
            .as_deref()
            .unwrap_or(".");

        match LspClient::start(
            &self.config.command,
            &self.config.args,
            &self.config.env,
            Some(workspace),
        ) {
            Ok(mut client) => {
                if let Err(e) = client.initialize(workspace) {
                    let _ = client.stop();
                    *self.state.lock().unwrap() = LspServerState::Error;
                    *self.last_error.lock().unwrap() = Some(e.to_string());
                    return Err(e);
                }
                *self.client.lock().unwrap() = Some(client);
                *self.state.lock().unwrap() = LspServerState::Running;
                info!(server = %self.name, "LSP server started");
                Ok(())
            }
            Err(e) => {
                *self.state.lock().unwrap() = LspServerState::Error;
                *self.last_error.lock().unwrap() = Some(e.to_string());
                Err(e)
            }
        }
    }

    /// Stop the server gracefully.
    pub fn stop(&self) -> Result<()> {
        *self.state.lock().unwrap() = LspServerState::Stopping;
        if let Some(mut client) = self.client.lock().unwrap().take() {
            client.stop()?;
        }
        *self.state.lock().unwrap() = LspServerState::Stopped;
        debug!(server = %self.name, "LSP server stopped");
        Ok(())
    }

    /// Restart the server (stop then start).
    pub fn restart(&self) -> Result<()> {
        let max = self.config.max_restarts.unwrap_or(3) as u64;
        let count = self.restart_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count > max {
            bail!(
                "max restart attempts ({}) exceeded for server '{}'",
                max,
                self.name
            );
        }
        self.stop()?;
        self.start()
    }

    pub fn is_healthy(&self) -> bool {
        let state = self.state();
        if state != LspServerState::Running {
            return false;
        }
        let guard = self.client.lock().unwrap();
        guard
            .as_ref()
            .map(|c| c.initialized.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// Send an LSP request with retry on transient "content modified" errors.
    pub fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        const MAX_RETRIES: usize = 3;
        const CONTENT_MODIFIED: i64 = -32801;
        const BASE_DELAY_MS: u64 = 500;

        if !self.is_healthy() {
            bail!(
                "cannot send request to LSP server '{}': server is {:?}",
                self.name,
                self.state()
            );
        }

        let mut last_err = None;
        for attempt in 0..=MAX_RETRIES {
            let mut guard = self.client.lock().unwrap();
            let client = guard.as_mut().context("LSP client not available")?;

            match client.send_request(method, params.clone()) {
                Ok(result) => return Ok(result),
                Err(e) => {
                    let is_content_modified = e
                        .to_string()
                        .contains(&CONTENT_MODIFIED.to_string());

                    if is_content_modified && attempt < MAX_RETRIES {
                        let delay = BASE_DELAY_MS * (1 << attempt);
                        debug!(
                            method,
                            attempt = attempt + 1,
                            delay_ms = delay,
                            "content modified error, retrying"
                        );
                        drop(guard);
                        std::thread::sleep(Duration::from_millis(delay));
                        continue;
                    }
                    last_err = Some(e);
                    break;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("unknown LSP error")))
    }

    /// Send an LSP notification (fire-and-forget).
    pub fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        if !self.is_healthy() {
            bail!(
                "cannot send notification to LSP server '{}': server is {:?}",
                self.name,
                self.state()
            );
        }
        let mut guard = self.client.lock().unwrap();
        let client = guard.as_mut().context("LSP client not available")?;
        client.send_notification(method, params)
    }
}

// ── Diagnostic Registry ─────────────────────────────────────────────────────

/// A single diagnostic reported by an LSP server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LspDiagnostic {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Value>,
}

/// Diagnostics grouped by file URI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticFile {
    pub uri: String,
    pub diagnostics: Vec<LspDiagnostic>,
}

/// A pending diagnostic batch from an LSP server.
#[derive(Clone, Debug)]
pub struct PendingDiagnostic {
    pub server_name: String,
    pub files: Vec<DiagnosticFile>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub attachment_sent: bool,
}

const MAX_DIAGNOSTICS_PER_FILE: usize = 10;
const MAX_TOTAL_DIAGNOSTICS: usize = 30;

/// Collects and deduplicates diagnostics from LSP servers.
pub struct DiagnosticRegistry {
    pending: Mutex<Vec<PendingDiagnostic>>,
    delivered: Mutex<HashMap<String, std::collections::HashSet<String>>>,
}

impl DiagnosticRegistry {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            delivered: Mutex::new(HashMap::new()),
        }
    }

    /// Register diagnostics received from a server.
    pub fn register(&self, server_name: &str, files: Vec<DiagnosticFile>) {
        let mut pending = self.pending.lock().unwrap();
        pending.push(PendingDiagnostic {
            server_name: server_name.to_string(),
            files,
            timestamp: chrono::Utc::now(),
            attachment_sent: false,
        });
        debug!(
            server = server_name,
            count = pending.len(),
            "registered pending LSP diagnostics"
        );
    }

    /// Retrieve pending diagnostics that haven't been delivered yet.
    ///
    /// Deduplicates across files and against previously delivered diagnostics.
    /// Applies volume limiting (per-file and total caps).
    pub fn check(&self) -> Vec<DiagnosticFile> {
        let mut pending = self.pending.lock().unwrap();
        let mut delivered = self.delivered.lock().unwrap();

        let mut all_files: Vec<DiagnosticFile> = Vec::new();
        for diag in pending.iter_mut() {
            if !diag.attachment_sent {
                all_files.extend(diag.files.clone());
                diag.attachment_sent = true;
            }
        }

        // Remove sent entries
        pending.retain(|d| !d.attachment_sent);

        if all_files.is_empty() {
            return Vec::new();
        }

        // Deduplicate
        let mut file_map: HashMap<String, Vec<LspDiagnostic>> = HashMap::new();
        for file in &all_files {
            let entry = file_map.entry(file.uri.clone()).or_default();
            let prev = delivered.entry(file.uri.clone()).or_default();

            for diag in &file.diagnostics {
                let key = diagnostic_key(diag);
                if !prev.contains(&key) {
                    entry.push(diag.clone());
                    prev.insert(key);
                }
            }
        }

        // Volume limiting
        let mut result = Vec::new();
        let mut total = 0;

        for (uri, mut diags) in file_map {
            if diags.is_empty() {
                continue;
            }
            // Sort by severity: Error < Warning < Info < Hint
            diags.sort_by_key(|d| severity_to_number(d.severity.as_deref()));

            diags.truncate(MAX_DIAGNOSTICS_PER_FILE);
            let remaining = MAX_TOTAL_DIAGNOSTICS.saturating_sub(total);
            diags.truncate(remaining);
            total += diags.len();

            if !diags.is_empty() {
                result.push(DiagnosticFile {
                    uri,
                    diagnostics: diags,
                });
            }
        }

        result
    }

    /// Clear all pending diagnostics.
    pub fn clear_pending(&self) {
        self.pending.lock().unwrap().clear();
    }

    /// Clear delivered tracking for a specific file (e.g. after edit).
    pub fn clear_delivered_for_file(&self, file_uri: &str) {
        self.delivered.lock().unwrap().remove(file_uri);
    }

    /// Full reset of all state.
    pub fn reset(&self) {
        self.pending.lock().unwrap().clear();
        self.delivered.lock().unwrap().clear();
    }
}

impl Default for DiagnosticRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn severity_to_number(severity: Option<&str>) -> u8 {
    match severity {
        Some("Error") => 1,
        Some("Warning") => 2,
        Some("Info") => 3,
        Some("Hint") => 4,
        _ => 4,
    }
}

fn diagnostic_key(diag: &LspDiagnostic) -> String {
    serde_json::json!({
        "message": diag.message,
        "severity": diag.severity,
        "range": diag.range,
        "source": diag.source,
        "code": diag.code,
    })
    .to_string()
}

// ── LSP Server Manager ──────────────────────────────────────────────────────

/// Manages multiple LSP server instances and routes requests by file extension.
pub struct LspManager {
    servers: Mutex<HashMap<String, Arc<LspServerInstance>>>,
    extension_map: Mutex<HashMap<String, Vec<String>>>,
    opened_files: Mutex<HashMap<String, String>>,
    pub diagnostics: DiagnosticRegistry,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            extension_map: Mutex::new(HashMap::new()),
            opened_files: Mutex::new(HashMap::new()),
            diagnostics: DiagnosticRegistry::new(),
        }
    }

    /// Initialize from a set of server configs.
    pub fn initialize(&self, configs: HashMap<String, LspServerConfig>) {
        let mut servers = self.servers.lock().unwrap();
        let mut ext_map = self.extension_map.lock().unwrap();

        for (name, config) in configs {
            if config.command.is_empty() {
                warn!(server = %name, "skipping LSP server with empty command");
                continue;
            }

            for ext in config.extension_to_language.keys() {
                let normalized = ext.to_lowercase();
                ext_map
                    .entry(normalized)
                    .or_default()
                    .push(name.clone());
            }

            servers.insert(name.clone(), Arc::new(LspServerInstance::new(name, config)));
        }

        info!(
            server_count = servers.len(),
            "LSP manager initialized"
        );
    }

    /// Shutdown all running servers.
    pub fn shutdown(&self) {
        let servers = self.servers.lock().unwrap();
        for (name, server) in servers.iter() {
            let state = server.state();
            if state == LspServerState::Running || state == LspServerState::Error {
                if let Err(e) = server.stop() {
                    warn!(server = %name, error = %e, "failed to stop LSP server");
                }
            }
        }
        self.servers.lock().unwrap().clear();
        self.extension_map.lock().unwrap().clear();
        self.opened_files.lock().unwrap().clear();
    }

    /// Get the server instance that handles a given file path.
    pub fn get_server_for_file(&self, file_path: &str) -> Option<Arc<LspServerInstance>> {
        let ext = Path::new(file_path)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();

        let ext_map = self.extension_map.lock().unwrap();
        let server_names = ext_map.get(&ext)?;
        let first = server_names.first()?;

        self.servers.lock().unwrap().get(first).cloned()
    }

    /// Ensure the server for a file is started, then return it.
    pub fn ensure_server_started(&self, file_path: &str) -> Result<Option<Arc<LspServerInstance>>> {
        let server = match self.get_server_for_file(file_path) {
            Some(s) => s,
            None => return Ok(None),
        };

        let state = server.state();
        if state == LspServerState::Stopped || state == LspServerState::Error {
            server.start()?;
        }

        Ok(Some(server))
    }

    /// Send an LSP request to the server handling the given file.
    pub fn send_request(&self, file_path: &str, method: &str, params: Value) -> Result<Option<Value>> {
        let server = match self.ensure_server_started(file_path)? {
            Some(s) => s,
            None => return Ok(None),
        };
        server.send_request(method, params).map(Some)
    }

    /// Get diagnostics for a file via `textDocument/diagnostic`.
    pub fn get_diagnostics(&self, file_path: &str) -> Result<Option<Value>> {
        let uri = file_path_to_uri(file_path)?;
        self.send_request(
            file_path,
            "textDocument/diagnostic",
            serde_json::json!({
                "textDocument": { "uri": uri }
            }),
        )
    }

    /// Go to definition.
    pub fn go_to_definition(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<Value>> {
        let uri = file_path_to_uri(file_path)?;
        self.send_request(
            file_path,
            "textDocument/definition",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
        )
    }

    /// Find references.
    pub fn find_references(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<Value>> {
        let uri = file_path_to_uri(file_path)?;
        self.send_request(
            file_path,
            "textDocument/references",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": true },
            }),
        )
    }

    /// Hover information.
    pub fn hover(&self, file_path: &str, line: u32, character: u32) -> Result<Option<Value>> {
        let uri = file_path_to_uri(file_path)?;
        self.send_request(
            file_path,
            "textDocument/hover",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
        )
    }

    /// Notify the server that a file has been opened.
    pub fn open_file(&self, file_path: &str, content: &str) -> Result<()> {
        let server = match self.ensure_server_started(file_path)? {
            Some(s) => s,
            None => return Ok(()),
        };

        let uri = file_path_to_uri(file_path)?;

        {
            let opened = self.opened_files.lock().unwrap();
            if opened.get(&uri).map(|s| s.as_str()) == Some(&server.name) {
                debug!(path = file_path, "file already open on server, skipping didOpen");
                return Ok(());
            }
        }

        let ext = Path::new(file_path)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();

        let language_id = server
            .config
            .extension_to_language
            .get(&ext)
            .cloned()
            .unwrap_or_else(|| "plaintext".into());

        server.send_notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": content,
                }
            }),
        )?;

        self.opened_files
            .lock()
            .unwrap()
            .insert(uri, server.name.clone());

        Ok(())
    }

    /// Notify the server of a file content change.
    pub fn change_file(&self, file_path: &str, content: &str) -> Result<()> {
        let uri = file_path_to_uri(file_path)?;

        // If not opened yet, do a full open instead.
        let is_open = {
            let opened = self.opened_files.lock().unwrap();
            opened.contains_key(&uri)
        };

        if !is_open {
            return self.open_file(file_path, content);
        }

        let server = match self.get_server_for_file(file_path) {
            Some(s) if s.state() == LspServerState::Running => s,
            _ => return self.open_file(file_path, content),
        };

        server.send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": 1 },
                "contentChanges": [{ "text": content }],
            }),
        )
    }

    /// Notify the server that a file has been saved.
    pub fn save_file(&self, file_path: &str) -> Result<()> {
        let server = match self.get_server_for_file(file_path) {
            Some(s) if s.state() == LspServerState::Running => s,
            _ => return Ok(()),
        };
        let uri = file_path_to_uri(file_path)?;
        server.send_notification(
            "textDocument/didSave",
            serde_json::json!({
                "textDocument": { "uri": uri }
            }),
        )
    }

    /// Notify the server that a file has been closed.
    pub fn close_file(&self, file_path: &str) -> Result<()> {
        let server = match self.get_server_for_file(file_path) {
            Some(s) if s.state() == LspServerState::Running => s,
            _ => return Ok(()),
        };
        let uri = file_path_to_uri(file_path)?;
        server.send_notification(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": { "uri": uri }
            }),
        )?;
        self.opened_files.lock().unwrap().remove(&uri);
        Ok(())
    }

    /// Check if a file is currently open on any server.
    pub fn is_file_open(&self, file_path: &str) -> bool {
        let uri = file_path_to_uri(file_path).unwrap_or_default();
        self.opened_files.lock().unwrap().contains_key(&uri)
    }

    /// Get all server instances.
    pub fn all_servers(&self) -> HashMap<String, Arc<LspServerInstance>> {
        self.servers.lock().unwrap().clone()
    }

    /// Check whether at least one server is connected and healthy.
    pub fn is_connected(&self) -> bool {
        self.servers
            .lock()
            .unwrap()
            .values()
            .any(|s| s.state() != LspServerState::Error && s.state() != LspServerState::Stopped)
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a filesystem path to a `file://` URI.
fn file_path_to_uri(path: &str) -> Result<String> {
    let abs = std::path::absolute(Path::new(path))
        .unwrap_or_else(|_| PathBuf::from(path));
    let url =
        Url::from_file_path(&abs).map_err(|_| anyhow::anyhow!("invalid file path: {}", path))?;
    Ok(url.to_string())
}
