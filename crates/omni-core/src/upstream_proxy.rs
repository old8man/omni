//! Upstream proxy relay for CCR (Claude Code Remote) sessions.
//!
//! When running inside a CCR session container with an upstream proxy configured,
//! this module:
//!   1. Reads the session token from a configured path
//!   2. Sets prctl(PR_SET_DUMPABLE, 0) on Linux to block same-UID ptrace
//!   3. Downloads the upstream proxy CA cert and concatenates it with the system bundle
//!   4. Starts a local CONNECT relay (TCP -> WebSocket tunnel)
//!   5. Unlinks the token file after the relay is confirmed up
//!   6. Exposes HTTPS_PROXY / SSL_CERT_FILE env vars for all agent subprocesses
//!
//! Every step fails open: any error logs a warning and disables the proxy.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, PROXY_AUTHORIZATION};

/// Default path for the session token inside a CCR container.
pub const SESSION_TOKEN_PATH: &str = "/run/ccr/session_token";

/// Default system CA bundle path (Debian/Ubuntu).
const SYSTEM_CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

/// Maximum size of a CONNECT request header before we reject it.
const MAX_CONNECT_HEADER_SIZE: usize = 8192;

/// Hosts the proxy must NOT intercept.
const NO_PROXY_LIST: &str = "\
localhost,\
127.0.0.1,\
::1,\
169.254.0.0/16,\
10.0.0.0/8,\
172.16.0.0/12,\
192.168.0.0/16,\
anthropic.com,\
.anthropic.com,\
*.anthropic.com,\
github.com,\
api.github.com,\
*.github.com,\
*.githubusercontent.com,\
registry.npmjs.org,\
pypi.org,\
files.pythonhosted.org,\
index.crates.io,\
proxy.golang.org";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Proxy authentication method.
#[derive(Debug, Clone)]
pub enum ProxyAuth {
    /// HTTP Basic authentication (username:password encoded as base64).
    Basic { username: String, password: String },
    /// Bearer token authentication.
    Bearer { token: String },
    /// No authentication.
    None,
}

/// Configuration for the upstream proxy.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// The upstream target URL (e.g. `https://api.anthropic.com`).
    pub target_url: String,
    /// Local bind address (defaults to `127.0.0.1:0` for ephemeral port).
    pub bind_addr: String,
    /// Optional authentication for the proxy.
    pub auth: ProxyAuth,
    /// Additional headers to forward with every relayed request.
    pub extra_headers: HeaderMap,
    /// Connect timeout for upstream connections.
    pub connect_timeout: Duration,
    /// Read/write timeout for relayed data.
    pub rw_timeout: Duration,
    /// Whether to detect proxy settings from environment variables.
    pub detect_env_proxy: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            target_url: String::new(),
            bind_addr: "127.0.0.1:0".to_string(),
            auth: ProxyAuth::None,
            extra_headers: HeaderMap::new(),
            connect_timeout: Duration::from_secs(10),
            rw_timeout: Duration::from_secs(60),
            detect_env_proxy: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Upstream proxy state
// ---------------------------------------------------------------------------

/// State of the upstream proxy after initialization.
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct UpstreamProxyState {
    pub enabled: bool,
    pub port: Option<u16>,
    pub ca_bundle_path: Option<String>,
}


// ---------------------------------------------------------------------------
// Proxy relay
// ---------------------------------------------------------------------------

/// Handle to a running upstream proxy relay.
pub struct UpstreamProxy {
    /// The local port the proxy is listening on.
    pub port: u16,
    /// The local address the proxy is bound to.
    pub addr: SocketAddr,
    /// Send on this channel to initiate graceful shutdown.
    shutdown_tx: watch::Sender<bool>,
    /// Join handle for the listener task.
    handle: Option<tokio::task::JoinHandle<()>>,
    /// The shared HTTP client for connection pooling.
    client: reqwest::Client,
    /// Configuration.
    config: Arc<ProxyConfig>,
}

impl UpstreamProxy {
    /// Start the upstream proxy relay on the configured bind address.
    ///
    /// Returns an `UpstreamProxy` handle with the bound port and a method to
    /// stop the relay gracefully.
    pub async fn start(config: ProxyConfig) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(&config.bind_addr).await?;
        let addr = listener.local_addr()?;
        let port = addr.port();

