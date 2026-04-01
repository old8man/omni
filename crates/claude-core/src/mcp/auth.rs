use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::config::paths::claude_dir;

// ---------------------------------------------------------------------------
// OAuth token types
// ---------------------------------------------------------------------------

/// OAuth 2.0 tokens for a single MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    /// Expiry as a Unix timestamp (seconds).
    pub expires_at: Option<u64>,
}

impl OAuthTokens {
    /// Whether the access token has expired (or will within `margin_secs`).
    pub fn is_expired(&self, margin_secs: u64) -> bool {
        match self.expires_at {
            Some(exp) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                now + margin_secs >= exp
            }
            None => false, // No expiry means we don't proactively expire.
        }
    }
}

/// Client registration info returned by Dynamic Client Registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthClientInfo {
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_access_token: Option<String>,
}

/// OAuth discovery metadata for an authorization server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthServerMetadata {
    pub issuer: Option<String>,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub response_types_supported: Vec<String>,
    #[serde(default)]
    pub grant_types_supported: Vec<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
}

// ---------------------------------------------------------------------------
// Credential storage (file-backed, per-server)
// ---------------------------------------------------------------------------

/// Per-server stored OAuth state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredServerOAuth {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Serialised OAuth server metadata so we can refresh without
    /// re-discovering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_metadata: Option<OAuthServerMetadata>,
}

/// Flat file that persists all per-server OAuth data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthStore {
    /// Keyed by `server_key` (see [`get_server_key`]).
    #[serde(default)]
    pub servers: HashMap<String, StoredServerOAuth>,
}

impl OAuthStore {
    fn path() -> Result<PathBuf> {
        Ok(claude_dir()?.join("mcp-oauth.json"))
    }

    /// Load from disk (returns an empty store on any error).
    pub fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn get(&self, server_key: &str) -> Option<&StoredServerOAuth> {
        self.servers.get(server_key)
    }

    pub fn set(&mut self, server_key: String, entry: StoredServerOAuth) {
        self.servers.insert(server_key, entry);
    }

    pub fn remove(&mut self, server_key: &str) {
        self.servers.remove(server_key);
    }
}

// ---------------------------------------------------------------------------
// Server key generation
// ---------------------------------------------------------------------------

/// Generate a unique key for server credentials based on server name + config
/// hash (matching the TS `getServerKey`).
pub fn get_server_key(server_name: &str, server_url: &str) -> String {
    let input = server_url.to_string();
    let hash = hex::encode(Sha256::digest(input.as_bytes()));
    format!("{}|{}", server_name, &hash[..16])
}

// ---------------------------------------------------------------------------
// Needs-auth cache (TTL-based)
// ---------------------------------------------------------------------------

const AUTH_CACHE_TTL_SECS: u64 = 15 * 60; // 15 minutes

/// Short-lived cache recording which servers need authentication.
///
/// Prevents repeated 401 round-trips during a session when the user hasn't
/// completed the OAuth flow yet.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NeedsAuthCache {
    entries: HashMap<String, u64>, // server_key -> unix timestamp
}

impl NeedsAuthCache {
    fn path() -> Result<PathBuf> {
        Ok(claude_dir()?.join("mcp-needs-auth-cache.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn is_cached(&self, server_key: &str) -> bool {
        if let Some(&ts) = self.entries.get(server_key) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            now - ts < AUTH_CACHE_TTL_SECS
        } else {
            false
        }
    }

    pub fn mark(&mut self, server_key: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.entries.insert(server_key.to_string(), now);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Remove a single entry.
    pub fn remove(&mut self, server_key: &str) {
        self.entries.remove(server_key);
    }

    pub fn clear_file() {
        if let Ok(path) = Self::path() {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ---------------------------------------------------------------------------
// OAuth flow helpers
// ---------------------------------------------------------------------------

/// PKCE code verifier + challenge pair.
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
}

impl PkceChallenge {
    /// Generate a new random PKCE challenge using S256.
    pub fn generate() -> Self {
        use base64::Engine;
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let verifier_bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&verifier_bytes);

        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);

        Self {
            verifier,
            challenge,
        }
    }
}

/// Build the authorization URL for an OAuth code grant.
pub fn build_authorization_url(
    metadata: &OAuthServerMetadata,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    pkce: &PkceChallenge,
    scopes: &[&str],
) -> String {
    let mut params = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("state", state),
        ("code_challenge", &pkce.challenge),
        ("code_challenge_method", "S256"),
    ];
    let scope_str = scopes.join(" ");
    if !scope_str.is_empty() {
        params.push(("scope", &scope_str));
    }
    let query = params
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, urlencoding_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{}", metadata.authorization_endpoint, query)
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code_for_tokens(
    metadata: &OAuthServerMetadata,
    client_id: &str,
    client_secret: Option<&str>,
    code: &str,
    redirect_uri: &str,
    pkce_verifier: &str,
) -> Result<OAuthTokens> {
    let client = reqwest::Client::new();
    let mut params = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", pkce_verifier),
    ];
    // client_secret only for confidential clients
    if let Some(secret) = client_secret {
        params.push(("client_secret", secret));
    }

    let resp = client
        .post(&metadata.token_endpoint)
        .form(&params)
        .send()
        .await
        .context("token exchange request failed")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("token exchange failed: {body}");
    }

