use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use claude_core::mcp::config::parse_mcp_tool_name;
use claude_core::mcp::manager::McpManager;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

/// A proxy tool that forwards calls to an MCP server tool.
pub struct McpToolProxy {
    full_name: String,
    tool_description: String,
    input_schema: Value,
    manager: Arc<McpManager>,
}

impl McpToolProxy {
    /// Create a new MCP tool proxy.
    pub fn new(
        full_name: String,
        description: String,
        input_schema: Value,
        manager: Arc<McpManager>,
    ) -> Self {
        Self {
            full_name,
            tool_description: description,
            input_schema,
            manager,
        }
    }
}

#[async_trait]
impl ToolExecutor for McpToolProxy {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> String {
        self.tool_description.clone()
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let (server_name, tool_name) = parse_mcp_tool_name(&self.full_name)
            .ok_or_else(|| anyhow::anyhow!("invalid MCP tool name: {}", self.full_name))?;

        match self.manager.call_tool(&server_name, &tool_name, input).await {
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
        // MCP tools may have side effects; treat as non-read-only by default.
        false
    }
}

/// Register all MCP tools from a manager into the tool registry.
pub async fn register_mcp_tools(
    registry: &mut crate::registry::ToolRegistry,
    manager: Arc<McpManager>,
) {
    for descriptor in manager.get_all_tools().await {
        let proxy = McpToolProxy::new(
            descriptor.full_name,
            descriptor.description,
            descriptor.input_schema,
            Arc::clone(&manager),
        );
        registry.register(Arc::new(proxy));
    }
}
