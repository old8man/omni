use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;

/// Returns structured output in the requested format.
///
/// Used in non-interactive (SDK/CLI) sessions to return the final
/// response as structured JSON conforming to a user-provided schema.
/// The model must call this tool exactly once at the end of its response.
pub struct SyntheticOutputTool;

#[async_trait]
impl ToolExecutor for SyntheticOutputTool {
    fn name(&self) -> &str {
        "StructuredOutput"
    }

    fn input_schema(&self) -> Value {
        // Accept any JSON object — the actual schema is provided dynamically
        // at session creation time and validated by the caller.
        json!({
            "type": "object",
            "additionalProperties": true,
            "description": "Return your final response as a structured JSON object matching the requested schema."
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        // The tool validates and returns the input as structured output.
        // The actual schema validation is done by the application layer
        // (which may inject a runtime-compiled JSON schema validator).
        Ok(ToolResultData {
            data: json!({
                "message": "Structured output provided successfully",
                "output": input,
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
