//! OAuth endpoint configuration matching the real Claude Code production config.
//!
//! Reference: original/claude-code/constants/oauth.ts

/// Production OAuth client ID
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// OAuth authorization URL (Console / API-key flow)
pub const CONSOLE_AUTHORIZE_URL: &str = "https://platform.claude.com/oauth/authorize";

/// OAuth authorization URL (Claude.ai subscriber flow, via claude.com for attribution)
pub const CLAUDE_AI_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";

/// Token exchange endpoint
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

/// API key creation endpoint (used after Console OAuth)
pub const API_KEY_URL: &str =
    "https://api.anthropic.com/api/oauth/omni_cli/create_api_key";

/// Success redirect for Claude.ai subscribers
pub const CLAUDEAI_SUCCESS_URL: &str =
    "https://platform.claude.com/oauth/code/success?app=claude-code";

/// Success redirect for Console users (includes buy-credits upsell)
pub const CONSOLE_SUCCESS_URL: &str =
    "https://platform.claude.com/buy_credits?returnUrl=/oauth/code/success%3Fapp%3Dclaude-code";

/// Manual redirect URL (user copies code from browser)
pub const MANUAL_REDIRECT_URL: &str =
    "https://platform.claude.com/oauth/code/callback";

/// OAuth beta header value
pub const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";

// ── Scopes ───────────────────────────────────────────────────────────────────

pub const CLAUDE_AI_INFERENCE_SCOPE: &str = "user:inference";
pub const CLAUDE_AI_PROFILE_SCOPE: &str = "user:profile";
pub const CONSOLE_SCOPE: &str = "org:create_api_key";

/// All OAuth scopes requested during login (union of Console + Claude.ai scopes).
pub const ALL_OAUTH_SCOPES: &[&str] = &[
    CONSOLE_SCOPE,
    CLAUDE_AI_PROFILE_SCOPE,
    CLAUDE_AI_INFERENCE_SCOPE,
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];

/// Check if scopes include Claude.ai inference (subscriber auth).
pub fn has_inference_scope(scopes: &[String]) -> bool {
    scopes.iter().any(|s| s == CLAUDE_AI_INFERENCE_SCOPE)
}

/// Build the localhost redirect URI for a given port.
pub fn localhost_redirect_uri(port: u16) -> String {
    format!("http://localhost:{}/callback", port)
}

/// Build the full authorization URL for the OAuth flow.
///
/// `is_manual` controls whether the redirect_uri is the manual (copy-paste)
/// URL or the localhost callback.
pub fn build_auth_url(
    code_challenge: &str,
    state: &str,
    port: u16,
    is_manual: bool,
    login_with_claude_ai: bool,
) -> String {
    let base = if login_with_claude_ai {
        CLAUDE_AI_AUTHORIZE_URL
    } else {
        CONSOLE_AUTHORIZE_URL
    };

    let redirect_uri = if is_manual {
        MANUAL_REDIRECT_URL.to_string()
    } else {
        localhost_redirect_uri(port)
    };

    let scope = ALL_OAUTH_SCOPES.join(" ");

    let mut url = url::Url::parse(base).expect("hard-coded URL must parse");
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &scope)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);

    url.to_string()
}
