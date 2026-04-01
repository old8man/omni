use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

/// Maximum number of characters shown in error message snippets.
const MAX_DISPLAY_LEN: usize = 100;

/// Truncate a string to at most `MAX_DISPLAY_LEN` characters for display in error messages.
fn truncate_display(s: &str) -> String {
    if s.chars().count() <= MAX_DISPLAY_LEN {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(MAX_DISPLAY_LEN).collect();
        format!("{}…", truncated)
    }
}

fn error_result(msg: impl Into<String>) -> ToolResultData {
    ToolResultData {
        data: json!({ "error": msg.into() }),
        is_error: true,
    }
}

pub struct FileEditTool;

#[async_trait]
impl ToolExecutor for FileEditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The string to search for and replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The string to replace old_string with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "If true, replace all occurrences; otherwise require exactly one match",
                    "default": false
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
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
            None => return Ok(error_result("Missing required field: file_path")),
        };
        let old_string = match input["old_string"].as_str() {
            Some(s) => s,
            None => return Ok(error_result("Missing required field: old_string")),
        };
        let new_string = match input["new_string"].as_str() {
            Some(s) => s,
            None => return Ok(error_result("Missing required field: new_string")),
        };
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        // Reject no-op edits where old_string == new_string
        if old_string == new_string {
            return Ok(error_result(
                "No changes to make: old_string and new_string are exactly the same.",
            ));
        }

        let path = std::path::Path::new(file_path);

        // If old_string is non-empty and the file doesn't exist → error
        if !path.exists() {
            if old_string.is_empty() {
                // Creating a new file with no old content to replace — write empty→new
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, new_string)?;
                return Ok(ToolResultData {
                    data: json!({
                        "filePath": file_path,
                        "oldString": old_string,
                        "newString": new_string,
                        "originalFile": "",
                        "replaceAll": replace_all
                    }),
                    is_error: false,
                });
            }
            return Ok(error_result(format!("File not found: {}", file_path)));
        }

        let original = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => return Ok(error_result(format!("Failed to read file: {}", e))),
        };

        // Redirect .ipynb edits to NotebookEdit tool
        if file_path.ends_with(".ipynb") {
            return Ok(error_result(
                "File is a Jupyter Notebook. Use the NotebookEdit tool to edit this file.",
            ));
        }

        // Empty old_string on an existing file with content means attempted file creation
        // which should fail — file already exists
        if old_string.is_empty() && !original.trim().is_empty() {
            return Ok(error_result("Cannot create new file - file already exists."));
        }

        // Empty old_string on an empty existing file is valid (replace empty with content)
        if old_string.is_empty() && original.trim().is_empty() {
            if let Err(e) = std::fs::write(path, new_string) {
                return Ok(error_result(format!("Failed to write file: {}", e)));
            }
            return Ok(ToolResultData {
                data: json!({
                    "filePath": file_path,
                    "oldString": old_string,
                    "newString": new_string,
                    "originalFile": original,
                    "replaceAll": replace_all
                }),
                is_error: false,
            });
        }

        // Count occurrences
        let count = original.matches(old_string).count();

        if count == 0 {
            return Ok(error_result(format!(
                "String not found in file.\nSearched for: {}",
                truncate_display(old_string)
            )));
        }

        if count > 1 && !replace_all {
            return Ok(error_result(format!(
                "Found {} occurrences of the search string but replace_all is false. \
                 Use replace_all=true to replace all occurrences, or provide a more specific \
                 old_string that matches exactly once.\nSearched for: {}",
                count,
                truncate_display(old_string)
            )));
        }

        let new_content = if replace_all {
            original.replace(old_string, new_string)
        } else {
            // replace first occurrence only
            original.replacen(old_string, new_string, 1)
        };

        // Ensure parent directories exist (in case of a new path — defensive)
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        if let Err(e) = std::fs::write(path, &new_content) {
            return Ok(error_result(format!("Failed to write file: {}", e)));
        }

        Ok(ToolResultData {
            data: json!({
                "filePath": file_path,
                "oldString": old_string,
                "newString": new_string,
                "originalFile": original,
                "replaceAll": replace_all
            }),
            is_error: false,
        })
    }

    fn is_destructive(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }

    fn max_result_size_chars(&self) -> usize {
        100_000
    }
}
