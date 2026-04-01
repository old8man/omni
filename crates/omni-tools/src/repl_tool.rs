use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;

const MAX_OUTPUT_CHARS: usize = 30_000;
const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// Executes code in an interactive REPL environment.
///
/// Supports Node.js and Python REPLs. The code is written to a temporary
/// file and executed via the appropriate interpreter. Output (stdout and
/// stderr) is captured and returned.
pub struct ReplTool;

#[async_trait]
impl ToolExecutor for ReplTool {
    fn name(&self) -> &str {
        "REPL"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["javascript", "python"],
                    "description": "The REPL language to use"
                },
                "code": {
                    "type": "string",
                    "description": "The code to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 60000)"
                }
            },
            "required": ["language", "code"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let language = input["language"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'language' field"))?;

        let code = input["code"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'code' field"))?;

        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS);

        let (interpreter, ext) = match language {
            "javascript" => {
                let node = find_interpreter(&["node", "bun"]).await;
                match node {
                    Some(exe) => (exe, "mjs"),
                    None => {
                        return Ok(ToolResultData {
                            data: json!({
                                "error": "No JavaScript runtime found. Install Node.js or Bun."
                            }),
                            is_error: true,
                        });
                    }
                }
            }
            "python" => {
                let python = find_interpreter(&["python3", "python"]).await;
                match python {
                    Some(exe) => (exe, "py"),
                    None => {
                        return Ok(ToolResultData {
                            data: json!({
                                "error": "Python not found. Install Python 3."
                            }),
                            is_error: true,
                        });
                    }
                }
            }
            other => {
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!("Unsupported language: '{}'. Use 'javascript' or 'python'.", other),
                    }),
                    is_error: true,
                });
            }
        };

        // Write code to a temp file
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("claude-repl-{}.{}", std::process::id(), ext));
        tokio::fs::write(&temp_file, code).await?;

        // Execute the code
        let mut child = match tokio::process::Command::new(&interpreter)
            .arg(&temp_file)
            .current_dir(&ctx.working_directory)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(&temp_file).await;
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!("Failed to spawn {}: {}", interpreter, e),
                    }),
                    is_error: true,
                });
            }
        };

        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();

        let stdout_handle = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(ref mut pipe) = stdout {
                let _ = pipe.read_to_end(&mut buf).await;
            }
            String::from_utf8_lossy(&buf).to_string()
        });

        let stderr_handle = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(ref mut pipe) = stderr {
                let _ = pipe.read_to_end(&mut buf).await;
            }
            String::from_utf8_lossy(&buf).to_string()
        });

        let timeout_duration = std::time::Duration::from_millis(timeout_ms);

        let status = tokio::select! {
            status = child.wait() => status.ok(),
            _ = tokio::time::sleep(timeout_duration) => {
                let _ = child.kill().await;
                let _ = tokio::fs::remove_file(&temp_file).await;
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!("Execution timed out after {}ms", timeout_ms),
                        "stdout": stdout_handle.await.unwrap_or_default(),
                        "stderr": stderr_handle.await.unwrap_or_default(),
                    }),
                    is_error: true,
                });
            }
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                let _ = tokio::fs::remove_file(&temp_file).await;
                return Ok(ToolResultData {
                    data: json!({ "error": "Execution was cancelled" }),
                    is_error: true,
                });
            }
        };

        // Clean up temp file
        let _ = tokio::fs::remove_file(&temp_file).await;

        let stdout_text = truncate_output(stdout_handle.await.unwrap_or_default());
        let stderr_text = truncate_output(stderr_handle.await.unwrap_or_default());

        let exit_code = status
            .and_then(|s| s.code())
            .unwrap_or(-1);

        Ok(ToolResultData {
            data: json!({
                "language": language,
                "stdout": stdout_text,
                "stderr": stderr_text,
                "exitCode": exit_code,
            }),
            is_error: exit_code != 0,
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }
}

/// Find the first available interpreter from a list of candidates.
async fn find_interpreter(candidates: &[&str]) -> Option<String> {
    for exe in candidates {
        let result = tokio::process::Command::new(exe)
            .arg("--version")
            .output()
            .await;

        if let Ok(output) = result {
            if output.status.success() {
                return Some(exe.to_string());
            }
        }
    }
    None
}

/// Truncate output to the maximum allowed characters.
fn truncate_output(s: String) -> String {
    if s.len() <= MAX_OUTPUT_CHARS {
        s
    } else {
        let mut end = MAX_OUTPUT_CHARS;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}... (truncated)", &s[..end])
    }
}
