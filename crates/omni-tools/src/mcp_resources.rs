use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use omni_core::mcp::manager::McpManager;
use omni_core::types::events::ToolResultData;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};

/// Tool that lists all MCP resources across connected servers.
pub struct ListMcpResourcesTool {
    manager: Arc<McpManager>,
}

impl ListMcpResourcesTool {
    /// Create a new list-resources tool backed by the given manager.
    pub fn new(manager: Arc<McpManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolExecutor for ListMcpResourcesTool {
    fn name(&self) -> &str {
        "mcp__list_resources"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Optional server name to filter resources by."
                }
            }
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let server_filter = input
            .get("server")
            .and_then(|v| v.as_str())
            .map(String::from);

        let resources = if let Some(server_name) = &server_filter {
            match self.manager.get_resources_for_server(server_name).await {
                Ok(r) => r
                    .into_iter()
                    .map(|res| {
                        serde_json::json!({
                            "uri": res.uri,
                            "name": res.name,
                            "description": res.description,
                            "mimeType": res.mime_type,
                            "server": server_name,
                        })
                    })
                    .collect::<Vec<_>>(),
                Err(e) => {
                    return Ok(ToolResultData {
                        data: serde_json::json!({ "error": format!("{e:#}") }),
                        is_error: true,
                    });
                }
            }
        } else {
            self.manager
                .get_all_resources()
                .await
                .into_iter()
                .map(|sr| {
                    serde_json::json!({
                        "uri": sr.resource.uri,
                        "name": sr.resource.name,
                        "description": sr.resource.description,
                        "mimeType": sr.resource.mime_type,
                        "server": sr.server,
                    })
                })
                .collect::<Vec<_>>()
        };

        Ok(ToolResultData {
            data: serde_json::json!(resources),
            is_error: false,
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }
}

/// Tool that reads a specific MCP resource by URI.
pub struct ReadMcpResourceTool {
    manager: Arc<McpManager>,
}

impl ReadMcpResourceTool {
    /// Create a new read-resource tool backed by the given manager.
    pub fn new(manager: Arc<McpManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolExecutor for ReadMcpResourceTool {
    fn name(&self) -> &str {
        "mcp__read_resource"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "The MCP server that provides the resource."
                },
                "uri": {
                    "type": "string",
                    "description": "The URI of the resource to read."
                }
            },
            "required": ["server", "uri"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let server = input
            .get("server")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: server"))?;
        let uri = input
            .get("uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: uri"))?;

        match self.manager.read_resource(server, uri).await {
            Ok(result) => Ok(ToolResultData {
                data: result,
                is_error: false,
            }),
            Err(e) => Ok(ToolResultData {
                data: serde_json::json!({ "error": format!("{e:#}") }),
                is_error: true,
            }),
        }
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }
}
