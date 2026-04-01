use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

/// KAIROS brief output tool — the primary way to send messages to the user
/// in brief/assistant mode.
///
/// When brief mode is active this is the sole visible output channel.
/// Supports markdown formatting and optional file attachments. Status
/// can be `"normal"` (replying to user) or `"proactive"` (unsolicited
/// updates).
pub struct BriefTool;

#[async_trait]
impl ToolExecutor for BriefTool {
    fn name(&self) -> &str {
        "SendUserMessage"
    }

    fn aliases(&self) -> &[&str] {
        &["Brief"]
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message for the user. Supports markdown formatting."
                },
                "attachments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional file paths to attach. Use for photos, screenshots, diffs, logs, or any file the user should see."
                },
                "status": {
                    "type": "string",
                    "enum": ["normal", "proactive"],
                    "description": "Use 'proactive' when surfacing something the user hasn't asked for. Use 'normal' when replying."
                }
            },
            "required": ["message", "status"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let message = input["message"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'message' field"))?;

        let attachments = input["attachments"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<&str>>()
            })
            .unwrap_or_default();

        let sent_at = chrono::Utc::now().to_rfc3339();

        // Resolve attachment metadata
        let mut resolved_attachments = Vec::new();
        for path_str in &attachments {
            let path = if std::path::Path::new(path_str).is_absolute() {
                std::path::PathBuf::from(path_str)
            } else {
                ctx.working_directory.join(path_str)
            };

            if !path.exists() {
                continue;
            }

            let metadata = tokio::fs::metadata(&path).await.ok();
            let size = metadata.map(|m| m.len()).unwrap_or(0);

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            let is_image = matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico"
            );

            resolved_attachments.push(json!({
                "path": path.display().to_string(),
                "size": size,
                "isImage": is_image,
            }));
        }

        let mut data = json!({
            "message": message,
            "sentAt": sent_at,
        });

        if !resolved_attachments.is_empty() {
            data["attachments"] = Value::Array(resolved_attachments.clone());
        }

        let suffix = if resolved_attachments.is_empty() {
            String::new()
        } else {
            let n = resolved_attachments.len();
            let word = if n == 1 { "attachment" } else { "attachments" };
            format!(" ({} {} included)", n, word)
        };

        // The tool result tells the model the message was delivered
        // The actual display is handled by the TUI layer via renderToolResultMessage
        Ok(ToolResultData {
            data: json!({
                "delivered": true,
                "message": message,
                "sentAt": sent_at,
                "attachments": if resolved_attachments.is_empty() { None } else { Some(&resolved_attachments) },
                "confirmation": format!("Message delivered to user.{}", suffix),
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
