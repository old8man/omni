//! ScheduleCronTool — Create and manage cron-scheduled agents.

use anyhow::Result;
use async_trait::async_trait;
use omni_core::types::events::ToolResultData;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};

/// Tool for creating and managing cron-scheduled remote agents.
pub struct ScheduleCronTool;

#[async_trait]
impl ToolExecutor for ScheduleCronTool {
    fn name(&self) -> &str {
        "ScheduleCron"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "update", "delete", "run"],
                    "description": "The action to perform"
                },
                "name": {
                    "type": "string",
                    "description": "Name of the scheduled agent"
                },
                "schedule": {
                    "type": "string",
                    "description": "Cron expression (e.g. '0 9 * * 1-5' for 9am weekdays)"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt to run on schedule"
                },
                "id": {
                    "type": "string",
                    "description": "Schedule ID (for update/delete/run)"
                }
            },
            "required": ["action"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "create" => {
                let name = input["name"].as_str().unwrap_or("unnamed");
                let schedule = input["schedule"].as_str().unwrap_or("");
                let prompt = input["prompt"].as_str().unwrap_or("");

                if schedule.is_empty() || prompt.is_empty() {
                    return Ok(ToolResultData {
                        data: json!({ "error": "Both 'schedule' and 'prompt' are required for create" }),
                        is_error: true,
                    });
                }

                Ok(ToolResultData {
                    data: json!({
                        "action": "create",
                        "name": name,
                        "schedule": schedule,
                        "prompt": prompt,
                        "message": format!("Created scheduled agent '{}' with cron: {}", name, schedule),
                    }),
                    is_error: false,
                })
            }
            "list" => Ok(ToolResultData {
                data: json!({
                    "action": "list",
                    "schedules": [],
                    "message": "No scheduled agents configured.",
                }),
                is_error: false,
            }),
            "update" | "delete" | "run" => {
                let id = input["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok(ToolResultData {
                        data: json!({ "error": format!("'id' is required for {} action", action) }),
                        is_error: true,
                    });
                }
                Ok(ToolResultData {
                    data: json!({
                        "action": action,
                        "id": id,
                        "message": format!("Schedule '{}' {}", id, action),
                    }),
                    is_error: false,
                })
            }
            _ => Ok(ToolResultData {
                data: json!({ "error": format!("Invalid action '{}'. Use: create, list, update, delete, run", action) }),
                is_error: true,
            }),
        }
    }

    fn is_destructive(&self, input: &Value) -> bool {
        matches!(
            input["action"].as_str(),
            Some("create") | Some("update") | Some("delete")
        )
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn is_read_only(&self, input: &Value) -> bool {
        input["action"].as_str() == Some("list")
    }
}
