//! Trusted device token management for bridge (remote-control) sessions.
//!
//! Bridge sessions have SecurityTier=ELEVATED on the server (CCR v2). The
//! server gates ConnectBridgeWorker on its own flag; this CLI-side module
//! controls whether the CLI sends X-Trusted-Device-Token at all.
//!
//! Enrollment (POST /auth/trusted_devices) is gated server-side by
//! `account_session.created_at < 10min`, so it must happen during /login.
//! The token is persistent (90d rolling expiry) and stored in the system
//! keychain or a local file.

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};

/// File name for the trusted device token within the Claude config directory.
const TOKEN_FILE_NAME: &str = "trusted_device_token";

/// Minimum length for a device token to be considered valid.
const MIN_TOKEN_LENGTH: usize = 16;

/// Default enrollment timeout.
const ENROLLMENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Cached trusted device token. `None` means not yet loaded; `Some(None)`
/// means loaded but no token exists; `Some(Some(token))` is the cached value.
static CACHED_TOKEN: Mutex<Option<Option<String>>> = Mutex::new(None);

/// Get the path to the trusted device token file.
fn token_file_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(crate::config::paths::OMNI_DIR_NAME).join(TOKEN_FILE_NAME))
}

/// Read the stored trusted device token from disk.
///
/// Returns `None` if no token is stored or the file doesn't exist.
/// Uses a process-level cache to avoid repeated filesystem reads
/// (the macOS keychain path in the TS original spawns a subprocess on
/// every read).
///
/// The environment variable `CLAUDE_TRUSTED_DEVICE_TOKEN` always takes
/// precedence and bypasses the cache entirely.
pub fn get_trusted_device_token() -> Option<String> {
    // Check environment variable override first -- bypasses cache
    if let Ok(env_token) = std::env::var("CLAUDE_TRUSTED_DEVICE_TOKEN") {
        if !env_token.is_empty() {
            return Some(env_token);
        }
    }

    let mut cache = CACHED_TOKEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = cache.as_ref() {
        return cached.clone();
    }

    let token = read_token_from_disk();
    *cache = Some(token.clone());
    token
}

/// Clear the cached token so the next call to [`get_trusted_device_token`]
/// re-reads from disk. Called after enrollment and on logout.
pub fn clear_trusted_device_token_cache() {
    let mut cache = CACHED_TOKEN.lock().unwrap_or_else(|e| e.into_inner());
    *cache = None;
}

/// Clear the stored trusted device token from disk and the cache.
///
/// Called before enrollment during /login so a stale token from the previous
/// account isn't sent while enrollment is in-flight.
pub fn clear_trusted_device_token() -> Result<()> {
    if let Some(path) = token_file_path() {
        if path.exists() {
            std::fs::remove_file(&path).context("failed to remove trusted device token file")?;
        }
    }
    clear_trusted_device_token_cache();
    Ok(())
}

/// Read the token from the filesystem.
fn read_token_from_disk() -> Option<String> {
    let path = token_file_path()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    let token = contents.trim().to_string();
    if token.len() >= MIN_TOKEN_LENGTH {
        Some(token)
    } else {
        None
    }
}

/// Persist a token to the filesystem.
fn write_token_to_disk(token: &str) -> Result<()> {
    let path = token_file_path().context("could not determine config directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("failed to create config directory")?;
    }
    std::fs::write(&path, token).context("failed to write trusted device token")?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms).context("failed to set token file permissions")?;
    }

    Ok(())
}

