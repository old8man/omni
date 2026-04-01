use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

/// Performs a web search and returns summarized results.
///
/// Uses a simple HTTP request to a search API endpoint. The results are
/// returned as structured JSON with titles, URLs, and snippets.
pub struct WebSearchTool {
    client: reqwest::Client,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    /// Create a new `WebSearchTool` with a default HTTP client.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[async_trait]
impl ToolExecutor for WebSearchTool {
    fn name(&self) -> &str {
        "WebSearch"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to look up on the web"
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default: 5, max: 20)"
                }
            },
            "required": ["query"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'query' field"))?;

        let num_results = input["num_results"]
            .as_u64()
            .unwrap_or(5)
            .min(20) as usize;

        // Check for Brave Search API key
        let api_key = std::env::var("BRAVE_SEARCH_API_KEY").ok();

        let results = if let Some(key) = api_key {
            self.brave_search(query, num_results, &key, &cancel).await?
        } else {
            // No API key available, return a helpful error
            return Ok(ToolResultData {
                data: json!({
                    "error": "No search API key configured. Set BRAVE_SEARCH_API_KEY environment variable to enable web search.",
                    "query": query,
                }),
                is_error: true,
            });
        };

        Ok(ToolResultData {
            data: json!({
                "query": query,
                "results": results,
                "num_results": results.len(),
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

impl WebSearchTool {
    /// Execute a search using the Brave Search API.
    async fn brave_search(
        &self,
        query: &str,
        num_results: usize,
        api_key: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<Value>> {
        let url = "https://api.search.brave.com/res/v1/web/search";

        let request = self
            .client
            .get(url)
            .header("Accept", "application/json")
            .header("Accept-Encoding", "gzip")
            .header("X-Subscription-Token", api_key)
            .query(&[("q", query), ("count", &num_results.to_string())]);

        let response = tokio::select! {
            res = request.send() => res?,
            _ = cancel.cancelled() => {
                return Err(anyhow::anyhow!("Search cancelled"));
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Search API returned status {}: {}",
                status,
                body
            ));
        }

        let body: Value = response.json().await?;

        let results = body["web"]["results"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .take(num_results)
                    .map(|r| {
                        json!({
                            "title": r["title"],
                            "url": r["url"],
                            "description": r["description"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }
}
