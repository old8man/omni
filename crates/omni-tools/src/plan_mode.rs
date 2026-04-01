use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;

/// Enters plan mode, restricting the model to read-only tool use.
///
/// In plan mode the model should explore the codebase and design an
/// implementation approach before writing any code. Only read-only tools
/// are permitted.
pub struct EnterPlanModeTool;

#[async_trait]
impl ToolExecutor for EnterPlanModeTool {
    fn name(&self) -> &str {
        "EnterPlanMode"
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
        // Agents should not enter plan mode
        if ctx.agent_name.is_some() {
            return Ok(ToolResultData {
                data: json!({ "error": "EnterPlanMode tool cannot be used in agent contexts" }),
                is_error: true,
            });
        }

        Ok(ToolResultData {
            data: json!({
                "message": "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach.",
                "instructions": concat!(
                    "In plan mode, you should:\n",
                    "1. Thoroughly explore the codebase to understand existing patterns\n",
                    "2. Identify similar features and architectural approaches\n",
                    "3. Consider multiple approaches and their trade-offs\n",
                    "4. Use AskUserQuestion if you need to clarify the approach\n",
                    "5. Design a concrete implementation strategy\n",
                    "6. When ready, use ExitPlanMode to present your plan for approval\n\n",
                    "Remember: DO NOT write or edit any files yet. This is a read-only exploration and planning phase."
                ),
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

/// Exits plan mode, presenting the plan for user approval before proceeding.
///
/// The model writes its plan to a file and calls this tool to request
/// approval. The plan is persisted to `<cwd>/.claude/plans/plan.md`.
/// In team mode, the plan is sent to the team lead for approval.
pub struct ExitPlanModeTool;

#[async_trait]
impl ToolExecutor for ExitPlanModeTool {
    fn name(&self) -> &str {
        "ExitPlanMode"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "allowedPrompts": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": { "type": "string" },
                            "prompt": { "type": "string" }
                        },
                        "required": ["tool", "prompt"]
                    },
                    "description": "Prompt-based permissions needed to implement the plan"
                }
            }
        })
    }

    async fn call(
        &self,
        _input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let is_agent = ctx.agent_name.is_some();

        // Read the plan from the standard location
        let plan_dir = ctx.working_directory.join(".claude-omni").join("plans");
        let plan_file = plan_dir.join("plan.md");

        let plan = if plan_file.exists() {
            tokio::fs::read_to_string(&plan_file).await.ok()
        } else {
            None
        };

        // If this is a teammate, send plan approval request to the team lead
        if is_agent {
            if let Some(ref plan_text) = plan {
                if let Some(ref sender) = ctx.message_sender {
                    let agent_name = ctx.agent_name.as_deref().unwrap_or("unknown");
                    let request_id = format!(
                        "plan-approval-{}-{}",
                        agent_name,
                        chrono::Utc::now().timestamp_millis()
                    );
                    let payload = json!({
                        "type": "plan_approval_request",
                        "from": agent_name,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                        "planFilePath": plan_file.display().to_string(),
                        "planContent": plan_text,
                        "requestId": request_id,
                    });
                    let _ = sender
                        .send(("team-lead".to_string(), payload.to_string(), None))
                        .await;
                }

                return Ok(ToolResultData {
                    data: json!({
                        "plan": plan_text,
                        "isAgent": true,
                        "filePath": plan_file.display().to_string(),
                        "awaitingLeaderApproval": true,
                        "message": "Your plan has been submitted to the team lead for approval. Wait for approval before proceeding.",
                    }),
                    is_error: false,
                });
            } else {
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!(
                            "No plan file found at {}. Please write your plan to this file before calling ExitPlanMode.",
                            plan_file.display()
                        ),
                    }),
                    is_error: true,
                });
            }
        }

        let plan_text = plan.unwrap_or_default();
        let has_plan = !plan_text.trim().is_empty();

        let message = if has_plan {
            format!(
                "User has approved your plan. You can now start coding.\n\n\
                 Your plan has been saved to: {}\n\
                 You can refer back to it if needed during implementation.\n\n\
                 ## Approved Plan:\n{}",
                plan_file.display(),
                plan_text
            )
        } else {
            "User has approved exiting plan mode. You can now proceed.".to_string()
        };

        Ok(ToolResultData {
            data: json!({
                "plan": if has_plan { Some(&plan_text) } else { None },
                "isAgent": false,
                "filePath": plan_file.display().to_string(),
                "message": message,
            }),
            is_error: false,
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
}
