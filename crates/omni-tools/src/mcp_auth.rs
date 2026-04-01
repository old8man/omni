use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use omni_core::mcp::auth::{
    build_authorization_url, discover_oauth_metadata, start_oauth_callback_server,
    NeedsAuthCache, PkceChallenge,
};
use omni_core::mcp::config::build_mcp_tool_name;
use omni_core::mcp::manager::McpManager;
use omni_core::mcp::types::{McpTransportType, ScopedMcpServerConfig};
use omni_core::types::events::ToolResultData;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};

/// A pseudo-tool surfaced in place of real tools when an MCP server requires
/// OAuth authentication. Calling it starts the OAuth flow and returns an
/// authorization URL for the user to visit in their browser.
///
/// Once the user completes the flow, the server reconnects in the background
/// and its real tools replace this pseudo-tool.
pub struct McpAuthTool {
    full_name: String,
    server_name: String,
    config: ScopedMcpServerConfig,
    manager: Arc<McpManager>,
    tool_description: String,
}

impl McpAuthTool {
    /// Create a new auth pseudo-tool for the given MCP server.
    pub fn new(
        server_name: String,
        config: ScopedMcpServerConfig,
        manager: Arc<McpManager>,
    ) -> Self {
        let transport_label = match config.config.transport {
            McpTransportType::Stdio => "stdio".to_string(),
            McpTransportType::Sse | McpTransportType::SseIde => {
                if let Some(ref url) = config.config.url {
                    format!("sse at {url}")
                } else {
                    "sse".to_string()
                }
            }
            McpTransportType::Http => {
                if let Some(ref url) = config.config.url {
                    format!("http at {url}")
                } else {
                    "http".to_string()
                }
            }
            McpTransportType::Ws => {
                if let Some(ref url) = config.config.url {
                    format!("ws at {url}")
                } else {
                    "ws".to_string()
                }
            }
            McpTransportType::Sdk => "sdk".to_string(),
        };

        let description = format!(
            "The `{server_name}` MCP server ({transport_label}) is installed but requires authentication. \
             Call this tool to start the OAuth flow \u{2014} you'll receive an authorization URL to share \
             with the user. Once the user completes authorization in their browser, the server's real \
             tools will become available automatically."
        );

        let full_name = build_mcp_tool_name(&server_name, "authenticate");

        Self {
            full_name,
            server_name,
            config,
            manager,
            tool_description: description,
        }
    }
}

