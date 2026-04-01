use anyhow::Result;
use async_trait::async_trait;
use omni_core::tasks::TaskManager;
use omni_core::types::events::{ToolProgressData, ToolResultData};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub type ProgressSender = mpsc::Sender<ToolProgressData>;

/// Sender side of the teammate messaging channel.
/// Messages are `(recipient, content, summary)`.
pub type MessageSender = mpsc::Sender<(String, String, Option<String>)>;

/// Context passed to every tool invocation.
pub struct ToolUseContext {
    /// The working directory for file operations and command execution.
    pub working_directory: PathBuf,
    /// Shared task manager for task CRUD operations.
    pub task_manager: Option<Arc<TaskManager>>,
    /// Channel for sending messages to teammates.
    pub message_sender: Option<MessageSender>,
    /// Name of the current agent (for team coordination).
    pub agent_name: Option<String>,
    /// Name of the current team.
    pub team_name: Option<String>,
}

impl ToolUseContext {
    /// Create a minimal context with only the working directory set.
    pub fn with_working_directory(working_directory: PathBuf) -> Self {
        Self {
            working_directory,
            task_manager: None,
            message_sender: None,
            agent_name: None,
            team_name: None,
        }
    }
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String {
        format!("Tool: {}", self.name())
    }
    fn aliases(&self) -> &[&str] {
        &[]
    }
    fn input_schema(&self) -> Value;
    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        cancel: CancellationToken,
        progress: Option<ProgressSender>,
    ) -> Result<ToolResultData>;
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _input: &Value) -> bool {
        false
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolExecutor>>,
    aliases: HashMap<String, String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn ToolExecutor>) {
        let name = tool.name().to_string();
        for alias in tool.aliases() {
            self.aliases.insert(alias.to_string(), name.clone());
        }
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolExecutor>> {
        self.tools
            .get(name)
            .or_else(|| self.aliases.get(name).and_then(|n| self.tools.get(n)))
            .cloned()
    }

    pub fn all(&self) -> Vec<Arc<dyn ToolExecutor>> {
        self.tools.values().cloned().collect()
    }

    pub fn schemas(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|t| serde_json::json!({"name": t.name(), "input_schema": t.input_schema()}))
            .collect()
    }

    pub fn tool_definitions(&self) -> Vec<omni_core::api::client::ToolDefinition> {
        self.tools
            .values()
            .map(|t| omni_core::api::client::ToolDefinition {
                name: t.name().to_string(),
                description: t.description(),
                input_schema: t.input_schema(),
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
