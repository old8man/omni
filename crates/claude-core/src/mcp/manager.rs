use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::client::McpClient;
use super::config::{build_mcp_tool_name, McpConfig};
use super::types::{
    McpCapabilities, McpResource, McpServerConfig, McpServerStatus, McpTool, ServerResource,
};

/// A tool descriptor that includes the owning server name.
#[derive(Debug, Clone)]
pub struct McpToolDescriptor {
    pub full_name: String,
    pub original_name: String,
    pub server_name: String,
    pub description: String,
    pub input_schema: Value,
}

struct ManagedServer {
    client: McpClient,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
}

/// Manages multiple MCP server connections.
pub struct McpManager {
    servers: Arc<RwLock<HashMap<String, ManagedServer>>>,
}

impl McpManager {
    /// Create a new, empty manager.
    pub fn new() -> Self {
        Self {
            servers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start and connect to all servers in the config.
    pub async fn start_servers(&self, config: &McpConfig) -> Result<()> {
        for (name, scoped) in &config.servers {
            info!(server = %name, scope = %scoped.scope, "starting MCP server");
            match self.start_server(name.clone(), scoped.config.clone()).await {
                Ok(caps) => {
                    info!(server = %name, tools = caps.tools, resources = caps.resources, "MCP server connected")
                }
                Err(e) => warn!(server = %name, "failed to start MCP server: {e:#}"),
            }
        }
        Ok(())
    }

    async fn start_server(&self, name: String, config: McpServerConfig) -> Result<McpCapabilities> {
        let mut client = McpClient::new(name.clone(), config);
        let capabilities = client.connect().await?;
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
        self.servers.write().await.insert(
            name,
            ManagedServer {
                client,
                tools,
                resources,
            },
        );
        Ok(capabilities)
    }

    /// Get all tool descriptors aggregated from all servers.
    pub async fn get_all_tools(&self) -> Vec<McpToolDescriptor> {
        let servers = self.servers.read().await;
        let mut result = Vec::new();
        for (name, managed) in servers.iter() {
            for tool in &managed.tools {
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

    /// Get all resources from all servers.
    pub async fn get_all_resources(&self) -> Vec<ServerResource> {
        let servers = self.servers.read().await;
        let mut result = Vec::new();
        for (name, managed) in servers.iter() {
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

    /// Call a tool on the specified server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<Value> {
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
                error: None,
            })
            .collect()
    }

    /// Get the names of all managed servers.
    pub async fn server_names(&self) -> Vec<String> {
        self.servers.read().await.keys().cloned().collect()
    }

    /// Stop all managed servers.
    pub async fn stop_all(&self) {
        for (name, mut m) in self.servers.write().await.drain() {
            debug!(server = %name, "stopping MCP server");
            m.client.disconnect().await;
        }
        info!("all MCP servers stopped");
    }

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
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
