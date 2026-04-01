//! Git Bundle Operations
//!
//! Creates git bundles of the current repository state for session migration.
//! Supports a fallback chain: `--all` -> `HEAD` -> squashed-root, falling back
//! to smaller bundles when the repo is too large.
//!
//! Flow:
//!   1. `git stash create` -> `update-ref refs/seed/stash` (makes WIP reachable)
//!   2. `git bundle create --all` (packs refs/seed/stash + its objects)
//!   3. Upload to Files API
//!   4. Cleanup refs/seed/stash (don't pollute user's repo)

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use tokio::process::Command;

/// Default maximum bundle size (100 MB).
const DEFAULT_BUNDLE_MAX_BYTES: u64 = 100 * 1024 * 1024;

/// Scope of the git bundle (what was included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleScope {
    /// All refs and objects.
    All,
    /// Only HEAD and its history.
    Head,
    /// A single parentless commit with HEAD's tree (no history).
    Squashed,
}

impl std::fmt::Display for BundleScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => write!(f, "all"),
            Self::Head => write!(f, "head"),
            Self::Squashed => write!(f, "squashed"),
        }
    }
}

/// The reason a bundle creation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleFailReason {
    GitError,
    TooLarge,
    EmptyRepo,
}

/// Result of creating and uploading a git bundle.
#[derive(Debug)]
pub enum BundleUploadResult {
    Success {
        file_id: String,
        bundle_size_bytes: u64,
        scope: BundleScope,
        has_wip: bool,
    },
    Failure {
        error: String,
        fail_reason: Option<BundleFailReason>,
    },
}

/// Result of the bundle creation step (before upload).
pub enum BundleCreateResult {
    Ok { size: u64, scope: BundleScope },
    Err { error: String, fail_reason: BundleFailReason },
}

/// Find the git repository root for a given working directory.
pub fn find_git_root(cwd: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(PathBuf::from(root))
    }
}

/// Run a git command and return (exit_code, stdout, stderr).
async fn git_exec(
    args: &[&str],
    cwd: &Path,
) -> Result<(i32, String, String)> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute git command")?;

    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((code, stdout, stderr))
}