    let body: serde_json::Value = resp.json().await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(OAuthTokens {
        access_token: body
            .get("access_token")
            .and_then(|v| v.as_str())
            .map(String::from),
        refresh_token: body
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(String::from),
        token_type: body
            .get("token_type")
            .and_then(|v| v.as_str())
            .map(String::from),
        expires_at: body
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .map(|secs| now + secs),
    })
}

/// Refresh an access token using a refresh token.
pub async fn refresh_access_token(
    metadata: &OAuthServerMetadata,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> Result<OAuthTokens> {
    let client = reqwest::Client::new();
    let mut params = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if let Some(secret) = client_secret {
        params.push(("client_secret", secret));
    }

    let resp = client
        .post(&metadata.token_endpoint)
        .form(&params)
        .send()
        .await
        .context("token refresh request failed")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("token refresh failed: {body}");
    }

    let body: serde_json::Value = resp.json().await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(OAuthTokens {
        access_token: body
            .get("access_token")
            .and_then(|v| v.as_str())
            .map(String::from),
        refresh_token: body
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| Some(refresh_token.to_string())),
        token_type: body
            .get("token_type")
            .and_then(|v| v.as_str())
            .map(String::from),
        expires_at: body
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .map(|secs| now + secs),
    })
}

/// Revoke a token at the server's revocation endpoint.
pub async fn revoke_token(
    endpoint: &str,
    token: &str,
    token_type_hint: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<()> {
    let client = reqwest::Client::new();
    let mut params = vec![
        ("token", token),
        ("token_type_hint", token_type_hint),
        ("client_id", client_id),
    ];
    if let Some(secret) = client_secret {
        params.push(("client_secret", secret));
    }

    let resp = client.post(endpoint).form(&params).send().await?;
    if !resp.status().is_success() {
        // RFC 7009: server may return an error but revocation is best-effort.
        let body = resp.text().await.unwrap_or_default();
        warn!("token revocation returned non-success: {body}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// OAuth discovery
// ---------------------------------------------------------------------------

/// Discover OAuth server metadata from the standard well-known endpoints.
///
/// Tries RFC 9728 (protected resource metadata) first, then falls back to
/// RFC 8414 (authorization server metadata).
pub async fn discover_oauth_metadata(server_url: &str) -> Result<OAuthServerMetadata> {
    let base = url::Url::parse(server_url).context("invalid server URL")?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Try RFC 8414 path-aware discovery first.
    let well_known = format!(
        "{}/.well-known/oauth-authorization-server{}",
        base.origin().unicode_serialization(),
        base.path().trim_end_matches('/')
    );
    if let Ok(resp) = client
        .get(&well_known)
        .header("Accept", "application/json")
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(meta) = resp.json::<OAuthServerMetadata>().await {
                return Ok(meta);
            }
        }
    }

    // Fallback: root-level .well-known.
    let root_well_known = format!(
        "{}/.well-known/oauth-authorization-server",
        base.origin().unicode_serialization()
    );
    let resp = client
        .get(&root_well_known)
        .header("Accept", "application/json")
        .send()
        .await
        .context("OAuth metadata discovery failed")?;
    if !resp.status().is_success() {
        bail!(
            "OAuth metadata endpoint returned HTTP {}",
            resp.status().as_u16()
        );
    }
    resp.json::<OAuthServerMetadata>()
        .await
        .context("failed to parse OAuth metadata")
}

// ---------------------------------------------------------------------------
// OAuth callback server (localhost redirect receiver)
// ---------------------------------------------------------------------------

/// Start a temporary HTTP server on a local port to receive the OAuth callback.
///
/// Returns `(port, receiver)`. The receiver yields the authorization code
/// (or an error) once the callback is received. The server shuts down after
/// a single request.
pub async fn start_oauth_callback_server(
    preferred_port: Option<u16>,
) -> Result<(u16, tokio::sync::oneshot::Receiver<Result<String>>)> {
    use tokio::net::TcpListener;

    let addr = format!("127.0.0.1:{}", preferred_port.unwrap_or(0));
    let listener = TcpListener::bind(&addr)
        .await
        .context("failed to bind OAuth callback listener")?;
    let port = listener.local_addr()?.port();

    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String>>();
    let tx = Arc::new(Mutex::new(Some(tx)));

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);

            // Parse the request line to extract query parameters.
            let code = parse_callback_code(&request);

            // Send a user-friendly response back to the browser.
            let body = if code.is_ok() {
                "<html><body><h1>Authentication successful</h1><p>You can close this tab.</p></body></html>"
            } else {
                "<html><body><h1>Authentication failed</h1><p>Please try again.</p></body></html>"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;

            if let Some(sender) = tx.lock().await.take() {
                let _ = sender.send(code);
            }
        }
    });

    Ok((port, rx))
}

