use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The kind of task being executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    LocalBash,
    LocalAgent,
    RemoteAgent,
    InProcessTeammate,
    LocalWorkflow,
}

/// The lifecycle status of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Running,
    Completed,
    Failed,
    Killed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Killed => write!(f, "killed"),
        }
    }
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalBash => write!(f, "local_bash"),
            Self::LocalAgent => write!(f, "local_agent"),
            Self::RemoteAgent => write!(f, "remote_agent"),
            Self::InProcessTeammate => write!(f, "in_process_teammate"),
            Self::LocalWorkflow => write!(f, "local_workflow"),
        }
    }
}

/// Complete state of a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub id: String,
    pub subject: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    pub status: TaskStatus,
    pub task_type: TaskType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Parameters for creating a new task.
#[derive(Debug, Clone)]
pub struct CreateTaskParams {
    pub subject: String,
    pub description: String,
    pub active_form: Option<String>,
    pub task_type: TaskType,
    pub owner: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Fields that can be updated on an existing task.
#[derive(Debug, Clone, Default)]
pub struct UpdateTaskParams {
    pub subject: Option<String>,
    pub description: Option<String>,
    pub active_form: Option<String>,
    pub status: Option<TaskStatus>,
    pub owner: Option<String>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub output: Option<String>,
    pub error: Option<String>,
    pub pid: Option<u32>,
    pub add_blocks: Vec<String>,
    pub add_blocked_by: Vec<String>,
}