/// Bundle with fallback chain: --all -> HEAD -> squashed-root.
///
/// Each step produces a smaller bundle. If the `--all` bundle exceeds
/// `max_bytes`, tries HEAD-only. If that's still too large, creates a
/// squashed single-commit snapshot.
async fn bundle_with_fallback(
    git_root: &Path,
    bundle_path: &Path,
    max_bytes: u64,
    has_stash: bool,
) -> BundleCreateResult {
    let extra_refs: Vec<&str> = if has_stash {
        vec!["refs/seed/stash"]
    } else {
        vec![]
    };

    // Try --all first
    let mut args = vec![
        "bundle",
        "create",
        bundle_path.to_str().unwrap_or_default(),
        "--all",
    ];
    args.extend(&extra_refs);

    let (code, _, stderr) = match git_exec(&args, git_root).await {
        Ok(r) => r,
        Err(e) => {
            return BundleCreateResult::Err {
                error: format!("git bundle create --all failed: {e}"),
                fail_reason: BundleFailReason::GitError,
            };
        }
    };

    if code != 0 {
        return BundleCreateResult::Err {
            error: format!(
                "git bundle create --all failed ({code}): {}",
                &stderr[..stderr.len().min(200)]
            ),
            fail_reason: BundleFailReason::GitError,
        };
    }

    let all_size = match tokio::fs::metadata(bundle_path).await {
        Ok(m) => m.len(),
        Err(e) => {
            return BundleCreateResult::Err {
                error: format!("Failed to stat bundle: {e}"),
                fail_reason: BundleFailReason::GitError,
            };
        }
    };

    if all_size <= max_bytes {
        return BundleCreateResult::Ok {
            size: all_size,
            scope: BundleScope::All,
        };
    }

    // Try HEAD-only
    tracing::debug!(
        "[gitBundle] --all bundle is {:.1}MB (> {}MB), retrying HEAD-only",
        all_size as f64 / 1024.0 / 1024.0,
        max_bytes / 1024 / 1024
    );

    let mut args = vec![
        "bundle",
        "create",
        bundle_path.to_str().unwrap_or_default(),
        "HEAD",
    ];
    args.extend(&extra_refs);

    let (code, _, stderr) = match git_exec(&args, git_root).await {
        Ok(r) => r,
        Err(e) => {
            return BundleCreateResult::Err {
                error: format!("git bundle create HEAD failed: {e}"),
                fail_reason: BundleFailReason::GitError,
            };
        }
    };

    if code != 0 {
        return BundleCreateResult::Err {
            error: format!(
                "git bundle create HEAD failed ({code}): {}",
                &stderr[..stderr.len().min(200)]
            ),
            fail_reason: BundleFailReason::GitError,
        };
    }

    let head_size = match tokio::fs::metadata(bundle_path).await {
        Ok(m) => m.len(),
        Err(e) => {
            return BundleCreateResult::Err {
                error: format!("Failed to stat bundle: {e}"),
                fail_reason: BundleFailReason::GitError,
            };
        }
    };

    if head_size <= max_bytes {
        return BundleCreateResult::Ok {
            size: head_size,
            scope: BundleScope::Head,
        };
    }

    // Last resort: squash to a single parentless commit
    tracing::debug!(
        "[gitBundle] HEAD bundle is {:.1}MB, retrying squashed-root",
        head_size as f64 / 1024.0 / 1024.0
    );

    let tree_ref = if has_stash {
        "refs/seed/stash^{tree}"
    } else {
        "HEAD^{tree}"
    };

    let (code, stdout, stderr) =
        match git_exec(&["commit-tree", tree_ref, "-m", "seed"], git_root).await {
            Ok(r) => r,
            Err(e) => {
                return BundleCreateResult::Err {
                    error: format!("git commit-tree failed: {e}"),
                    fail_reason: BundleFailReason::GitError,
                };
            }
        };

    if code != 0 {
        return BundleCreateResult::Err {
            error: format!(
                "git commit-tree failed ({code}): {}",
                &stderr[..stderr.len().min(200)]
            ),
            fail_reason: BundleFailReason::GitError,
        };
    }

    let squashed_sha = stdout.trim();
    let _ = git_exec(
        &["update-ref", "refs/seed/root", squashed_sha],
        git_root,
    )
    .await;

    let (code, _, stderr) = match git_exec(
        &[
            "bundle",
            "create",
            bundle_path.to_str().unwrap_or_default(),
            "refs/seed/root",
        ],
        git_root,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return BundleCreateResult::Err {
                error: format!("git bundle create refs/seed/root failed: {e}"),
                fail_reason: BundleFailReason::GitError,
            };
        }
    };

    if code != 0 {
        return BundleCreateResult::Err {
            error: format!(
                "git bundle create refs/seed/root failed ({code}): {}",
                &stderr[..stderr.len().min(200)]
            ),
            fail_reason: BundleFailReason::GitError,
        };
    }

    let squash_size = match tokio::fs::metadata(bundle_path).await {
        Ok(m) => m.len(),
        Err(e) => {
            return BundleCreateResult::Err {
                error: format!("Failed to stat bundle: {e}"),
                fail_reason: BundleFailReason::GitError,
            };
        }
    };

    if squash_size <= max_bytes {
        return BundleCreateResult::Ok {
            size: squash_size,
            scope: BundleScope::Squashed,
        };
    }

    BundleCreateResult::Err {
        error: "Repo is too large to bundle. Please setup GitHub on https://claude.ai/code"
            .into(),
        fail_reason: BundleFailReason::TooLarge,
    }
}

