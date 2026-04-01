use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::tasks::{
    CreateTaskParams, TaskManager, TaskStatus, TaskType, UpdateTaskParams,
};
use omni_core::types::events::ToolResultData;

/// Helper to get the task manager from context, returning an error result if absent.
fn require_task_manager(ctx: &ToolUseContext) -> std::result::Result<&Arc<TaskManager>, ToolResultData> {
    ctx.task_manager.as_ref().ok_or_else(|| ToolResultData {
        data: json!({ "error": "Task manager not available" }),
        is_error: true,
    })
}

// ---------------------------------------------------------------------------
// TaskCreateTool
// ---------------------------------------------------------------------------

/// Creates a new task in the task list.
pub struct TaskCreateTool;

#[async_trait]
impl ToolExecutor for TaskCreateTool {
    fn name(&self) -> &str {
        "TaskCreate"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "subject": {
                    "type": "string",
                    "description": "A brief title for the task"
                },
                "description": {
                    "type": "string",
                    "description": "What needs to be done"
                },
                "activeForm": {
                    "type": "string",
                    "description": "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")"
                },
                "metadata": {
                    "type": "object",
                    "description": "Arbitrary metadata to attach to the task"
                }
            },
            "required": ["subject", "description"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let manager = match require_task_manager(ctx) {
            Ok(m) => m,
            Err(e) => return Ok(e),
        };

        let subject = input["subject"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'subject' field"))?
            .to_string();

        let description = input["description"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'description' field"))?
            .to_string();

        let active_form = input["activeForm"].as_str().map(String::from);

        let metadata: HashMap<String, serde_json::Value> = input["metadata"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        let task_id = manager
            .create(CreateTaskParams {
                subject: subject.clone(),
                description,
                active_form,
                task_type: TaskType::InProcessTeammate,
                owner: None,
                metadata,
            })
            .await;

        Ok(ToolResultData {
            data: json!({
                "task": {
                    "id": task_id,
                    "subject": subject,
                }
            }),
            is_error: false,
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// TaskListTool
// ---------------------------------------------------------------------------

/// Lists all tasks in the task list.
pub struct TaskListTool;

#[async_trait]
impl ToolExecutor for TaskListTool {
    fn name(&self) -> &str {
        "TaskList"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(
        &self,
        _input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let manager = match require_task_manager(ctx) {
            Ok(m) => m,
            Err(e) => return Ok(e),
        };

        let all_tasks = manager.list(None).await;

        let resolved_ids: std::collections::HashSet<&str> = all_tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .map(|t| t.id.as_str())
            .collect();

        let tasks: Vec<Value> = all_tasks
            .iter()
            .map(|task| {
                let blocked_by: Vec<&str> = task
                    .blocked_by
                    .iter()
                    .filter(|id| !resolved_ids.contains(id.as_str()))
                    .map(|s| s.as_str())
                    .collect();

                let mut v = json!({
                    "id": task.id,
                    "subject": task.subject,
                    "status": task.status.to_string(),
                });

                if let Some(ref owner) = task.owner {
                    v["owner"] = json!(owner);
                }
                if !blocked_by.is_empty() {
                    v["blockedBy"] = json!(blocked_by);
                }
                v
            })
            .collect();

        Ok(ToolResultData {
            data: json!({ "tasks": tasks }),
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

// ---------------------------------------------------------------------------
// TaskGetTool
// ---------------------------------------------------------------------------

/// Retrieves a single task by ID.
pub struct TaskGetTool;

#[async_trait]
impl ToolExecutor for TaskGetTool {
    fn name(&self) -> &str {
        "TaskGet"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "taskId": {
                    "type": "string",
                    "description": "The ID of the task to retrieve"
                }
            },
            "required": ["taskId"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let manager = match require_task_manager(ctx) {
            Ok(m) => m,
            Err(e) => return Ok(e),
        };

        let task_id = input["taskId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'taskId' field"))?;

        match manager.get(task_id).await {
            Some(task) => Ok(ToolResultData {
                data: json!({
                    "task": {
                        "id": task.id,
                        "subject": task.subject,
                        "description": task.description,
                        "status": task.status.to_string(),
                        "blocks": task.blocks,
                        "blockedBy": task.blocked_by,
                        "owner": task.owner,
                        "activeForm": task.active_form,
                        "metadata": task.metadata,
                    }
                }),
                is_error: false,
            }),
            None => Ok(ToolResultData {
                data: json!({ "task": null }),
                is_error: false,
            }),
        }
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// TaskUpdateTool
// ---------------------------------------------------------------------------

/// Updates an existing task's fields.
pub struct TaskUpdateTool;

/// Parse a status string into a TaskStatus.
fn parse_status(s: &str) -> Option<TaskStatus> {
    match s {
        "pending" => Some(TaskStatus::Pending),
        "in_progress" => Some(TaskStatus::InProgress),
        "running" => Some(TaskStatus::Running),
        "completed" => Some(TaskStatus::Completed),
        "failed" => Some(TaskStatus::Failed),
        "killed" => Some(TaskStatus::Killed),
        _ => None,
    }
}

#[async_trait]
impl ToolExecutor for TaskUpdateTool {
    fn name(&self) -> &str {
        "TaskUpdate"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "taskId": {
                    "type": "string",
                    "description": "The ID of the task to update"
                },
                "subject": {
                    "type": "string",
                    "description": "New subject for the task"
                },
                "description": {
                    "type": "string",
                    "description": "New description for the task"
                },
                "activeForm": {
                    "type": "string",
                    "description": "Present continuous form shown in spinner when in_progress"
                },
                "status": {
                    "type": "string",
                    "description": "New status for the task (pending, in_progress, completed, or deleted)"
                },
                "owner": {
                    "type": "string",
                    "description": "New owner for the task"
                },
                "addBlocks": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Task IDs that this task blocks"
                },
                "addBlockedBy": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Task IDs that block this task"
                },
                "metadata": {
                    "type": "object",
                    "description": "Metadata keys to merge into the task. Set a key to null to delete it."
                }
            },
            "required": ["taskId"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let manager = match require_task_manager(ctx) {
            Ok(m) => m,
            Err(e) => return Ok(e),
        };

        let task_id = input["taskId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'taskId' field"))?;

        // Check if task exists
        let existing = match manager.get(task_id).await {
            Some(t) => t,
            None => {
                return Ok(ToolResultData {
                    data: json!({
                        "success": false,
                        "taskId": task_id,
                        "updatedFields": [],
                        "error": "Task not found",
                    }),
                    is_error: false,
                });
            }
        };

        // Handle deletion
        let status_str = input["status"].as_str();
        if status_str == Some("deleted") {
            let deleted = manager.delete(task_id).await;
            return Ok(ToolResultData {
                data: json!({
                    "success": deleted,
                    "taskId": task_id,
                    "updatedFields": if deleted { vec!["deleted"] } else { vec![] },
                    "statusChange": if deleted {
                        Some(json!({ "from": existing.status.to_string(), "to": "deleted" }))
                    } else {
                        None
                    },
                }),
                is_error: false,
            });
        }

        let mut updated_fields: Vec<&str> = Vec::new();
        let mut params = UpdateTaskParams::default();

        if let Some(subject) = input["subject"].as_str() {
            if subject != existing.subject {
                params.subject = Some(subject.to_string());
                updated_fields.push("subject");
            }
        }
        if let Some(description) = input["description"].as_str() {
            if description != existing.description {
                params.description = Some(description.to_string());
                updated_fields.push("description");
            }
        }
        if let Some(active_form) = input["activeForm"].as_str() {
            params.active_form = Some(active_form.to_string());
            updated_fields.push("activeForm");
        }
        if let Some(owner) = input["owner"].as_str() {
            params.owner = Some(owner.to_string());
            updated_fields.push("owner");
        }

        if let Some(metadata_obj) = input["metadata"].as_object() {
            let merged: HashMap<String, serde_json::Value> = metadata_obj
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            params.metadata = Some(merged);
            updated_fields.push("metadata");
        }

        let mut status_change = None;
        if let Some(s) = status_str {
            if let Some(new_status) = parse_status(s) {
                if new_status != existing.status {
                    status_change = Some(json!({
                        "from": existing.status.to_string(),
                        "to": new_status.to_string(),
                    }));
                    params.status = Some(new_status);
                    updated_fields.push("status");
                }
            }
        }

        if let Some(arr) = input["addBlocks"].as_array() {
            let ids: Vec<String> = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
            if !ids.is_empty() {
                params.add_blocks = ids;
                updated_fields.push("blocks");
            }
        }
        if let Some(arr) = input["addBlockedBy"].as_array() {
            let ids: Vec<String> = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
            if !ids.is_empty() {
                params.add_blocked_by = ids;
                updated_fields.push("blockedBy");
            }
        }

        manager.update(task_id, params).await;

        let mut result = json!({
            "success": true,
            "taskId": task_id,
            "updatedFields": updated_fields,
        });

        if let Some(sc) = status_change {
            result["statusChange"] = sc;
        }

        // Reminder when completing a task in team mode
        if input["status"].as_str() == Some("completed") && ctx.agent_name.is_some() {
            result["reminder"] = json!(
                "Task completed. Call TaskList now to find your next available task or see if your work unblocked others."
            );
        }

        Ok(ToolResultData {
            data: result,
            is_error: false,
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// TaskStopTool
// ---------------------------------------------------------------------------

/// Stops a running background task by marking it as killed.
pub struct TaskStopTool;

#[async_trait]
impl ToolExecutor for TaskStopTool {
    fn name(&self) -> &str {
        "TaskStop"
    }

    fn aliases(&self) -> &[&str] {
        &["KillShell"]
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The ID of the background task to stop"
                }
            },
            "required": ["task_id"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let manager = match require_task_manager(ctx) {
            Ok(m) => m,
            Err(e) => return Ok(e),
        };

        let task_id = input["task_id"]
            .as_str()
            .or_else(|| input["shell_id"].as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'task_id' field"))?;

        let task = match manager.get(task_id).await {
            Some(t) => t,
            None => {
                return Ok(ToolResultData {
                    data: json!({ "error": format!("No task found with ID: {}", task_id) }),
                    is_error: true,
                });
            }
        };

        if task.status != TaskStatus::Running
            && task.status != TaskStatus::InProgress
            && task.status != TaskStatus::Pending
        {
            return Ok(ToolResultData {
                data: json!({
                    "error": format!("Task {} is not running (status: {})", task_id, task.status)
                }),
                is_error: true,
            });
        }

        // Kill the process if we have a PID
        if let Some(pid) = task.pid {
            #[cfg(unix)]
            {
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            }
        }

        let stopped = manager.stop(task_id).await;

        match stopped {
            Some(t) => Ok(ToolResultData {
                data: json!({
                    "message": format!("Successfully stopped task: {}", task_id),
                    "task_id": task_id,
                    "task_type": t.task_type.to_string(),
                    "command": t.subject,
                }),
                is_error: false,
            }),
            None => Ok(ToolResultData {
                data: json!({ "error": "Failed to stop task" }),
                is_error: true,
            }),
        }
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// TaskOutputTool
// ---------------------------------------------------------------------------

/// Retrieves the output/log of a task.
pub struct TaskOutputTool;

#[async_trait]
impl ToolExecutor for TaskOutputTool {
    fn name(&self) -> &str {
        "TaskOutput"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The ID of the task whose output to retrieve"
                }
            },
            "required": ["task_id"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let manager = match require_task_manager(ctx) {
            Ok(m) => m,
            Err(e) => return Ok(e),
        };

        let task_id = input["task_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'task_id' field"))?;

        let task = match manager.get(task_id).await {
            Some(t) => t,
            None => {
                return Ok(ToolResultData {
                    data: json!({ "error": format!("No task found with ID: {}", task_id) }),
                    is_error: true,
                });
            }
        };

        Ok(ToolResultData {
            data: json!({
                "task_id": task_id,
                "status": task.status.to_string(),
                "output": task.output,
                "error": task.error,
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