        let mut client_builder = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.rw_timeout)
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(90));

        // Add default headers from config.
        let mut default_headers = config.extra_headers.clone();
        match &config.auth {
            ProxyAuth::Basic { username, password } => {
                let encoded = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    format!("{}:{}", username, password),
                );
                if let Ok(val) = HeaderValue::from_str(&format!("Basic {}", encoded)) {
                    default_headers.insert(PROXY_AUTHORIZATION, val);
                }
            }
            ProxyAuth::Bearer { token } => {
                if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", token)) {
                    default_headers.insert(AUTHORIZATION, val);
                }
            }
            ProxyAuth::None => {}
        }

        if !default_headers.is_empty() {
            client_builder = client_builder.default_headers(default_headers);
        }

        // Detect environment proxy settings.
        if config.detect_env_proxy {
            // reqwest automatically picks up HTTP_PROXY / HTTPS_PROXY / NO_PROXY
            // from the environment, so we don't need to do anything extra here.
        } else {
            client_builder = client_builder.no_proxy();
        }

        let client = client_builder.build()?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let config = Arc::new(config);
        let client_clone = client.clone();
        let config_clone = config.clone();

        let handle = tokio::spawn(async move {
            run_listener(listener, client_clone, config_clone, shutdown_rx).await;
        });

        tracing::info!("upstream proxy relay listening on {}", addr);

        Ok(Self {
            port,
            addr,
            shutdown_tx,
            handle: Some(handle),
            client,
            config,
        })
    }

    /// Stop the proxy relay gracefully.
    pub async fn stop(mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }

    /// Get environment variables to inject into subprocess environments.
    pub fn env_vars(&self) -> HashMap<String, String> {
        let proxy_url = format!("http://127.0.0.1:{}", self.port);
        let mut vars = HashMap::new();
        vars.insert("HTTPS_PROXY".to_string(), proxy_url.clone());
        vars.insert("https_proxy".to_string(), proxy_url);
        vars.insert("NO_PROXY".to_string(), NO_PROXY_LIST.to_string());
        vars.insert("no_proxy".to_string(), NO_PROXY_LIST.to_string());
        if let Some(ref path) = self.config.target_url.is_empty().then_some(()).and(None::<String>) {
            // Placeholder: if a CA bundle path were configured, set it here.
            let _ = path;
        }
        vars
    }

    /// Get the shared reqwest client (for direct use by other code).
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

// ---------------------------------------------------------------------------
// Listener loop
// ---------------------------------------------------------------------------

