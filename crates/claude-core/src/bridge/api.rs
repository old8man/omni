//! Bridge API client for the Anthropic environments API.
//!
//! Implements the HTTP interface between a local bridge instance and the
//! Anthropic backend: environment registration, work polling, acknowledgement,
//! heartbeats, session archival, and deregistration.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::json;

use super::types::{
    BridgeConfig, EnvironmentRegistration, HeartbeatResponse, PermissionResponseEvent,
    WorkResponse, BRIDGE_LOGIN_INSTRUCTION,
};

/// Anthropic API beta header for the environments protocol.
const BETA_HEADER: &str = "environments-2025-11-01";

/// Request timeout for all bridge API calls.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Poll timeout (slightly shorter to allow for network overhead).
const POLL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Allowlist pattern for server-provided IDs used in URL path segments.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Validate that a server-provided ID is safe to interpolate into a URL path.
///
/// Returns the ID on success, or an error if it contains unsafe characters.
pub fn validate_bridge_id<'a>(id: &'a str, label: &str) -> Result<&'a str> {
    if is_safe_id(id) {
        Ok(id)
    } else {
        bail!("Invalid {label}: contains unsafe characters")
    }
}

/// Fatal bridge errors that should not be retried (e.g. auth failures).
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct BridgeFatalError {
    /// Human-readable error message.
    pub message: String,
    /// HTTP status code that triggered the error.
    pub status: u16,
    /// Server-provided error type, e.g. `"environment_expired"`.
    pub error_type: Option<String>,
}

/// Check whether an error type string indicates session/environment expiry.
pub fn is_expired_error_type(error_type: Option<&str>) -> bool {
    match error_type {
        Some(t) => t.contains("expired") || t.contains("lifetime"),
        None => false,
    }
}

/// Callback for handling 401 responses by refreshing the OAuth token.
pub type OnAuth401Fn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Bridge API client for the Anthropic environments API.
pub struct BridgeApiClient {
    http: reqwest::Client,
    base_url: String,
    get_access_token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    runner_version: String,
    on_auth_401: Option<OnAuth401Fn>,
}

impl BridgeApiClient {
    /// Create a new bridge API client.
    ///
    /// # Arguments
    /// * `base_url` - API base URL (e.g. `https://api.anthropic.com`)
    /// * `get_access_token` - Closure returning the current OAuth access token
    /// * `runner_version` - Version string for the `x-environment-runner-version` header
    pub fn new(
        base_url: String,
        get_access_token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        runner_version: String,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("failed to build reqwest client")?;
        Ok(Self {
            http,
            base_url,
            get_access_token,
            runner_version,
            on_auth_401: None,
        })
    }

    /// Set a callback invoked on 401 to attempt OAuth token refresh.
    pub fn set_on_auth_401(&mut self, handler: OnAuth401Fn) {
        self.on_auth_401 = Some(handler);
    }

    /// Resolve the current access token, returning an error if none is available.
    fn resolve_auth(&self) -> Result<String> {
        (self.get_access_token)().ok_or_else(|| anyhow::anyhow!("{}", BRIDGE_LOGIN_INSTRUCTION))
    }

