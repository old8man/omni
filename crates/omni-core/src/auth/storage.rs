use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Keychain service name used by the real Claude Code (production).
// Format: "Claude Code{OAUTH_FILE_SUFFIX}{serviceSuffix}{dirHash}"
// Production: OAUTH_FILE_SUFFIX="" , serviceSuffix="-credentials", dirHash="" (default dir)
const KEYCHAIN_SERVICE_NAME: &str = "Claude Code-credentials";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStoredTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub subscription_type: Option<String>,
    #[serde(default)]
    pub rate_limit_tier: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecureStorageData {
    claude_ai_oauth: Option<OAuthStoredTokens>,
}

/// Load OAuth tokens from the macOS Keychain (same location as real Claude Code).
/// Falls back to ~/.claude/.credentials.json on non-macOS.
pub async fn load_tokens() -> Result<Option<OAuthStoredTokens>> {
    // Try macOS Keychain first
    if cfg!(target_os = "macos") {
        if let Some(tokens) = load_from_keychain().await? {
            return Ok(Some(tokens));
        }
    }

    // Fallback to file
    load_from_file().await
}

/// Read from macOS Keychain using `security find-generic-password`.
/// This reads the exact same entry that the real Claude Code writes.
async fn load_from_keychain() -> Result<Option<OAuthStoredTokens>> {
    let username = std::env::var("USER").unwrap_or_else(|_| "claude-code-user".into());

    let output = tokio::process::Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            &username,
            "-w",
            "-s",
            KEYCHAIN_SERVICE_NAME,
        ])
        .output()
        .await
        .context("Failed to run security command")?;

    if !output.status.success() {
        // No entry found — not an error, just means not logged in
        return Ok(None);
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return Ok(None);
    }

    // The real Claude Code stores data as raw JSON (or sometimes hex-encoded).
    // Try JSON first, then hex decode.
    let json_str = if raw.starts_with('{') {
        raw
    } else {
        // Hex-encoded JSON (used when data contains special chars)
        let bytes = hex::decode(&raw).context("Keychain value is neither JSON nor valid hex")?;
        String::from_utf8(bytes).context("Hex-decoded keychain value is not valid UTF-8")?
    };

    let data: SecureStorageData =
        serde_json::from_str(&json_str).context("Failed to parse keychain JSON")?;

    Ok(data.claude_ai_oauth)
}

/// Fallback: read from ~/.claude/.credentials.json
async fn load_from_file() -> Result<Option<OAuthStoredTokens>> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let content = tokio::fs::read_to_string(&path).await?;
    if content.trim().is_empty() {
        return Ok(None);
    }
    let data: SecureStorageData = serde_json::from_str(&content)?;
    Ok(data.claude_ai_oauth)
}

/// Store tokens to ~/.claude/.credentials.json and (on macOS) to the Keychain.
pub async fn store_tokens(tokens: &OAuthStoredTokens) -> Result<()> {
    // Always write to the credentials file
    store_to_file(tokens).await?;

    // On macOS, also write to the Keychain
    if cfg!(target_os = "macos") {
        if let Err(e) = store_to_keychain(tokens).await {
            tracing::warn!("Failed to store tokens in macOS Keychain: {}", e);
            // Not fatal — file storage is the primary store
        }
    }

    Ok(())
}

/// Write tokens to ~/.claude/.credentials.json
async fn store_to_file(tokens: &OAuthStoredTokens) -> Result<()> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let data = SecureStorageData {
        claude_ai_oauth: Some(tokens.clone()),
    };
    let json = serde_json::to_string_pretty(&data)?;
    tokio::fs::write(&path, json).await?;

    // Set restrictive file permissions (owner read/write only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms).ok();
    }

    Ok(())
}

/// Write tokens to macOS Keychain using `security add-generic-password`.
async fn store_to_keychain(tokens: &OAuthStoredTokens) -> Result<()> {
    let username = std::env::var("USER").unwrap_or_else(|_| "claude-code-user".into());

    let data = SecureStorageData {
        claude_ai_oauth: Some(tokens.clone()),
    };
    let json = serde_json::to_string(&data)?;

    // Delete existing entry first (ignore errors — may not exist)
    let _ = tokio::process::Command::new("security")
        .args([
            "delete-generic-password",
            "-a",
            &username,
            "-s",
            KEYCHAIN_SERVICE_NAME,
        ])
        .output()
        .await;

    // Add new entry
    let output = tokio::process::Command::new("security")
        .args([
            "add-generic-password",
            "-a",
            &username,
            "-s",
            KEYCHAIN_SERVICE_NAME,
            "-w",
            &json,
        ])
        .output()
        .await
        .context("Failed to run security add-generic-password")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "Failed to store in Keychain: {}",
            stderr.trim()
        ));
    }

    Ok(())
}

/// Delete stored tokens from both file and Keychain.
pub async fn delete_tokens() -> Result<()> {
    // Delete credentials file
    let path = credentials_path()?;
    if path.exists() {
        tokio::fs::remove_file(&path)
            .await
            .context("Failed to delete credentials file")?;
    }

    // Delete from macOS Keychain
    if cfg!(target_os = "macos") {
        let username = std::env::var("USER").unwrap_or_else(|_| "claude-code-user".into());
        let _ = tokio::process::Command::new("security")
            .args([
                "delete-generic-password",
                "-a",
                &username,
                "-s",
                KEYCHAIN_SERVICE_NAME,
            ])
            .output()
            .await;
        // Ignore errors — entry may not exist
    }

    Ok(())
}

/// Load tokens and refresh them if expired. Returns None if no tokens stored
/// or refresh fails.
pub async fn load_and_refresh_tokens() -> Result<Option<OAuthStoredTokens>> {
    let tokens = match load_tokens().await? {
        Some(t) => t,
        None => return Ok(None),
    };

    // Check if token is expired (with 5-minute buffer)
    if super::pkce::is_token_expired(tokens.expires_at) {
        // Need to refresh
        let refresh_token = match &tokens.refresh_token {
            Some(rt) => rt.clone(),
            None => {
                tracing::warn!("OAuth token expired but no refresh token available");
                return Ok(None);
            }
        };

        match super::pkce::refresh_oauth_token(&refresh_token).await {
            Ok(mut new_tokens) => {
                // Preserve subscription info from old tokens if not in new response
                if new_tokens.subscription_type.is_none() {
                    new_tokens.subscription_type = tokens.subscription_type;
                }
                if new_tokens.rate_limit_tier.is_none() {
                    new_tokens.rate_limit_tier = tokens.rate_limit_tier;
                }
                // Store refreshed tokens
                store_tokens(&new_tokens).await?;
                Ok(Some(new_tokens))
            }
            Err(e) => {
                tracing::warn!("Failed to refresh OAuth token: {}", e);
                // Return the old token anyway — it may still work for a few more seconds
                // or the caller can handle the 401
                Ok(Some(tokens))
            }
        }
    } else {
        Ok(Some(tokens))
    }
}

fn credentials_path() -> Result<PathBuf> {
    let dir = crate::config::paths::claude_dir()?;
    Ok(dir.join(".credentials.json"))
}
