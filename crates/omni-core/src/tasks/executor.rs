use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tracing::warn;

use super::manager::TaskManager;
use super::types::{TaskStatus, UpdateTaskParams};

/// Spawn a background shell task and track its output.
pub async fn spawn_shell_task(
    manager: Arc<TaskManager>,
    task_id: String,
    command: String,
    working_directory: std::path::PathBuf,
) {
    let mut child = match tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&command)
        .current_dir(&working_directory)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            manager
                .update(
                    &task_id,
                    UpdateTaskParams {
                        status: Some(TaskStatus::Failed),
                        error: Some(format!("Failed to spawn: {}", e)),
                        ..Default::default()
                    },
                )
                .await;
            return;
        }
    };

    let pid = child.id();
    manager
        .update(
            &task_id,
            UpdateTaskParams {
                status: Some(TaskStatus::Running),
                pid,
                ..Default::default()
            },
        )
        .await;

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    let mgr_stdout = Arc::clone(&manager);
    let tid_stdout = task_id.clone();
    let stdout_handle = tokio::spawn(async move {
        if let Some(ref mut pipe) = stdout {
            let mut buf = [0u8; 4096];
            loop {
                match pipe.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        mgr_stdout.append_output(&tid_stdout, &text).await;
                    }
                    Err(e) => {
                        warn!("Error reading stdout for task {}: {}", tid_stdout, e);
                        break;
                    }
                }
            }
        }
    });

    let mgr_stderr = Arc::clone(&manager);
    let tid_stderr = task_id.clone();
    let stderr_handle = tokio::spawn(async move {
        if let Some(ref mut pipe) = stderr {
            let mut buf = [0u8; 4096];
            loop {
                match pipe.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        mgr_stderr.append_output(&tid_stderr, &text).await;
                    }
                    Err(e) => {
                        warn!("Error reading stderr for task {}: {}", tid_stderr, e);
                        break;
                    }
                }
            }
        }
    });

    let status = child.wait().await;
    let _ = stdout_handle.await;
    let _ = stderr_handle.await;

    match status {
        Ok(exit_status) => {
            let (new_status, error) = if exit_status.success() {
                (TaskStatus::Completed, None)
            } else {
                let code = exit_status.code().unwrap_or(-1);
                (TaskStatus::Failed, Some(format!("Exit code: {}", code)))
            };
            manager
                .update(
                    &task_id,
                    UpdateTaskParams {
                        status: Some(new_status),
                        error,
                        ..Default::default()
                    },
                )
                .await;
        }
        Err(e) => {
            manager
                .update(
                    &task_id,
                    UpdateTaskParams {
                        status: Some(TaskStatus::Failed),
                        error: Some(format!("Wait failed: {}", e)),
                        ..Default::default()
                    },
                )
                .await;
        }
    }
}

/// Spawn a subagent task by running a new Claude process in the background.
pub async fn spawn_agent_task(
    manager: Arc<TaskManager>,
    task_id: String,
    prompt: String,
    working_directory: std::path::PathBuf,
    model: Option<String>,
) {
    let mut cmd = tokio::process::Command::new("claude");
    cmd.arg("--print")
        .arg("--prompt")
        .arg(&prompt)
        .current_dir(&working_directory)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(ref model_name) = model {
        cmd.arg("--model").arg(model_name);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            manager
                .update(
                    &task_id,
                    UpdateTaskParams {
                        status: Some(TaskStatus::Failed),
                        error: Some(format!("Failed to spawn agent: {}", e)),
                        ..Default::default()
                    },
                )
                .await;
            return;
        }
    };

    let pid = child.id();
    manager
        .update(
            &task_id,
            UpdateTaskParams {
                status: Some(TaskStatus::Running),
                pid,
                ..Default::default()
            },
        )
        .await;

    let mut stdout = child.stdout.take();
    let mgr_out = Arc::clone(&manager);
    let tid_out = task_id.clone();
    let output_handle = tokio::spawn(async move {
        if let Some(ref mut pipe) = stdout {
            let mut buf = [0u8; 4096];
            loop {
                match pipe.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        mgr_out.append_output(&tid_out, &text).await;
                    }
                    Err(e) => {
                        warn!("Error reading agent output for task {}: {}", tid_out, e);
                        break;
                    }
                }
            }
        }
    });

    let status = child.wait().await;
    let _ = output_handle.await;

    match status {
        Ok(exit_status) => {
            let (new_status, error) = if exit_status.success() {
                (TaskStatus::Completed, None)
            } else {
                let code = exit_status.code().unwrap_or(-1);
                (
                    TaskStatus::Failed,
                    Some(format!("Agent exited with code: {}", code)),
                )
            };
            manager
                .update(
                    &task_id,
                    UpdateTaskParams {
                        status: Some(new_status),
                        error,
                        ..Default::default()
                    },
                )
                .await;
        }
        Err(e) => {
            manager
                .update(
                    &task_id,
                    UpdateTaskParams {
                        status: Some(TaskStatus::Failed),
                        error: Some(format!("Wait failed: {}", e)),
                        ..Default::default()
                    },
                )
                .await;
        }
    }
}