#[async_trait]
impl ToolExecutor for McpAuthTool {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> String {
        self.tool_description.clone()
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(
        &self,
        _input: &Value,
        _ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let transport = &self.config.config.transport;

        // Only SSE and HTTP transports support OAuth.
        if *transport != McpTransportType::Sse && *transport != McpTransportType::Http {
            let transport_name = format!("{transport:?}").to_lowercase();
            return Ok(ToolResultData {
                data: serde_json::json!({
                    "status": "unsupported",
                    "message": format!(
                        "Server \"{}\" uses {} transport which does not support OAuth from this tool. \
                         Ask the user to run /mcp and authenticate manually.",
                        self.server_name, transport_name
                    )
                }),
                is_error: false,
            });
        }

        let server_url = match &self.config.config.url {
            Some(url) => url.clone(),
            None => {
                return Ok(ToolResultData {
                    data: serde_json::json!({
                        "status": "error",
                        "message": format!(
                            "Server \"{}\" has no URL configured for OAuth. \
                             Ask the user to run /mcp and authenticate manually.",
                            self.server_name
                        )
                    }),
                    is_error: true,
                });
            }
        };

        let oauth_config = self.config.config.oauth.clone();

        // Discover OAuth server metadata.
        let metadata = match discover_oauth_metadata(&server_url).await {
            Ok(meta) => meta,
            Err(e) => {
                return Ok(ToolResultData {
                    data: serde_json::json!({
                        "status": "error",
                        "message": format!(
                            "Failed to discover OAuth metadata for {}: {e:#}. \
                             Ask the user to run /mcp and authenticate manually.",
                            self.server_name
                        )
                    }),
                    is_error: true,
                });
            }
        };

        let client_id = oauth_config
            .as_ref()
            .and_then(|c| c.client_id.clone())
            .unwrap_or_else(|| "claude-code".to_string());

        let callback_port = oauth_config.as_ref().and_then(|c| c.callback_port);

        // Start the local callback server.
        let (port, code_rx) = match start_oauth_callback_server(callback_port).await {
            Ok(pair) => pair,
            Err(e) => {
                return Ok(ToolResultData {
                    data: serde_json::json!({
                        "status": "error",
                        "message": format!(
                            "Failed to start OAuth callback server for {}: {e:#}. \
                             Ask the user to run /mcp and authenticate manually.",
                            self.server_name
                        )
                    }),
                    is_error: true,
                });
            }
        };

        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let pkce = PkceChallenge::generate();
        let state = generate_random_state();

        let auth_url = build_authorization_url(
            &metadata,
            &client_id,
            &redirect_uri,
            &state,
            &pkce,
            &[],
        );

        // Spawn a background task that waits for the OAuth callback, exchanges
        // the code for tokens, persists them, and reconnects the server.
        let server_name = self.server_name.clone();
        let manager = Arc::clone(&self.manager);
        let config = self.config.clone();

        tokio::spawn(async move {
            let code = match tokio::time::timeout(
                std::time::Duration::from_secs(300),
                code_rx,
            )
            .await
            {
                Ok(Ok(Ok(code))) => code,
                Ok(Ok(Err(e))) => {
                    warn!(server = %server_name, "OAuth callback error: {e:#}");
                    return;
                }
                Ok(Err(_)) => {
                    warn!(server = %server_name, "OAuth callback channel closed");
                    return;
                }
                Err(_) => {
                    warn!(server = %server_name, "OAuth flow timed out after 300s");
                    return;
                }
            };

            debug!(server = %server_name, "received OAuth authorization code");

            // Exchange the code for tokens.
            // Since the provider's `authenticate` method opens a browser (which
            // we already did), we exchange directly here.
            match omni_core::mcp::auth::exchange_code_for_tokens(
                &metadata,
                &client_id,
                None,
                &code,
                &redirect_uri,
                &pkce.verifier,
            )
            .await
            {
                Ok(tokens) => {
                    debug!(server = %server_name, "OAuth token exchange successful");

                    // Persist tokens via the OAuthStore.
                    let server_key = omni_core::mcp::auth::get_server_key(
                        &server_name,
                        config.config.url.as_deref().unwrap_or(""),
                    );
                    let mut store = omni_core::mcp::auth::OAuthStore::load();
                    store.set(
                        server_key.clone(),
                        omni_core::mcp::auth::StoredServerOAuth {
                            access_token: tokens.access_token,
                            refresh_token: tokens.refresh_token,
                            expires_at: tokens.expires_at,
                            client_id: Some(client_id),
                            client_secret: None,
                            server_metadata: Some(metadata),
                        },
                    );
                    store.save().ok();

                    // Clear the needs-auth cache entry.
                    let mut cache = NeedsAuthCache::load();
                    cache.remove(&server_key);
                    cache.save().ok();

                    // Reconnect the server so its real tools become available.
                    match manager
                        .start_server(server_name.clone(), config)
                        .await
                    {
                        Ok(caps) => {
                            debug!(
                                server = %server_name,
                                tools = caps.tools,
                                resources = caps.resources,
                                "OAuth complete, server reconnected"
                            );
                        }
                        Err(e) => {
                            warn!(
                                server = %server_name,
                                "failed to reconnect server after OAuth: {e:#}"
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        server = %server_name,
                        "OAuth token exchange failed: {e:#}"
                    );
                }
            }
        });

        // Return the auth URL immediately so the model can share it with the user.
        Ok(ToolResultData {
            data: serde_json::json!({
                "status": "auth_url",
                "authUrl": auth_url,
                "message": format!(
                    "Ask the user to open this URL in their browser to authorize the {} MCP server:\n\n{}\n\n\
                     Once they complete the flow, the server's tools will become available automatically.",
                    self.server_name, auth_url
                )
            }),
            is_error: false,
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }
}

/// Generate a random hex string suitable for OAuth state parameter.
fn generate_random_state() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let s = RandomState::new();
    let a = s.build_hasher().finish();
    let s2 = RandomState::new();
    let b = s2.build_hasher().finish();
    format!("{a:016x}{b:016x}")
}
