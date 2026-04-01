use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolRegistry, ToolUseContext};
use claude_core::types::events::ToolResultData;

/// Searches the tool registry for tools matching a query.
///
/// Useful for discovering available tools when the user or agent isn't sure
/// which tool to use. Supports keyword search and exact name lookup via
/// the `select:` prefix.
pub struct ToolSearchTool {
    /// Snapshot of tool schemas taken at construction time.
    tool_schemas: Vec<ToolInfo>,
}

/// Summary information about a registered tool.
#[derive(Debug, Clone)]
struct ToolInfo {
    name: String,
    schema: Value,
}

impl ToolSearchTool {
    /// Create a `ToolSearchTool` by snapshotting all tools currently in the registry.
    pub fn from_registry(registry: &ToolRegistry) -> Self {
        let tool_schemas = registry
            .all()
            .iter()
            .map(|t| ToolInfo {
                name: t.name().to_string(),
                schema: t.input_schema(),
            })
            .collect();

        Self { tool_schemas }
    }
}

#[async_trait]
impl ToolExecutor for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query. Use 'select:ToolName' for exact lookup, or keywords for fuzzy search."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 5)"
                }
            },
            "required": ["query"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'query' field"))?;

        let max_results = input["max_results"].as_u64().unwrap_or(5) as usize;

        // Handle direct selection: "select:Read,Edit,Grep"
        if let Some(names) = query.strip_prefix("select:") {
            let requested: Vec<&str> = names.split(',').map(|s| s.trim()).collect();
            let matches: Vec<Value> = self
                .tool_schemas
                .iter()
                .filter(|t| requested.iter().any(|r| r.eq_ignore_ascii_case(&t.name)))
                .map(|t| {
                    json!({
                        "name": t.name,
                        "input_schema": t.schema,
                    })
                })
                .collect();

            return Ok(ToolResultData {
                data: json!({
                    "tools": matches,
                    "total_available": self.tool_schemas.len(),
                }),
                is_error: false,
            });
        }

        // Keyword search: score each tool by relevance
        let query_lower = query.to_lowercase();
        let query_terms: Vec<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<(usize, &ToolInfo)> = self
            .tool_schemas
            .iter()
            .filter_map(|tool| {
                let name_lower = tool.name.to_lowercase();
                let schema_str = tool.schema.to_string().to_lowercase();

                let mut score = 0usize;

                // Exact name match
                if name_lower == query_lower {
                    score += 100;
                }

                // Name contains query
                if name_lower.contains(&query_lower) {
                    score += 50;
                }

                // Per-term scoring
                for term in &query_terms {
                    if name_lower.contains(term) {
                        score += 20;
                    }
                    if schema_str.contains(term) {
                        score += 5;
                    }
                }

                if score > 0 {
                    Some((score, tool))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));

        let matches: Vec<Value> = scored
            .into_iter()
            .take(max_results)
            .map(|(_, t)| {
                json!({
                    "name": t.name,
                    "input_schema": t.schema,
                })
            })
            .collect();

        Ok(ToolResultData {
            data: json!({
                "tools": matches,
                "total_available": self.tool_schemas.len(),
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
