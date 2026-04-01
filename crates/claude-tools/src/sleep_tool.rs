use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

const MAX_SLEEP_SECONDS: u64 = 300;

/// Pauses execution for a specified duration.
///
/// Useful for waiting on background tasks, rate limiting, or timing-sensitive
/// operations. The sleep is cancellable and capped at 300 seconds.
pub struct SleepTool;

#[async_trait]
impl ToolExecutor for SleepTool {
    fn name(&self) -> &str {
        "Sleep"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "seconds": {
                    "type": "number",
                    "description": "Number of seconds to sleep (max 300)"
                }
            },
            "required": ["seconds"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let seconds = input["seconds"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("missing or invalid 'seconds' field"))?;

        if seconds < 0.0 {
            return Ok(ToolResultData {
                data: json!({ "error": "seconds must be non-negative" }),
                is_error: true,
            });
        }

        let capped = if seconds > MAX_SLEEP_SECONDS as f64 {
            MAX_SLEEP_SECONDS as f64
        } else {
            seconds
        };

        let duration = std::time::Duration::from_secs_f64(capped);

        tokio::select! {
            _ = tokio::time::sleep(duration) => {
                Ok(ToolResultData {
                    data: json!({
                        "slept_seconds": capped,
                        "requested_seconds": seconds,
                        "capped": seconds > MAX_SLEEP_SECONDS as f64,
                    }),
                    is_error: false,
                })
            }
            _ = cancel.cancelled() => {
                Ok(ToolResultData {
                    data: json!({
                        "error": "Sleep was cancelled",
                        "requested_seconds": seconds,
                    }),
                    is_error: true,
                })
            }
        }
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }
}
