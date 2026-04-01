use std::collections::HashMap;
use tokio::sync::RwLock;

use super::types::{CreateTaskParams, TaskState, TaskStatus, UpdateTaskParams};

/// Manages the lifecycle of tasks: creation, querying, updates, and stopping.
///
/// Thread-safe via interior mutability (`RwLock`). Designed to be shared
/// across tool invocations and the TUI via `Arc<TaskManager>`.
#[derive(Debug)]
pub struct TaskManager {
    tasks: RwLock<HashMap<String, TaskState>>,
    next_id: RwLock<u64>,
}

impl TaskManager {
    /// Create a new empty `TaskManager`.
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            next_id: RwLock::new(1),
        }
    }

    /// Create a new task and return its ID.
    pub async fn create(&self, params: CreateTaskParams) -> String {
        let mut next_id = self.next_id.write().await;
        let id = next_id.to_string();
        *next_id += 1;

        let now = chrono::Utc::now();
        let task = TaskState {
            id: id.clone(),
            subject: params.subject,
            description: params.description,
            active_form: params.active_form,
            status: TaskStatus::Pending,
            task_type: params.task_type,
            owner: params.owner,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: params.metadata,
            created_at: now,
            updated_at: now,
            output: None,
            error: None,
            pid: None,
        };

        self.tasks.write().await.insert(id.clone(), task);
        id
    }

    /// Retrieve a task by ID.
    pub async fn get(&self, id: &str) -> Option<TaskState> {
        self.tasks.read().await.get(id).cloned()
    }

    /// List all tasks, optionally filtered by status.
    pub async fn list(&self, status_filter: Option<TaskStatus>) -> Vec<TaskState> {
        let tasks = self.tasks.read().await;
        let mut result: Vec<TaskState> = match status_filter {
            Some(status) => tasks
                .values()
                .filter(|t| t.status == status)
                .cloned()
                .collect(),
            None => tasks.values().cloned().collect(),
        };
        result.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        result
    }

    /// Update a task's fields. Returns the updated task, or `None` if not found.
    pub async fn update(&self, id: &str, params: UpdateTaskParams) -> Option<TaskState> {
        let mut tasks = self.tasks.write().await;
        let task = tasks.get_mut(id)?;

        if let Some(subject) = params.subject {
            task.subject = subject;
        }
        if let Some(description) = params.description {
            task.description = description;
        }
        if let Some(active_form) = params.active_form {
            task.active_form = Some(active_form);
        }
        if let Some(status) = params.status {
            task.status = status;
        }
        if let Some(owner) = params.owner {
            task.owner = Some(owner);
        }
        if let Some(metadata) = params.metadata {
            for (key, value) in metadata {
                if value.is_null() {
                    task.metadata.remove(&key);
                } else {
                    task.metadata.insert(key, value);
                }
            }
        }
        if let Some(output) = params.output {
            task.output = Some(output);
        }
        if let Some(error) = params.error {
            task.error = Some(error);
        }
        if let Some(pid) = params.pid {
            task.pid = Some(pid);
        }

        // Collect block changes, then apply to avoid double-borrow
        let id_string = id.to_string();
        let new_blocks: Vec<String> = params
            .add_blocks
            .into_iter()
            .filter(|b| !task.blocks.contains(b))
            .collect();
        for block_id in &new_blocks {
            task.blocks.push(block_id.clone());
        }

        let new_blocked_by: Vec<String> = params
            .add_blocked_by
            .into_iter()
            .filter(|b| !task.blocked_by.contains(b))
            .collect();
        for blocker_id in &new_blocked_by {
            task.blocked_by.push(blocker_id.clone());
        }

        // Update reciprocal relationships
        let _ = task; // release mutable borrow
        for block_id in &new_blocks {
            if let Some(blocked_task) = tasks.get_mut(block_id.as_str()) {
                if !blocked_task.blocked_by.contains(&id_string) {
                    blocked_task.blocked_by.push(id_string.clone());
                }
            }
        }
        for blocker_id in &new_blocked_by {
            if let Some(blocker_task) = tasks.get_mut(blocker_id.as_str()) {
                if !blocker_task.blocks.contains(&id_string) {
                    blocker_task.blocks.push(id_string.clone());
                }
            }
        }

        let task = tasks.get_mut(id)?;
        task.updated_at = chrono::Utc::now();
        Some(task.clone())
    }

    /// Stop a running task by marking it as killed.
    pub async fn stop(&self, id: &str) -> Option<TaskState> {
        let mut tasks = self.tasks.write().await;
        let task = tasks.get_mut(id)?;

        if task.status == TaskStatus::Running
            || task.status == TaskStatus::InProgress
            || task.status == TaskStatus::Pending
        {
            task.status = TaskStatus::Killed;
            task.updated_at = chrono::Utc::now();
        }

        Some(task.clone())
    }

    /// Delete a task. Returns `true` if the task was found and removed.
    pub async fn delete(&self, id: &str) -> bool {
        self.tasks.write().await.remove(id).is_some()
    }

    /// Get the output of a task.
    pub async fn output(&self, id: &str) -> Option<String> {
        self.tasks
            .read()
            .await
            .get(id)
            .and_then(|t| t.output.clone())
    }

    /// Append text to a task's output buffer.
    pub async fn append_output(&self, id: &str, text: &str) {
        if let Some(task) = self.tasks.write().await.get_mut(id) {
            match &mut task.output {
                Some(existing) => existing.push_str(text),
                None => task.output = Some(text.to_string()),
            }
        }
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}
