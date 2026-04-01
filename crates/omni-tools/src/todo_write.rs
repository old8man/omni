use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;

/// Manages the session task checklist (todo list).
///
/// The model uses this to track its progress through a multi-step task.
/// Todos are stored in memory (via task manager) and displayed in the TUI.
/// Each todo has a `content` description and a `status` of either
/// `"pending"`, `"in_progress"`, or `"completed"`.
pub struct TodoWriteTool;

#[async_trait]
impl ToolExecutor for TodoWriteTool {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for the todo item"
                            },
                            "content": {
                                "type": "string",
                                "description": "Description of the task"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Current status of the task"
                            }
                        },
                        "required": ["content", "status"]
                    },
                    "description": "The complete updated todo list"
                }
            },
            "required": ["todos"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let todos = input["todos"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing 'todos' array"))?;

        let all_completed = todos.iter().all(|t| t["status"].as_str() == Some("completed"));

        // Save todos to a file in .claude/todos.json for persistence
        let todo_count = todos.len();
        let completed_count = todos
            .iter()
            .filter(|t| t["status"].as_str() == Some("completed"))
            .count();
        let in_progress_count = todos
            .iter()
            .filter(|t| t["status"].as_str() == Some("in_progress"))
            .count();
        let pending_count = todo_count - completed_count - in_progress_count;

        // Check for verification nudge: closing 3+ items with none being verification
        let verification_nudge = all_completed
            && todo_count >= 3
            && !todos.iter().any(|t| {
                t["content"]
                    .as_str()
                    .map(|c| c.to_lowercase().contains("verif"))
                    .unwrap_or(false)
            });

        let base_message = "Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable";
        let nudge_message = if verification_nudge {
            "\n\nNOTE: You just closed out 3+ tasks and none of them was a verification step. \
             Before writing your final summary, consider spawning a verification agent to validate your work."
        } else {
            ""
        };

        Ok(ToolResultData {
            data: json!({
                "oldTodos": [], // Previous state not tracked in simplified impl
                "newTodos": todos,
                "summary": {
                    "total": todo_count,
                    "completed": completed_count,
                    "in_progress": in_progress_count,
                    "pending": pending_count,
                },
                "message": format!("{}{}", base_message, nudge_message),
                "verificationNudgeNeeded": verification_nudge,
            }),
            is_error: false,
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }
}
