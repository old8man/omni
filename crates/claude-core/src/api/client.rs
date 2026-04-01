use anyhow::Result;
use reqwest::Response;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::api::retry::{RetryDecision, RetryPolicy};
use crate::types::content::ContentBlock;
use crate::types::events::StreamEvent;

// ── Public types ──────────────────────────────────────────────────────────────

/// Authentication method for the Anthropic API.
#[derive(Clone, Debug)]
pub enum AuthMethod {
    /// A standard API key (x-api-key header).
    ApiKey(String),
    /// An OAuth bearer token (Authorization: Bearer header).
    OAuthToken(String),
}

impl AuthMethod {
    /// Return the `(header_name, header_value)` pair for this auth method.
    pub fn to_header(&self) -> (&'static str, String) {
        match self {
            AuthMethod::ApiKey(key) => ("x-api-key", key.clone()),
            AuthMethod::OAuthToken(token) => ("authorization", format!("Bearer {}", token)),
        }
    }

    /// Whether this auth method requires the OAuth beta header.
    pub fn is_oauth(&self) -> bool {
        matches!(self, AuthMethod::OAuthToken(_))
    }
}

/// Thinking / extended reasoning configuration.
#[derive(Clone, Debug, Default)]
pub enum ThinkingConfig {
    /// Disable extended thinking (default).
    #[default]
    Disabled,
    /// Enable extended thinking with a given token budget.
    Enabled { budget_tokens: u64 },
    /// Let the model decide adaptively.
    Adaptive,
}

/// Speed hint for the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Speed {
    /// Optimise for lower latency.
    Fast,
    /// Optimise for higher quality / throughput.
    Standard,
}

/// Configuration for a single API request / session.
#[derive(Clone, Debug)]
pub struct ApiConfig {
    /// Base URL for the Anthropic API.
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// Maximum number of output tokens.
    pub max_tokens: u64,
    /// Thinking / extended-reasoning configuration.
    pub thinking: ThinkingConfig,
    /// Optional speed hint.
    pub speed: Option<Speed>,
    /// Anthropic API version header value.
    pub api_version: String,
    /// Optional task budget token limit (enables task-budgets beta).
    pub task_budget: Option<u64>,
    /// Prompt caching breakpoint TTL in seconds (0 = disabled).
    pub cache_ttl: Option<u64>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".into(),
            model: "claude-sonnet-4-6".into(),
            max_tokens: 8000,
            thinking: ThinkingConfig::Disabled,
            speed: None,
            api_version: "2023-06-01".into(),
            task_budget: None,
            cache_ttl: None,
        }
    }
}

/// Rate-limit quota information extracted from response headers.
#[derive(Clone, Debug, Default)]
pub struct QuotaStatus {
    /// Remaining requests in the current window.
    pub requests_remaining: Option<u64>,
    /// Remaining tokens in the current window.
    pub tokens_remaining: Option<u64>,
    /// Unix timestamp (seconds) when the rate limit resets.
    pub reset_at: Option<f64>,
}

// ── Tool definition (for the request body) ───────────────────────────────────

/// A tool definition sent to the API.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

// ── Request body builder ──────────────────────────────────────────────────────

/// Build the JSON body for a `/v1/messages` streaming request.
///
/// Parameters:
/// - `config`   – request configuration.
/// - `messages` – conversation history as `(role, content_blocks)` pairs.
/// - `system`   – system prompt content blocks (may be empty).
/// - `tools`    – tool definitions (may be empty).
pub fn build_request_body(
    config: &ApiConfig,
    messages: &[Value],
    system: &[ContentBlock],
    tools: &[ToolDefinition],
) -> Value {
    let api_model = crate::utils::model::normalize_model_string_for_api(&config.model);
    let mut body = json!({
        "model": api_model,
        "max_tokens": config.max_tokens,
        "stream": true,
        "messages": messages,
    });

    // Thinking configuration.
    let thinking_obj = match &config.thinking {
        ThinkingConfig::Disabled => None,
        ThinkingConfig::Enabled { budget_tokens } => Some(json!({
            "type": "enabled",
            "budget_tokens": budget_tokens,
        })),
        ThinkingConfig::Adaptive => Some(json!({ "type": "adaptive" })),
    };
    if let Some(thinking) = thinking_obj {
        body["thinking"] = thinking;
    }

    // Optional speed hint.
    if let Some(speed) = &config.speed {
        body["speed"] = match speed {
            Speed::Fast => json!("fast"),
            Speed::Standard => json!("standard"),
        };
    }

    // System prompt (only include if non-empty).
    if !system.is_empty() {
        body["system"] = serde_json::to_value(system).unwrap_or(Value::Null);
    }

    // Tools (only include if non-empty).
    if !tools.is_empty() {
        body["tools"] = serde_json::to_value(tools).unwrap_or(Value::Null);
    }

    body
}

