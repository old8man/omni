//! Team memory paths, validation, prompt generation, and sync.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::fs;
use tracing::{debug, warn};

use super::core::{ENTRYPOINT_NAME, MAX_ENTRYPOINT_LINES};
use super::memory_types::{
    memory_drift_caveat, memory_frontmatter_example, trusting_recall_section,
    types_section_combined, what_not_to_save_section,
};
use super::paths::{get_auto_mem_path, is_auto_memory_enabled};

const TEAM_DIR: &str = "team";
const DIRS_EXIST_GUIDANCE: &str = "Both directories already exist \u{2014} write to them directly with the Write tool (do not run mkdir or check for their existence).";

/// Whether team memory features are enabled.
pub fn is_team_memory_enabled() -> bool {
    if !is_auto_memory_enabled() { return false; }
    std::env::var("CLAUDE_TEAM_MEMORY_ENABLED")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

/// Returns the team memory path: `<auto_mem_path>/team/`
pub fn get_team_mem_path() -> PathBuf {
    get_auto_mem_path().join(TEAM_DIR)
}

/// Returns the team memory entrypoint.
pub fn get_team_mem_entrypoint() -> PathBuf {
    get_team_mem_path().join(ENTRYPOINT_NAME)
}

/// Check if a resolved absolute path is within the team memory directory.
pub fn is_team_mem_path(path: &Path) -> bool {
    path.starts_with(get_team_mem_path())
}

/// Check if a file path is within team memory AND team memory is enabled.
pub fn is_team_mem_file(path: &Path) -> bool {
    is_team_memory_enabled() && is_team_mem_path(path)
}

/// Validate that a relative path key is safe for use in the team memory directory.
///
/// Rejects null bytes, URL-encoded traversals, backslashes, absolute paths,
/// and Unicode normalization attacks.
pub fn validate_team_mem_key(key: &str) -> Result<PathBuf, TeamMemPathError> {
    // Null bytes can truncate paths in syscalls
    if key.contains('\0') {
        return Err(TeamMemPathError::NullByte(key.to_string()));
    }
    // Reject backslashes (Windows path separator used as traversal vector)
    if key.contains('\\') {
        return Err(TeamMemPathError::Backslash(key.to_string()));
    }
    // Reject absolute paths
    if key.starts_with('/') {
        return Err(TeamMemPathError::AbsolutePath(key.to_string()));
    }
    // Reject .. traversal
    if key.contains("..") {
        return Err(TeamMemPathError::Traversal(key.to_string()));
    }

    let team_dir = get_team_mem_path();
    let full_path = team_dir.join(key);

    // Verify the resolved path is still within team dir
    let canonical = full_path
        .canonicalize()
        .unwrap_or_else(|_| full_path.clone());
    if !canonical.starts_with(&team_dir) && !full_path.starts_with(&team_dir) {
        return Err(TeamMemPathError::Traversal(key.to_string()));
    }

    Ok(full_path)
}

/// Errors from team memory path validation.
#[derive(Debug, thiserror::Error)]
pub enum TeamMemPathError {
    #[error("null byte in path key: {0}")]
    NullByte(String),
    #[error("backslash in path key: {0}")]
    Backslash(String),
    #[error("absolute path key: {0}")]
    AbsolutePath(String),
    #[error("path traversal in key: {0}")]
    Traversal(String),
}

/// Result of a team memory sync operation.
#[derive(Clone, Debug, Default)]
pub struct TeamMemorySyncResult {
    /// Number of files synced from the team directory.
    pub files_synced: usize,
    /// Number of files that were new (not previously seen).
    pub files_new: usize,
    /// Number of files that were updated (newer mtime).
    pub files_updated: usize,
    /// Number of files that failed to sync.
    pub files_failed: usize,
    /// Total bytes transferred.
    pub bytes_transferred: u64,
}

/// Synchronize team memories from a source directory to the team memory directory.
///
/// This copies `.md` files from `source_dir` into the team memory directory,
/// preserving directory structure. Files are only copied if they are newer than
/// the existing version (or don't exist locally). This provides one-directional
/// sync suitable for team memory distribution.
///
/// In the TypeScript original, team memory sync happens at session start and
/// is managed by a server-side sync mechanism. This function provides the
/// local-side logic for pulling updates.
pub async fn sync_team_memories(source_dir: &Path) -> TeamMemorySyncResult {
    let team_dir = get_team_mem_path();
    let mut result = TeamMemorySyncResult::default();

    if !source_dir.exists() {
        debug!("team memory source dir does not exist: {}", source_dir.display());
        return result;
    }

    // Ensure team directory exists
    if let Err(e) = fs::create_dir_all(&team_dir).await {
        warn!("failed to create team memory directory: {e}");
        return result;
    }

    // Scan source for .md files
    let source_files = match collect_md_files_flat(source_dir).await {
        Ok(files) => files,
        Err(e) => {
            warn!("failed to scan team memory source: {e}");
            return result;
        }
    };

    for (relative_path, source_path) in &source_files {
        let dest_path = team_dir.join(relative_path);

        // Get source mtime
        let source_meta = match fs::metadata(source_path).await {
            Ok(m) => m,
            Err(e) => {
                debug!("failed to stat source file {}: {e}", source_path.display());
                result.files_failed += 1;
                continue;
            }
        };
        let source_mtime = source_meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Check if we need to update
        let needs_update = if let Ok(dest_meta) = fs::metadata(&dest_path).await {
            let dest_mtime = dest_meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            source_mtime > dest_mtime
        } else {
            true // File doesn't exist locally
        };

        if !needs_update {
            continue;
        }

        // Ensure parent directory exists
        if let Some(parent) = dest_path.parent() {
            if let Err(e) = fs::create_dir_all(parent).await {
                debug!("failed to create parent dir for {}: {e}", dest_path.display());
                result.files_failed += 1;
                continue;
            }
        }

        // Copy the file
        match fs::copy(source_path, &dest_path).await {
            Ok(bytes) => {
                result.bytes_transferred += bytes;
                result.files_synced += 1;
                if fs::metadata(&dest_path).await.is_ok() {
                    result.files_updated += 1;
                } else {
                    result.files_new += 1;
                }
                debug!("synced team memory: {} ({} bytes)", relative_path, bytes);
            }
            Err(e) => {
                debug!("failed to sync team memory {}: {e}", relative_path);
                result.files_failed += 1;
            }
        }
    }

    result
}

/// Collect all .md files in a directory tree, returning (relative_path, absolute_path).
async fn collect_md_files_flat(base: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut results = Vec::new();
    collect_md_recursive(base, base, &mut results).await?;
    Ok(results)
}

fn collect_md_recursive<'a>(
    base: &'a Path,
    dir: &'a Path,
    results: &'a mut Vec<(String, PathBuf)>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;

            if file_type.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if !name.starts_with('.') {
                        collect_md_recursive(base, &path, results).await?;
                    }
                }
            } else if file_type.is_file() {
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                if name.ends_with(".md") {
                    let relative = path.strip_prefix(base)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    results.push((relative, path));
                }
            }
        }
        Ok(())
    })
}