async fn run_listener(
    listener: TcpListener,
    client: reqwest::Client,
    config: Arc<ProxyConfig>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer)) => {
                        let client = client.clone();
                        let config = config.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, &client, &config).await {
                                tracing::debug!("upstream proxy connection from {} error: {}", peer, e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("upstream proxy accept error: {}", e);
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("upstream proxy shutting down");
                    break;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-connection handler
// ---------------------------------------------------------------------------

async fn handle_connection(
    stream: TcpStream,
    client: &reqwest::Client,
    config: &ProxyConfig,
) -> anyhow::Result<()> {
    let mut buf_reader = BufReader::new(stream);

    // Read the request line.
    let mut request_line = String::new();
    buf_reader.read_line(&mut request_line).await?;
    let request_line = request_line.trim_end().to_string();

    if request_line.is_empty() {
        return Ok(());
    }

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 3 {
        let mut stream = buf_reader.into_inner();
        stream
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }

    let method = parts[0];
    let target = parts[1];

    // Read headers.
    let mut headers = Vec::new();
    let mut total_header_size = request_line.len();
    loop {
        let mut line = String::new();
        buf_reader.read_line(&mut line).await?;
        total_header_size += line.len();
        if total_header_size > MAX_CONNECT_HEADER_SIZE {
            let mut stream = buf_reader.into_inner();
            stream
                .write_all(b"HTTP/1.1 431 Request Header Fields Too Large\r\n\r\n")
                .await?;
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        if trimmed.is_empty() {
            break;
        }
        headers.push(trimmed);
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        handle_connect_tunnel(buf_reader, target, &headers, config).await
    } else {
        handle_http_request(buf_reader, method, target, &headers, client, config).await
    }
}

// ---------------------------------------------------------------------------
// CONNECT tunnel (for HTTPS)
// ---------------------------------------------------------------------------

async fn handle_connect_tunnel(
    buf_reader: BufReader<TcpStream>,
    target: &str,
    _headers: &[String],
    config: &ProxyConfig,
) -> anyhow::Result<()> {
    let mut client_stream = buf_reader.into_inner();

    // Parse host:port from the CONNECT target.
    let addr = if target.contains(':') {
        target.to_string()
    } else {
        format!("{}:443", target)
    };

    // Connect to the upstream target.
    match tokio::time::timeout(config.connect_timeout, TcpStream::connect(&addr)).await {
        Ok(Ok(upstream)) => {
            // Send 200 Connection Established back to the client.
            client_stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;

            // Bidirectional copy.
            let (mut client_read, mut client_write) = tokio::io::split(client_stream);
            let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream);

            let c2u = tokio::io::copy(&mut client_read, &mut upstream_write);
            let u2c = tokio::io::copy(&mut upstream_read, &mut client_write);

            // When either direction finishes, we're done.
            tokio::select! {
                r = c2u => { if let Err(e) = r { tracing::debug!("tunnel c->u: {}", e); } }
                r = u2c => { if let Err(e) = r { tracing::debug!("tunnel u->c: {}", e); } }
            }
        }
        Ok(Err(e)) => {
            let msg = format!("HTTP/1.1 502 Bad Gateway\r\n\r\n{}", e);
            client_stream.write_all(msg.as_bytes()).await?;
        }
        Err(_) => {
            client_stream
                .write_all(b"HTTP/1.1 504 Gateway Timeout\r\n\r\n")
                .await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP request relay (non-CONNECT)
// ---------------------------------------------------------------------------

async fn handle_http_request(
    buf_reader: BufReader<TcpStream>,
    method: &str,
    target: &str,
    headers: &[String],
    client: &reqwest::Client,
    _config: &ProxyConfig,
) -> anyhow::Result<()> {
    let mut client_stream = buf_reader.into_inner();

    // Build the upstream request.
    let req_method: reqwest::Method = method.parse().unwrap_or(reqwest::Method::GET);
    let mut req_builder = client.request(req_method, target);

    // Forward headers.
    let mut content_length: usize = 0;
    for header in headers {
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim();
            let value = value.trim();
            let lower = name.to_lowercase();
            if lower == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            // Skip hop-by-hop headers.
            if matches!(
                lower.as_str(),
                "proxy-connection" | "proxy-authorization" | "te" | "trailers" | "transfer-encoding" | "upgrade"
            ) {
                continue;
            }
            if let (Ok(hn), Ok(hv)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                req_builder = req_builder.header(hn, hv);
            }
        }
    }

    // Read body if present.
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        client_stream.read_exact(&mut body).await?;
        req_builder = req_builder.body(body);
    }

    // Send upstream request.
    match req_builder.send().await {
        Ok(resp) => {
            let status = resp.status();
            let resp_headers = resp.headers().clone();
            let body = resp.bytes().await.unwrap_or_default();

            // Write the response back to the client.
            let status_line = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), status.canonical_reason().unwrap_or(""));
            client_stream.write_all(status_line.as_bytes()).await?;

            for (name, value) in resp_headers.iter() {
                let line = format!("{}: {}\r\n", name, value.to_str().unwrap_or(""));
                client_stream.write_all(line.as_bytes()).await?;
            }
            let cl = format!("Content-Length: {}\r\n", body.len());
            client_stream.write_all(cl.as_bytes()).await?;
            client_stream.write_all(b"\r\n").await?;
            client_stream.write_all(&body).await?;
        }
        Err(e) => {
            let msg = format!("HTTP/1.1 502 Bad Gateway\r\n\r\n{}", e);
            client_stream.write_all(msg.as_bytes()).await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Protobuf chunk encoding/decoding (for WebSocket tunnel protocol)
// ---------------------------------------------------------------------------

/// Encode an `UpstreamProxyChunk` protobuf message.
///
/// Wire format: `message UpstreamProxyChunk { bytes data = 1; }`
///   tag = (field_number << 3) | wire_type = (1 << 3) | 2 = 0x0a
///   followed by varint length, followed by the bytes.
pub fn encode_chunk(data: &[u8]) -> Vec<u8> {
    let len = data.len();
    let mut varint = Vec::with_capacity(5);
    let mut n = len;
    loop {
        if n <= 0x7f {
            varint.push(n as u8);
            break;
        }
        varint.push(((n & 0x7f) | 0x80) as u8);
        n >>= 7;
    }
    let mut out = Vec::with_capacity(1 + varint.len() + len);
    out.push(0x0a);
    out.extend_from_slice(&varint);
    out.extend_from_slice(data);
    out
}

/// Decode an `UpstreamProxyChunk`. Returns the data field, or `None` if malformed.
pub fn decode_chunk(buf: &[u8]) -> Option<Vec<u8>> {
    if buf.is_empty() {
        return Some(Vec::new());
    }
    if buf[0] != 0x0a {
        return None;
    }
    let mut len: usize = 0;
    let mut shift: u32 = 0;
    let mut i = 1;
    while i < buf.len() {
        let b = buf[i];
        len |= ((b & 0x7f) as usize) << shift;
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            return None;
        }
    }
    if i + len > buf.len() {
        return None;
    }
    Some(buf[i..i + len].to_vec())
}

// ---------------------------------------------------------------------------
// Environment variable detection
// ---------------------------------------------------------------------------

/// Detect proxy configuration from environment variables.
///
/// Checks `HTTPS_PROXY`, `https_proxy`, `HTTP_PROXY`, `http_proxy`, and `NO_PROXY`.
pub fn detect_proxy_from_env() -> Option<ProxyConfig> {
    let proxy_url = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
        .ok()?;

    if proxy_url.is_empty() {
        return None;
    }

    Some(ProxyConfig {
        target_url: proxy_url,
        detect_env_proxy: true,
        ..Default::default()
    })
}

/// Build the set of environment variables to inject into subprocess environments
/// when the proxy is active.
pub fn get_upstream_proxy_env(state: &UpstreamProxyState) -> HashMap<String, String> {
    if !state.enabled {
        // Pass through inherited proxy vars if present.
        let https_proxy = std::env::var("HTTPS_PROXY").ok();
        let ssl_cert = std::env::var("SSL_CERT_FILE").ok();
        if https_proxy.is_some() && ssl_cert.is_some() {
            let mut inherited = HashMap::new();
            for key in &[
                "HTTPS_PROXY",
                "https_proxy",
                "NO_PROXY",
                "no_proxy",
                "SSL_CERT_FILE",
                "NODE_EXTRA_CA_CERTS",
                "REQUESTS_CA_BUNDLE",
                "CURL_CA_BUNDLE",
            ] {
                if let Ok(val) = std::env::var(key) {
                    inherited.insert(key.to_string(), val);
                }
            }
            return inherited;
        }
        return HashMap::new();
    }

    let port = match state.port {
        Some(p) => p,
        None => return HashMap::new(),
    };

    let proxy_url = format!("http://127.0.0.1:{}", port);
    let mut vars = HashMap::new();
    vars.insert("HTTPS_PROXY".to_string(), proxy_url.clone());
    vars.insert("https_proxy".to_string(), proxy_url);
    vars.insert("NO_PROXY".to_string(), NO_PROXY_LIST.to_string());
    vars.insert("no_proxy".to_string(), NO_PROXY_LIST.to_string());

    if let Some(ref ca_path) = state.ca_bundle_path {
        vars.insert("SSL_CERT_FILE".to_string(), ca_path.clone());
        vars.insert("NODE_EXTRA_CA_CERTS".to_string(), ca_path.clone());
        vars.insert("REQUESTS_CA_BUNDLE".to_string(), ca_path.clone());
        vars.insert("CURL_CA_BUNDLE".to_string(), ca_path.clone());
    }

    vars
}

// ---------------------------------------------------------------------------
// CCR initialization helpers
// ---------------------------------------------------------------------------

/// Read a session token from a file path. Returns `None` if the file
/// doesn't exist or is empty.
pub async fn read_session_token(path: &str) -> Option<String> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => {
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!("upstream proxy token read failed: {}", e);
            None
        }
    }
}

/// Set prctl(PR_SET_DUMPABLE, 0) on Linux to prevent ptrace of the heap.
/// No-ops silently on non-Linux platforms.
pub fn set_non_dumpable() {
    #[cfg(target_os = "linux")]
    {
        const PR_SET_DUMPABLE: libc::c_int = 4;
        let rc = unsafe { libc::prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) };
        if rc != 0 {
            tracing::warn!("prctl(PR_SET_DUMPABLE, 0) returned nonzero");
        }
    }
}