/// Parse an authorization code from an HTTP request's query string.
fn parse_callback_code(request: &str) -> Result<String> {
    // Extract the request path from the first line (e.g. "GET /callback?code=abc&state=xyz HTTP/1.1")
    let first_line = request.lines().next().unwrap_or("");
    let path = first_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("");

    if let Some(query) = path.split_once('?').map(|(_, q)| q) {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key == "code" {
                    return Ok(urlencoding_decode(value));
                }
                if key == "error" {
                    bail!("OAuth error: {}", urlencoding_decode(value));
                }
            }
        }
    }
    bail!("no authorization code in callback")
}

// ---------------------------------------------------------------------------
// Auth provider (high-level, per-server)
// ---------------------------------------------------------------------------

/// Errors specific to MCP authentication.
#[derive(Debug, thiserror::Error)]
pub enum McpAuthError {
    #[error("MCP server {server_name} requires authentication")]
    AuthRequired { server_name: String },
    #[error("authentication cancelled for {server_name}")]
    Cancelled { server_name: String },
    #[error("OAuth token refresh failed for {server_name}: {reason}")]
    RefreshFailed {
        server_name: String,
        reason: String,
    },
    #[error("MCP session expired for {server_name}")]
    SessionExpired { server_name: String },
}

/// High-level auth provider for a single MCP server.
///
/// Manages token storage, refresh, and the full OAuth code-grant flow.
pub struct McpAuthProvider {
    server_name: String,
    server_url: String,
    server_key: String,
    oauth_config: Option<super::types::McpOAuthConfig>,
    store: Arc<Mutex<OAuthStore>>,
}

impl McpAuthProvider {
    pub fn new(
        server_name: &str,
        server_url: &str,
        oauth_config: Option<super::types::McpOAuthConfig>,
    ) -> Self {
        let server_key = get_server_key(server_name, server_url);
        Self {
            server_name: server_name.to_string(),
            server_url: server_url.to_string(),
            server_key,
            oauth_config,
            store: Arc::new(Mutex::new(OAuthStore::load())),
        }
    }

    /// Return stored tokens if available and not expired.
    pub async fn tokens(&self) -> Option<OAuthTokens> {
        let store = self.store.lock().await;
        let entry = store.get(&self.server_key)?;
        let tokens = OAuthTokens {
            access_token: entry.access_token.clone(),
            refresh_token: entry.refresh_token.clone(),
            token_type: Some("Bearer".into()),
            expires_at: entry.expires_at,
        };
        if tokens.access_token.is_some() {
            Some(tokens)
        } else {
            None
        }
    }

    /// Whether we have stored discovery state but no usable token.
    pub async fn has_discovery_but_no_token(&self) -> bool {
        let store = self.store.lock().await;
        match store.get(&self.server_key) {
            Some(entry) => entry.access_token.is_none() && entry.refresh_token.is_none(),
            None => false,
        }
    }

    /// Attempt to refresh the access token.
    pub async fn refresh(&self) -> Result<OAuthTokens> {
        let (metadata, client_id, client_secret, refresh_token) = {
            let store = self.store.lock().await;
            let entry = store
                .get(&self.server_key)
                .ok_or_else(|| anyhow!("no stored OAuth state for {}", self.server_name))?;
            let meta = entry
                .server_metadata
                .clone()
                .ok_or_else(|| anyhow!("no server metadata for refresh"))?;
            let cid = entry
                .client_id
                .clone()
                .ok_or_else(|| anyhow!("no client_id for refresh"))?;
            let rt = entry
                .refresh_token
                .clone()
                .ok_or_else(|| anyhow!("no refresh_token for {}", self.server_name))?;
            (meta, cid, entry.client_secret.clone(), rt)
        };

        let tokens = refresh_access_token(
            &metadata,
            &client_id,
            client_secret.as_deref(),
            &refresh_token,
        )
        .await?;

        // Persist the refreshed tokens.
        {
            let mut store = self.store.lock().await;
            if let Some(entry) = store.servers.get_mut(&self.server_key) {
                entry.access_token = tokens.access_token.clone();
                if let Some(ref rt) = tokens.refresh_token {
                    entry.refresh_token = Some(rt.clone());
                }
                entry.expires_at = tokens.expires_at;
            }
            store.save().ok();
        }

        Ok(tokens)
    }

