use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Generate a 43-character base64url-encoded code verifier (32 random bytes)
pub fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate SHA-256 code challenge from verifier
pub fn generate_code_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

/// Generate a 43-character random state parameter
pub fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// ── Full OAuth flow ──────────────────────────────────────────────────────────

use anyhow::{Context, Result};

use super::oauth_config;
use super::storage::OAuthStoredTokens;

/// Token exchange response from the OAuth server (snake_case JSON).
#[derive(Debug, serde::Deserialize)]
struct TokenExchangeResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
    scope: Option<String>,
}

/// Result of a successful OAuth login.
#[derive(Debug, Clone)]
pub struct OAuthLoginResult {
    pub tokens: OAuthStoredTokens,
    /// The manual fallback URL (for display in TUI when browser doesn't open).
    pub manual_url: String,
}

/// Prepared OAuth flow — URL is available immediately, completion is awaited separately.
pub struct PreparedOAuthFlow {
    /// The fallback URL the user can open manually (displayed in the TUI dialog).
    pub manual_url: String,
    /// Await this to get the final `OAuthLoginResult`.
    waiter: std::pin::Pin<Box<dyn std::future::Future<Output = Result<OAuthLoginResult>> + Send>>,
}

impl PreparedOAuthFlow {
    /// Block (async) until the OAuth callback arrives and tokens are exchanged.
    pub async fn wait(self) -> Result<OAuthLoginResult> {
        self.waiter.await
    }
}

/// Prepare the PKCE OAuth login flow:
/// 1. Generate PKCE verifier + challenge + state
/// 2. Start a localhost HTTP server to receive the callback
/// 3. Open the browser to the authorization URL
/// 4. Return immediately with the manual URL and a future to await.
///
/// The returned `PreparedOAuthFlow` exposes `manual_url` so the TUI can
/// display it inside the dialog (no terminal output).  Call `.wait()` to
/// block until the browser callback arrives.
pub async fn prepare_oauth_login(login_with_claude_ai: bool) -> Result<PreparedOAuthFlow> {
    let code_verifier = generate_code_verifier();
    let code_challenge = generate_code_challenge(&code_verifier);
    let state = generate_state();

    // Start local callback server on an OS-assigned port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("Failed to bind localhost callback server")?;
    let port = listener.local_addr()?.port();

    // Build URLs
    let automatic_url =
        oauth_config::build_auth_url(&code_challenge, &state, port, false, login_with_claude_ai);
    let manual_url =
        oauth_config::build_auth_url(&code_challenge, &state, port, true, login_with_claude_ai);

    // Try to open browser (non-blocking, ignore errors).
    let _ = open_browser(&automatic_url);

    let manual_url_clone = manual_url.clone();
    let waiter = Box::pin(async move {
        // Wait for the OAuth callback with a 5-minute timeout
        let auth_code = wait_for_callback(listener, &state).await?;

        // Exchange authorization code for tokens
        let tokens = exchange_code_for_tokens(
            &auth_code,
            &state,
            &code_verifier,
            port,
            false,
        )
        .await?;

        Ok(OAuthLoginResult {
            tokens,
            manual_url: manual_url_clone,
        })
    });

    Ok(PreparedOAuthFlow { manual_url, waiter })
}

/// Run the full PKCE OAuth login flow (convenience wrapper around
/// [`prepare_oauth_login`]).
pub async fn run_oauth_login(login_with_claude_ai: bool) -> Result<OAuthLoginResult> {
    let flow = prepare_oauth_login(login_with_claude_ai).await?;
    flow.wait().await
}