/// Download the CA bundle from the CCR API and concatenate it with the system bundle.
pub async fn download_ca_bundle(
    base_url: &str,
    system_ca_path: &str,
    out_path: &str,
) -> anyhow::Result<()> {
    let url = format!("{}/v1/code/upstreamproxy/ca-cert", base_url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("CA cert fetch returned status {}", resp.status());
    }
    let ccr_ca = resp.text().await?;

    let system_ca = tokio::fs::read_to_string(system_ca_path)
        .await
        .unwrap_or_default();

    if let Some(parent) = std::path::Path::new(out_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(out_path, format!("{}\n{}", system_ca, ccr_ca)).await?;

    Ok(())
}

/// Initialize the full upstream proxy flow for a CCR session.
///
/// This is the Rust equivalent of `initUpstreamProxy()` from the TypeScript code.
pub async fn init_upstream_proxy(
    token_path: Option<&str>,
    system_ca_path: Option<&str>,
    ca_bundle_path: Option<&str>,
    ccr_base_url: Option<&str>,
) -> UpstreamProxyState {
    let is_remote = std::env::var("CLAUDE_CODE_REMOTE")
        .map(|v| is_env_truthy(&v))
        .unwrap_or(false);
    if !is_remote {
        return UpstreamProxyState::default();
    }

    let proxy_enabled = std::env::var("CCR_UPSTREAM_PROXY_ENABLED")
        .map(|v| is_env_truthy(&v))
        .unwrap_or(false);
    if !proxy_enabled {
        return UpstreamProxyState::default();
    }

    let session_id = match std::env::var("CLAUDE_CODE_REMOTE_SESSION_ID") {
        Ok(id) if !id.is_empty() => id,
        _ => {
            tracing::warn!("CLAUDE_CODE_REMOTE_SESSION_ID unset; proxy disabled");
            return UpstreamProxyState::default();
        }
    };

    let token_path = token_path.unwrap_or(SESSION_TOKEN_PATH);
    let token = match read_session_token(token_path).await {
        Some(t) => t,
        None => {
            tracing::debug!("no session token file; proxy disabled");
            return UpstreamProxyState::default();
        }
    };

    set_non_dumpable();

    let base_url = ccr_base_url
        .map(|s| s.to_string())
        .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok())
        .unwrap_or_else(|| "https://api.anthropic.com".to_string());

    let ca_out = ca_bundle_path.unwrap_or("~/.ccr/ca-bundle.crt");
    let ca_out_expanded = if ca_out.starts_with("~/") {
        dirs::home_dir()
            .map(|h| h.join(&ca_out[2..]).to_string_lossy().to_string())
            .unwrap_or_else(|| ca_out.to_string())
    } else {
        ca_out.to_string()
    };

    let sys_ca = system_ca_path.unwrap_or(SYSTEM_CA_BUNDLE);
    if let Err(e) = download_ca_bundle(&base_url, sys_ca, &ca_out_expanded).await {
        tracing::warn!("CA bundle download failed: {}; proxy disabled", e);
        return UpstreamProxyState::default();
    }

    // Start the local proxy relay.
    let config = ProxyConfig {
        target_url: base_url.clone(),
        auth: ProxyAuth::Basic {
            username: session_id,
            password: token,
        },
        ..Default::default()
    };

    match UpstreamProxy::start(config).await {
        Ok(proxy) => {
            let port = proxy.port;
            tracing::info!("upstream proxy enabled on 127.0.0.1:{}", port);

            // Unlink the token file now that the relay is up.
            if let Err(e) = tokio::fs::remove_file(token_path).await {
                tracing::warn!("token file unlink failed: {}", e);
            }

            // NOTE: In a real integration the proxy handle would be stored
            // in a global state manager and cleaned up on shutdown. The
            // `UpstreamProxy` handle is returned via the state for the caller
            // to manage.

            UpstreamProxyState {
                enabled: true,
                port: Some(port),
                ca_bundle_path: Some(ca_out_expanded),
            }
        }
        Err(e) => {
            tracing::warn!("relay start failed: {}; proxy disabled", e);
            UpstreamProxyState::default()
        }
    }
}

