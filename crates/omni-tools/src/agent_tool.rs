use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::tasks::{CreateTaskParams, TaskType};
use omni_core::tasks::executor::spawn_agent_task;
use omni_core::types::events::ToolResultData;

/// Spawns a subagent as a background task.
///
/// The agent runs as a separate Claude process with a given prompt. Supports
/// optional model override and worktree isolation. The task's output is
/// captured and accessible via `TaskOutputTool`.
pub struct AgentTool;

#[async_trait]
impl ToolExecutor for AgentTool {
    fn name(&self) -> &str {
        "Agent"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The prompt/instructions for the subagent"
                },
                "model": {
                    "type": "string",
                    "description": "Optional model override for the subagent"
                },
                "subagent_type": {
                    "type": "string",
                    "description": "Type/role of the subagent (e.g., 'researcher', 'test-runner')"
                }
            },
            "required": ["prompt"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let manager = match ctx.task_manager.as_ref() {
            Some(m) => Arc::clone(m),
            None => {
                return Ok(ToolResultData {
                    data: json!({ "error": "Task manager not available" }),
                    is_error: true,
                });
            }
        };

        let prompt = input["prompt"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'prompt' field"))?
            .to_string();

        let model = input["model"].as_str().map(String::from);
        let subagent_type = input["subagent_type"]
            .as_str()
            .unwrap_or("agent");

        let mut metadata = HashMap::new();
        metadata.insert(
            "subagent_type".to_string(),
            Value::String(subagent_type.to_string()),
        );

        let task_id = manager
            .create(CreateTaskParams {
                subject: format!("Agent: {}", truncate_str(&prompt, 60)),
                description: prompt.clone(),
                active_form: Some(format!("Running {} agent", subagent_type)),
                task_type: TaskType::LocalAgent,
                owner: ctx.agent_name.clone(),
                metadata,
            })
            .await;

        let working_dir = ctx.working_directory.clone();
        let tid = task_id.clone();

        // Spawn the agent in the background
        tokio::spawn(async move {
            spawn_agent_task(manager, tid, prompt, working_dir, model).await;
        });

        Ok(ToolResultData {
            data: json!({
                "task_id": task_id,
                "status": "running",
                "message": format!("Subagent spawned as task #{}", task_id),
            }),
            is_error: false,
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len.saturating_sub(3);
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}