/// Start a minimal HTTP server that waits for a single OAuth callback request.
/// Returns the authorization code from the callback.
async fn wait_for_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let timeout = std::time::Duration::from_secs(300); // 5 minutes

    let result = tokio::time::timeout(timeout, async {
        loop {
            let (mut stream, _) = listener
                .accept()
                .await
                .context("Failed to accept connection")?;

            let mut buf = vec![0u8; 8192];
            let n = stream
                .read(&mut buf)
                .await
                .context("Failed to read request")?;
            let request = String::from_utf8_lossy(&buf[..n]);

            // Parse the HTTP request line to extract the path
            let first_line = request.lines().next().unwrap_or("");
            let path = first_line.split_whitespace().nth(1).unwrap_or("");

            // Only handle /callback requests
            if !path.starts_with("/callback") {
                let response =
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found";
                let _ = stream.write_all(response.as_bytes()).await;
                continue;
            }

            // Parse query parameters
            let url_str = format!("http://localhost{}", path);
            let parsed =
                url::Url::parse(&url_str).context("Failed to parse callback URL")?;

            let code = parsed
                .query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.to_string());

            let state = parsed
                .query_pairs()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.to_string());

            // Validate CSRF state parameter
            if state.as_deref() != Some(expected_state) {
                let response = "HTTP/1.1 400 Bad Request\r\n\
                                Content-Length: 23\r\n\r\n\
                                Invalid state parameter";
                let _ = stream.write_all(response.as_bytes()).await;
                return Err(anyhow::anyhow!(
                    "OAuth state mismatch: possible CSRF attack"
                ));
            }

            let code = match code {
                Some(c) => c,
                None => {
                    // Check for error parameter (user denied, etc.)
                    let error = parsed
                        .query_pairs()
                        .find(|(k, _)| k == "error")
                        .map(|(_, v)| v.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    let error_desc = parsed
                        .query_pairs()
                        .find(|(k, _)| k == "error_description")
                        .map(|(_, v)| v.to_string())
                        .unwrap_or_default();

                    let body = "Authorization code missing";
                    let response = format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    return Err(anyhow::anyhow!(
                        "OAuth authorization denied: {} {}",
                        error,
                        error_desc
                    ));
                }
            };

            // Redirect the browser to the success page
            let success_url = oauth_config::CLAUDEAI_SUCCESS_URL;
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\n\r\n",
                success_url
            );
            let _ = stream.write_all(response.as_bytes()).await;

            return Ok(code);
        }
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err(anyhow::anyhow!(
            "Timed out waiting for OAuth callback (5 minutes). Please try again."
        )),
    }
}

/// Exchange an authorization code for access/refresh tokens via the token endpoint.
async fn exchange_code_for_tokens(
    authorization_code: &str,
    state: &str,
    code_verifier: &str,
    port: u16,
    use_manual_redirect: bool,
) -> Result<OAuthStoredTokens> {
    let redirect_uri = if use_manual_redirect {
        oauth_config::MANUAL_REDIRECT_URL.to_string()
    } else {
        oauth_config::localhost_redirect_uri(port)
    };

    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": authorization_code,
        "redirect_uri": redirect_uri,
        "client_id": oauth_config::CLIENT_ID,
        "code_verifier": code_verifier,
        "state": state,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(oauth_config::TOKEN_URL)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .json(&body)
        .send()
        .await
        .context("Failed to connect to token endpoint")?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(anyhow::anyhow!(
            "Authentication failed: invalid authorization code"
        ));
    }
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Token exchange failed ({}): {}",
            status,
            body_text
        ));
    }

    let data: TokenExchangeResponse = response
        .json()
        .await
        .context("Failed to parse token exchange response")?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let scopes = data
        .scope
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    Ok(OAuthStoredTokens {
        access_token: data.access_token,
        refresh_token: data.refresh_token,
        expires_at: Some(now_ms + data.expires_in * 1000),
        scopes,
        subscription_type: None,
        rate_limit_tier: None,
    })
}

/// Refresh an OAuth token using the refresh_token grant.
pub async fn refresh_oauth_token(refresh_token: &str) -> Result<OAuthStoredTokens> {
    let scope = oauth_config::ALL_OAUTH_SCOPES.join(" ");

    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": oauth_config::CLIENT_ID,
        "scope": scope,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(oauth_config::TOKEN_URL)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .json(&body)
        .send()
        .await
        .context("Failed to connect to token endpoint for refresh")?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Token refresh failed ({}): {}",
            status,
            body_text
        ));
    }

    let data: TokenExchangeResponse = response
        .json()
        .await
        .context("Failed to parse token refresh response")?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let scopes = data
        .scope
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    Ok(OAuthStoredTokens {
        access_token: data.access_token,
        // Keep original refresh token if the server didn't return a new one
        refresh_token: data
            .refresh_token
            .or_else(|| Some(refresh_token.to_string())),
        expires_at: Some(now_ms + data.expires_in * 1000),
        scopes,
        subscription_type: None,
        rate_limit_tier: None,
    })
}

/// Check whether a token is expired (with 5-minute buffer, matching TS client).
pub fn is_token_expired(expires_at: Option<u64>) -> bool {
    let expires_at = match expires_at {
        Some(t) => t,
        None => return false, // No expiry set means it doesn't expire
    };

    let buffer_ms: u64 = 5 * 60 * 1000; // 5 minutes
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    now_ms + buffer_ms >= expires_at
}

/// Open a URL in the default browser (platform-specific).
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .context("Failed to open browser")?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .context("Failed to open browser")?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", url])
            .spawn()
            .context("Failed to open browser")?;
    }
    Ok(())
}
