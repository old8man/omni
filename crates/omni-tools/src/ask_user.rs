use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;

/// Callback type for asking the user a question and receiving their answer.
pub type AskUserCallback = Box<dyn Fn(String) -> oneshot::Receiver<String> + Send + Sync>;

/// Prompts the user with a question and waits for their response.
///
/// This tool is used when the model needs clarification or input from the
/// user before proceeding. The question is displayed in the TUI and the
/// model blocks until the user provides a response.
pub struct AskUserQuestionTool {
    callback: AskUserCallback,
}

impl AskUserQuestionTool {
    /// Create a new `AskUserQuestionTool` with the given callback.
    ///
    /// The callback receives the question text and returns a `oneshot::Receiver`
    /// that will eventually deliver the user's answer.
    pub fn new(callback: AskUserCallback) -> Self {
        Self { callback }
    }
}

#[async_trait]
impl ToolExecutor for AskUserQuestionTool {
    fn name(&self) -> &str {
        "AskUserQuestion"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                }
            },
            "required": ["question"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let question = input["question"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'question' field"))?;

        let receiver = (self.callback)(question.to_string());

        let answer = tokio::select! {
            result = receiver => {
                match result {
                    Ok(answer) => answer,
                    Err(_) => {
                        return Ok(ToolResultData {
                            data: json!({ "error": "User response channel closed unexpectedly" }),
                            is_error: true,
                        });
                    }
                }
            }
            _ = cancel.cancelled() => {
                return Ok(ToolResultData {
                    data: json!({ "error": "Question was cancelled before user responded" }),
                    is_error: true,
                });
            }
        };

        Ok(ToolResultData {
            data: json!({ "answer": answer }),
            is_error: false,
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }
}
