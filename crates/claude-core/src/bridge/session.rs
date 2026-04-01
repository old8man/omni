//! Bridge session creation and management.
//!
//! Provides functions to create, fetch, archive, and update sessions on
//! the Anthropic backend via the sessions API. These are used by the
//! bridge to manage remote sessions tied to an environment.

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::json;

/// Beta header required for the sessions API.
const SESSIONS_BETA_HEADER: &str = "ccr-byoc-2025-07-29";

/// Request timeout for session API calls.
const SESSION_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Options for creating a bridge session.
pub struct CreateSessionOptions {
    /// Environment ID the session belongs to.
    pub environment_id: String,
    /// Optional session title.
    pub title: Option<String>,
    /// Pre-populated events (conversation history).
    pub events: Vec<serde_json::Value>,
    /// Git remote URL.
    pub git_repo_url: Option<String>,
    /// Current git branch.
    pub branch: String,
    /// API base URL override.
    pub base_url: String,
    /// OAuth access token.
    pub access_token: String,
    /// Organization UUID.
    pub org_uuid: String,
    /// Model to use for the session.
    pub model: Option<String>,
    /// Permission mode (e.g. "default", "plan").
    pub permission_mode: Option<String>,
}

/// Response from fetching a bridge session.
#[derive(Clone, Debug, Deserialize)]
pub struct BridgeSessionInfo {
    /// Environment ID this session is associated with.
    #[serde(default)]
    pub environment_id: Option<String>,
    /// Session title.
    #[serde(default)]
    pub title: Option<String>,
}

/// Build standard headers for session API calls.
fn session_headers(access_token: &str, org_uuid: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}"))
            .expect("token contains invalid header chars"),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert(
        "anthropic-beta",
        HeaderValue::from_static(SESSIONS_BETA_HEADER),
    );
    headers.insert(
        "x-organization-uuid",
        HeaderValue::from_str(org_uuid).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers
}

/// Create a session on a bridge environment via `POST /v1/sessions`.
///
/// Returns the session ID on success, or `None` if creation fails.
pub async fn create_bridge_session(opts: &CreateSessionOptions) -> Result<Option<String>> {
    let http = reqwest::Client::builder()
        .timeout(SESSION_REQUEST_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;

    // Build git source/outcome context
    let (sources, outcomes) = build_git_context(&opts.git_repo_url, &opts.branch);

    let mut body = json!({
        "events": opts.events,
        "session_context": {
            "sources": sources,
            "outcomes": outcomes,
            "model": opts.model,
        },
        "environment_id": opts.environment_id,
        "source": "remote-control",
    });
    if let Some(title) = &opts.title {
        body["title"] = json!(title);
    }
    if let Some(mode) = &opts.permission_mode {
        body["permission_mode"] = json!(mode);
    }

    let url = format!("{}/v1/sessions", opts.base_url);
    let headers = session_headers(&opts.access_token, &opts.org_uuid);

    let resp = http
        .post(&url)
        .headers(headers)
        .json(&body)
        .send()
        .await
        .context("session creation request failed")?;

    let status = resp.status().as_u16();
    if status != 200 && status != 201 {
        let text = resp.text().await.unwrap_or_default();
        tracing::debug!("Session creation failed with status {status}: {text}");
        return Ok(None);
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse session response")?;
    let session_id = data
        .get("id")
        .and_then(|id| id.as_str())
        .map(|s| s.to_string());

    Ok(session_id)
}

/// Fetch a bridge session via `GET /v1/sessions/{id}`.
///
/// Returns the session's environment_id and title, or `None` on failure.
pub async fn get_bridge_session(
    session_id: &str,
    base_url: &str,
    access_token: &str,
    org_uuid: &str,
) -> Result<Option<BridgeSessionInfo>> {
    let http = reqwest::Client::builder()
        .timeout(SESSION_REQUEST_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;

    let url = format!("{base_url}/v1/sessions/{session_id}");
    let headers = session_headers(access_token, org_uuid);

    let resp = http
        .get(&url)
        .headers(headers)
        .send()
        .await
        .context("session fetch request failed")?;

    if resp.status().as_u16() != 200 {
        return Ok(None);
    }

    let info: BridgeSessionInfo = resp
        .json()
        .await
        .context("failed to parse session fetch response")?;
    Ok(Some(info))
}

/// Archive a bridge session via `POST /v1/sessions/{id}/archive`.
///
/// Best-effort — returns `Ok(())` even if already archived (409).
pub async fn archive_bridge_session(
    session_id: &str,
    base_url: &str,
    access_token: &str,
    org_uuid: &str,
) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(SESSION_REQUEST_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;

    let url = format!("{base_url}/v1/sessions/{session_id}/archive");
    let headers = session_headers(access_token, org_uuid);

    let resp = http
        .post(&url)
        .headers(headers)
        .json(&json!({}))
        .send()
        .await
        .context("session archive request failed")?;

    let status = resp.status().as_u16();
    if status == 200 || status == 409 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_default();
        tracing::debug!("Session archive failed with status {status}: {text}");
        Ok(()) // Best-effort
    }
}

/// Update a bridge session title via `PATCH /v1/sessions/{id}`.
///
/// Best-effort — errors are logged but not propagated.
pub async fn update_bridge_session_title(
    session_id: &str,
    title: &str,
    base_url: &str,
    access_token: &str,
    org_uuid: &str,
) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(SESSION_REQUEST_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;

    let url = format!("{base_url}/v1/sessions/{session_id}");
    let headers = session_headers(access_token, org_uuid);

    let resp = http
        .patch(&url)
        .headers(headers)
        .json(&json!({ "title": title }))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            tracing::debug!("Session title updated: {session_id} -> {title}");
        }
        Ok(r) => {
            tracing::debug!(
                "Session title update failed with status {}",
                r.status().as_u16()
            );
        }
        Err(e) => {
            tracing::debug!("Session title update request failed: {e}");
        }
    }

    Ok(())
}