    /// Run the full interactive OAuth flow (discover, authorize, exchange).
    ///
    /// Opens a browser for user consent, waits for the callback, then
    /// exchanges the code for tokens.
    pub async fn authenticate(&self) -> Result<OAuthTokens> {
        debug!(server = %self.server_name, "starting OAuth flow");

        let metadata = discover_oauth_metadata(&self.server_url).await?;
        let client_id = self
            .oauth_config
            .as_ref()
            .and_then(|c| c.client_id.clone())
            .unwrap_or_else(|| "claude-code".to_string());

        let callback_port = self
            .oauth_config
            .as_ref()
            .and_then(|c| c.callback_port);

        let (port, code_rx) = start_oauth_callback_server(callback_port).await?;
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let pkce = PkceChallenge::generate();
        let state = uuid::Uuid::new_v4().to_string();

        let auth_url = build_authorization_url(
            &metadata,
            &client_id,
            &redirect_uri,
            &state,
            &pkce,
            &[],
        );

        // Open the browser.
        debug!(server = %self.server_name, "opening browser for OAuth");
        if let Err(e) = open_browser(&auth_url) {
            warn!(server = %self.server_name, "failed to open browser: {e}");
        }

        // Wait for the callback (with timeout).
        let code = tokio::time::timeout(std::time::Duration::from_secs(300), code_rx)
            .await
            .map_err(|_| {
                McpAuthError::Cancelled {
                    server_name: self.server_name.clone(),
                }
            })?
            .map_err(|_| {
                McpAuthError::Cancelled {
                    server_name: self.server_name.clone(),
                }
            })??;

        let tokens = exchange_code_for_tokens(
            &metadata,
            &client_id,
            None,
            &code,
            &redirect_uri,
            &pkce.verifier,
        )
        .await?;

        // Persist.
        {
            let mut store = self.store.lock().await;
            store.set(
                self.server_key.clone(),
                StoredServerOAuth {
                    access_token: tokens.access_token.clone(),
                    refresh_token: tokens.refresh_token.clone(),
                    expires_at: tokens.expires_at,
                    client_id: Some(client_id),
                    client_secret: None,
                    server_metadata: Some(metadata),
                },
            );
            store.save().ok();
        }

        // Clear the needs-auth cache entry.
        let mut cache = NeedsAuthCache::load();
        cache.remove(&self.server_key);
        cache.save().ok();

        Ok(tokens)
    }

    /// Remove all stored credentials for this server.
    pub async fn invalidate(&self) {
        let mut store = self.store.lock().await;
        store.remove(&self.server_key);
        store.save().ok();
    }
}

// ---------------------------------------------------------------------------
// Detect session-expired errors
// ---------------------------------------------------------------------------

/// Returns `true` if an error looks like an MCP "Session not found" error
/// (HTTP 404 + JSON-RPC code -32001).
pub fn is_session_expired_error(error_message: &str) -> bool {
    error_message.contains("404")
        && (error_message.contains("\"code\":-32001")
            || error_message.contains("\"code\": -32001"))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Minimal percent-encoding for URL query values.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

/// Minimal percent-decoding.
fn urlencoding_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) =
                u8::from_str_radix(&s[i + 1..i + 3], 16)
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Try to open a URL in the system browser.
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_key() {
        let key = get_server_key("my-server", "https://example.com/mcp");
        assert!(key.starts_with("my-server|"));
        assert_eq!(key.len(), "my-server|".len() + 16);
    }

    #[test]
    fn test_pkce_generation() {
        let p = PkceChallenge::generate();
        assert!(!p.verifier.is_empty());
        assert!(!p.challenge.is_empty());
        assert_ne!(p.verifier, p.challenge);
    }

    #[test]
    fn test_urlencoding_roundtrip() {
        let original = "hello world&foo=bar";
        let encoded = urlencoding_encode(original);
        let decoded = urlencoding_decode(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_is_session_expired() {
        assert!(is_session_expired_error(
            "HTTP 404: {\"error\":{\"code\":-32001,\"message\":\"Session not found\"}}"
        ));
        assert!(!is_session_expired_error("HTTP 500: internal error"));
    }

    #[test]
    fn test_token_expiry() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let expired = OAuthTokens {
            access_token: Some("tok".into()),
            expires_at: Some(now - 60),
            ..Default::default()
        };
        assert!(expired.is_expired(0));

        let valid = OAuthTokens {
            access_token: Some("tok".into()),
            expires_at: Some(now + 3600),
            ..Default::default()
        };
        assert!(!valid.is_expired(0));
    }

    #[test]
    fn test_parse_callback_code() {
        let req = "GET /callback?code=abc123&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(parse_callback_code(req).unwrap(), "abc123");
    }

    #[test]
    fn test_parse_callback_error() {
        let req =
            "GET /callback?error=access_denied&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert!(parse_callback_code(req).is_err());
    }
}