// ── Beta header constants ─────────────────────────────────────────────────────
// Matches the latest values from the original TypeScript constants/betas.ts.

const BETA_CLAUDE_CODE: &str = "claude-code-20250219";
const BETA_INTERLEAVED_THINKING: &str = "interleaved-thinking-2025-05-14";
const BETA_CONTEXT_MANAGEMENT: &str = "context-management-2025-06-27";
const BETA_PROMPT_CACHING_SCOPE: &str = "prompt-caching-scope-2026-01-05";
const BETA_STRUCTURED_OUTPUTS: &str = "structured-outputs-2025-12-15";
const BETA_REDACT_THINKING: &str = "redact-thinking-2026-02-12";
const BETA_FAST_MODE: &str = "fast-mode-2026-02-01";
const BETA_TASK_BUDGETS: &str = "task-budgets-2026-03-13";
const BETA_OAUTH: &str = "oauth-2025-04-20";

// ── API client ────────────────────────────────────────────────────────────────

/// A thin HTTP client for the Anthropic Messages API.
#[derive(Clone)]
pub struct ApiClient {
    pub config: ApiConfig,
    auth: AuthMethod,
    http: reqwest::Client,
}

impl ApiClient {
    /// Create a new `ApiClient` with the given configuration and auth method.
    pub fn new(config: ApiConfig, auth: AuthMethod) -> Self {
        Self {
            config,
            auth,
            http: reqwest::Client::new(),
        }
    }

    /// Build the comma-separated `anthropic-beta` header value.
    ///
    /// Includes every beta that the original TypeScript client sends for
    /// first-party API requests, conditional on the current configuration.
    fn build_beta_header(&self) -> String {
        let mut betas: Vec<&str> = vec![
            BETA_CLAUDE_CODE,
            BETA_INTERLEAVED_THINKING,
            BETA_CONTEXT_MANAGEMENT,
            BETA_PROMPT_CACHING_SCOPE,
            BETA_STRUCTURED_OUTPUTS,
            BETA_REDACT_THINKING,
        ];

        if self.config.speed == Some(Speed::Fast) {
            betas.push(BETA_FAST_MODE);
        }
        if self.config.task_budget.is_some() {
            betas.push(BETA_TASK_BUDGETS);
        }
        if self.auth.is_oauth() {
            betas.push(BETA_OAUTH);
        }

        betas.join(",")
    }

    /// Apply common headers shared by both streaming and non-streaming requests.
    fn apply_common_headers(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let (auth_header, auth_value) = self.auth.to_header();
        req = req
            .header("anthropic-version", &self.config.api_version)
            .header("content-type", "application/json")
            .header(auth_header, auth_value)
            .header("anthropic-beta", self.build_beta_header());

        // Prompt-cache control: set breakpoint TTL when configured.
        if let Some(ttl) = self.config.cache_ttl {
            if ttl > 0 {
                req = req.header(
                    "anthropic-cache-control",
                    format!("max-age={ttl}"),
                );
            }
        }

        req
    }