/// Build git source and outcome arrays from a repo URL and branch.
fn build_git_context(
    git_repo_url: &Option<String>,
    branch: &str,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let Some(repo_url) = git_repo_url else {
        return (Vec::new(), Vec::new());
    };

    // Try to extract owner/repo from various URL formats
    let owner_repo = extract_owner_repo(repo_url);
    let Some((owner, repo)) = owner_repo else {
        return (Vec::new(), Vec::new());
    };

    let revision = if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    };

    let source = json!({
        "type": "git_repository",
        "url": format!("https://github.com/{owner}/{repo}"),
        "revision": revision,
    });

    let outcome = json!({
        "type": "git_repository",
        "git_info": {
            "type": "github",
            "repo": format!("{owner}/{repo}"),
            "branches": [format!("claude/{}", if branch.is_empty() { "task" } else { branch })],
        }
    });

    (vec![source], vec![outcome])
}

/// Extract `(owner, repo)` from a git URL.
///
/// Handles HTTPS URLs like `https://github.com/owner/repo.git` and
/// SSH URLs like `git@github.com:owner/repo.git`.
fn extract_owner_repo(url: &str) -> Option<(String, String)> {
    // HTTPS format
    if let Some(path) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        let path = path.trim_end_matches(".git");
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        if parts.len() >= 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // SSH format
    if let Some(path) = url.strip_prefix("git@github.com:") {
        let path = path.trim_end_matches(".git");
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        if parts.len() >= 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // owner/repo format
    let parts: Vec<&str> = url.splitn(3, '/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() && !parts[0].contains(':') {
        return Some((parts[0].to_string(), parts[1].to_string()));
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_owner_repo_https() {
        let result = extract_owner_repo("https://github.com/anthropics/claude-code.git");
        assert_eq!(
            result,
            Some(("anthropics".to_string(), "claude-code".to_string()))
        );
    }

    #[test]
    fn test_extract_owner_repo_ssh() {
        let result = extract_owner_repo("git@github.com:anthropics/claude-code.git");
        assert_eq!(
            result,
            Some(("anthropics".to_string(), "claude-code".to_string()))
        );
    }

    #[test]
    fn test_extract_owner_repo_slug() {
        let result = extract_owner_repo("anthropics/claude-code");
        assert_eq!(
            result,
            Some(("anthropics".to_string(), "claude-code".to_string()))
        );
    }

    #[test]
    fn test_extract_owner_repo_invalid() {
        assert!(extract_owner_repo("not-a-repo").is_none());
        assert!(extract_owner_repo("").is_none());
    }

    #[test]
    fn test_build_git_context_with_repo() {
        let (sources, outcomes) = build_git_context(
            &Some("https://github.com/anthropics/claude-code".to_string()),
            "main",
        );
        assert_eq!(sources.len(), 1);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            sources[0]["url"],
            "https://github.com/anthropics/claude-code"
        );
    }

    #[test]
    fn test_build_git_context_without_repo() {
        let (sources, outcomes) = build_git_context(&None, "main");
        assert!(sources.is_empty());
        assert!(outcomes.is_empty());
    }
}