/// Enroll this device via POST /auth/trusted_devices and persist the token.
///
/// Best-effort: logs and returns on failure so callers (post-login hooks)
/// don't block the login flow.
///
/// The server gates enrollment on `account_session.created_at < 10min`, so
/// this must be called immediately after a fresh /login. Calling it later
/// will fail with 403 stale_session.
pub async fn enroll_trusted_device(
    base_url: &str,
    access_token: &str,
    display_name: &str,
) -> Result<String> {
    let http = reqwest::Client::builder()
        .timeout(ENROLLMENT_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;

    let url = format!("{base_url}/api/auth/trusted_devices");
    let body = serde_json::json!({
        "display_name": display_name,
    });

    let resp = http
        .post(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .context("enrollment request failed")?;

    let status = resp.status().as_u16();
    if status != 200 && status != 201 {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "Enrollment failed with status {status}: {}",
            &text[..text.len().min(200)]
        );
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse enrollment response")?;

    let token = data
        .get("device_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("enrollment response missing device_token field"))?;

    if token.is_empty() {
        anyhow::bail!("enrollment response contained empty device_token");
    }

    // Persist the token to disk
    write_token_to_disk(token)?;
    clear_trusted_device_token_cache();

    let device_id = data
        .get("device_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    tracing::debug!("Enrolled trusted device: device_id={device_id}");

    Ok(token.to_string())
}

/// Build the `X-Trusted-Device-Token` header value for bridge API requests.
///
/// Returns `None` if no token is available (not enrolled, env var not set,
/// or gate is disabled).
pub fn trusted_device_header() -> Option<(String, String)> {
    let token = get_trusted_device_token()?;
    Some(("X-Trusted-Device-Token".to_string(), token))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Serialize env-var-touching tests to avoid parallel races
    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn test_env_var_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        let original = std::env::var("CLAUDE_TRUSTED_DEVICE_TOKEN").ok();

        unsafe { std::env::set_var("CLAUDE_TRUSTED_DEVICE_TOKEN", "test-token-abcdefghijklmnop") };
        clear_trusted_device_token_cache();

        let token = get_trusted_device_token();
        assert_eq!(token, Some("test-token-abcdefghijklmnop".to_string()));

        match original {
            Some(val) => unsafe { std::env::set_var("CLAUDE_TRUSTED_DEVICE_TOKEN", val) },
            None => unsafe { std::env::remove_var("CLAUDE_TRUSTED_DEVICE_TOKEN") },
        }
        clear_trusted_device_token_cache();
    }

    #[test]
    fn test_empty_env_var_returns_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        let original = std::env::var("CLAUDE_TRUSTED_DEVICE_TOKEN").ok();

        unsafe { std::env::set_var("CLAUDE_TRUSTED_DEVICE_TOKEN", "") };
        clear_trusted_device_token_cache();

        // Empty env var should fall through to disk read
        let _token = get_trusted_device_token();

        match original {
            Some(val) => unsafe { std::env::set_var("CLAUDE_TRUSTED_DEVICE_TOKEN", val) },
            None => unsafe { std::env::remove_var("CLAUDE_TRUSTED_DEVICE_TOKEN") },
        }
        clear_trusted_device_token_cache();
    }

    #[test]
    fn test_clear_cache() {
        clear_trusted_device_token_cache();
        let cache = CACHED_TOKEN.lock().unwrap();
        assert!(cache.is_none());
    }

    #[test]
    fn test_trusted_device_header_with_token() {
        let _guard = ENV_LOCK.lock().unwrap();
        let original = std::env::var("CLAUDE_TRUSTED_DEVICE_TOKEN").ok();

        unsafe { std::env::set_var("CLAUDE_TRUSTED_DEVICE_TOKEN", "test-header-token-abcdef123") };
        clear_trusted_device_token_cache();

        let header = trusted_device_header();
        assert!(header.is_some());
        let (name, value) = header.unwrap();
        assert_eq!(name, "X-Trusted-Device-Token");
        assert_eq!(value, "test-header-token-abcdef123");

        match original {
            Some(val) => unsafe { std::env::set_var("CLAUDE_TRUSTED_DEVICE_TOKEN", val) },
            None => unsafe { std::env::remove_var("CLAUDE_TRUSTED_DEVICE_TOKEN") },
        }
        clear_trusted_device_token_cache();
    }

    #[test]
    fn test_token_file_path() {
        let path = token_file_path();
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.to_string_lossy().contains("trusted_device_token"));
    }
}
