use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

/// Interacts with Language Server Protocol (LSP) servers for code intelligence.
///
/// Supports starting LSP servers for various languages, sending requests
/// (hover, definition, references, completion), and managing server lifecycle.
/// Each language gets its own server process.
pub struct LspTool {
    servers: Mutex<HashMap<String, LspServer>>,
    next_id: Mutex<i64>,
}

struct LspServer {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
}

impl Default for LspTool {
    fn default() -> Self {
        Self::new()
    }
}

impl LspTool {
    /// Create a new `LspTool` with no running servers.
    pub fn new() -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            next_id: Mutex::new(1),
        }
    }

    /// Get the next request ID.
    async fn next_request_id(&self) -> i64 {
        let mut id = self.next_id.lock().await;
        let current = *id;
        *id += 1;
        current
    }

    /// Send a JSON-RPC message to the LSP server and read the response.
    async fn send_request(
        stdin: &mut ChildStdin,
        stdout: &mut BufReader<ChildStdout>,
        method: &str,
        params: Value,
        id: i64,
    ) -> Result<Value> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let body = serde_json::to_string(&request)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        stdin.write_all(header.as_bytes()).await?;
        stdin.write_all(body.as_bytes()).await?;
        stdin.flush().await?;

        // Read response headers
        let mut header_line = String::new();
        let mut content_length: usize = 0;

        loop {
            header_line.clear();
            let bytes_read = stdout.read_line(&mut header_line).await?;
            if bytes_read == 0 {
                return Err(anyhow::anyhow!("LSP server closed connection"));
            }

            let trimmed = header_line.trim();
            if trimmed.is_empty() {
                break;
            }

            if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                content_length = len_str.trim().parse().map_err(|e| {
                    anyhow::anyhow!("Invalid Content-Length: {}", e)
                })?;
            }
        }

        if content_length == 0 {
            return Err(anyhow::anyhow!("Missing Content-Length in LSP response"));
        }

        // Read response body
        let mut body_buf = vec![0u8; content_length];
        tokio::io::AsyncReadExt::read_exact(stdout, &mut body_buf).await?;

        let response: Value = serde_json::from_slice(&body_buf)?;
        Ok(response)
    }
}

#[async_trait]
impl ToolExecutor for LspTool {
    fn name(&self) -> &str {
        "Lsp"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["start", "stop", "hover", "definition", "references", "completion", "diagnostics"],
                    "description": "The LSP command to execute"
                },
                "language": {
                    "type": "string",
                    "description": "Programming language (e.g., 'rust', 'python', 'typescript')"
                },
                "file_path": {
                    "type": "string",
                    "description": "Path to the file for context-sensitive operations"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (0-based) for position-sensitive operations"
                },
                "character": {
                    "type": "integer",
                    "description": "Character offset (0-based) for position-sensitive operations"
                }
            },
            "required": ["command", "language"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let command = input["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'command' field"))?;

        let language = input["language"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'language' field"))?;

        match command {
            "start" => self.start_server(language, ctx).await,
            "stop" => self.stop_server(language).await,
            "hover" => self.position_request(language, input, "textDocument/hover").await,
            "definition" => self.position_request(language, input, "textDocument/definition").await,
            "references" => self.position_request(language, input, "textDocument/references").await,
            "completion" => self.position_request(language, input, "textDocument/completion").await,
            "diagnostics" => self.get_diagnostics(language, input).await,
            other => Ok(ToolResultData {
                data: json!({ "error": format!("Unknown LSP command: '{}'", other) }),
                is_error: true,
            }),
        }
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn is_read_only(&self, input: &Value) -> bool {
        matches!(
            input["command"].as_str(),
            Some("hover" | "definition" | "references" | "completion" | "diagnostics")
        )
    }
}

impl LspTool {
    /// Resolve the LSP server command for a language.
    fn server_command(language: &str) -> Option<(&'static str, Vec<&'static str>)> {
        match language {
            "rust" => Some(("rust-analyzer", vec![])),
            "python" => Some(("pyright-langserver", vec!["--stdio"])),
            "typescript" | "javascript" => Some(("typescript-language-server", vec!["--stdio"])),
            "go" => Some(("gopls", vec!["serve"])),
            "c" | "cpp" | "c++" => Some(("clangd", vec![])),
            _ => None,
        }
    }