/// Create a git bundle of the repository at `cwd`.
///
/// This captures:
/// - All refs and objects (falls back to HEAD-only or squashed if too large)
/// - Uncommitted changes via `git stash create` (tracked files only)
///
/// Returns the path to the created bundle file and metadata about what was included.
///
/// # Arguments
///
/// * `cwd` - Working directory inside the git repository
/// * `max_bytes` - Maximum bundle size in bytes (defaults to 100MB)
///
/// # Errors
///
/// Returns an error if:
/// - `cwd` is not inside a git repository
/// - The repository has no commits
/// - Git commands fail
/// - The bundle exceeds `max_bytes` even after fallback compression
pub async fn create_git_bundle(
    cwd: &Path,
    max_bytes: Option<u64>,
) -> Result<(PathBuf, BundleScope, bool)> {
    let git_root = find_git_root(cwd)
        .context("Not in a git repository")?;

    let max_bytes = max_bytes.unwrap_or(DEFAULT_BUNDLE_MAX_BYTES);

    // Sweep stale refs from a crashed prior run
    for seed_ref in &["refs/seed/stash", "refs/seed/root"] {
        let _ = git_exec(&["update-ref", "-d", seed_ref], &git_root).await;
    }

    // Check for any refs -- empty repos cannot be bundled
    let (code, stdout, _) = git_exec(
        &["for-each-ref", "--count=1", "refs/"],
        &git_root,
    )
    .await?;

    if code == 0 && stdout.trim().is_empty() {
        bail!("Repository has no commits yet");
    }

    // Capture WIP via stash create (doesn't touch working tree)
    let (stash_code, stash_stdout, stash_stderr) =
        git_exec(&["stash", "create"], &git_root).await?;

    let wip_sha = if stash_code == 0 {
        stash_stdout.trim().to_string()
    } else {
        tracing::debug!(
            "[gitBundle] git stash create failed ({stash_code}), proceeding without WIP: {}",
            &stash_stderr[..stash_stderr.len().min(200)]
        );
        String::new()
    };
    let has_wip = !wip_sha.is_empty();

    if has_wip {
        tracing::debug!("[gitBundle] Captured WIP as stash {wip_sha}");
        let _ = git_exec(
            &["update-ref", "refs/seed/stash", &wip_sha],
            &git_root,
        )
        .await;
    }

    // Create the bundle using the fallback chain
    let bundle_path = std::env::temp_dir().join(format!(
        "ccr-seed-{}.bundle",
        uuid::Uuid::new_v4()
    ));

    let result =
        bundle_with_fallback(&git_root, &bundle_path, max_bytes, has_wip).await;

    // Always clean up seed refs
    let cleanup = async {
        for seed_ref in &["refs/seed/stash", "refs/seed/root"] {
            let _ = git_exec(&["update-ref", "-d", seed_ref], &git_root).await;
        }
    };

    match result {
        BundleCreateResult::Ok { scope, .. } => {
            cleanup.await;
            Ok((bundle_path, scope, has_wip))
        }
        BundleCreateResult::Err { error, .. } => {
            // Clean up the bundle file on failure
            let _ = tokio::fs::remove_file(&bundle_path).await;
            cleanup.await;
            bail!("{error}");
        }
    }
}

/// Validate that a git bundle is compatible with the target repository.
///
/// Checks that the bundle's prerequisite commits exist in the target repo.
pub async fn validate_bundle_compatibility(
    bundle_path: &Path,
    target_git_root: &Path,
) -> Result<bool> {
    let (code, _, _) = git_exec(
        &[
            "bundle",
            "verify",
            bundle_path.to_str().unwrap_or_default(),
        ],
        target_git_root,
    )
    .await?;

    Ok(code == 0)
}

/// Apply a git bundle to a target repository.
///
/// Fetches all refs from the bundle into the target repository.
pub async fn apply_git_bundle(
    bundle_path: &Path,
    target_git_root: &Path,
) -> Result<()> {
    let (code, _, stderr) = git_exec(
        &[
            "fetch",
            bundle_path.to_str().unwrap_or_default(),
            "+refs/*:refs/*",
        ],
        target_git_root,
    )
    .await?;

    if code != 0 {
        bail!(
            "Failed to apply git bundle: {}",
            &stderr[..stderr.len().min(500)]
        );
    }

    // If there's a seed stash ref, apply it
    let (code, stdout, _) =
        git_exec(&["rev-parse", "--verify", "refs/seed/stash"], target_git_root).await?;

    if code == 0 && !stdout.trim().is_empty() {
        tracing::debug!("[gitBundle] Applying WIP from refs/seed/stash");
        let (apply_code, _, apply_stderr) =
            git_exec(&["stash", "apply", "refs/seed/stash"], target_git_root).await?;

        if apply_code != 0 {
            tracing::warn!(
                "[gitBundle] Failed to apply WIP stash (non-fatal): {}",
                &apply_stderr[..apply_stderr.len().min(200)]
            );
        }

        // Clean up the seed ref
        let _ = git_exec(&["update-ref", "-d", "refs/seed/stash"], target_git_root).await;
    }

    // Clean up seed root ref if present
    let _ = git_exec(&["update-ref", "-d", "refs/seed/root"], target_git_root).await;

    Ok(())
}

/// Check if the working directory has uncommitted changes.
pub async fn has_dirty_working_directory(git_root: &Path) -> Result<bool> {
    let (code, stdout, _) =
        git_exec(&["status", "--porcelain"], git_root).await?;

    if code != 0 {
        bail!("git status failed");
    }

    Ok(!stdout.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_scope_display() {
        assert_eq!(BundleScope::All.to_string(), "all");
        assert_eq!(BundleScope::Head.to_string(), "head");
        assert_eq!(BundleScope::Squashed.to_string(), "squashed");
    }

    #[test]
    fn find_git_root_outside_repo() {
        // /tmp is unlikely to be a git repo
        let result = find_git_root(Path::new("/tmp"));
        // This may or may not be None depending on the environment,
        // so we just verify it doesn't panic.
        let _ = result;
    }
}
