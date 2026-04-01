use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Transport type for MCP server connections.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransportType {
    #[default]
    Stdio,
    Sse,
    #[serde(rename = "sse-ide")]
    SseIde,
    Http,
    Ws,
    Sdk,
}

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// Transport type (defaults to stdio).
    #[serde(rename = "type", default)]
    pub transport: McpTransportType,

    /// Command to spawn for stdio transport.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Arguments for the command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// Environment variables to set for the server process.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,

    /// URL for SSE/HTTP/WS transport.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// HTTP headers for SSE/HTTP transport.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,

    /// External command to generate headers dynamically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_helper: Option<String>,

    /// IDE name for sse-ide / ws-ide transports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ide_name: Option<String>,

    /// OAuth configuration for SSE/HTTP servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuthConfig>,
}

/// OAuth configuration for an MCP server that supports OAuth 2.0.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthConfig {
    /// OAuth client ID (if pre-registered).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,

    /// Preferred local port for the OAuth callback redirect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_port: Option<u16>,

    /// Explicit authorization server metadata URL (must be https).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_server_metadata_url: Option<String>,
}

/// Scope from which an MCP server config was loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigScope {
    User,
    Project,
    Local,
    Dynamic,
    Enterprise,
    ClaudeAi,
    Managed,
}

impl std::fmt::Display for ConfigScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Project => write!(f, "project"),
            Self::Local => write!(f, "local"),
            Self::Dynamic => write!(f, "dynamic"),
            Self::Enterprise => write!(f, "enterprise"),
            Self::ClaudeAi => write!(f, "claudeai"),
            Self::Managed => write!(f, "managed"),
        }
    }
}

/// An MCP server config together with its originating scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedMcpServerConfig {
    #[serde(flatten)]
    pub config: McpServerConfig,
    pub scope: ConfigScope,
}

/// A tool exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_object_schema")]
    pub input_schema: Value,
    /// Tool annotations (readOnlyHint, destructiveHint, openWorldHint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpToolAnnotations>,
    /// Vendor-extension metadata from `_meta` field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// MCP tool annotations as defined by the MCP spec.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolAnnotations {
    /// If true, the tool does not modify state and can be run concurrently.
    #[serde(default)]
    pub read_only_hint: bool,
    /// If true, the tool may perform destructive operations.
    #[serde(default)]
    pub destructive_hint: bool,
    /// If true, the tool may interact with external systems.
    #[serde(default)]
    pub open_world_hint: bool,
}

fn default_object_schema() -> Value {
    serde_json::json!({"type": "object", "properties": {}})
}

/// A resource exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// A resource tagged with the server that provides it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerResource {
    #[serde(flatten)]
    pub resource: McpResource,
    pub server: String,
}

/// Capabilities reported by an MCP server during initialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpCapabilities {
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub resources: bool,
    #[serde(default)]
    pub prompts: bool,
    /// Server-provided instructions for the model (injected into system prompt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Connection state of an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpServerState {
    Connected,
    Pending,
    Disconnected,
    Failed,
    NeedsAuth,
}

impl std::fmt::Display for McpServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected => write!(f, "connected"),
            Self::Pending => write!(f, "pending"),
            Self::Disconnected => write!(f, "disconnected"),
            Self::Failed => write!(f, "failed"),
            Self::NeedsAuth => write!(f, "needs-auth"),
        }
    }
}

/// A JSON-RPC 2.0 request message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new JSON-RPC 2.0 request.
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 response message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

/// A JSON-RPC 2.0 notification (no id field).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// Create a new JSON-RPC 2.0 notification.
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// A server-initiated JSON-RPC 2.0 request (has both `id` and `method`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcServerRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// An incoming JSON-RPC message from an MCP server.
///
/// Deserialization order matters for `untagged`: Response is tried first
/// (has `result` or `error`), then Request (has `id` + `method`), then
/// Notification (has `method` only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Response(JsonRpcResponse),
    Request(JsonRpcServerRequest),
    Notification(JsonRpcNotification),
}

/// A prompt exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<McpPromptArgument>,
}

/// An argument for an MCP prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// A message returned from an MCP prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptMessage {
    pub role: String,
    pub content: Value,
}

/// Aggregated status info for an MCP server connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub state: McpServerState,
    pub capabilities: Option<McpCapabilities>,
    pub tool_count: usize,
    pub resource_count: usize,
    pub error: Option<String>,
}
