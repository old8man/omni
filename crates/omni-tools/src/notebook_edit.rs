use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;

/// Edits Jupyter notebook (.ipynb) files by modifying individual cells.
///
/// Supports inserting new cells, replacing existing cells, and deleting cells.
/// Validates notebook structure and cell indices before making changes.
pub struct NotebookEditTool;

#[async_trait]
impl ToolExecutor for NotebookEditTool {
    fn name(&self) -> &str {
        "NotebookEdit"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "notebook_path": {
                    "type": "string",
                    "description": "Path to the Jupyter notebook file (.ipynb)"
                },
                "command": {
                    "type": "string",
                    "enum": ["insert_cell", "replace_cell", "delete_cell"],
                    "description": "The edit operation to perform"
                },
                "cell_index": {
                    "type": "integer",
                    "description": "The index of the cell to modify (0-based)"
                },
                "cell_type": {
                    "type": "string",
                    "enum": ["code", "markdown", "raw"],
                    "description": "Type of cell (for insert/replace operations)"
                },
                "source": {
                    "type": "string",
                    "description": "The new cell content (for insert/replace operations)"
                }
            },
            "required": ["notebook_path", "command", "cell_index"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let notebook_path = input["notebook_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'notebook_path' field"))?;

        let command = input["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'command' field"))?;

        let cell_index = input["cell_index"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("missing 'cell_index' field"))? as usize;

        let abs_path = if std::path::Path::new(notebook_path).is_absolute() {
            std::path::PathBuf::from(notebook_path)
        } else {
            ctx.working_directory.join(notebook_path)
        };

        if !abs_path.exists() {
            return Ok(ToolResultData {
                data: json!({ "error": format!("Notebook not found: {}", abs_path.display()) }),
                is_error: true,
            });
        }

        let content = tokio::fs::read_to_string(&abs_path).await?;
        let mut notebook: Value = serde_json::from_str(&content).map_err(|e| {
            anyhow::anyhow!("Invalid notebook JSON: {}", e)
        })?;

        let cells = notebook["cells"]
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("Notebook has no 'cells' array"))?;

        let cell_count = cells.len();

        match command {
            "insert_cell" => {
                if cell_index > cell_count {
                    return Ok(ToolResultData {
                        data: json!({
                            "error": format!(
                                "cell_index {} out of range for insert (notebook has {} cells, valid range 0..={})",
                                cell_index, cell_count, cell_count
                            )
                        }),
                        is_error: true,
                    });
                }

                let cell_type = input["cell_type"].as_str().unwrap_or("code");
                let source = input["source"].as_str().unwrap_or("");

                let source_lines: Vec<Value> = source
                    .lines()
                    .enumerate()
                    .map(|(i, line)| {
                        let line_count = source.lines().count();
                        if i < line_count.saturating_sub(1) {
                            Value::String(format!("{}\n", line))
                        } else {
                            Value::String(line.to_string())
                        }
                    })
                    .collect();

                let new_cell = json!({
                    "cell_type": cell_type,
                    "metadata": {},
                    "source": source_lines,
                    "outputs": if cell_type == "code" { json!([]) } else { json!(null) },
                });

                cells.insert(cell_index, new_cell);
            }

            "replace_cell" => {
                if cell_index >= cell_count {
                    return Ok(ToolResultData {
                        data: json!({
                            "error": format!(
                                "cell_index {} out of range (notebook has {} cells, valid range 0..{})",
                                cell_index, cell_count, cell_count
                            )
                        }),
                        is_error: true,
                    });
                }

                let cell_type = input["cell_type"]
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| cells[cell_index]["cell_type"].as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "code".to_string());

                let source = input["source"].as_str().unwrap_or("");

                let line_count = source.lines().count();
                let source_lines: Vec<Value> = source
                    .lines()
                    .enumerate()
                    .map(|(i, line)| {
                        if i < line_count.saturating_sub(1) {
                            Value::String(format!("{}\n", line))
                        } else {
                            Value::String(line.to_string())
                        }
                    })
                    .collect();

                cells[cell_index]["cell_type"] = Value::String(cell_type.clone());
                cells[cell_index]["source"] = Value::Array(source_lines);
                // Clear outputs on replace
                if cell_type == "code" {
                    cells[cell_index]["outputs"] = json!([]);
                    cells[cell_index]["execution_count"] = Value::Null;
                }
            }

            "delete_cell" => {
                if cell_index >= cell_count {
                    return Ok(ToolResultData {
                        data: json!({
                            "error": format!(
                                "cell_index {} out of range (notebook has {} cells, valid range 0..{})",
                                cell_index, cell_count, cell_count
                            )
                        }),
                        is_error: true,
                    });
                }

                cells.remove(cell_index);
            }

            other => {
                return Ok(ToolResultData {
                    data: json!({ "error": format!("Unknown command: '{}'. Use insert_cell, replace_cell, or delete_cell.", other) }),
                    is_error: true,
                });
            }
        }

        let output = serde_json::to_string_pretty(&notebook)?;
        tokio::fs::write(&abs_path, &output).await?;

        let new_cell_count = notebook["cells"]
            .as_array()
            .map(|c| c.len())
            .unwrap_or(0);

        Ok(ToolResultData {
            data: json!({
                "success": true,
                "command": command,
                "cell_index": cell_index,
                "total_cells": new_cell_count,
                "notebook_path": abs_path.display().to_string(),
            }),
            is_error: false,
        })
    }

    fn is_destructive(&self, _input: &Value) -> bool {
        true
    }
}