/// Build the combined prompt when both auto memory and team memory are enabled.
pub fn build_combined_memory_prompt(extra_guidelines: Option<&[String]>) -> String {
    let auto_dir = get_auto_mem_path();
    let team_dir = get_team_mem_path();
    let auto_dir_str = auto_dir.to_string_lossy();
    let team_dir_str = team_dir.to_string_lossy();

    let mut lines: Vec<String> = vec![
        "# Memory".into(),
        String::new(),
        format!("You have a persistent, file-based memory system with two directories: a private directory at `{auto_dir_str}` and a shared team directory at `{team_dir_str}`. {DIRS_EXIST_GUIDANCE}"),
        String::new(),
        "You should build up this memory system over time so that future conversations can have a complete picture of who the user is, how they'd like to collaborate with you, what behaviors to avoid or repeat, and the context behind the work the user gives you.".into(),
        String::new(),
        "If the user explicitly asks you to remember something, save it immediately as whichever type fits best. If they ask you to forget something, find and remove the relevant entry.".into(),
        String::new(),
        "## Memory scope".into(),
        String::new(),
        "There are two scope levels:".into(),
        String::new(),
        format!("- private: memories that are private between you and the current user. They persist across conversations with only this specific user and are stored at the root `{auto_dir_str}`."),
        format!("- team: memories that are shared with and contributed by all of the users who work within this project directory. Team memories are synced at the beginning of every session and they are stored at `{team_dir_str}`."),
        String::new(),
    ];

    lines.extend(types_section_combined());
    lines.extend(what_not_to_save_section());
    lines.push("- You MUST avoid saving sensitive data within shared team memories. For example, never save API keys or user credentials.".into());
    lines.push(String::new());

    lines.push("## How to save memories".into());
    lines.push(String::new());
    lines.push("Saving a memory is a two-step process:".into());
    lines.push(String::new());
    lines.push("**Step 1** \u{2014} write the memory to its own file in the chosen directory (private or team, per the type's scope guidance) using this frontmatter format:".into());
    lines.push(String::new());
    lines.extend(memory_frontmatter_example());
    lines.push(String::new());
    lines.push(format!("**Step 2** \u{2014} add a pointer to that file in the same directory's `{ENTRYPOINT_NAME}`. Each directory (private and team) has its own `{ENTRYPOINT_NAME}` index \u{2014} each entry should be one line, under ~150 characters: `- [Title](file.md) \u{2014} one-line hook`. They have no frontmatter. Never write memory content directly into a `{ENTRYPOINT_NAME}`."));
    lines.push(String::new());
    lines.push(format!("- Both `{ENTRYPOINT_NAME}` indexes are loaded into your conversation context \u{2014} lines after {MAX_ENTRYPOINT_LINES} will be truncated, so keep them concise"));
    lines.push("- Keep the name, description, and type fields in memory files up-to-date with the content".into());
    lines.push("- Organize memory semantically by topic, not chronologically".into());
    lines.push("- Update or remove memories that turn out to be wrong or outdated".into());
    lines.push("- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".into());
    lines.push(String::new());

    lines.push("## When to access memories".into());
    lines.push("- When memories (personal or team) seem relevant, or the user references prior work with them or others in their organization.".into());
    lines.push("- You MUST access memory when the user explicitly asks you to check, recall, or remember.".into());
    lines.push("- If the user says to *ignore* or *not use* memory: proceed as if MEMORY.md were empty. Do not apply remembered facts, cite, compare against, or mention memory content.".into());
    lines.push(memory_drift_caveat());
    lines.push(String::new());

    lines.extend(trusting_recall_section());
    lines.push(String::new());

    lines.push("## Memory and other forms of persistence".into());
    lines.push("Memory is one of several persistence mechanisms available to you as you assist the user in a given conversation. The distinction is often that memory can be recalled in future conversations and should not be used for persisting information that is only useful within the scope of the current conversation.".into());
    lines.push("- When to use or update a plan instead of memory: If you are about to start a non-trivial implementation task and would like to reach alignment with the user on your approach you should use a Plan rather than saving this information to memory. Similarly, if you already have a plan within the conversation and you have changed your approach persist that change by updating the plan rather than saving a memory.".into());
    lines.push("- When to use or update tasks instead of memory: When you need to break your work in current conversation into discrete steps or keep track of your progress use tasks instead of saving to memory. Tasks are great for persisting information about the work that needs to be done in the current conversation, but memory should be reserved for information that will be useful in future conversations.".into());

    if let Some(extra) = extra_guidelines {
        for guideline in extra { lines.push(guideline.clone()); }
    }
    lines.push(String::new());

    lines.extend(super::core::build_searching_past_context_section(
        &auto_dir.to_string_lossy(),
    ));

    lines.join("\n")
}

