//! Tool that lets the model invoke skills by name.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use omni_core::skills::SkillRegistry;
use omni_core::types::events::ToolResultData;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};

/// Tool allowing the model to invoke a registered skill.
pub struct SkillTool {
    registry: Arc<SkillRegistry>,
}

impl SkillTool {
    /// Create a new SkillTool backed by the given registry.
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ToolExecutor for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name. E.g., \"commit\", \"review-pr\", or \"pdf\""
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments for the skill"
                }
            },
            "required": ["skill"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let raw_name = input["skill"].as_str().unwrap_or("").trim();
        // Strip leading slash for compatibility (model sometimes includes it)
        let skill_name = raw_name.strip_prefix('/').unwrap_or(raw_name);

        if skill_name.is_empty() {
            return Ok(ToolResultData {
                data: json!({ "error": "Missing required parameter: skill" }),
                is_error: true,
            });
        }

        let skill = match self.registry.find_by_name(skill_name) {
            Some(s) => s,
            None => {
                let available: Vec<&str> = self
                    .registry
                    .list_all()
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect();
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!("Unknown skill: '{}'. Available: {}", skill_name, available.join(", ")),
                    }),
                    is_error: true,
                });
            }
        };

        if skill.disable_model_invocation {
            return Ok(ToolResultData {
                data: json!({
                    "error": format!(
                        "Skill '{}' cannot be used with the Skill tool due to disable-model-invocation",
                        skill_name
                    ),
                }),
                is_error: true,
            });
        }

        let args = input["args"].as_str().unwrap_or("").trim();
        let mut body = String::new();

        // Prepend working directory context
        body.push_str(&format!(
            "Working directory: {}\n\n",
            ctx.working_directory.display()
        ));

        body.push_str(&skill.body);

        if !args.is_empty() {
            body.push_str("\n\n---\nArguments: ");
            body.push_str(args);
        }

        Ok(ToolResultData {
            data: json!({
                "skill": skill.name,
                "description": skill.description,
                "content": body,
                "tokenEstimate": skill.rough_token_estimate(),
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