    /// Build standard headers for an authenticated request.
    fn headers(&self, token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .expect("token contains invalid header chars"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("anthropic-beta", HeaderValue::from_static(BETA_HEADER));
        headers.insert(
            "x-environment-runner-version",
            HeaderValue::from_str(&self.runner_version)
                .unwrap_or_else(|_| HeaderValue::from_static("unknown")),
        );
        headers
    }

    /// Register a bridge environment with the backend.
    ///
    /// Returns the server-assigned environment ID and secret.
    pub async fn register_environment(
        &self,
        config: &BridgeConfig,
    ) -> Result<EnvironmentRegistration> {
        let token = self.resolve_auth()?;
        let mut body = json!({
            "machine_name": config.machine_name,
            "directory": config.dir,
            "branch": config.branch,
            "git_repo_url": config.git_repo_url,
            "max_sessions": config.max_sessions,
            "metadata": { "worker_type": config.worker_type },
        });
        if let Some(reuse_id) = &config.reuse_environment_id {
            body["environment_id"] = json!(reuse_id);
        }

        let resp = self
            .http
            .post(format!("{}/v1/environments/bridge", self.base_url))
            .headers(self.headers(&token))
            .json(&body)
            .send()
            .await
            .context("bridge registration request failed")?;

        let status = resp.status().as_u16();
        if status >= 500 {
            bail!("Registration: server error ({})", status);
        }
        let data: serde_json::Value = resp.json().await.context("failed to read response body")?;
        handle_error_status(status, &data, "Registration")?;

        serde_json::from_value(data).context("failed to parse registration response")
    }

    /// Poll for available work items.
    ///
    /// Returns `None` when no work is available (204 or empty body).
    pub async fn poll_for_work(
        &self,
        environment_id: &str,
        environment_secret: &str,
        reclaim_older_than_ms: Option<u64>,
    ) -> Result<Option<WorkResponse>> {
        validate_bridge_id(environment_id, "environmentId")?;

        let mut url = format!(
            "{}/v1/environments/{}/work/poll",
            self.base_url, environment_id
        );
        if let Some(ms) = reclaim_older_than_ms {
            url.push_str(&format!("?reclaim_older_than_ms={ms}"));
        }

        let resp = self
            .http
            .get(&url)
            .headers(self.headers(environment_secret))
            .timeout(POLL_TIMEOUT)
            .send()
            .await
            .context("poll request failed")?;

        let status = resp.status().as_u16();
        if status >= 500 {
            bail!("Poll: server error ({})", status);
        }

        let text = resp.text().await.context("failed to read poll response")?;
        if text.is_empty() || status == 204 {
            return Ok(None);
        }

        let data: serde_json::Value =
            serde_json::from_str(&text).context("failed to parse poll response")?;
        handle_error_status(status, &data, "Poll")?;

        // null response also means no work
        if data.is_null() {
            return Ok(None);
        }

        let work: WorkResponse =
            serde_json::from_value(data).context("failed to parse work response")?;
        Ok(Some(work))
    }

    /// Acknowledge a work item, confirming the bridge will handle it.
    pub async fn acknowledge_work(
        &self,
        environment_id: &str,
        work_id: &str,
        session_token: &str,
    ) -> Result<()> {
        validate_bridge_id(environment_id, "environmentId")?;
        validate_bridge_id(work_id, "workId")?;

        let resp = self
            .http
            .post(format!(
                "{}/v1/environments/{}/work/{}/ack",
                self.base_url, environment_id, work_id
            ))
            .headers(self.headers(session_token))
            .json(&json!({}))
            .send()
            .await
            .context("acknowledge request failed")?;

        let status = resp.status().as_u16();
        if status >= 500 {
            bail!("Acknowledge: server error ({})", status);
        }
        let data: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        handle_error_status(status, &data, "Acknowledge")
    }

    /// Stop a work item.
    pub async fn stop_work(&self, environment_id: &str, work_id: &str, force: bool) -> Result<()> {
        validate_bridge_id(environment_id, "environmentId")?;
        validate_bridge_id(work_id, "workId")?;

        let token = self.resolve_auth()?;
        let resp = self
            .http
            .post(format!(
                "{}/v1/environments/{}/work/{}/stop",
                self.base_url, environment_id, work_id
            ))
            .headers(self.headers(&token))
            .json(&json!({ "force": force }))
            .send()
            .await
            .context("stop work request failed")?;

        let status = resp.status().as_u16();
        if status >= 500 {
            bail!("StopWork: server error ({})", status);
        }
        let data: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        handle_error_status(status, &data, "StopWork")
    }

    /// Deregister/delete the bridge environment on graceful shutdown.
    pub async fn deregister_environment(&self, environment_id: &str) -> Result<()> {
        validate_bridge_id(environment_id, "environmentId")?;

        let token = self.resolve_auth()?;
        let resp = self
            .http
            .delete(format!(
                "{}/v1/environments/bridge/{}",
                self.base_url, environment_id
            ))
            .headers(self.headers(&token))
            .send()
            .await
            .context("deregister request failed")?;

        let status = resp.status().as_u16();
        if status >= 500 {
            bail!("Deregister: server error ({})", status);
        }
        let data: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        handle_error_status(status, &data, "Deregister")
    }

    /// Send a heartbeat for an active work item, extending its lease.
    pub async fn heartbeat_work(
        &self,
        environment_id: &str,
        work_id: &str,
        session_token: &str,
    ) -> Result<HeartbeatResponse> {
        validate_bridge_id(environment_id, "environmentId")?;
        validate_bridge_id(work_id, "workId")?;

        let resp = self
            .http
            .post(format!(
                "{}/v1/environments/{}/work/{}/heartbeat",
                self.base_url, environment_id, work_id
            ))
            .headers(self.headers(session_token))
            .json(&json!({}))
            .send()
            .await
            .context("heartbeat request failed")?;

        let status = resp.status().as_u16();
        if status >= 500 {
            bail!("Heartbeat: server error ({})", status);
        }
        let data: serde_json::Value = resp.json().await.context("failed to read heartbeat body")?;
        handle_error_status(status, &data, "Heartbeat")?;
        serde_json::from_value(data).context("failed to parse heartbeat response")
    }

    /// Send a permission response event to a session.
    pub async fn send_permission_response_event(
        &self,
        session_id: &str,
        event: &PermissionResponseEvent,
        session_token: &str,
    ) -> Result<()> {
        validate_bridge_id(session_id, "sessionId")?;

        let resp = self
            .http
            .post(format!(
                "{}/v1/sessions/{}/events",
                self.base_url, session_id
            ))
            .headers(self.headers(session_token))
            .json(&json!({ "events": [event] }))
            .send()
            .await
            .context("send permission response request failed")?;

        let status = resp.status().as_u16();
        if status >= 500 {
            bail!("SendPermissionResponseEvent: server error ({})", status);
        }
        let data: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        handle_error_status(status, &data, "SendPermissionResponseEvent")
    }

    /// Archive a session so it no longer appears as active on the server.
    pub async fn archive_session(&self, session_id: &str) -> Result<()> {
        validate_bridge_id(session_id, "sessionId")?;

        let token = self.resolve_auth()?;
        let resp = self
            .http
            .post(format!(
                "{}/v1/sessions/{}/archive",
                self.base_url, session_id
            ))
            .headers(self.headers(&token))
            .json(&json!({}))
            .send()
            .await
            .context("archive session request failed")?;

        let status = resp.status().as_u16();
        // 409 = already archived (idempotent, not an error)
        if status == 409 {
            return Ok(());
        }
        if status >= 500 {
            bail!("ArchiveSession: server error ({})", status);
        }
        let data: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        handle_error_status(status, &data, "ArchiveSession")
    }

    /// Force-stop stale workers and re-queue a session on an environment.
    pub async fn reconnect_session(&self, environment_id: &str, session_id: &str) -> Result<()> {
        validate_bridge_id(environment_id, "environmentId")?;
        validate_bridge_id(session_id, "sessionId")?;

        let token = self.resolve_auth()?;
        let resp = self
            .http
            .post(format!(
                "{}/v1/environments/{}/bridge/reconnect",
                self.base_url, environment_id
            ))
            .headers(self.headers(&token))
            .json(&json!({ "session_id": session_id }))
            .send()
            .await
            .context("reconnect session request failed")?;

        let status = resp.status().as_u16();
        if status >= 500 {
            bail!("ReconnectSession: server error ({})", status);
        }
        let data: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        handle_error_status(status, &data, "ReconnectSession")
    }
}

/// Map HTTP error status codes to appropriate error types.
fn handle_error_status(status: u16, data: &serde_json::Value, context: &str) -> Result<()> {
    if status == 200 || status == 204 {
        return Ok(());
    }

    let detail = extract_error_detail(data);
    let error_type = extract_error_type(data);

    match status {
        401 => Err(BridgeFatalError {
            message: format!(
                "{context}: Authentication failed (401){}. {BRIDGE_LOGIN_INSTRUCTION}",
                detail
                    .as_ref()
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default()
            ),
            status: 401,
            error_type,
        }
        .into()),
        403 => {
            let msg = if is_expired_error_type(error_type.as_deref()) {
                "Remote Control session has expired. Please restart with `claude remote-control` or /remote-control.".to_string()
            } else {
                format!(
                    "{context}: Access denied (403){}. Check your organization permissions.",
                    detail.as_ref().map(|d| format!(": {d}")).unwrap_or_default()
                )
            };
            Err(BridgeFatalError {
                message: msg,
                status: 403,
                error_type,
            }
            .into())
        }
        404 => Err(BridgeFatalError {
            message: detail.unwrap_or_else(|| {
                format!(
                    "{context}: Not found (404). Remote Control may not be available for this organization."
                )
            }),
            status: 404,
            error_type,
        }
        .into()),
        410 => Err(BridgeFatalError {
            message: detail.unwrap_or_else(|| {
                "Remote Control session has expired. Please restart with `claude remote-control` or /remote-control.".to_string()
            }),
            status: 410,
            error_type: error_type.or_else(|| Some("environment_expired".to_string())),
        }
        .into()),
        429 => bail!("{context}: Rate limited (429). Polling too frequently."),
        _ => bail!(
            "{context}: Failed with status {status}{}",
            detail.as_ref().map(|d| format!(": {d}")).unwrap_or_default()
        ),
    }
}

/// Extract a human-readable error detail from a response body.
fn extract_error_detail(data: &serde_json::Value) -> Option<String> {
    // Try { error: { message: "..." } }
    if let Some(msg) = data
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return Some(msg.to_string());
    }
    // Try { error: "..." }
    if let Some(msg) = data.get("error").and_then(|e| e.as_str()) {
        return Some(msg.to_string());
    }
    // Try { message: "..." }
    data.get("message")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
}

