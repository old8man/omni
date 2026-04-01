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

/// Resolve authentication method by priority (matches real Claude Code):
/// 1. ANTHROPIC_API_KEY env var -> direct API
/// 2. Stored OAuth tokens (file/keychain) -> direct API with Bearer auth
///    a. Refresh token if expired
/// 3. Claude binary available (already logged in) -> proxy
/// 4. None
pub async fn resolve_auth() -> Result<AuthResolution> {
    // 1. Check env var
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Ok(AuthResolution::ApiKey(AuthMethod::ApiKey(key)));
        }
    }

    // 2. Check stored OAuth tokens and refresh if needed
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

    // 3. Check if claude binary is available (user may be logged in there)
    if crate::api::claude_proxy::is_claude_available() {
        return Ok(AuthResolution::OAuthProxy);
    }

    Ok(AuthResolution::None)
}
