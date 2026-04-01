use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use omni_core::types::events::ToolResultData;
use omni_core::worktree;

/// Creates an isolated git worktree and switches the session into it.
///
/// Git worktrees allow parallel development on multiple branches from the
/// same repository. The worktree is created under `.claude/worktrees/`.
pub struct EnterWorktreeTool;

#[async_trait]
impl ToolExecutor for EnterWorktreeTool {
    fn name(&self) -> &str {
        "EnterWorktree"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name for the worktree branch. Must contain only alphanumeric, dot, underscore, dash, or slash-separated segments. Max 64 chars."
                }
            }
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let name = input["name"]
            .as_str()
            .unwrap_or("claude-worktree");

        // Validate the name
        if let Err(e) = worktree::validate_worktree_slug(name) {
            return Ok(ToolResultData {
                data: json!({ "error": format!("Invalid worktree name: {}", e) }),
                is_error: true,
            });
        }

        // Find the git root from the working directory
        let repo_root = find_git_root(&ctx.working_directory).await?;

        // Create the worktree
        let info = match worktree::create_worktree(&repo_root, name, None) {
            Ok(info) => info,
            Err(e) => {
                return Ok(ToolResultData {
                    data: json!({ "error": format!("Failed to create worktree: {}", e) }),
                    is_error: true,
                });
            }
        };

        // Enter the worktree
        if let Err(e) = worktree::enter_worktree(&info.path) {
            return Ok(ToolResultData {
                data: json!({ "error": format!("Failed to enter worktree: {}", e) }),
                is_error: true,
            });
        }

        Ok(ToolResultData {
            data: json!({
                "worktreePath": info.path.display().to_string(),
                "worktreeBranch": info.branch,
                "message": format!(
                    "Created worktree at {} on branch {}. The session is now working in the worktree. Use ExitWorktree to leave.",
                    info.path.display(),
                    info.branch
                ),
            }),
            is_error: false,
        })
    }

    fn is_destructive(&self, _input: &Value) -> bool {
        true
    }
}

/// Exits a worktree session and returns to the original working directory.
///
/// Supports keeping the worktree on disk (preserving the branch and changes)
/// or removing it entirely. When removing, uncommitted changes require
/// explicit confirmation via `discard_changes: true`.
pub struct ExitWorktreeTool;

#[async_trait]
impl ToolExecutor for ExitWorktreeTool {
    fn name(&self) -> &str {
        "ExitWorktree"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["keep", "remove"],
                    "description": "\"keep\" leaves the worktree on disk; \"remove\" deletes both the worktree and branch."
                },
                "discard_changes": {
                    "type": "boolean",
                    "description": "Required true when action is \"remove\" and the worktree has uncommitted changes."
                }
            },
            "required": ["action"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let action = input["action"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'action' field"))?;

        let discard_changes = input["discard_changes"].as_bool().unwrap_or(false);

        // Detect whether we're currently in a worktree
        let cwd = &ctx.working_directory;
        let git_dir_output = tokio::process::Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(cwd)
            .output()
            .await?;

        let git_dir = String::from_utf8_lossy(&git_dir_output.stdout)
            .trim()
            .to_string();

        // Worktrees have a .git file (not directory) pointing to the main repo
        let is_worktree = git_dir.contains(".git/worktrees/");
        if !is_worktree {
            return Ok(ToolResultData {
                data: json!({
                    "error": "Not currently in a git worktree. This tool only operates on worktrees."
                }),
                is_error: true,
            });
        }

        let cleanup = action == "remove";

        // Check for uncommitted changes before removing
        if cleanup && !discard_changes {
            let status_output = tokio::process::Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(cwd)
                .output()
                .await?;

            let status_text = String::from_utf8_lossy(&status_output.stdout);
            let changed_files: usize = status_text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count();

            if changed_files > 0 {
                return Ok(ToolResultData {
                    data: json!({
                        "error": format!(
                            "Worktree has {} uncommitted file(s). Removing will discard this work permanently. \
                             Re-invoke with discard_changes: true, or use action: \"keep\" to preserve the worktree.",
                            changed_files
                        ),
                    }),
                    is_error: true,
                });
            }
        }

        // Find the main repo root to return to
        let main_output = tokio::process::Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(cwd)
            .output()
            .await?;

        let main_stdout = String::from_utf8_lossy(&main_output.stdout);
        let original_dir = main_stdout
            .lines()
            .find(|l| l.starts_with("worktree "))
            .and_then(|l| l.strip_prefix("worktree "))
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.working_directory.clone());

        let worktree_path = cwd.clone();

        // Exit the worktree
        if let Err(e) = worktree::exit_worktree(&worktree_path, &original_dir, cleanup) {
            return Ok(ToolResultData {
                data: json!({ "error": format!("Failed to exit worktree: {}", e) }),
                is_error: true,
            });
        }

        let message = if cleanup {
            format!(
                "Exited and removed worktree at {}. Session is now back in {}.",
                worktree_path.display(),
                original_dir.display()
            )
        } else {
            format!(
                "Exited worktree. Your work is preserved at {}. Session is now back in {}.",
                worktree_path.display(),
                original_dir.display()
            )
        };

        Ok(ToolResultData {
            data: json!({
                "action": action,
                "originalCwd": original_dir.display().to_string(),
                "worktreePath": worktree_path.display().to_string(),
                "message": message,
            }),
            is_error: false,
        })
    }

    fn is_destructive(&self, input: &Value) -> bool {
        input["action"].as_str() == Some("remove")
    }
}

/// Resolve the git repository root from a given directory.
async fn find_git_root(from: &std::path::Path) -> Result<PathBuf> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(from)
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!("Not a git repository: {}", from.display());
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(root))
}
