//! Teleport API Client
//!
//! Handles communication with the Sessions API for session migration.
//! Includes upload/download of session bundles, session status checking,
//! and retry logic with exponential backoff.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};

/// Retry delays in milliseconds for transient network errors.
/// 4 retries with exponential backoff: 2s, 4s, 8s, 16s.
const RETRY_DELAYS: &[u64] = &[2000, 4000, 8000, 16000];

/// Maximum number of retries before giving up.
const MAX_RETRIES: usize = RETRY_DELAYS.len();

/// Beta header value for CCR BYOC API access.
pub const CCR_BYOC_BETA: &str = "ccr-byoc-2025-07-29";

// ---------------------------------------------------------------------------
// Types matching the Sessions API (api/schemas/sessions/sessions.py)
// ---------------------------------------------------------------------------

/// Session lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    RequiresAction,
    Running,
    Idle,
    Archived,
}

/// A git repository context source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSource {
    #[serde(rename = "type")]
    pub source_type: String, // always "git_repository"
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_unrestricted_git_push: Option<bool>,
}

/// A knowledge base context source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBaseSource {
    #[serde(rename = "type")]
    pub source_type: String, // always "knowledge_base"
    pub knowledge_base_id: String,
}

/// Union of context source types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionContextSource {
    #[serde(rename = "git_repository")]
    Git(GitSource),
    #[serde(rename = "knowledge_base")]
    KnowledgeBase(KnowledgeBaseSource),
}

/// Git info attached to an outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeGitInfo {
    #[serde(rename = "type")]
    pub git_type: String, // "github"
    pub repo: String,
    pub branches: Vec<String>,
}

/// A git repository outcome from a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRepositoryOutcome {
    #[serde(rename = "type")]
    pub outcome_type: String, // "git_repository"
    pub git_info: OutcomeGitInfo,
}

/// GitHub PR reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubPr {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

/// Full session context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    pub sources: Vec<SessionContextSource>,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcomes: Option<Vec<GitRepositoryOutcome>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_bundle_file_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_pr: Option<GitHubPr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reuse_outcome_branches: Option<bool>,
}

/// A session resource returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResource {
    #[serde(rename = "type")]
    pub resource_type: String, // "session"
    pub id: String,
    pub title: Option<String>,
    pub session_status: SessionStatus,
    pub environment_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub session_context: SessionContext,
}

/// Paginated list of sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSessionsResponse {
    pub data: Vec<SessionResource>,
    pub has_more: bool,
    pub first_id: Option<String>,
    pub last_id: Option<String>,
}

/// Content for a remote session message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RemoteMessageContent {
    /// Plain text content.
    Text(String),
    /// Array of content blocks (text, image, etc.).
    Blocks(Vec<serde_json::Value>),
}

/// Event sent to a remote session.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionEvent {
    uuid: String,
    session_id: String,
    #[serde(rename = "type")]
    event_type: String,
    parent_tool_use_id: Option<String>,
    message: SessionEventMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionEventMessage {
    role: String,
    content: RemoteMessageContent,
}

#[derive(Debug, Clone, Serialize)]
struct SendEventsRequest {
    events: Vec<SessionEvent>,
}

// ---------------------------------------------------------------------------
// API client
// ---------------------------------------------------------------------------

/// Configuration for the Teleport API client.
#[derive(Debug, Clone)]
pub struct TeleportApiConfig {
    /// Base API URL (e.g. "https://api.anthropic.com").
    pub base_url: String,
    /// OAuth access token.
    pub access_token: String,
    /// Organization UUID.
    pub org_uuid: String,
}

/// Construct standard OAuth headers for API requests.
fn oauth_headers(access_token: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("Authorization".into(), format!("Bearer {access_token}"));
    headers.insert("Content-Type".into(), "application/json".into());
    headers.insert("anthropic-version".into(), "2023-06-01".into());
    headers
}

/// Check if an HTTP error is transient and should be retried.
pub fn is_transient_error(status: Option<StatusCode>) -> bool {
    match status {
        // No response received -- network error
        None => true,
        // Server errors (5xx) are transient
        Some(s) if s.is_server_error() => true,
        // Client errors (4xx) are not transient
        _ => false,
    }
}

