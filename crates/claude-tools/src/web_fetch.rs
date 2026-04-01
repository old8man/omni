use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024; // 5 MB
const MAX_CONTENT_CHARS: usize = 100_000;

/// Fetches the content of a URL and returns it as text.
///
/// Supports HTML pages (converted to readable text), JSON, and plain text.
/// Large responses are truncated. Follows redirects and respects timeouts.
pub struct WebFetchTool {
    client: reqwest::Client,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    /// Create a new `WebFetchTool` with a default HTTP client.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[async_trait]
impl ToolExecutor for WebFetchTool {
    fn name(&self) -> &str {
        "WebFetch"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch content from"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional prompt to focus the extraction on specific content"
                }
            },
            "required": ["url"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
        cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let url = input["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'url' field"))?;

        // Basic URL validation
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(ToolResultData {
                data: json!({ "error": "URL must start with http:// or https://" }),
                is_error: true,
            });
        }

        let request = self
            .client
            .get(url)
            .header("User-Agent", "Claude-Code/1.0");

        let response = tokio::select! {
            res = request.send() => {
                match res {
                    Ok(r) => r,
                    Err(e) => {
                        return Ok(ToolResultData {
                            data: json!({
                                "error": format!("Failed to fetch URL: {}", e),
                                "url": url,
                            }),
                            is_error: true,
                        });
                    }
                }
            }
            _ = cancel.cancelled() => {
                return Ok(ToolResultData {
                    data: json!({ "error": "Fetch cancelled" }),
                    is_error: true,
                });
            }
        };

        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Ok(ToolResultData {
                data: json!({
                    "error": format!("HTTP {} {}", status, response.status().canonical_reason().unwrap_or("Unknown")),
                    "url": url,
                    "status_code": status,
                }),
                is_error: true,
            });
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Read the body with a size limit
        let bytes = response.bytes().await?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Ok(ToolResultData {
                data: json!({
                    "error": format!("Response too large: {} (max {})", claude_core::utils::format::format_file_size(bytes.len() as u64), claude_core::utils::format::format_file_size(MAX_RESPONSE_BYTES as u64)),
                    "url": url,
                }),
                is_error: true,
            });
        }

        let raw_text = String::from_utf8_lossy(&bytes).to_string();

        let content = if content_type.contains("text/html") {
            html_to_text(&raw_text)
        } else {
            raw_text
        };

        // Truncate if needed
        let (content, truncated) = if content.len() > MAX_CONTENT_CHARS {
            let mut end = MAX_CONTENT_CHARS;
            while end > 0 && !content.is_char_boundary(end) {
                end -= 1;
            }
            (content[..end].to_string(), true)
        } else {
            (content, false)
        };

        Ok(ToolResultData {
            data: json!({
                "url": url,
                "content": content,
                "content_type": content_type,
                "status_code": status,
                "truncated": truncated,
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

/// Convert HTML to readable plain text by stripping tags and normalizing whitespace.
///
/// This is a lightweight conversion that handles common HTML elements without
/// requiring a full parser. It strips `<script>` and `<style>` blocks, converts
/// block-level tags to line breaks, and collapses whitespace.
fn html_to_text(html: &str) -> String {
    let mut result = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_name = String::new();
    let mut collecting_tag_name = false;

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
            tag_name.clear();
            collecting_tag_name = true;
            continue;
        }

        if ch == '>' {
            in_tag = false;
            collecting_tag_name = false;

            let lower_tag = tag_name.to_lowercase();

            if lower_tag == "script" {
                in_script = true;
            } else if lower_tag == "/script" {
                in_script = false;
            } else if lower_tag == "style" {
                in_style = true;
            } else if lower_tag == "/style" {
                in_style = false;
            }

            // Insert newlines for block-level elements
            let block_tags = [
                "p", "/p", "br", "br/", "div", "/div", "h1", "/h1", "h2", "/h2",
                "h3", "/h3", "h4", "/h4", "h5", "/h5", "h6", "/h6", "li", "/li",
                "tr", "/tr", "blockquote", "/blockquote", "pre", "/pre",
                "hr", "hr/",
            ];
            if block_tags.iter().any(|&t| lower_tag == t) {
                result.push('\n');
            }

            continue;
        }

        if in_tag {
            if collecting_tag_name {
                if ch.is_whitespace() {
                    collecting_tag_name = false;
                } else {
                    tag_name.push(ch);
                }
            }
            continue;
        }

        if in_script || in_style {
            continue;
        }

        result.push(ch);
    }

    // Decode common HTML entities
    let result = result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");

    // Collapse whitespace: multiple spaces to one, multiple newlines to two
    let mut collapsed = String::with_capacity(result.len());
    let mut prev_newline_count = 0;
    let mut prev_space = false;

    for ch in result.chars() {
        if ch == '\n' {
            prev_newline_count += 1;
            prev_space = false;
            if prev_newline_count <= 2 {
                collapsed.push('\n');
            }
        } else if ch.is_whitespace() {
            prev_newline_count = 0;
            if !prev_space {
                collapsed.push(' ');
                prev_space = true;
            }
        } else {
            prev_newline_count = 0;
            prev_space = false;
            collapsed.push(ch);
        }
    }

    collapsed.trim().to_string()
}
