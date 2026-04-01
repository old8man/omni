use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;

const TRIGGERS_BETA: &str = "ccr-triggers-2026-01-30";
const REQUEST_TIMEOUT_SECS: u64 = 20;

/// Manages remote agent triggers (scheduled cron jobs).
///
/// Supports listing, getting, creating, updating, and running triggers
/// via the Anthropic API. Requires OAuth authentication.
pub struct RemoteTriggerTool {
    client: reqwest::Client,
}

impl Default for RemoteTriggerTool {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteTriggerTool {
    /// Create a new `RemoteTriggerTool`.
    ///
    /// Falls back to a default client if the builder fails (e.g. TLS unavailable).
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

#[async_trait]
impl ToolExecutor for RemoteTriggerTool {
    fn name(&self) -> &str {
        "RemoteTrigger"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "get", "create", "update", "run"],
                    "description": "The trigger action to perform"
                },
                "trigger_id": {
                    "type": "string",
                    "description": "Required for get, update, and run actions"
                },
                "body": {
                    "type": "object",
                    "description": "JSON body for create and update actions"
                }
            },
            "required": ["action"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let action = input["action"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'action' field"))?;

        let trigger_id = input["trigger_id"].as_str();
        let body = input.get("body");

        // Resolve OAuth tokens from environment
        let access_token = match std::env::var("CLAUDE_OAUTH_TOKEN") {
            Ok(token) if !token.is_empty() => token,
            _ => {
                return Ok(ToolResultData {
                    data: json!({
                        "error": "Not authenticated with a claude.ai account. Run /login and try again."
                    }),
                    is_error: true,
                });
            }
        };

        let org_uuid = std::env::var("CLAUDE_ORG_UUID").unwrap_or_default();
        if org_uuid.is_empty() {
            return Ok(ToolResultData {
                data: json!({ "error": "Unable to resolve organization UUID." }),
                is_error: true,
            });
        }

        let base_url = std::env::var("CLAUDE_API_BASE")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
        let triggers_base = format!("{}/v1/code/triggers", base_url);

        let (method, url, request_body) = match action {
            "list" => ("GET", triggers_base.clone(), None),
            "get" => {
                let id = trigger_id.ok_or_else(|| anyhow::anyhow!("get requires trigger_id"))?;
                validate_trigger_id(id)?;
                ("GET", format!("{}/{}", triggers_base, id), None)
            }
            "create" => {
                let b = body.ok_or_else(|| anyhow::anyhow!("create requires body"))?;
                ("POST", triggers_base.clone(), Some(b.clone()))
            }
            "update" => {
                let id = trigger_id.ok_or_else(|| anyhow::anyhow!("update requires trigger_id"))?;
                validate_trigger_id(id)?;
                let b = body.ok_or_else(|| anyhow::anyhow!("update requires body"))?;
                ("POST", format!("{}/{}", triggers_base, id), Some(b.clone()))
            }
            "run" => {
                let id = trigger_id.ok_or_else(|| anyhow::anyhow!("run requires trigger_id"))?;
                validate_trigger_id(id)?;
                (
                    "POST",
                    format!("{}/{}/run", triggers_base, id),
                    Some(json!({})),
                )
            }
            other => {
                return Ok(ToolResultData {
                    data: json!({ "error": format!("Unknown action: '{}'", other) }),
                    is_error: true,
                });
            }
        };

        let mut request = match method {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            other => {
                return Ok(ToolResultData {
                    data: json!({ "error": format!("Unsupported HTTP method: '{}'", other) }),
                    is_error: true,
                });
            }
        };

        request = request
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", TRIGGERS_BETA)
            .header("x-organization-uuid", &org_uuid);

        if let Some(b) = request_body {
            request = request.json(&b);
        }

        let response = tokio::select! {
            res = request.send() => {
                match res {
                    Ok(r) => r,
                    Err(e) => {
                        return Ok(ToolResultData {
                            data: json!({ "error": format!("Request failed: {}", e) }),
                            is_error: true,
                        });
                    }
                }
            }
            _ = cancel.cancelled() => {
                return Ok(ToolResultData {
                    data: json!({ "error": "Request cancelled" }),
                    is_error: true,
                });
            }
        };

        let status = response.status().as_u16();
        let response_body: Value = response.json().await.unwrap_or(json!(null));

        let response_str = serde_json::to_string_pretty(&response_body)
            .unwrap_or_else(|_| response_body.to_string());

        Ok(ToolResultData {
            data: json!({
                "status": status,
                "json": response_str,
            }),
            is_error: status >= 400,
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn is_read_only(&self, input: &Value) -> bool {
        matches!(input["action"].as_str(), Some("list" | "get"))
    }
}

/// Validate that a trigger ID contains only safe characters.
fn validate_trigger_id(id: &str) -> Result<()> {
    if id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        Ok(())
    } else {
        anyhow::bail!("trigger_id contains unsafe characters")
    }
}