/// Build the memory prompt section for team memory injection into system prompt.
///
/// Reads both auto and team MEMORY.md entrypoints, combines with behavioral
/// instructions. This is used when both auto and team memory are enabled.
pub fn build_team_memory_prompt_section(
    auto_dir: &str,
    team_dir: &str,
    extra_guidelines: Option<&[String]>,
) -> String {
    let auto_entrypoint = format!("{auto_dir}{ENTRYPOINT_NAME}");
    let team_entrypoint = format!("{team_dir}{ENTRYPOINT_NAME}");

    let auto_content = std::fs::read_to_string(&auto_entrypoint).unwrap_or_default();
    let team_content = std::fs::read_to_string(&team_entrypoint).unwrap_or_default();

    let mut lines = Vec::new();
    lines.push(build_combined_memory_prompt(extra_guidelines));

    // Append auto memory content
    if !auto_content.trim().is_empty() {
        let t = super::core::truncate_entrypoint_content(&auto_content);
        lines.push(format!("\n## Private {ENTRYPOINT_NAME}\n"));
        lines.push(t.content);
    }

    // Append team memory content
    if !team_content.trim().is_empty() {
        let t = super::core::truncate_entrypoint_content(&team_content);
        lines.push(format!("\n## Team {ENTRYPOINT_NAME}\n"));
        lines.push(t.content);
    }

    lines.join("\n")
}

/// Scan the project for relevant memory files (CLAUDE.md, .claude/ directory).
///
/// Returns a map of file paths to their content. This is used during project
/// initialization to discover existing memory-like files that should be
/// considered alongside the formal memory system.
pub async fn scan_project_memory_files(project_root: &Path) -> HashMap<String, String> {
    let mut results = HashMap::new();

    // Check for CLAUDE.md in project root
    let claude_md = project_root.join("CLAUDE.md");
    if let Ok(content) = fs::read_to_string(&claude_md).await {
        results.insert(claude_md.to_string_lossy().to_string(), content);
    }

    // Check for .claude/ directory
    let claude_dir = project_root.join(crate::config::paths::PROJECT_DIR_NAME);
    if claude_dir.is_dir() {
        // Scan for .md files in .claude/
        if let Ok(mut entries) = fs::read_dir(&claude_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".md") {
                        if let Ok(content) = fs::read_to_string(&path).await {
                            results.insert(path.to_string_lossy().to_string(), content);
                        }
                    }
                }
            }
        }
    }

    // Check for CLAUDE.md files in subdirectories (one level deep)
    if let Ok(mut entries) = fs::read_dir(project_root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                let subdir_claude_md = entry.path().join("CLAUDE.md");
                if let Ok(content) = fs::read_to_string(&subdir_claude_md).await {
                    results.insert(subdir_claude_md.to_string_lossy().to_string(), content);
                }
            }
        }
    }

    results
}