/// Make a GET request with automatic retry for transient network errors.
/// Uses exponential backoff: 2s, 4s, 8s, 16s (4 retries = 5 total attempts).
async fn get_with_retry(client: &Client, url: &str, headers: &HashMap<String, String>) -> Result<Response> {
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..=MAX_RETRIES {
        let mut builder = client.get(url);
        for (k, v) in headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        match builder.send().await {
            Ok(resp) => {
                if resp.status().is_success() || resp.status().is_client_error() {
                    return Ok(resp);
                }
                // Server error -- may be transient
                if attempt >= MAX_RETRIES {
                    bail!(
                        "Request failed after {} attempts: {} {}",
                        attempt + 1,
                        resp.status().as_u16(),
                        resp.status().canonical_reason().unwrap_or("Unknown")
                    );
                }
                last_error = Some(anyhow::anyhow!(
                    "Server error: {}",
                    resp.status().as_u16()
                ));
            }
            Err(e) => {
                if attempt >= MAX_RETRIES {
                    return Err(e).context(format!(
                        "Request failed after {} attempts",
                        attempt + 1
                    ));
                }
                last_error = Some(e.into());
            }
        }

        let delay = RETRY_DELAYS.get(attempt).copied().unwrap_or(2000);
        tracing::debug!(
            "Teleport request failed (attempt {}/{}), retrying in {}ms",
            attempt + 1,
            MAX_RETRIES + 1,
            delay
        );
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Request failed")))
}

/// Fetch the list of sessions from the Sessions API.
pub async fn fetch_sessions(config: &TeleportApiConfig) -> Result<Vec<SessionResource>> {
    let client = Client::new();
    let url = format!("{}/v1/sessions", config.base_url);

    let mut headers = oauth_headers(&config.access_token);
    headers.insert("anthropic-beta".into(), CCR_BYOC_BETA.into());
    headers.insert("x-organization-uuid".into(), config.org_uuid.clone());

    let resp = get_with_retry(&client, &url, &headers).await?;
    let status = resp.status();

    if !status.is_success() {
        bail!("Failed to fetch sessions: {}", status);
    }

    let list: ListSessionsResponse = resp
        .json()
        .await
        .context("Failed to parse sessions response")?;

    Ok(list.data)
}

/// Fetch a single session by ID.
pub async fn fetch_session(config: &TeleportApiConfig, session_id: &str) -> Result<SessionResource> {
    let client = Client::new();
    let url = format!("{}/v1/sessions/{}", config.base_url, session_id);

    let mut headers = oauth_headers(&config.access_token);
    headers.insert("anthropic-beta".into(), CCR_BYOC_BETA.into());
    headers.insert("x-organization-uuid".into(), config.org_uuid.clone());

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.access_token))
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", CCR_BYOC_BETA)
        .header("x-organization-uuid", &config.org_uuid)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .context("Failed to send session fetch request")?;

    let status = resp.status();
    if status == StatusCode::NOT_FOUND {
        bail!("Session not found: {session_id}");
    }
    if status == StatusCode::UNAUTHORIZED {
        bail!("Session expired. Please run /login to sign in again.");
    }
    if !status.is_success() {
        bail!("Failed to fetch session: {status}");
    }

    resp.json()
        .await
        .context("Failed to parse session response")
}

/// Extract the first branch name from a session's git repository outcomes.
pub fn get_branch_from_session(session: &SessionResource) -> Option<&str> {
    session
        .session_context
        .outcomes
        .as_ref()?
        .iter()
        .find(|o| o.outcome_type == "git_repository")
        .and_then(|o| o.git_info.branches.first())
        .map(|s| s.as_str())
}

