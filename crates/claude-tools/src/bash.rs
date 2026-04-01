use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

const MAX_OUTPUT_CHARS: usize = 30_000;
/// Default timeout for bash commands (120 seconds).
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// Maximum allowed timeout (10 minutes).
const MAX_TIMEOUT_MS: u64 = 600_000;

pub struct BashTool;

/// Truncate a string to at most `max_chars` characters, respecting char boundaries.
fn truncate(s: String) -> String {
    if s.len() <= MAX_OUTPUT_CHARS {
        s
    } else {
        // Find the char boundary at or before MAX_OUTPUT_CHARS
        let mut end = MAX_OUTPUT_CHARS;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        s[..end].to_string()
    }
}

#[async_trait]
impl ToolExecutor for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout": {
                    "type": "number",
                    "description": format!("Optional timeout in milliseconds (max {})", MAX_TIMEOUT_MS)
                },
                "description": {
                    "type": "string",
                    "description": "Optional description of what the command does"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Whether to run the command in the background"
                },
                "dangerouslyDisableSandbox": {
                    "type": "boolean",
                    "description": "Set this to true to dangerously override sandbox mode and run commands without sandboxing."
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
            .ok_or_else(|| anyhow::anyhow!("missing 'command' field"))?
            .to_string();

        let run_in_background = input["run_in_background"].as_bool().unwrap_or(false);

        // Compute timeout: user-specified (clamped to max) or default.
        let timeout_ms = input["timeout"]
            .as_u64()
            .map(|t| t.min(MAX_TIMEOUT_MS))
            .unwrap_or(DEFAULT_TIMEOUT_MS);

        // If already cancelled before we even start, return immediately
        if cancel.is_cancelled() {
            return Ok(ToolResultData {
                data: json!({
                    "stdout": "",
                    "stderr": "",
                    "code": -1,
                    "interrupted": true
                }),
                is_error: false,
            });
        }

        let mut child = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(&command)
            .current_dir(&ctx.working_directory)
            .env("CLAUDECODE", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        // Handle background execution: spawn the process and return immediately
        // with an empty result. The task system will track the background process.
        if run_in_background {
            // Spawn a background task to wait for the child and capture output.
            let task_manager = ctx.task_manager.clone();
            let description = input["description"]
                .as_str()
                .unwrap_or(&command)
                .to_string();
            let cmd_clone = command.clone();

            if let Some(tm) = task_manager {
                let task_id = {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert(
                        "command".to_string(),
                        serde_json::Value::String(cmd_clone.clone()),
                    );
                    let params = claude_core::tasks::types::CreateTaskParams {
                        subject: description.clone(),
                        description: description.clone(),
                        task_type: claude_core::tasks::types::TaskType::LocalBash,
                        active_form: None,
                        owner: None,
                        metadata,
                    };
                    tm.create(params).await
                };

                // Spawn detached task to manage the background process
                let task_id_clone = task_id.clone();
                tokio::spawn(async move {
                    let output = child.wait_with_output().await;
                    if let Ok(output) = output {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        let code = output.status.code().unwrap_or(-1);
                        let combined = format!(
                            "{}{}Exit code: {}",
                            if stdout.is_empty() { String::new() } else { format!("{}\n", stdout) },
                            if stderr.is_empty() { String::new() } else { format!("stderr: {}\n", stderr) },
                            code
                        );
                        tm.append_output(&task_id_clone, &combined).await;
                        let status = if code == 0 {
                            claude_core::tasks::types::TaskStatus::Completed
                        } else {
                            claude_core::tasks::types::TaskStatus::Failed
                        };
                        let _ = tm.update(&task_id_clone, claude_core::tasks::types::UpdateTaskParams {
                            status: Some(status),
                            ..Default::default()
                        }).await;
                    }
                });

                return Ok(ToolResultData {
                    data: json!({
                        "stdout": "",
                        "stderr": "",
                        "code": 0,
                        "interrupted": false,
                        "backgroundTaskId": task_id
                    }),
                    is_error: false,
                });
            }

            // No task manager available — run in background without tracking
            tokio::spawn(async move {
                let _ = child.wait_with_output().await;
            });

            return Ok(ToolResultData {
                data: json!({
                    "stdout": "",
                    "stderr": "",
                    "code": 0,
                    "interrupted": false,
                    "backgroundTaskId": "unknown"
                }),
                is_error: false,
            });
        }

        // Foreground execution with timeout and cancellation support
        // Take pipes before any select so we can read them after wait
        let mut stdout_pipe = child.stdout.take().expect("stdout pipe");
        let mut stderr_pipe = child.stderr.take().expect("stderr pipe");

        let timeout_duration = std::time::Duration::from_millis(timeout_ms);

        tokio::select! {
            _ = cancel.cancelled() => {
                // Kill the child process on cancellation
                let _ = child.kill().await;
                let _ = child.wait().await;
                Ok(ToolResultData {
                    data: json!({
                        "stdout": "",
                        "stderr": "",
                        "code": -1,
                        "interrupted": true
                    }),
                    is_error: false,
                })
            }
            _ = tokio::time::sleep(timeout_duration) => {
                // Timeout: kill the process
                let _ = child.kill().await;
                let _ = child.wait().await;
                // Read any partial output
                let mut stdout_bytes = Vec::new();
                let mut stderr_bytes = Vec::new();
                let _ = stdout_pipe.read_to_end(&mut stdout_bytes).await;
                let _ = stderr_pipe.read_to_end(&mut stderr_bytes).await;
                let stdout = truncate(String::from_utf8_lossy(&stdout_bytes).into_owned());
                let stderr = truncate(String::from_utf8_lossy(&stderr_bytes).into_owned());
                Ok(ToolResultData {
                    data: json!({
                        "stdout": format!("{}\n\nCommand timed out after {}ms", stdout, timeout_ms),
                        "stderr": stderr,
                        "code": -1,
                        "interrupted": true
                    }),
                    is_error: false,
                })
            }
            status = child.wait() => {
                let exit_status = status?;
                // Read remaining output after process has exited
                let mut stdout_bytes = Vec::new();
                let mut stderr_bytes = Vec::new();
                let _ = stdout_pipe.read_to_end(&mut stdout_bytes).await;
                let _ = stderr_pipe.read_to_end(&mut stderr_bytes).await;

                let stdout = truncate(String::from_utf8_lossy(&stdout_bytes).into_owned());
                let stderr = truncate(String::from_utf8_lossy(&stderr_bytes).into_owned());
                let code = exit_status.code().unwrap_or(-1);
                Ok(ToolResultData {
                    data: json!({
                        "stdout": stdout,
                        "stderr": stderr,
                        "code": code,
                        "interrupted": false
                    }),
                    is_error: false,
                })
            }
        }
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }

    fn max_result_size_chars(&self) -> usize {
        30_000
    }
}
