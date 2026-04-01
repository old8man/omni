//! Work secret encoding, decoding, and session URL construction.
//!
//! Handles the base64url-encoded work secret from poll responses,
//! WebSocket/SSE URL construction, and session ID comparison across
//! tagged-ID formats (e.g. `session_*` vs `cse_*`).

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use super::types::WorkSecret;

/// Decode a base64url-encoded work secret and validate its version.
///
/// The secret must have `version: 1` and non-empty `session_ingress_token`
/// and `api_base_url` fields.
pub fn decode_work_secret(secret: &str) -> Result<WorkSecret> {
    let bytes = URL_SAFE_NO_PAD
        .decode(secret)
        .context("failed to base64url-decode work secret")?;
    let parsed: WorkSecret =
        serde_json::from_slice(&bytes).context("failed to parse work secret JSON")?;

    if parsed.version != 1 {
        bail!(
            "Unsupported work secret version: {}",
            parsed.version
        );
    }
    if parsed.session_ingress_token.is_empty() {
        bail!("Invalid work secret: missing or empty session_ingress_token");
    }

    Ok(parsed)
}

/// Build a WebSocket SDK URL from the API base URL and session ID.
///
/// Strips the HTTP(S) protocol and constructs a ws(s):// ingress URL.
/// Uses /v2/ for localhost (direct to session-ingress, no Envoy rewrite)
/// and /v1/ for production (Envoy rewrites /v1/ -> /v2/).
pub fn build_sdk_url(api_base_url: &str, session_id: &str) -> String {
    let is_localhost =
        api_base_url.contains("localhost") || api_base_url.contains("127.0.0.1");
    let protocol = if is_localhost { "ws" } else { "wss" };
    let version = if is_localhost { "v2" } else { "v1" };
    let host = api_base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    format!("{protocol}://{host}/{version}/session_ingress/ws/{session_id}")
}

/// Build a CCR v2 session URL from the API base URL and session ID.
///
/// Unlike [`build_sdk_url`], this returns an HTTP(S) URL (not ws://) and
/// points at /v1/code/sessions/{id}. The child process derives the SSE
/// stream path and worker endpoints from this base.
pub fn build_ccr_v2_sdk_url(api_base_url: &str, session_id: &str) -> String {
    let base = api_base_url.trim_end_matches('/');
    format!("{base}/v1/code/sessions/{session_id}")
}

/// Compare two session IDs regardless of their tagged-ID prefix.
///
/// Tagged IDs have the form `{tag}_{body}` or `{tag}_staging_{body}`, where
/// the body encodes a UUID. CCR v2's compat layer returns `session_*` to v1
/// API clients but the infrastructure layer uses `cse_*`. Both have the same
/// underlying UUID.
///
/// Without this, the bridge would reject its own session as "foreign" when
/// the `ccr_v2_compat_enabled` gate is on.
pub fn same_session_id(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // The body is everything after the last underscore. This handles both
    // `{tag}_{body}` and `{tag}_staging_{body}`.
    let a_body = match a.rfind('_') {
        Some(idx) => &a[idx + 1..],
        None => a,
    };
    let b_body = match b.rfind('_') {
        Some(idx) => &b[idx + 1..],
        None => b,
    };
    // Require minimum length to avoid accidental matches on short suffixes
    a_body.len() >= 4 && a_body == b_body
}

/// Convert a session ID to the compat `session_*` format if it uses a
/// different prefix (e.g. `cse_*`).
///
/// If the ID already starts with `session_`, returns it unchanged.
/// If it has a known prefix (`cse_`), replaces it. Otherwise returns as-is.
pub fn to_compat_session_id(id: &str) -> String {
    if id.starts_with("session_") {
        return id.to_string();
    }
    if let Some(rest) = id.strip_prefix("cse_") {
        return format!("session_{rest}");
    }
    id.to_string()
}

/// Convert a session ID to the infrastructure `cse_*` format.
///
/// If the ID already starts with `cse_`, returns it unchanged.
/// If it has the compat prefix (`session_`), replaces it. Otherwise returns as-is.
pub fn to_infra_session_id(id: &str) -> String {
    if id.starts_with("cse_") {
        return id.to_string();
    }
    if let Some(rest) = id.strip_prefix("session_") {
        return format!("cse_{rest}");
    }
    id.to_string()
}

