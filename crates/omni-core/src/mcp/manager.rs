use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::client::McpClient;
use super::config::{build_mcp_tool_name, hash_server_config, McpConfig};
use super::permissions::{McpPolicySettings, McpToolRestrictions};
use super::types::{
    McpCapabilities, McpResource, McpServerState, McpServerStatus, McpTool,
    ScopedMcpServerConfig, ServerResource,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default number of local (stdio) servers to connect in parallel.
const LOCAL_SERVER_BATCH_SIZE: usize = 3;

/// Default number of remote (SSE/HTTP/WS) servers to connect in parallel.
const REMOTE_SERVER_BATCH_SIZE: usize = 20;

/// How often the health monitor checks server health (seconds).
const HEALTH_CHECK_INTERVAL_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// Tool descriptor
// ---------------------------------------------------------------------------

/// A tool descriptor that includes the owning server name.
#[derive(Debug, Clone)]
pub struct McpToolDescriptor {
    pub full_name: String,
    pub original_name: String,
    pub server_name: String,
    pub description: String,
    pub input_schema: Value,
}

// ---------------------------------------------------------------------------
// ManagedServer
// ---------------------------------------------------------------------------

struct ManagedServer {
    client: McpClient,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    config: ScopedMcpServerConfig,
    config_hash: String,
}

// ---------------------------------------------------------------------------
// McpManager
// ---------------------------------------------------------------------------

/// Manages multiple MCP server connections with full lifecycle support.
///
/// Features:
/// - Server discovery from multiple config sources
/// - Parallel server startup (batched by transport type)
/// - Tool and resource registration
/// - Server restart on crash
/// - Graceful shutdown
/// - Health monitoring
pub struct McpManager {
    servers: Arc<RwLock<HashMap<String, ManagedServer>>>,
    /// Optional policy settings for server allow/deny.
    policy: Arc<RwLock<McpPolicySettings>>,
    /// Optional per-server tool restrictions.
    tool_restrictions: Arc<RwLock<McpToolRestrictions>>,
    /// Handle for the background health monitor task.
    health_monitor: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl McpManager {
    /// Create a new, empty manager.
    pub fn new() -> Self {
        Self {
            servers: Arc::new(RwLock::new(HashMap::new())),
            policy: Arc::new(RwLock::new(McpPolicySettings::default())),
            tool_restrictions: Arc::new(RwLock::new(McpToolRestrictions::default())),
            health_monitor: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a manager with policy settings.
    pub fn with_policy(policy: McpPolicySettings) -> Self {
        Self {
            servers: Arc::new(RwLock::new(HashMap::new())),
            policy: Arc::new(RwLock::new(policy)),
            tool_restrictions: Arc::new(RwLock::new(McpToolRestrictions::default())),
            health_monitor: Arc::new(RwLock::new(None)),
        }
    }

    /// Update policy settings.
    pub async fn set_policy(&self, policy: McpPolicySettings) {
        *self.policy.write().await = policy;
    }

    /// Update tool restrictions.
    pub async fn set_tool_restrictions(&self, restrictions: McpToolRestrictions) {
        *self.tool_restrictions.write().await = restrictions;
    }

    // -- Server startup --

    /// Start and connect to all servers in the config.
    ///
    /// Servers are started in parallel batches: local (stdio/sdk) servers in
    /// smaller batches to avoid process exhaustion, remote servers in larger
    /// batches since they're just network connections.
    pub async fn start_servers(&self, config: &McpConfig) -> Result<()> {
        let policy = self.policy.read().await.clone();

        // Separate local and remote servers.
        let mut local_servers: Vec<(String, ScopedMcpServerConfig)> = Vec::new();
        let mut remote_servers: Vec<(String, ScopedMcpServerConfig)> = Vec::new();

        for (name, scoped) in &config.servers {
            if !super::permissions::is_server_allowed(
                name,
                Some(&scoped.config),
                &policy,
            ) {
                warn!(server = %name, "MCP server blocked by policy, skipping");
                continue;
            }
            if super::client::is_local_server(&scoped.config) {
                local_servers.push((name.clone(), scoped.clone()));
            } else {
                remote_servers.push((name.clone(), scoped.clone()));
            }
        }

        let local_batch = std::env::var("MCP_SERVER_CONNECTION_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(LOCAL_SERVER_BATCH_SIZE);

        let remote_batch = std::env::var("MCP_REMOTE_SERVER_CONNECTION_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(REMOTE_SERVER_BATCH_SIZE);

        // Start local servers in batches.
        for chunk in local_servers.chunks(local_batch) {
            let mut handles = Vec::new();
            for (name, scoped) in chunk {
                let name = name.clone();
                let scoped = scoped.clone();
                let servers = self.servers.clone();
                handles.push(tokio::spawn(async move {
                    start_single_server(&servers, name, scoped).await
                }));
            }
            for handle in handles {
                let _ = handle.await;
            }
        }

        // Start remote servers in batches.
        for chunk in remote_servers.chunks(remote_batch) {
            let mut handles = Vec::new();
            for (name, scoped) in chunk {
                let name = name.clone();
                let scoped = scoped.clone();
                let servers = self.servers.clone();
                handles.push(tokio::spawn(async move {
                    start_single_server(&servers, name, scoped).await
                }));
            }
            for handle in handles {
                let _ = handle.await;
            }
        }

        Ok(())
    }

    /// Start a single server by name. If it already exists, it will be
    /// stopped first.
    pub async fn start_server(
        &self,
        name: String,
        scoped_config: ScopedMcpServerConfig,
    ) -> Result<McpCapabilities> {
        // Stop existing server if present.
        if self.servers.read().await.contains_key(&name) {
            self.stop_server(&name).await;
        }
        start_single_server(&self.servers, name, scoped_config).await
    }

    /// Restart a single server (disconnect then reconnect).
    pub async fn restart_server(&self, server_name: &str) -> Result<McpCapabilities> {
        let scoped = {
            let servers = self.servers.read().await;
            match servers.get(server_name) {
                Some(m) => m.config.clone(),
                None => bail!("MCP server {server_name:?} not found"),
            }
        };
        self.stop_server(server_name).await;
        self.start_server(server_name.to_string(), scoped).await
    }

    // -- Reload / hot-swap --

    /// Reload configuration: stop servers whose configs changed, start new
    /// servers, leave unchanged servers untouched.
    pub async fn reload_config(&self, new_config: &McpConfig) -> Result<()> {
        let policy = self.policy.read().await.clone();
        let current_names: Vec<String> = self.servers.read().await.keys().cloned().collect();

        // Find servers to stop (removed or config changed).
        let mut to_stop = Vec::new();
        for name in &current_names {
            match new_config.servers.get(name) {
                None => {
                    // Server was removed.
                    to_stop.push(name.clone());
                }
                Some(new_scoped) => {
                    let servers = self.servers.read().await;
                    if let Some(managed) = servers.get(name) {
                        let new_hash = hash_server_config(new_scoped);
                        if managed.config_hash != new_hash {
                            to_stop.push(name.clone());
                        }
                    }
                }
            }
        }

        for name in &to_stop {
            self.stop_server(name).await;
        }

        // Start new or changed servers.
        let mut to_start: Vec<(String, ScopedMcpServerConfig)> = Vec::new();
        for (name, scoped) in &new_config.servers {
            if !super::permissions::is_server_allowed(name, Some(&scoped.config), &policy) {
                continue;
            }
            let needs_start = !self.servers.read().await.contains_key(name);
            if needs_start {
                to_start.push((name.clone(), scoped.clone()));
            }
        }

        // Start them in parallel.
        let mut handles = Vec::new();
        for (name, scoped) in to_start {
            let servers = self.servers.clone();
            handles.push(tokio::spawn(async move {
                start_single_server(&servers, name, scoped).await
            }));
        }
        for handle in handles {
            let _ = handle.await;
        }

        Ok(())
    }

    // -- Tool access --

    /// Get all tool descriptors aggregated from all connected servers.
    pub async fn get_all_tools(&self) -> Vec<McpToolDescriptor> {
        let servers = self.servers.read().await;
        let restrictions = self.tool_restrictions.read().await;
        let mut result = Vec::new();
        for (name, managed) in servers.iter() {
            if managed.client.state() != &McpServerState::Connected {
                continue;
            }
            for tool in &managed.tools {
                if !restrictions.is_tool_allowed(name, &tool.name) {
                    continue;
                }
                result.push(McpToolDescriptor {
                    full_name: build_mcp_tool_name(name, &tool.name),
                    original_name: tool.name.clone(),
                    server_name: name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                });
            }
        }
        result
    }

    /// Get tools for a specific server.
    pub async fn get_tools_for_server(&self, server_name: &str) -> Result<Vec<McpTool>> {
        let servers = self.servers.read().await;
        match servers.get(server_name) {
            Some(m) => Ok(m.tools.clone()),
            None => bail!("MCP server {server_name:?} not found"),
        }
    }

    // -- Resource access --

    /// Get all resources from all connected servers.
    pub async fn get_all_resources(&self) -> Vec<ServerResource> {
        let servers = self.servers.read().await;
        let mut result = Vec::new();
        for (name, managed) in servers.iter() {
            if managed.client.state() != &McpServerState::Connected {
                continue;
            }
            for r in &managed.resources {
                result.push(ServerResource {
                    resource: r.clone(),
                    server: name.clone(),
                });
            }
        }
        result
    }

    /// Get resources from a specific server.
    pub async fn get_resources_for_server(&self, server_name: &str) -> Result<Vec<McpResource>> {
        let servers = self.servers.read().await;
        match servers.get(server_name) {
            Some(m) => Ok(m.resources.clone()),
            None => bail!("MCP server {server_name:?} not found"),
        }
    }

    // -- Tool call --

    /// Call a tool on the specified server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<Value> {
        // Check tool restrictions.
        {
            let restrictions = self.tool_restrictions.read().await;
            if !restrictions.is_tool_allowed(server_name, tool_name) {
                bail!("tool {tool_name} is not allowed on server {server_name}");
            }
        }

        let servers = self.servers.read().await;
        let managed = servers
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server {server_name:?} not found"))?;
        managed.client.call_tool(tool_name, arguments, None).await
    }

    /// Read a resource from the specified server.
    pub async fn read_resource(&self, server_name: &str, uri: &str) -> Result<Value> {
        let servers = self.servers.read().await;
        let managed = servers
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server {server_name:?} not found"))?;
        managed.client.read_resource(uri).await
    }

    // -- Status --

    /// Get the status of all managed servers.
    pub async fn server_statuses(&self) -> Vec<McpServerStatus> {
        self.servers
            .read()
            .await
            .iter()
            .map(|(name, m)| McpServerStatus {
                name: name.clone(),
                state: m.client.state().clone(),
                capabilities: m.client.capabilities().cloned(),
                tool_count: m.tools.len(),
                resource_count: m.resources.len(),
                error: m.client.last_error().map(String::from),
            })
            .collect()
    }

    /// Get the names of all managed servers.
    pub async fn server_names(&self) -> Vec<String> {
        self.servers.read().await.keys().cloned().collect()
    }

    /// Get the config for a specific server.
    pub async fn get_server_config(&self, name: &str) -> Option<ScopedMcpServerConfig> {
        self.servers.read().await.get(name).map(|m| m.config.clone())
    }

    // -- Refresh --

    /// Refresh the tool list for a specific server.
    pub async fn refresh_tools(&self, server_name: &str) -> Result<Vec<McpTool>> {
        let mut servers = self.servers.write().await;
        let managed = servers
            .get_mut(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server {server_name:?} not found"))?;
        let tools = managed.client.list_tools().await?;
        managed.tools = tools.clone();
        Ok(tools)
    }

    /// Refresh tools for all servers that signalled a change.
    pub async fn refresh_changed_tools(&self) {
        let names: Vec<String> = {
            self.servers
                .read()
                .await
                .iter()
                .filter(|(_, m)| m.client.tools_changed())
                .map(|(name, _)| name.clone())
                .collect()
        };
        for name in names {
            match self.refresh_tools(&name).await {
                Ok(tools) => {
                    debug!(
                        server = %name,
                        count = tools.len(),
                        "refreshed changed tools"
                    );
                }
                Err(e) => {
                    warn!(server = %name, "failed to refresh tools: {e:#}");
                }
            }
        }
    }

    // -- Shutdown --

    /// Stop a single server.
    pub async fn stop_server(&self, server_name: &str) {
        if let Some(mut managed) = self.servers.write().await.remove(server_name) {
            debug!(server = %server_name, "stopping MCP server");
            managed.client.disconnect().await;
        }
    }

    /// Stop all managed servers.
    pub async fn stop_all(&self) {
        // Stop health monitor first.
        if let Some(handle) = self.health_monitor.write().await.take() {
            handle.abort();
        }

        let mut servers = self.servers.write().await;
        for (name, mut m) in servers.drain() {
            debug!(server = %name, "stopping MCP server");
            m.client.disconnect().await;
        }
        info!("all MCP servers stopped");
    }

    // -- Health monitoring --

    /// Start a background task that periodically checks server health and
    /// attempts reconnection for crashed servers.
    pub fn start_health_monitor(&self) {
        let servers = self.servers.clone();
        let handle = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS));
            loop {
                interval.tick().await;

                let names: Vec<String> = servers.read().await.keys().cloned().collect();
                for name in names {
                    let needs_reconnect = {
                        let servers_guard = servers.read().await;
                        if let Some(managed) = servers_guard.get(&name) {
                            managed.client.state() == &McpServerState::Connected
                                && !managed.client.is_healthy()
                        } else {
                            false
                        }
                    };

                    if needs_reconnect {
                        warn!(server = %name, "health check failed, attempting reconnect");
                        let mut servers_guard = servers.write().await;
                        if let Some(managed) = servers_guard.get_mut(&name) {
                            match managed.client.reconnect().await {
                                Ok(caps) => {
                                    info!(server = %name, "reconnected after health check failure");
                                    // Re-fetch tools and resources.
                                    if caps.tools {
                                        managed.tools =
                                            managed.client.list_tools().await.unwrap_or_default();
                                    }
                                    if caps.resources {
                                        managed.resources = managed
                                            .client
                                            .list_resources()
                                            .await
                                            .unwrap_or_default();
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        server = %name,
                                        "reconnect after health check failure: {e:#}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        });

        // Store handle so we can cancel it on shutdown.
        let health_monitor = self.health_monitor.clone();
        tokio::spawn(async move {
            *health_monitor.write().await = Some(handle);
        });
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Internal helper
// ---------------------------------------------------------------------------

async fn start_single_server(
    servers: &Arc<RwLock<HashMap<String, ManagedServer>>>,
    name: String,
    scoped: ScopedMcpServerConfig,
) -> Result<McpCapabilities> {
    info!(server = %name, scope = %scoped.scope, "starting MCP server");
    let config_hash = hash_server_config(&scoped);
    let mut client = McpClient::new(name.clone(), scoped.config.clone());

    match client.connect().await {
        Ok(capabilities) => {
            let tools = if capabilities.tools {
                client.list_tools().await.unwrap_or_default()
            } else {
                vec![]
            };
            let resources = if capabilities.resources {
                client.list_resources().await.unwrap_or_default()
            } else {
                vec![]
            };

            info!(
                server = %name,
                tools = tools.len(),
                resources = resources.len(),
                "MCP server connected"
            );

            servers.write().await.insert(
                name,
                ManagedServer {
                    client,
                    tools,
                    resources,
                    config: scoped,
                    config_hash,
                },
            );
            Ok(capabilities)
        }
        Err(e) => {
            warn!(server = %name, "failed to start MCP server: {e:#}");

            // Record the failed server so its status is visible.
            servers.write().await.insert(
                name,
                ManagedServer {
                    client,
                    tools: vec![],
                    resources: vec![],
                    config: scoped,
                    config_hash,
                },
            );
            Err(e)
        }
    }
}
