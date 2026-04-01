use crate::api::client::AuthMethod;
use anyhow::Result;

use super::oauth_config;

/// Authentication resolution result
pub enum AuthResolution {
    /// Direct API key — can call the API directly
    ApiKey(AuthMethod),
    /// OAuth token — can call the API directly with Bearer auth and beta header
    OAuthToken(AuthMethod),
    /// OAuth token found — need to use claude binary proxy (legacy fallback)
    OAuthProxy,
    /// No auth found
    None,
}

/// Resolve authentication method by priority:
/// 1. ANTHROPIC_API_KEY env var -> direct API
/// 2. Active profile credentials (multi-profile system)
/// 3. Stored OAuth tokens (file/keychain) -> direct API with Bearer auth
///    a. Refresh token if expired
/// 4. Claude binary available (already logged in) -> proxy
/// 5. None
pub async fn resolve_auth() -> Result<AuthResolution> {
    // 1. Check env var
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Ok(AuthResolution::ApiKey(AuthMethod::ApiKey(key)));
        }
    }

    // 2. Check active profile credentials
    if let Some(creds) = super::profiles::get_active_credentials() {
        // API key profiles
        if let Some(ref api_key) = creds.api_key {
            return Ok(AuthResolution::ApiKey(AuthMethod::ApiKey(api_key.clone())));
        }
        // OAuth token profiles
        if let Some(ref access_token) = creds.access_token {
            if oauth_config::has_inference_scope(&creds.scopes) {
                // Check if token is expired and try to refresh
                if super::pkce::is_token_expired(creds.expires_at) {
                    if let Some(ref refresh_token) = creds.refresh_token {
                        match super::pkce::refresh_oauth_token(refresh_token).await {
                            Ok(new_tokens) => {
                                // Update the profile with refreshed tokens
                                if let Some(mut profile) = super::profiles::get_active_profile() {
                                    profile.credentials.access_token =
                                        Some(new_tokens.access_token.clone());
                                    profile.credentials.expires_at = new_tokens.expires_at;
                                    if let Some(rt) = &new_tokens.refresh_token {
                                        profile.credentials.refresh_token = Some(rt.clone());
                                    }
                                    let _ = super::profiles::save_profile(&profile);
                                }
                                return Ok(AuthResolution::OAuthToken(AuthMethod::OAuthToken(
                                    new_tokens.access_token,
                                )));
                            }
                            Err(e) => {
                                tracing::warn!("Failed to refresh profile OAuth token: {}", e);
                                // Fall through to try other methods
                            }
                        }
                    } else {
                        tracing::warn!("Active profile token expired with no refresh token");
                    }
                } else {
                    return Ok(AuthResolution::OAuthToken(AuthMethod::OAuthToken(
                        access_token.clone(),
                    )));
                }
            }
        }
    }

    // 3. Check stored OAuth tokens and refresh if needed
    if let Some(tokens) = super::storage::load_and_refresh_tokens().await? {
        // Check if the token has inference scope (Claude.ai subscriber)
        if oauth_config::has_inference_scope(&tokens.scopes) {
            return Ok(AuthResolution::OAuthToken(AuthMethod::OAuthToken(
                tokens.access_token,
            )));
        }

        // Token exists but lacks inference scope — try proxy if available
        if crate::api::claude_proxy::is_claude_available() {
            return Ok(AuthResolution::OAuthProxy);
        }

        // Have tokens but can't use them directly (no inference scope) and
        // no proxy available
        tracing::warn!(
            "OAuth tokens found but lack inference scope and `claude` binary not available"
        );
    }

    // 4. Check if claude binary is available (user may be logged in there)
    if crate::api::claude_proxy::is_claude_available() {
        return Ok(AuthResolution::OAuthProxy);
    }

    Ok(AuthResolution::None)
}
