use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

const MAX_OUTPUT_CHARS: usize = 30_000;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

/// Executes PowerShell commands on Windows systems.
///
/// Provides the same general interface as BashTool but uses PowerShell
/// (`pwsh` or `powershell.exe`) as the shell. Supports command execution
/// with configurable timeouts, background mode, and output capture.
pub struct PowerShellTool;

#[async_trait]
impl ToolExecutor for PowerShellTool {
    fn name(&self) -> &str {
        "PowerShell"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The PowerShell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 120000, max: 600000)"
                },
                "description": {
                    "type": "string",
                    "description": "Clear description of what this command does"
                }
            },
            "required": ["command"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let command = input["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'command' field"))?;

        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        // Find PowerShell executable
        let ps_exe = find_powershell().await;
        let ps_exe = match ps_exe {
            Some(exe) => exe,
            None => {
                return Ok(ToolResultData {
                    data: json!({
                        "error": "PowerShell not found. Install pwsh (PowerShell 7+) or use the Bash tool instead."
                    }),
                    is_error: true,
                });
            }
        };

        let mut child = match tokio::process::Command::new(&ps_exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", command])
            .current_dir(&ctx.working_directory)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!("Failed to spawn PowerShell: {}", e),
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
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!("Command timed out after {}ms", timeout_ms),
                        "stdout": stdout_handle.await.unwrap_or_default(),
                        "stderr": stderr_handle.await.unwrap_or_default(),
                    }),
                    is_error: true,
                });
            }
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                return Ok(ToolResultData {
                    data: json!({ "error": "Command was cancelled" }),
                    is_error: true,
                });
            }
        };

        let stdout_text = truncate_output(stdout_handle.await.unwrap_or_default());
        let stderr_text = truncate_output(stderr_handle.await.unwrap_or_default());

        let exit_code = status
            .and_then(|s| s.code())
            .unwrap_or(-1);

        let is_error = exit_code != 0;

        Ok(ToolResultData {
            data: json!({
                "stdout": stdout_text,
                "stderr": stderr_text,
                "exitCode": exit_code,
            }),
            is_error,
        })
    }

    fn is_read_only(&self, input: &Value) -> bool {
        // Heuristic: check for known read-only cmdlets
        let cmd = input["command"].as_str().unwrap_or("");
        let lower = cmd.to_lowercase();
        let read_prefixes = [
            "get-", "test-", "select-string", "find", "where", "measure-",
            "format-", "out-string", "write-output", "echo",
        ];
        read_prefixes.iter().any(|p| lower.starts_with(p))
    }

    fn is_destructive(&self, input: &Value) -> bool {
        let cmd = input["command"].as_str().unwrap_or("");
        let lower = cmd.to_lowercase();
        let destructive = [
            "remove-item", "del ", "rm ", "rmdir", "format-volume",
            "clear-content", "stop-process", "restart-computer",
        ];
        destructive.iter().any(|d| lower.contains(d))
    }
}

/// Find the PowerShell executable on the system.
async fn find_powershell() -> Option<String> {
    // Prefer pwsh (PowerShell 7+) over legacy powershell.exe
    for exe in &["pwsh", "powershell"] {
        let result = tokio::process::Command::new(exe)
            .arg("-NoProfile")
            .arg("-Command")
            .arg("$PSVersionTable.PSVersion.ToString()")
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
