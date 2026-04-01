use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

const MAX_WORKTREE_SLUG_LENGTH: usize = 64;

/// Information about a git worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub is_temporary: bool,
    pub is_main: bool,
}

/// Validate a worktree slug to prevent path traversal.
pub fn validate_worktree_slug(slug: &str) -> Result<()> {
    if slug.len() > MAX_WORKTREE_SLUG_LENGTH {
        bail!("worktree name too long (max {MAX_WORKTREE_SLUG_LENGTH})");
    }
    if slug.is_empty() {
        bail!("worktree name must not be empty");
    }
    for segment in slug.split('/') {
        if segment == "." || segment == ".." {
            bail!("worktree name must not contain . or .. segments");
        }
        if segment.is_empty() {
            bail!("worktree name must not contain empty segments");
        }
        if !segment
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            bail!("worktree name segments must contain only alphanumeric, dot, underscore, dash");
        }
    }
    Ok(())
}

/// Create a new git worktree.
pub fn create_worktree(
    repo_root: &Path,
    branch_name: &str,
    base_branch: Option<&str>,
) -> Result<WorktreeInfo> {
    validate_worktree_slug(branch_name)?;
    let worktree_dir = repo_root
        .join(".claude")
        .join("worktrees")
        .join(branch_name);
    if worktree_dir.exists() {
        bail!(
            "worktree directory already exists: {}",
            worktree_dir.display()
        );
    }

    let worktree_dir_str = worktree_dir.to_string_lossy();
    let mut args = vec!["worktree", "add", "-b", branch_name, &*worktree_dir_str];
    let base_str;
    if let Some(base) = base_branch {
        base_str = base.to_string();
        args.push(&base_str);
    }

    let output = std::process::Command::new("git")
        .args(&args)
        .current_dir(repo_root)
        .output()
        .context("git worktree add")?;
    if !output.status.success() {
        bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    info!(branch = %branch_name, path = %worktree_dir.display(), "created worktree");
    Ok(WorktreeInfo {
        path: worktree_dir,
        branch: branch_name.to_string(),
        is_temporary: true,
        is_main: false,
    })
}

/// Enter a worktree by changing the working directory.
pub fn enter_worktree(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        bail!("worktree path does not exist: {}", path.display());
    }
    let previous = std::env::current_dir().context("failed to get current directory")?;
    std::env::set_current_dir(path)
        .with_context(|| format!("failed to chdir to {}", path.display()))?;
    info!(worktree = %path.display(), "entered worktree");
    Ok(previous)
}

/// Exit a worktree, optionally cleaning it up.
pub fn exit_worktree(worktree_path: &Path, original_dir: &Path, cleanup: bool) -> Result<()> {
    std::env::set_current_dir(original_dir)
        .with_context(|| format!("failed to chdir to {}", original_dir.display()))?;
    if cleanup {
        let output = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(worktree_path)
            .output()
            .context("git worktree remove")?;
        if !output.status.success() {
            bail!(
                "git worktree remove failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        debug!(path = %worktree_path.display(), "removed worktree");
    }
    Ok(())
}

/// List all git worktrees in the repository.
pub fn list_worktrees(repo_root: &Path) -> Result<Vec<WorktreeInfo>> {
    let output = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output()
        .context("git worktree list")?;
    if !output.status.success() {
        bail!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let claude_dir = repo_root.join(".claude").join("worktrees");
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch = String::new();
    let mut is_bare = false;

    for line in stdout.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(path) = current_path.take() {
                worktrees.push(WorktreeInfo {
                    is_temporary: path.starts_with(&claude_dir),
                    path,
                    branch: current_branch.clone(),
                    is_main: is_bare,
                });
            }
            current_path = Some(PathBuf::from(p));
            current_branch = String::new();
            is_bare = false;
        } else if let Some(b) = line.strip_prefix("branch ") {
            current_branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
        } else if line == "bare" {
            is_bare = true;
        } else if line.is_empty() {
            if let Some(path) = current_path.take() {
                worktrees.push(WorktreeInfo {
                    is_temporary: path.starts_with(&claude_dir),
                    path,
                    branch: current_branch.clone(),
                    is_main: is_bare,
                });
            }
            current_branch = String::new();
            is_bare = false;
        }
    }
    if let Some(path) = current_path {
        worktrees.push(WorktreeInfo {
            is_temporary: path.starts_with(&claude_dir),
            path,
            branch: current_branch,
            is_main: is_bare,
        });
    }
    Ok(worktrees)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_validate_slug() {
        assert!(validate_worktree_slug("feature-foo").is_ok());
        assert!(validate_worktree_slug("user/feature").is_ok());
        assert!(validate_worktree_slug("").is_err());
        assert!(validate_worktree_slug("..").is_err());
        assert!(validate_worktree_slug("foo/../bar").is_err());
    }
}