/// Send a user message event to an existing remote session.
pub async fn send_event_to_session(
    config: &TeleportApiConfig,
    session_id: &str,
    content: RemoteMessageContent,
    event_uuid: Option<String>,
) -> Result<bool> {
    let client = Client::new();
    let url = format!("{}/v1/sessions/{}/events", config.base_url, session_id);

    let event = SessionEvent {
        uuid: event_uuid.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        session_id: session_id.to_string(),
        event_type: "user".into(),
        parent_tool_use_id: None,
        message: SessionEventMessage {
            role: "user".into(),
            content,
        },
    };

    let body = SendEventsRequest {
        events: vec![event],
    };

    tracing::debug!("Sending event to session {session_id}");

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.access_token))
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", CCR_BYOC_BETA)
        .header("x-organization-uuid", &config.org_uuid)
        .timeout(Duration::from_secs(30))
        .json(&body)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            tracing::debug!("Successfully sent event to session {session_id}");
            Ok(true)
        }
        Ok(r) => {
            tracing::debug!(
                "Failed to send event to session {session_id}: {}",
                r.status()
            );
            Ok(false)
        }
        Err(e) => {
            tracing::debug!("Error sending event to session {session_id}: {e}");
            Ok(false)
        }
    }
}

/// Update the title of an existing remote session.
pub async fn update_session_title(
    config: &TeleportApiConfig,
    session_id: &str,
    title: &str,
) -> Result<bool> {
    let client = Client::new();
    let url = format!("{}/v1/sessions/{}", config.base_url, session_id);

    tracing::debug!("Updating title for session {session_id}: \"{title}\"");

    let resp = client
        .patch(&url)
        .header("Authorization", format!("Bearer {}", config.access_token))
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", CCR_BYOC_BETA)
        .header("x-organization-uuid", &config.org_uuid)
        .json(&serde_json::json!({ "title": title }))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            tracing::debug!("Successfully updated title for session {session_id}");
            Ok(true)
        }
        Ok(r) => {
            tracing::debug!(
                "Failed to update title for session {session_id}: {}",
                r.status()
            );
            Ok(false)
        }
        Err(e) => {
            tracing::debug!("Error updating session title: {e}");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_headers_are_correct() {
        let h = oauth_headers("test-token");
        assert_eq!(h.get("Authorization").unwrap(), "Bearer test-token");
        assert_eq!(h.get("Content-Type").unwrap(), "application/json");
        assert_eq!(h.get("anthropic-version").unwrap(), "2023-06-01");
    }

    #[test]
    fn transient_error_detection() {
        assert!(is_transient_error(None));
        assert!(is_transient_error(Some(StatusCode::INTERNAL_SERVER_ERROR)));
        assert!(is_transient_error(Some(StatusCode::BAD_GATEWAY)));
        assert!(is_transient_error(Some(StatusCode::SERVICE_UNAVAILABLE)));
        assert!(!is_transient_error(Some(StatusCode::BAD_REQUEST)));
        assert!(!is_transient_error(Some(StatusCode::NOT_FOUND)));
        assert!(!is_transient_error(Some(StatusCode::OK)));
    }

    #[test]
    fn branch_extraction_from_session() {
        let session = SessionResource {
            resource_type: "session".into(),
            id: "test-id".into(),
            title: Some("Test".into()),
            session_status: SessionStatus::Idle,
            environment_id: "env-1".into(),
            created_at: "2025-01-01".into(),
            updated_at: "2025-01-01".into(),
            session_context: SessionContext {
                sources: vec![],
                cwd: "/home/user".into(),
                outcomes: Some(vec![GitRepositoryOutcome {
                    outcome_type: "git_repository".into(),
                    git_info: OutcomeGitInfo {
                        git_type: "github".into(),
                        repo: "owner/repo".into(),
                        branches: vec!["feature-branch".into()],
                    },
                }]),
                custom_system_prompt: None,
                append_system_prompt: None,
                model: None,
                seed_bundle_file_id: None,
                github_pr: None,
                reuse_outcome_branches: None,
            },
        };

        assert_eq!(
            get_branch_from_session(&session),
            Some("feature-branch")
        );
    }

    #[test]
    fn branch_extraction_empty_outcomes() {
        let session = SessionResource {
            resource_type: "session".into(),
            id: "test-id".into(),
            title: None,
            session_status: SessionStatus::Idle,
            environment_id: "env-1".into(),
            created_at: "2025-01-01".into(),
            updated_at: "2025-01-01".into(),
            session_context: SessionContext {
                sources: vec![],
                cwd: "/home/user".into(),
                outcomes: None,
                custom_system_prompt: None,
                append_system_prompt: None,
                model: None,
                seed_bundle_file_id: None,
                github_pr: None,
                reuse_outcome_branches: None,
            },
        };

        assert_eq!(get_branch_from_session(&session), None);
    }
}