/// Extract the `error.type` field from a response body.
fn extract_error_type(data: &serde_json::Value) -> Option<String> {
    data.get("error")
        .and_then(|e| e.get("type"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_bridge_id_valid() {
        assert!(validate_bridge_id("abc-123_DEF", "test").is_ok());
    }

    #[test]
    fn test_validate_bridge_id_invalid() {
        assert!(validate_bridge_id("../../admin", "test").is_err());
        assert!(validate_bridge_id("", "test").is_err());
        assert!(validate_bridge_id("has space", "test").is_err());
        assert!(validate_bridge_id("has/slash", "test").is_err());
    }

    #[test]
    fn test_handle_error_status_success() {
        let data = json!({});
        assert!(handle_error_status(200, &data, "Test").is_ok());
        assert!(handle_error_status(204, &data, "Test").is_ok());
    }

    #[test]
    fn test_handle_error_status_401() {
        let data = json!({"error": {"message": "bad token"}});
        let err = handle_error_status(401, &data, "Test").unwrap_err();
        assert!(err.to_string().contains("Authentication failed"));
    }

    #[test]
    fn test_handle_error_status_410() {
        let data = json!({"error": {"type": "environment_expired"}});
        let err = handle_error_status(410, &data, "Test").unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[test]
    fn test_extract_error_detail_nested() {
        let data = json!({"error": {"message": "something went wrong"}});
        assert_eq!(
            extract_error_detail(&data),
            Some("something went wrong".to_string())
        );
    }

    #[test]
    fn test_extract_error_detail_flat() {
        let data = json!({"error": "simple error"});
        assert_eq!(
            extract_error_detail(&data),
            Some("simple error".to_string())
        );
    }

    #[test]
    fn test_is_expired_error_type() {
        assert!(is_expired_error_type(Some("environment_expired")));
        assert!(is_expired_error_type(Some("session_lifetime_exceeded")));
        assert!(!is_expired_error_type(Some("not_found")));
        assert!(!is_expired_error_type(None));
    }
}
