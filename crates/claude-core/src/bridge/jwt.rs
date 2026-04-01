//! JWT token parsing and expiry utilities.
//!
//! Provides lightweight JWT decoding without signature verification,
//! used by the bridge to schedule proactive token refreshes before
//! session ingress tokens expire.

use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;

/// Prefix used on session ingress tokens.
const SESSION_INGRESS_PREFIX: &str = "sk-ant-si-";

/// Refresh buffer: request a new token this many seconds before expiry.
pub const TOKEN_REFRESH_BUFFER_SECS: u64 = 5 * 60;

/// Fallback refresh interval (30 minutes) when the new token's expiry is unknown.
pub const FALLBACK_REFRESH_INTERVAL_SECS: u64 = 30 * 60;

/// Maximum consecutive refresh failures before giving up.
pub const MAX_REFRESH_FAILURES: u32 = 3;

/// Retry delay in seconds when the access token is unavailable.
pub const REFRESH_RETRY_DELAY_SECS: u64 = 60;

/// Minimal JWT payload structure for extracting the expiry claim.
#[derive(Debug, Deserialize)]
struct JwtPayload {
    /// Expiry time as Unix seconds.
    #[serde(default)]
    exp: Option<u64>,
}

/// Decode a JWT's payload segment without verifying the signature.
///
/// Strips the `sk-ant-si-` session-ingress prefix if present.
/// Returns the parsed JSON payload, or `None` if the token is
/// malformed or the payload is not valid JSON.
pub fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let jwt = token.strip_prefix(SESSION_INGRESS_PREFIX).unwrap_or(token);
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    serde_json::from_slice(&payload_bytes).ok()
}

/// Decode the `exp` (expiry) claim from a JWT without verifying the signature.
///
/// Returns the `exp` value in Unix seconds, or `None` if the token is
/// malformed or has no `exp` claim.
pub fn decode_jwt_expiry(token: &str) -> Option<u64> {
    let jwt = token.strip_prefix(SESSION_INGRESS_PREFIX).unwrap_or(token);
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let payload: JwtPayload = serde_json::from_slice(&payload_bytes).ok()?;
    payload.exp
}

/// Calculate how many seconds until a token should be refreshed.
///
/// Returns `Some(delay_secs)` if the token has a decodable expiry and the
/// refresh time is in the future. Returns `None` if the token should be
/// refreshed immediately (already expired or within the buffer window),
/// or if the expiry cannot be determined.
pub fn seconds_until_refresh(token: &str, buffer_secs: u64) -> Option<u64> {
    let exp = decode_jwt_expiry(token)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_secs();

    let refresh_at = exp.saturating_sub(buffer_secs);
    if refresh_at <= now {
        return None; // Should refresh immediately
    }
    Some(refresh_at - now)
}

/// Decode a base64url-encoded work secret from a [`WorkResponse`](super::types::WorkResponse).
pub fn decode_work_secret(secret_b64: &str) -> Result<super::types::WorkSecret> {
    let bytes = URL_SAFE_NO_PAD
        .decode(secret_b64)
        .context("failed to base64url-decode work secret")?;
    serde_json::from_slice(&bytes).context("failed to parse work secret JSON")
}

/// Format a duration in milliseconds as a human-readable string (e.g. "5m 30s").
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 60_000 {
        format!("{}s", ms / 1000)
    } else {
        let m = ms / 60_000;
        let s = (ms % 60_000) / 1000;
        if s > 0 {
            format!("{m}m {s}s")
        } else {
            format!("{m}m")
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    /// Build a minimal JWT with the given payload JSON.
    fn make_jwt(payload_json: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload_json);
        let signature = URL_SAFE_NO_PAD.encode("fakesig");
        format!("{header}.{payload}.{signature}")
    }

    #[test]
    fn test_decode_jwt_payload_basic() {
        let token = make_jwt(r#"{"sub":"user1","exp":1700000000}"#);
        let payload = decode_jwt_payload(&token).unwrap();
        assert_eq!(payload["sub"], "user1");
        assert_eq!(payload["exp"], 1700000000u64);
    }

    #[test]
    fn test_decode_jwt_payload_with_prefix() {
        let inner = make_jwt(r#"{"exp":1700000000}"#);
        let token = format!("sk-ant-si-{inner}");
        let exp = decode_jwt_expiry(&token);
        assert_eq!(exp, Some(1700000000));
    }

    #[test]
    fn test_decode_jwt_expiry_no_exp_claim() {
        let token = make_jwt(r#"{"sub":"user1"}"#);
        assert_eq!(decode_jwt_expiry(&token), None);
    }

    #[test]
    fn test_decode_jwt_malformed() {
        assert!(decode_jwt_payload("not-a-jwt").is_none());
        assert!(decode_jwt_payload("a.b").is_none());
        assert!(decode_jwt_payload("").is_none());
    }

    #[test]
    fn test_format_duration_ms() {
        assert_eq!(format_duration_ms(0), "0s");
        assert_eq!(format_duration_ms(5_000), "5s");
        assert_eq!(format_duration_ms(60_000), "1m");
        assert_eq!(format_duration_ms(90_000), "1m 30s");
        assert_eq!(format_duration_ms(330_000), "5m 30s");
    }

    #[test]
    fn test_seconds_until_refresh_expired() {
        // Token that expired in the past
        let token = make_jwt(r#"{"exp":1000000}"#);
        assert!(seconds_until_refresh(&token, 300).is_none());
    }

    #[test]
    fn test_decode_work_secret() {
        let secret_json = r#"{"version":1,"session_ingress_token":"tok","api_base_url":"https://api.example.com","sources":[],"auth":[]}"#;
        let encoded = URL_SAFE_NO_PAD.encode(secret_json);
        let secret = decode_work_secret(&encoded).unwrap();
        assert_eq!(secret.version, 1);
        assert_eq!(secret.api_base_url, "https://api.example.com");
    }
}
