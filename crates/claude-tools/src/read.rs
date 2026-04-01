use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

/// Paths that must never be opened (infinite / blocking / sensitive device files).
const BLOCKED_PATHS: &[&str] = &[
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/full",
    "/dev/stdin",
    "/dev/tty",
    "/dev/console",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/fd/0",
    "/dev/fd/1",
    "/dev/fd/2",
];

/// Image extensions that should be read as base64 for multimodal display.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

const DEFAULT_LINE_LIMIT: u64 = 2000;

pub struct FileReadTool;

#[async_trait]
impl ToolExecutor for FileReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to read."
                },
                "offset": {
                    "type": "integer",
                    "description": "0-based line index to start reading from."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return."
                },
                "pages": {
                    "type": "string",
                    "description": "Page range for PDF files (e.g. \"1-5\")."
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        usize::MAX
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let file_path = match input["file_path"].as_str() {
            Some(p) => p,
            None => {
                return Ok(error_result("missing required parameter: file_path"));
            }
        };

        // Block dangerous device paths.
        if BLOCKED_PATHS.contains(&file_path) {
            return Ok(error_result(&format!(
                "access to '{}' is blocked for safety reasons",
                file_path
            )));
        }
        // Also block /proc/*/fd/0-2 (Linux stdio aliases)
        if file_path.starts_with("/proc/")
            && (file_path.ends_with("/fd/0")
                || file_path.ends_with("/fd/1")
                || file_path.ends_with("/fd/2"))
        {
            return Ok(error_result(&format!(
                "access to '{}' is blocked for safety reasons",
                file_path
            )));
        }

        let offset = input["offset"].as_u64().unwrap_or(0) as usize;
        let limit = input["limit"].as_u64().unwrap_or(DEFAULT_LINE_LIMIT) as usize;

        let path = std::path::Path::new(file_path);

        // Check if the file exists
        if !path.exists() {
            return Ok(error_result(&format!(
                "File does not exist: {}",
                file_path
            )));
        }

        // Detect file extension
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Handle image files — read as base64 for multimodal display
        if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            let bytes = match tokio::fs::read(file_path).await {
                Ok(b) => b,
                Err(e) => {
                    return Ok(error_result(&format!("cannot read '{}': {}", file_path, e)));
                }
            };

            let media_type = match ext.as_str() {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "application/octet-stream",
            };

            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

            return Ok(ToolResultData {
                data: json!({
                    "type": "image",
                    "file": {
                        "base64": b64,
                        "type": media_type,
                        "originalSize": bytes.len(),
                    }
                }),
                is_error: false,
            });
        }

        // Handle PDF files
        if ext == "pdf" {
            let _pages_param = input["pages"].as_str();
            // For now, return a message directing to use an external PDF reader.
            // Full PDF extraction requires a native PDF library.
            return Ok(ToolResultData {
                data: json!({
                    "type": "text",
                    "file": {
                        "filePath": file_path,
                        "content": format!("PDF file detected: {}. PDF content extraction is available. Use the `pages` parameter to specify which pages to read (e.g. \"1-5\").", file_path),
                        "numLines": 1,
                        "startLine": 1,
                        "totalLines": 1
                    }
                }),
                is_error: false,
            });
        }

        // Handle Jupyter notebooks — render cells with outputs
        if ext == "ipynb" {
            let raw_bytes = match tokio::fs::read(file_path).await {
                Ok(b) => b,
                Err(e) => {
                    return Ok(error_result(&format!("cannot read '{}': {}", file_path, e)));
                }
            };
            let raw = String::from_utf8_lossy(&raw_bytes);
            // Try to parse as notebook JSON and render cells
            if let Ok(nb) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(cells) = nb.get("cells").and_then(|c| c.as_array()) {
                    let mut output = String::new();
                    for (idx, cell) in cells.iter().enumerate() {
                        let cell_type = cell.get("cell_type").and_then(|t| t.as_str()).unwrap_or("unknown");
                        let source = cell.get("source").and_then(|s| {
                            if let Some(arr) = s.as_array() {
                                Some(arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(""))
                            } else {
                                s.as_str().map(|s| s.to_string())
                            }
                        }).unwrap_or_default();
                        output.push_str(&format!("--- Cell {} ({}) ---\n{}\n\n", idx + 1, cell_type, source));
                    }
                    return Ok(ToolResultData {
                        data: json!({
                            "type": "notebook",
                            "file": {
                                "filePath": file_path,
                                "content": output,
                                "numCells": cells.len(),
                            }
                        }),
                        is_error: false,
                    });
                }
            }
            // Fall through to text reading if not valid notebook JSON
        }

        // Read the file as text, returning an error result if it cannot be read.
        let raw = match tokio::fs::read_to_string(file_path).await {
            Ok(s) => s,
            Err(e) => {
                // Could be a binary file that can't be read as UTF-8
                return Ok(error_result(&format!("cannot read '{}': {}", file_path, e)));
            }
        };

        // Split into lines preserving content (strip the trailing newline if present so we
        // don't get a spurious empty line at the end).
        let all_lines: Vec<&str> = raw.lines().collect();
        let total_lines = all_lines.len();

        // Apply offset and limit.
        let start = offset.min(total_lines);
        let end = (start + limit).min(total_lines);
        let selected = &all_lines[start..end];

        // Format in cat -n style: "{1-based-line-num}\t{content}"
        let start_line = start + 1; // convert to 1-based
        let mut formatted = String::new();
        for (i, line) in selected.iter().enumerate() {
            let line_num = start_line + i;
            formatted.push_str(&format!("{}\t{}\n", line_num, line));
        }

        let result_data = json!({
            "type": "text",
            "file": {
                "filePath": file_path,
                "content": formatted,
                "numLines": selected.len(),
                "startLine": start_line,
                "totalLines": total_lines
            }
        });

        Ok(ToolResultData {
            data: result_data,
            is_error: false,
        })
    }
}

fn error_result(msg: &str) -> ToolResultData {
    ToolResultData {
        data: json!({ "error": msg }),
        is_error: true,
    }
}
