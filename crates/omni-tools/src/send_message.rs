use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;

/// Sends a message to a teammate or broadcasts to all teammates.
///
/// Supports plain text messages (with a required summary), broadcast via
/// `to: "*"`, and structured protocol messages (shutdown_request,
/// shutdown_response, plan_approval_response).
pub struct SendMessageTool;

#[async_trait]
impl ToolExecutor for SendMessageTool {
    fn name(&self) -> &str {
        "SendMessage"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient: teammate name, or \"*\" for broadcast to all teammates"
                },
                "summary": {
                    "type": "string",
                    "description": "A 5-10 word summary shown as a preview in the UI (required when message is a string)"
                },
                "message": {
                    "description": "Plain text message content, or a structured protocol message object"
                }
            },
            "required": ["to", "message"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let to = input["to"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'to' field"))?;

        if to.trim().is_empty() {
            return Ok(ToolResultData {
                data: json!({ "error": "'to' must not be empty" }),
                is_error: true,
            });
        }

        let sender_name = ctx
            .agent_name
            .as_deref()
            .unwrap_or("team-lead");

        let summary = input["summary"].as_str();

        // Handle structured messages
        if let Some(msg_obj) = input["message"].as_object() {
            if to == "*" {
                return Ok(ToolResultData {
                    data: json!({ "error": "structured messages cannot be broadcast (to: \"*\")" }),
                    is_error: true,
                });
            }

            return self.handle_structured_message(to, msg_obj, sender_name, ctx).await;
        }

        // Handle plain text messages
        let content = match input["message"].as_str() {
            Some(s) => s,
            None => {
                return Ok(ToolResultData {
                    data: json!({ "error": "message must be a string or structured object" }),
                    is_error: true,
                });
            }
        };

        if to != "*"
            && (summary.is_none() || summary.is_some_and(|s| s.trim().is_empty()))
        {
            return Ok(ToolResultData {
                data: json!({ "error": "summary is required when message is a string" }),
                is_error: true,
            });
        }

        // Send via the message channel if available
        if let Some(ref sender) = ctx.message_sender {
            let _ = sender
                .send((to.to_string(), content.to_string(), summary.map(String::from)))
                .await;
        }

        if to == "*" {
            Ok(ToolResultData {
                data: json!({
                    "success": true,
                    "message": "Message broadcast to all teammates",
                    "routing": {
                        "sender": sender_name,
                        "target": "@team",
                        "summary": summary,
                        "content": content,
                    }
                }),
                is_error: false,
            })
        } else {
            Ok(ToolResultData {
                data: json!({
                    "success": true,
                    "message": format!("Message sent to {}'s inbox", to),
                    "routing": {
                        "sender": sender_name,
                        "target": format!("@{}", to),
                        "summary": summary,
                        "content": content,
                    }
                }),
                is_error: false,
            })
        }
    }

    fn is_read_only(&self, input: &Value) -> bool {
        // Plain text messages are read-only; structured ones may have side effects
        input["message"].is_string()
    }
}

impl SendMessageTool {
    /// Handle structured protocol messages (shutdown, plan approval, etc.)
    async fn handle_structured_message(
        &self,
        to: &str,
        msg: &serde_json::Map<String, Value>,
        sender_name: &str,
        ctx: &ToolUseContext,
    ) -> Result<ToolResultData> {
        let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "shutdown_request" => {
                let reason = msg.get("reason").and_then(|v| v.as_str());
                let request_id = format!("shutdown-{}-{}", to, chrono::Utc::now().timestamp_millis());

                if let Some(ref sender) = ctx.message_sender {
                    let payload = json!({
                        "type": "shutdown_request",
                        "request_id": request_id,
                        "from": sender_name,
                        "reason": reason,
                    });
                    let _ = sender
                        .send((to.to_string(), payload.to_string(), None))
                        .await;
                }

                Ok(ToolResultData {
                    data: json!({
                        "success": true,
                        "message": format!("Shutdown request sent to {}. Request ID: {}", to, request_id),
                        "request_id": request_id,
                        "target": to,
                    }),
                    is_error: false,
                })
            }

            "shutdown_response" => {
                let request_id = msg.get("request_id").and_then(|v| v.as_str()).unwrap_or("");
                let approve = msg.get("approve").and_then(|v| v.as_bool()).unwrap_or(false);

                if to != "team-lead" {
                    return Ok(ToolResultData {
                        data: json!({ "error": "shutdown_response must be sent to \"team-lead\"" }),
                        is_error: true,
                    });
                }

                if !approve {
                    let reason = msg
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if reason.is_empty() {
                        return Ok(ToolResultData {
                            data: json!({ "error": "reason is required when rejecting a shutdown request" }),
                            is_error: true,
                        });
                    }
                }

                if let Some(ref sender) = ctx.message_sender {
                    let payload = json!({
                        "type": "shutdown_response",
                        "request_id": request_id,
                        "from": sender_name,
                        "approve": approve,
                    });
                    let _ = sender
                        .send((to.to_string(), payload.to_string(), None))
                        .await;
                }

                let message = if approve {
                    format!("Shutdown approved. Agent {} is now exiting.", sender_name)
                } else {
                    "Shutdown rejected. Continuing to work.".to_string()
                };

                Ok(ToolResultData {
                    data: json!({
                        "success": true,
                        "message": message,
                        "request_id": request_id,
                    }),
                    is_error: false,
                })
            }

            "plan_approval_response" => {
                let request_id = msg.get("request_id").and_then(|v| v.as_str()).unwrap_or("");
                let approve = msg.get("approve").and_then(|v| v.as_bool()).unwrap_or(false);
                let feedback = msg.get("feedback").and_then(|v| v.as_str()).unwrap_or("");

                if let Some(ref sender) = ctx.message_sender {
                    let payload = json!({
                        "type": "plan_approval_response",
                        "request_id": request_id,
                        "approved": approve,
                        "feedback": feedback,
                    });
                    let _ = sender
                        .send((to.to_string(), payload.to_string(), None))
                        .await;
                }

                let message = if approve {
                    format!("Plan approved for {}.", to)
                } else {
                    format!("Plan rejected for {} with feedback: \"{}\"", to, feedback)
                };

                Ok(ToolResultData {
                    data: json!({
                        "success": true,
                        "message": message,
                        "request_id": request_id,
                    }),
                    is_error: false,
                })
            }

            _ => Ok(ToolResultData {
                data: json!({ "error": format!("Unknown structured message type: '{}'", msg_type) }),
                is_error: true,
            }),
        }
    }
}
