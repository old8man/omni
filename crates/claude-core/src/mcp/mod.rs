pub mod client;
pub mod config;
pub mod manager;
/// MCP (Model Context Protocol) integration.
pub mod types;

pub use client::McpClient;
pub use config::{
    build_mcp_tool_name, load_mcp_config, normalize_name_for_mcp, parse_mcp_tool_name, McpConfig,
};
pub use manager::{McpManager, McpToolDescriptor};
pub use types::{
    McpCapabilities, McpResource, McpServerConfig, McpServerState, McpServerStatus, McpTool,
    ServerResource,
};