/// Register this bridge as the worker for a CCR v2 session.
///
/// Returns the `worker_epoch`, which must be passed to the child process so
/// its CCRClient can include it in every heartbeat/state/event request.
pub async fn register_worker(
    session_url: &str,
    access_token: &str,
) -> Result<i64> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("failed to build HTTP client")?;

    let url = format!("{session_url}/worker/register");
    let resp = http
        .post(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({}))
        .send()
        .await
        .context("register worker request failed")?;

    let status = resp.status().as_u16();
    if status != 200 && status != 201 {
        let text = resp.text().await.unwrap_or_default();
        bail!("registerWorker: HTTP {status}: {text}");
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse register worker response")?;

    // protojson serializes int64 as a string to avoid JS number precision loss;
    // the Go side may also return a number depending on encoder settings.
    let epoch = match &data["worker_epoch"] {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    };

    epoch.ok_or_else(|| {
        anyhow::anyhow!(
            "registerWorker: invalid worker_epoch in response: {}",
            data
        )
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_work_secret_valid() {
        let secret_json = r#"{"version":1,"session_ingress_token":"sk-ant-si-abc","api_base_url":"https://api.example.com","sources":[],"auth":[]}"#;
        let encoded = URL_SAFE_NO_PAD.encode(secret_json);
        let secret = decode_work_secret(&encoded).unwrap();
        assert_eq!(secret.version, 1);
        assert_eq!(secret.api_base_url, "https://api.example.com");
        assert_eq!(secret.session_ingress_token, "sk-ant-si-abc");
    }

    #[test]
    fn test_decode_work_secret_wrong_version() {
        let secret_json = r#"{"version":2,"session_ingress_token":"tok","api_base_url":"https://api.example.com","sources":[],"auth":[]}"#;
        let encoded = URL_SAFE_NO_PAD.encode(secret_json);
        let err = decode_work_secret(&encoded).unwrap_err();
        assert!(err.to_string().contains("Unsupported work secret version"));
    }

    #[test]
    fn test_decode_work_secret_empty_token() {
        let secret_json = r#"{"version":1,"session_ingress_token":"","api_base_url":"https://api.example.com","sources":[],"auth":[]}"#;
        let encoded = URL_SAFE_NO_PAD.encode(secret_json);
        let err = decode_work_secret(&encoded).unwrap_err();
        assert!(err.to_string().contains("empty session_ingress_token"));
    }

    #[test]
    fn test_decode_work_secret_invalid_base64() {
        assert!(decode_work_secret("!!!invalid!!!").is_err());
    }

    #[test]
    fn test_build_sdk_url_production() {
        let url = build_sdk_url("https://api.anthropic.com", "session_abc123");
        assert_eq!(
            url,
            "wss://api.anthropic.com/v1/session_ingress/ws/session_abc123"
        );
    }

    #[test]
    fn test_build_sdk_url_localhost() {
        let url = build_sdk_url("http://localhost:8080", "session_abc123");
        assert_eq!(
            url,
            "ws://localhost:8080/v2/session_ingress/ws/session_abc123"
        );
    }

    #[test]
    fn test_build_sdk_url_trailing_slash() {
        let url = build_sdk_url("https://api.anthropic.com/", "session_abc123");
        assert_eq!(
            url,
            "wss://api.anthropic.com/v1/session_ingress/ws/session_abc123"
        );
    }

    #[test]
    fn test_build_ccr_v2_sdk_url() {
        let url = build_ccr_v2_sdk_url("https://api.anthropic.com", "cse_abc123");
        assert_eq!(
            url,
            "https://api.anthropic.com/v1/code/sessions/cse_abc123"
        );
    }

    #[test]
    fn test_same_session_id_identical() {
        assert!(same_session_id("session_abc123", "session_abc123"));
    }

    #[test]
    fn test_same_session_id_different_prefix() {
        assert!(same_session_id("session_abc123", "cse_abc123"));
        assert!(same_session_id("cse_abc123", "session_abc123"));
    }

    #[test]
    fn test_same_session_id_staging() {
        assert!(same_session_id(
            "session_staging_abc123",
            "cse_staging_abc123"
        ));
    }

    #[test]
    fn test_same_session_id_different() {
        assert!(!same_session_id("session_abc123", "session_def456"));
    }

    #[test]
    fn test_same_session_id_short_suffix() {
        // Should not match on suffixes shorter than 4 chars
        assert!(!same_session_id("session_ab", "cse_ab"));
    }

    #[test]
    fn test_to_compat_session_id() {
        assert_eq!(
            to_compat_session_id("cse_abc123"),
            "session_abc123"
        );
        assert_eq!(
            to_compat_session_id("session_abc123"),
            "session_abc123"
        );
        assert_eq!(to_compat_session_id("bare_uuid"), "bare_uuid");
    }

    #[test]
    fn test_to_infra_session_id() {
        assert_eq!(
            to_infra_session_id("session_abc123"),
            "cse_abc123"
        );
        assert_eq!(
            to_infra_session_id("cse_abc123"),
            "cse_abc123"
        );
        assert_eq!(to_infra_session_id("bare_uuid"), "bare_uuid");
    }
}
