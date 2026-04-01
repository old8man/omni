use serde::{Deserialize, Serialize};

/// Token and cost usage from a single API call.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// Server-side tool usage (web search requests, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_tool_use: Option<ServerToolUse>,
    /// Speed tier for fast mode pricing (e.g. "fast").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
}

/// Server-side tool usage counters.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServerToolUse {
    #[serde(default)]
    pub web_search_requests: u64,
}