    /// Start an LSP server for the given language.
    ///
    /// NOTE: The Mutex guard is intentionally held across I/O (process spawn +
    /// LSP initialize handshake).  This is acceptable because (a) there is no
    /// concurrent caller contention — tool invocations are serialized, and
    /// (b) releasing and re-acquiring would create a TOCTOU race on the server map.
    async fn start_server(&self, language: &str, ctx: &ToolUseContext) -> Result<ToolResultData> {
        let mut servers = self.servers.lock().await;

        if servers.contains_key(language) {
            return Ok(ToolResultData {
                data: json!({
                    "message": format!("LSP server for '{}' is already running", language),
                    "language": language,
                }),
                is_error: false,
            });
        }

        let (cmd, args) = match Self::server_command(language) {
            Some(c) => c,
            None => {
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!("No LSP server configured for language: '{}'", language),
                        "supported": ["rust", "python", "typescript", "javascript", "go", "c", "cpp"],
                    }),
                    is_error: true,
                });
            }
        };

        let mut child = match tokio::process::Command::new(cmd)
            .args(&args)
            .current_dir(&ctx.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!("Failed to start '{}': {}. Is it installed?", cmd, e),
                        "language": language,
                    }),
                    is_error: true,
                });
            }
        };

        let stdin = child.stdin.take().ok_or_else(|| {
            anyhow::anyhow!("Failed to capture stdin of LSP server")
        })?;
        let stdout_raw = child.stdout.take().ok_or_else(|| {
            anyhow::anyhow!("Failed to capture stdout of LSP server")
        })?;

        let server = LspServer {
            stdin,
            stdout: BufReader::new(stdout_raw),
            _child: child,
        };

        servers.insert(language.to_string(), server);

        // Send initialize request
        let server = servers.get_mut(language).ok_or_else(|| {
            anyhow::anyhow!("Server not found after insert")
        })?;

        let id = self.next_request_id().await;
        let root_uri = format!("file://{}", ctx.working_directory.display());

        let init_params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "hover": { "contentFormat": ["plaintext"] },
                    "definition": {},
                    "references": {},
                    "completion": {
                        "completionItem": { "snippetSupport": false }
                    }
                }
            }
        });

        let response = Self::send_request(
            &mut server.stdin,
            &mut server.stdout,
            "initialize",
            init_params,
            id,
        )
        .await;

        match response {
            Ok(resp) => {
                // Send initialized notification
                let notif = json!({
                    "jsonrpc": "2.0",
                    "method": "initialized",
                    "params": {},
                });
                let body = serde_json::to_string(&notif)?;
                let header = format!("Content-Length: {}\r\n\r\n", body.len());
                server.stdin.write_all(header.as_bytes()).await?;
                server.stdin.write_all(body.as_bytes()).await?;
                server.stdin.flush().await?;

                Ok(ToolResultData {
                    data: json!({
                        "message": format!("LSP server for '{}' started successfully", language),
                        "language": language,
                        "server_name": resp["result"]["serverInfo"]["name"],
                    }),
                    is_error: false,
                })
            }
            Err(e) => {
                servers.remove(language);
                Ok(ToolResultData {
                    data: json!({
                        "error": format!("LSP initialization failed: {}", e),
                        "language": language,
                    }),
                    is_error: true,
                })
            }
        }
    }

    /// Stop an LSP server for the given language.
    ///
    /// NOTE: Lock held across I/O (shutdown request + exit notification) — see
    /// [`start_server`](Self::start_server) for rationale.
    async fn stop_server(&self, language: &str) -> Result<ToolResultData> {
        let mut servers = self.servers.lock().await;

        match servers.remove(language) {
            Some(mut server) => {
                // Send shutdown request
                let id = self.next_request_id().await;
                let _ = Self::send_request(
                    &mut server.stdin,
                    &mut server.stdout,
                    "shutdown",
                    json!(null),
                    id,
                )
                .await;

                // Send exit notification
                let notif = json!({
                    "jsonrpc": "2.0",
                    "method": "exit",
                    "params": null,
                });
                let body = serde_json::to_string(&notif).unwrap_or_default();
                let header = format!("Content-Length: {}\r\n\r\n", body.len());
                let _ = server.stdin.write_all(header.as_bytes()).await;
                let _ = server.stdin.write_all(body.as_bytes()).await;

                Ok(ToolResultData {
                    data: json!({
                        "message": format!("LSP server for '{}' stopped", language),
                        "language": language,
                    }),
                    is_error: false,
                })
            }
            None => Ok(ToolResultData {
                data: json!({
                    "error": format!("No LSP server running for '{}'", language),
                    "language": language,
                }),
                is_error: true,
            }),
        }
    }

    /// Execute a position-based LSP request (hover, definition, references, completion).
    ///
    /// NOTE: Lock held across I/O (JSON-RPC round-trip) — see
    /// [`start_server`](Self::start_server) for rationale.
    async fn position_request(
        &self,
        language: &str,
        input: &Value,
        method: &str,
    ) -> Result<ToolResultData> {
        let mut servers = self.servers.lock().await;

        let server = match servers.get_mut(language) {
            Some(s) => s,
            None => {
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!("No LSP server running for '{}'. Start one first.", language),
                        "language": language,
                    }),
                    is_error: true,
                });
            }
        };

        let file_path = input["file_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'file_path' for position request"))?;

        let line = input["line"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("missing 'line' for position request"))?;

        let character = input["character"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("missing 'character' for position request"))?;

        let uri = format!("file://{}", file_path);
        let id = self.next_request_id().await;

        let mut params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        });

        // References needs an additional context param
        if method == "textDocument/references" {
            params["context"] = json!({ "includeDeclaration": true });
        }

        let response = Self::send_request(
            &mut server.stdin,
            &mut server.stdout,
            method,
            params,
            id,
        )
        .await?;

        if let Some(error) = response.get("error") {
            return Ok(ToolResultData {
                data: json!({
                    "error": format!("LSP error: {}", error),
                    "method": method,
                }),
                is_error: true,
            });
        }

        Ok(ToolResultData {
            data: json!({
                "method": method,
                "file_path": file_path,
                "line": line,
                "character": character,
                "result": response.get("result"),
            }),
            is_error: false,
        })
    }

    /// Get diagnostics for a file (placeholder - diagnostics are typically pushed via notifications).
    async fn get_diagnostics(
        &self,
        language: &str,
        input: &Value,
    ) -> Result<ToolResultData> {
        let servers = self.servers.lock().await;

        if !servers.contains_key(language) {
            return Ok(ToolResultData {
                data: json!({
                    "error": format!("No LSP server running for '{}'. Start one first.", language),
                    "language": language,
                }),
                is_error: true,
            });
        }

        let file_path = input["file_path"]
            .as_str()
            .unwrap_or("(none)");

        // Diagnostics in LSP are push-based (textDocument/publishDiagnostics).
        // For a pull-based approach, we would need to implement the notification handler.
        Ok(ToolResultData {
            data: json!({
                "message": "Diagnostics are delivered via LSP notifications. Open/change the file to trigger diagnostic updates.",
                "file_path": file_path,
                "language": language,
            }),
            is_error: false,
        })
    }
}