    /// Extract rate-limit quota information from response headers.
    pub fn extract_quota(response: &Response) -> QuotaStatus {
        let headers = response.headers();
        QuotaStatus {
            requests_remaining: headers
                .get("anthropic-ratelimit-requests-remaining")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok()),
            tokens_remaining: headers
                .get("anthropic-ratelimit-tokens-remaining")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok()),
            reset_at: headers
                .get("anthropic-ratelimit-requests-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok()),
        }
    }

    /// POST a non-streaming request to `/v1/messages` and return the parsed JSON.
    ///
    /// Used for operations like compaction summaries and as the 529 fallback.
    pub async fn send_request(
        &self,
        messages: &[Value],
        system: &[ContentBlock],
        tools: &[ToolDefinition],
    ) -> Result<Value> {
        let url = format!("{}/v1/messages?beta=true", self.config.base_url);
        let mut body = build_request_body(&self.config, messages, system, tools);
        body["stream"] = json!(false);

        let request = self.http.post(&url);
        let request = self.apply_common_headers(request);

        let response = request.json(&body).send().await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("API error {}: {}", status, &text[..text.len().min(500)]);
        }

        let json: Value = response.json().await?;
        Ok(json)
    }

    /// POST a streaming request to `/v1/messages` and return the raw response.
    ///
    /// The caller is responsible for reading the SSE stream from the response body.
    pub async fn stream_request(
        &self,
        messages: &[Value],
        system: &[ContentBlock],
        tools: &[ToolDefinition],
    ) -> Result<Response> {
        let url = format!("{}/v1/messages?beta=true", self.config.base_url);
        let body = build_request_body(&self.config, messages, system, tools);

        let request = self.http.post(&url);
        let request = self.apply_common_headers(request);

        let response = request.json(&body).send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(String::from);
            let text = response.text().await.unwrap_or_default();
            let truncated = &text[..text.len().min(500)];
            tracing::error!("API error {}: {}", status, truncated);
            if let Some(ra) = retry_after {
                anyhow::bail!("API error {}: {} [retry-after: {}]", status, truncated, ra);
            }
            anyhow::bail!("API error {}: {}", status, truncated);
        }

        Ok(response)
    }

    /// Streaming request with a full retry loop.
    ///
    /// Behaviour:
    /// - Retries 429 / 5xx according to `RetryPolicy` with exponential backoff.
    /// - Parses the `retry-after` header from error responses and honours it.
    /// - On 529 exhaustion, falls back to a single non-streaming request.
    /// - On network errors, retries with backoff.
    /// - Emits `StreamEvent::RetryWait` so the TUI can display retry status.
    ///
    /// Returns either a streaming `Response` or, on 529 fallback, a parsed
    /// `Value` from the non-streaming path wrapped in `StreamResponse`.
    pub async fn stream_request_with_retry(
        &self,
        messages: &[Value],
        system: &[ContentBlock],
        tools: &[ToolDefinition],
        retry_policy: &RetryPolicy,
        event_tx: &mpsc::Sender<StreamEvent>,
    ) -> Result<StreamResponse> {
        let url = format!("{}/v1/messages?beta=true", self.config.base_url);
        let body = build_request_body(&self.config, messages, system, tools);

        let mut attempt: u32 = 0;

        loop {
            let request = self.http.post(&url);
            let request = self.apply_common_headers(request);

            let result = request.json(&body).send().await;

            match result {
                Ok(response) if response.status().is_success() => {
                    return Ok(StreamResponse::Streaming(response));
                }
                Ok(response) => {
                    // HTTP error — extract status + retry-after before consuming body
                    let status = response.status().as_u16();
                    let retry_after = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .map(String::from);
                    let error_body = response.text().await.unwrap_or_default();
                    let truncated = &error_body[..error_body.len().min(500)];

                    if !RetryPolicy::is_retryable(status) {
                        anyhow::bail!("API error {}: {}", status, truncated);
                    }

                    attempt += 1;
                    let decision =
                        retry_policy.should_retry(status, attempt, retry_after.as_deref());

                    match decision {
                        RetryDecision::Retry { delay } => {
                            tracing::warn!(
                                status,
                                attempt,
                                delay_ms = delay.as_millis() as u64,
                                "retrying API request"
                            );
                            let _ = event_tx
                                .send(StreamEvent::RetryWait {
                                    attempt,
                                    delay_ms: delay.as_millis() as u64,
                                    status,
                                })
                                .await;
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        RetryDecision::FallbackToNonStreaming => {
                            tracing::warn!(
                                "529 retries exhausted after {attempt} attempts, \
                                 falling back to non-streaming request"
                            );
                            let _ = event_tx
                                .send(StreamEvent::RetryWait {
                                    attempt,
                                    delay_ms: 0,
                                    status: 529,
                                })
                                .await;

                            let value =
                                self.send_request(messages, system, tools).await?;
                            return Ok(StreamResponse::NonStreaming(value));
                        }
                        RetryDecision::Fatal { .. } => {
                            anyhow::bail!("API error {}: {}", status, truncated);
                        }
                    }
                }
                Err(network_err) => {
                    // Network / connection error — retry with backoff
                    attempt += 1;
                    if attempt > retry_policy.max_retries {
                        return Err(network_err.into());
                    }
                    let delay = retry_policy.backoff_delay(attempt);
                    tracing::warn!(
                        attempt,
                        delay_ms = delay.as_millis() as u64,
                        "network error, retrying: {network_err}"
                    );
                    let _ = event_tx
                        .send(StreamEvent::RetryWait {
                            attempt,
                            delay_ms: delay.as_millis() as u64,
                            status: 0,
                        })
                        .await;
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }
        }
    }
}

/// The result of `stream_request_with_retry`: either a live SSE stream or a
/// fully-parsed non-streaming response (used after 529 fallback).
pub enum StreamResponse {
    /// A streaming HTTP response whose body emits SSE events.
    Streaming(Response),
    /// A non-streaming JSON response (529 fallback path).
    NonStreaming(Value),
}