/// Prune stale team memories that haven't been updated in the given number of days.
///
/// Returns the number of files removed.
pub async fn prune_stale_team_memories(max_age_days: u64) -> usize {
    let team_dir = get_team_mem_path();
    let headers = super::scan::scan_memory_files(&team_dir).await;
    let mut pruned = 0;

    for header in &headers {
        if super::age::is_stale(header.mtime_ms, max_age_days) {
            match fs::remove_file(&header.file_path).await {
                Ok(()) => {
                    debug!("pruned stale team memory: {}", header.filename);
                    pruned += 1;
                }
                Err(e) => {
                    debug!("failed to prune team memory {}: {e}", header.filename);
                }
            }
        }
    }

    pruned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_team_mem_path() {
        let team_path = get_team_mem_path();
        let s = team_path.to_string_lossy();
        assert!(s.ends_with("team") || s.ends_with("team/") || s.ends_with("team\\"));
    }

    #[test]
    fn test_get_team_mem_entrypoint() {
        let ep = get_team_mem_entrypoint();
        let s = ep.to_string_lossy();
        assert!(s.ends_with("MEMORY.md"));
        assert!(s.contains("team"));
    }

    #[test]
    fn test_is_team_mem_path() {
        let team_dir = get_team_mem_path();
        assert!(is_team_mem_path(&team_dir.join("some_memory.md")));
        assert!(!is_team_mem_path(&get_auto_mem_path().join("private.md")));
    }

    #[test]
    fn test_build_combined_memory_prompt_basic() {
        let prompt = build_combined_memory_prompt(None);
        assert!(prompt.contains("# Memory"));
        assert!(prompt.contains("## Memory scope"));
        assert!(prompt.contains("## Types of memory"));
        assert!(prompt.contains("## What NOT to save in memory"));
        assert!(prompt.contains("## When to access memories"));
        assert!(prompt.contains("## Before recommending from memory"));
        assert!(prompt.contains("## Searching past context"));
    }

    #[test]
    fn test_build_combined_with_extra() {
        let extra = vec!["Custom guideline.".to_string()];
        let prompt = build_combined_memory_prompt(Some(&extra));
        assert!(prompt.contains("Custom guideline."));
    }

    #[test]
    fn test_validate_team_mem_key_valid() {
        let result = validate_team_mem_key("user_role.md");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_team_mem_key_traversal() {
        let result = validate_team_mem_key("../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_team_mem_key_null_byte() {
        let result = validate_team_mem_key("file\0.md");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_team_mem_key_backslash() {
        let result = validate_team_mem_key("sub\\file.md");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_team_mem_key_absolute() {
        let result = validate_team_mem_key("/etc/passwd");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sync_team_memories() {
        let source = tempfile::TempDir::new().unwrap();
        let _dest = tempfile::TempDir::new().unwrap();

        // Create source files
        tokio::fs::write(
            source.path().join("shared.md"),
            "---\nname: shared\ndescription: shared memory\ntype: project\n---\nContent",
        ).await.unwrap();

        // sync_team_memories uses get_team_mem_path() which is fixed to the env,
        // so we test the underlying file copy logic instead
        let files = collect_md_files_flat(source.path()).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "shared.md");
    }

    #[tokio::test]
    async fn test_scan_project_memory_files() {
        let tmp = tempfile::TempDir::new().unwrap();

        // Create CLAUDE.md
        tokio::fs::write(tmp.path().join("CLAUDE.md"), "# Project Rules").await.unwrap();

        // Create .claude-omni/ directory with a file
        let claude_dir = tmp.path().join(crate::config::paths::PROJECT_DIR_NAME);
        tokio::fs::create_dir(&claude_dir).await.unwrap();
        tokio::fs::write(claude_dir.join("settings.md"), "# Settings").await.unwrap();

        let results = scan_project_memory_files(tmp.path()).await;
        assert!(results.len() >= 2);
        assert!(results.values().any(|v| v.contains("Project Rules")));
        assert!(results.values().any(|v| v.contains("Settings")));
    }
}