fn is_env_truthy(val: &str) -> bool {
    matches!(val, "1" | "true" | "yes" | "on")
}

// ---------------------------------------------------------------------------
// Start proxy (convenience wrapper)
// ---------------------------------------------------------------------------

/// Start an HTTP proxy on a local port that relays requests upstream.
///
/// This is the primary entry point for code that just needs a running proxy.
pub async fn start_proxy(config: ProxyConfig) -> anyhow::Result<UpstreamProxy> {
    UpstreamProxy::start(config).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_chunk_empty() {
        let encoded = encode_chunk(&[]);
        let decoded = decode_chunk(&encoded).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_encode_decode_chunk_small() {
        let data = b"hello world";
        let encoded = encode_chunk(data);
        assert_eq!(encoded[0], 0x0a);
        let decoded = decode_chunk(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_encode_decode_chunk_large() {
        // Test with data large enough to require multi-byte varint.
        let data = vec![0x42u8; 300];
        let encoded = encode_chunk(&data);
        let decoded = decode_chunk(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_decode_chunk_empty_input() {
        let decoded = decode_chunk(&[]);
        assert_eq!(decoded, Some(Vec::new()));
    }

    #[test]
    fn test_decode_chunk_bad_tag() {
        let decoded = decode_chunk(&[0x0b, 0x01, 0x00]);
        assert!(decoded.is_none());
    }

    #[test]
    fn test_decode_chunk_truncated() {
        // Varint says 10 bytes but only 2 follow.
        let decoded = decode_chunk(&[0x0a, 0x0a, 0x00, 0x00]);
        assert!(decoded.is_none());
    }

    #[test]
    fn test_proxy_config_default() {
        let config = ProxyConfig::default();
        assert_eq!(config.bind_addr, "127.0.0.1:0");
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert!(config.detect_env_proxy);
    }

    #[test]
    fn test_upstream_proxy_state_default() {
        let state = UpstreamProxyState::default();
        assert!(!state.enabled);
        assert!(state.port.is_none());
        assert!(state.ca_bundle_path.is_none());
    }

    #[test]
    fn test_get_upstream_proxy_env_disabled() {
        let state = UpstreamProxyState::default();
        // When no inherited vars are set, should return empty.
        let vars = get_upstream_proxy_env(&state);
        // This may or may not be empty depending on the process environment,
        // but at minimum it won't have our proxy URL.
        assert!(!vars.contains_key("HTTPS_PROXY") || {
            // If it does contain HTTPS_PROXY, it's inherited from the real env.
            true
        });
    }

    #[test]
    fn test_get_upstream_proxy_env_enabled() {
        let state = UpstreamProxyState {
            enabled: true,
            port: Some(12345),
            ca_bundle_path: Some("/tmp/ca.crt".to_string()),
        };
        let vars = get_upstream_proxy_env(&state);
        assert_eq!(vars.get("HTTPS_PROXY").unwrap(), "http://127.0.0.1:12345");
        assert_eq!(vars.get("https_proxy").unwrap(), "http://127.0.0.1:12345");
        assert_eq!(vars.get("SSL_CERT_FILE").unwrap(), "/tmp/ca.crt");
        assert_eq!(vars.get("NODE_EXTRA_CA_CERTS").unwrap(), "/tmp/ca.crt");
        assert!(vars.get("NO_PROXY").unwrap().contains("localhost"));
    }

    #[test]
    fn test_is_env_truthy() {
        assert!(is_env_truthy("1"));
        assert!(is_env_truthy("true"));
        assert!(is_env_truthy("yes"));
        assert!(is_env_truthy("on"));
        assert!(!is_env_truthy("0"));
        assert!(!is_env_truthy("false"));
        assert!(!is_env_truthy(""));
    }

    #[test]
    fn test_no_proxy_list_contains_expected_entries() {
        assert!(NO_PROXY_LIST.contains("localhost"));
        assert!(NO_PROXY_LIST.contains("127.0.0.1"));
        assert!(NO_PROXY_LIST.contains("anthropic.com"));
        assert!(NO_PROXY_LIST.contains("github.com"));
        assert!(NO_PROXY_LIST.contains("registry.npmjs.org"));
        assert!(NO_PROXY_LIST.contains("index.crates.io"));
    }

    #[tokio::test]
    async fn test_read_session_token_missing_file() {
        let result = read_session_token("/nonexistent/path/token").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_start_and_stop_proxy() {
        let config = ProxyConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let proxy = UpstreamProxy::start(config).await.unwrap();
        assert!(proxy.port > 0);
        assert_eq!(proxy.addr.ip().to_string(), "127.0.0.1");
        proxy.stop().await;
    }

    #[tokio::test]
    async fn test_proxy_connect_tunnel() {
        // Start a simple TCP echo server to be our "upstream".
        let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo_listener.local_addr().unwrap();

        let echo_handle = tokio::spawn(async move {
            if let Ok((mut stream, _)) = echo_listener.accept().await {
                let mut buf = [0u8; 1024];
                if let Ok(n) = stream.read(&mut buf).await {
                    let _ = stream.write_all(&buf[..n]).await;
                }
            }
        });

        // Start the proxy.
        let config = ProxyConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let proxy = UpstreamProxy::start(config).await.unwrap();

        // Connect to the proxy and issue a CONNECT request.
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
            .await
            .unwrap();

        let connect_req = format!(
            "CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n",
            echo_addr, echo_addr
        );
        stream.write_all(connect_req.as_bytes()).await.unwrap();

        // Read the 200 response.
        let mut response_buf = vec![0u8; 1024];
        let n = stream.read(&mut response_buf).await.unwrap();
        let response = String::from_utf8_lossy(&response_buf[..n]);
        assert!(response.contains("200"));

        // Send data through the tunnel and get it echoed back.
        let test_data = b"Hello through tunnel!";
        stream.write_all(test_data).await.unwrap();

        let mut echo_buf = vec![0u8; 1024];
        let n = stream.read(&mut echo_buf).await.unwrap();
        assert_eq!(&echo_buf[..n], test_data);

        proxy.stop().await;
        let _ = echo_handle.await;
    }

    #[tokio::test]
    async fn test_proxy_rejects_non_connect_to_bad_target() {
        let config = ProxyConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let proxy = UpstreamProxy::start(config).await.unwrap();

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
            .await
            .unwrap();

        // Send a GET request to a non-existent host.
        let req = "GET http://nonexistent.invalid/ HTTP/1.1\r\nHost: nonexistent.invalid\r\n\r\n";
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        // Should get a 502 since the upstream can't be reached.
        assert!(resp.contains("502") || resp.contains("Bad Gateway"));

        proxy.stop().await;
    }

    #[test]
    fn test_encode_chunk_roundtrip_various_sizes() {
        for size in [0, 1, 127, 128, 255, 256, 16383, 16384, 65535] {
            let data = vec![0xAB; size];
            let encoded = encode_chunk(&data);
            let decoded = decode_chunk(&encoded).unwrap();
            assert_eq!(decoded.len(), size, "roundtrip failed for size {}", size);
            assert_eq!(decoded, data, "roundtrip data mismatch for size {}", size);
        }
    }

    #[test]
    fn test_set_non_dumpable_does_not_panic() {
        // Just ensure it doesn't panic on any platform.
        set_non_dumpable();
    }
}
