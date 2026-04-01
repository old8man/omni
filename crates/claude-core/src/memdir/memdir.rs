//! Main memory directory system.

use std::path::Path;

use tokio::fs;
use tracing::{debug, warn};

use crate::utils::format::format_file_size;

use super::memory_types::{
    memory_frontmatter_example, trusting_recall_section, types_section_individual,
    what_not_to_save_section, when_to_access_section,
};
use super::paths::{get_auto_mem_path, is_auto_memory_enabled};
use super::team_mem::{build_combined_memory_prompt, is_team_memory_enabled};

pub const ENTRYPOINT_NAME: &str = "MEMORY.md";
pub const MAX_ENTRYPOINT_LINES: usize = 200;
pub const MAX_ENTRYPOINT_BYTES: usize = 25_000;

const AUTO_MEM_DISPLAY_NAME: &str = "auto memory";

pub const DIR_EXISTS_GUIDANCE: &str = "This directory already exists \u{2014} write to it directly with the Write tool (do not run mkdir or check for its existence).";

/// Result of truncating entrypoint content.
#[derive(Clone, Debug)]
pub struct EntrypointTruncation {
    pub content: String,
    pub line_count: usize,
    pub byte_count: usize,
    pub was_line_truncated: bool,
    pub was_byte_truncated: bool,
}

/// Truncate MEMORY.md content to the line AND byte caps.
pub fn truncate_entrypoint_content(raw: &str) -> EntrypointTruncation {
    let trimmed = raw.trim();
    let content_lines: Vec<&str> = trimmed.split('\n').collect();
    let line_count = content_lines.len();
    let byte_count = trimmed.len();

    let was_line_truncated = line_count > MAX_ENTRYPOINT_LINES;
    let was_byte_truncated = byte_count > MAX_ENTRYPOINT_BYTES;

    if !was_line_truncated && !was_byte_truncated {
        return EntrypointTruncation {
            content: trimmed.to_string(), line_count, byte_count,
            was_line_truncated, was_byte_truncated,
        };
    }

    let mut truncated = if was_line_truncated {
        content_lines[..MAX_ENTRYPOINT_LINES].join("\n")
    } else {
        trimmed.to_string()
    };

    if truncated.len() > MAX_ENTRYPOINT_BYTES {
        let cut_at = truncated[..MAX_ENTRYPOINT_BYTES].rfind('\n').unwrap_or(MAX_ENTRYPOINT_BYTES);
        truncated.truncate(if cut_at > 0 { cut_at } else { MAX_ENTRYPOINT_BYTES });
    }

    let reason = if was_byte_truncated && !was_line_truncated {
        format!("{} (limit: {}) \u{2014} index entries are too long",
            format_file_size(byte_count as u64), format_file_size(MAX_ENTRYPOINT_BYTES as u64))
    } else if was_line_truncated && !was_byte_truncated {
        format!("{line_count} lines (limit: {MAX_ENTRYPOINT_LINES})")
    } else {
        format!("{line_count} lines and {}", format_file_size(byte_count as u64))
    };

    truncated.push_str(&format!(
        "\n\n> WARNING: {ENTRYPOINT_NAME} is {reason}. Only part of it was loaded. \
         Keep index entries to one line under ~200 chars; move detail into topic files."));

    EntrypointTruncation {
        content: truncated, line_count, byte_count,
        was_line_truncated, was_byte_truncated,
    }
}

/// Ensure a memory directory exists. Idempotent.
pub async fn ensure_memory_dir_exists(memory_dir: &Path) -> std::io::Result<()> {
    match fs::create_dir_all(memory_dir).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => {
            debug!("ensure_memory_dir_exists failed for {}: {}", memory_dir.display(), e);
            Err(e)
        }
    }
}

/// Build the typed-memory behavioral instructions (without MEMORY.md content).
pub fn build_memory_lines(
    display_name: &str,
    memory_dir: &str,
    extra_guidelines: Option<&[String]>,
    skip_index: bool,
) -> Vec<String> {
    let how_to_save = if skip_index {
        let mut lines = vec![
            "## How to save memories".into(), String::new(),
            "Write each memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) using this frontmatter format:".into(),
            String::new(),
        ];
        lines.extend(memory_frontmatter_example());
        lines.push(String::new());
        lines.push("- Keep the name, description, and type fields in memory files up-to-date with the content".into());
        lines.push("- Organize memory semantically by topic, not chronologically".into());
        lines.push("- Update or remove memories that turn out to be wrong or outdated".into());
        lines.push("- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".into());
        lines
    } else {
        let mut lines = vec![
            "## How to save memories".into(), String::new(),
            "Saving a memory is a two-step process:".into(), String::new(),
            "**Step 1** \u{2014} write the memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) using this frontmatter format:".into(),
            String::new(),
        ];
        lines.extend(memory_frontmatter_example());
        lines.push(String::new());
        lines.push(format!("**Step 2** \u{2014} add a pointer to that file in `{ENTRYPOINT_NAME}`. `{ENTRYPOINT_NAME}` is an index, not a memory \u{2014} each entry should be one line, under ~150 characters: `- [Title](file.md) \u{2014} one-line hook`. It has no frontmatter. Never write memory content directly into `{ENTRYPOINT_NAME}`."));
        lines.push(String::new());
        lines.push(format!("- `{ENTRYPOINT_NAME}` is always loaded into your conversation context \u{2014} lines after {MAX_ENTRYPOINT_LINES} will be truncated, so keep the index concise"));
        lines.push("- Keep the name, description, and type fields in memory files up-to-date with the content".into());
        lines.push("- Organize memory semantically by topic, not chronologically".into());
        lines.push("- Update or remove memories that turn out to be wrong or outdated".into());
        lines.push("- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".into());
        lines
    };

    let mut lines: Vec<String> = vec![
        format!("# {display_name}"), String::new(),
        format!("You have a persistent, file-based memory system at `{memory_dir}`. {DIR_EXISTS_GUIDANCE}"),
        String::new(),
        "You should build up this memory system over time so that future conversations can have a complete picture of who the user is, how they'd like to collaborate with you, what behaviors to avoid or repeat, and the context behind the work the user gives you.".into(),
        String::new(),
        "If the user explicitly asks you to remember something, save it immediately as whichever type fits best. If they ask you to forget something, find and remove the relevant entry.".into(),
        String::new(),
    ];
    lines.extend(types_section_individual());
    lines.extend(what_not_to_save_section());
    lines.push(String::new());
    lines.extend(how_to_save);
    lines.push(String::new());
    lines.extend(when_to_access_section());
    lines.push(String::new());
    lines.extend(trusting_recall_section());
    lines.push(String::new());

    lines.push("## Memory and other forms of persistence".into());
    lines.push("Memory is one of several persistence mechanisms available to you as you assist the user in a given conversation. The distinction is often that memory can be recalled in future conversations and should not be used for persisting information that is only useful within the scope of the current conversation.".into());
    lines.push("- When to use or update a plan instead of memory: If you are about to start a non-trivial implementation task and would like to reach alignment with the user on your approach you should use a Plan rather than saving this information to memory. Similarly, if you already have a plan within the conversation and you have changed your approach persist that change by updating the plan rather than saving a memory.".into());
    lines.push("- When to use or update tasks instead of memory: When you need to break your work in current conversation into discrete steps or keep track of your progress use tasks instead of saving to memory. Tasks are great for persisting information about the work that needs to be done in the current conversation, but memory should be reserved for information that will be useful in future conversations.".into());
    lines.push(String::new());

    if let Some(extra) = extra_guidelines {
        for guideline in extra { lines.push(guideline.clone()); }
        lines.push(String::new());
    }
    lines
}

/// Build the typed-memory prompt with MEMORY.md content included.
pub fn build_memory_prompt(display_name: &str, memory_dir: &str, extra_guidelines: Option<&[String]>) -> String {
    let entrypoint = format!("{memory_dir}{ENTRYPOINT_NAME}");
    let entrypoint_content = std::fs::read_to_string(&entrypoint).unwrap_or_default();

    let mut lines = build_memory_lines(display_name, memory_dir, extra_guidelines, false);

    if !entrypoint_content.trim().is_empty() {
        let t = truncate_entrypoint_content(&entrypoint_content);
        if t.was_line_truncated || t.was_byte_truncated {
            debug!(line_count = t.line_count, byte_count = t.byte_count,
                was_line_truncated = t.was_line_truncated, was_byte_truncated = t.was_byte_truncated,
                "truncated MEMORY.md content");
        }
        lines.push(format!("## {ENTRYPOINT_NAME}"));
        lines.push(String::new());
        lines.push(t.content);
    } else {
        lines.push(format!("## {ENTRYPOINT_NAME}"));
        lines.push(String::new());
        lines.push(format!("Your {ENTRYPOINT_NAME} is currently empty. When you save new memories, they will appear here."));
    }
    lines.join("\n")
}

/// Read a single memory file and return its content.
pub async fn read_memory_file(path: &Path) -> Option<String> {
    match fs::read_to_string(path).await {
        Ok(content) => Some(content),
        Err(e) => { debug!("failed to read memory file {}: {e}", path.display()); None }
    }
}

/// Load the unified memory prompt for inclusion in the system prompt.
pub async fn load_memory_prompt() -> Option<String> {
    if !is_auto_memory_enabled() {
        debug!("auto memory is disabled");
        return None;
    }
    let auto_dir = get_auto_mem_path();
    if let Err(e) = ensure_memory_dir_exists(&auto_dir).await {
        warn!("failed to create memory directory: {e}");
    }
    if is_team_memory_enabled() {
        let team_dir = super::team_mem::get_team_mem_path();
        if let Err(e) = ensure_memory_dir_exists(&team_dir).await {
            warn!("failed to create team memory directory: {e}");
        }
        return Some(build_combined_memory_prompt(None));
    }
    let auto_dir_str = auto_dir.to_string_lossy().to_string();
    Some(build_memory_lines(AUTO_MEM_DISPLAY_NAME, &auto_dir_str, None, false).join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_no_truncation() {
        let content = "line 1\nline 2\nline 3";
        let result = truncate_entrypoint_content(content);
        assert_eq!(result.content, content.trim());
        assert_eq!(result.line_count, 3);
        assert!(!result.was_line_truncated);
        assert!(!result.was_byte_truncated);
    }

    #[test]
    fn test_truncate_by_lines() {
        let lines: Vec<String> = (0..250).map(|i| format!("line {i}")).collect();
        let content = lines.join("\n");
        let result = truncate_entrypoint_content(&content);
        assert!(result.was_line_truncated);
        assert_eq!(result.line_count, 250);
        assert!(result.content.contains("WARNING"));
        assert!(result.content.contains("250 lines"));
    }

    #[test]
    fn test_truncate_by_bytes() {
        let long_line = "x".repeat(500);
        let lines: Vec<String> = (0..100).map(|_| long_line.clone()).collect();
        let content = lines.join("\n");
        let result = truncate_entrypoint_content(&content);
        assert!(result.was_byte_truncated);
        assert!(!result.was_line_truncated);
        assert!(result.content.contains("WARNING"));
        assert!(result.content.contains("index entries are too long"));
    }

    #[test]
    fn test_build_memory_lines_contains_sections() {
        let lines = build_memory_lines("test memory", "/tmp/memory/", None, false);
        let prompt = lines.join("\n");
        assert!(prompt.contains("# test memory"));
        assert!(prompt.contains("## Types of memory"));
        assert!(prompt.contains("## What NOT to save in memory"));
        assert!(prompt.contains("## How to save memories"));
        assert!(prompt.contains("## When to access memories"));
        assert!(prompt.contains("## Before recommending from memory"));
    }

    #[test]
    fn test_build_memory_lines_skip_index() {
        let lines = build_memory_lines("test", "/tmp/mem/", None, true);
        let prompt = lines.join("\n");
        assert!(!prompt.contains("Step 2"));
        assert!(prompt.contains("Write each memory to its own file"));
    }

    #[test]
    fn test_build_memory_prompt_empty_entrypoint() {
        let prompt = build_memory_prompt("test", "/nonexistent/path/", None);
        assert!(prompt.contains("currently empty"));
    }

    #[tokio::test]
    async fn test_ensure_memory_dir_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let new_dir = tmp.path().join("memory").join("subdir");
        assert!(!new_dir.exists());
        ensure_memory_dir_exists(&new_dir).await.unwrap();
        assert!(new_dir.exists());
        ensure_memory_dir_exists(&new_dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_read_memory_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("test.md");
        tokio::fs::write(&file, "test content").await.unwrap();
        assert_eq!(read_memory_file(&file).await.as_deref(), Some("test content"));
    }

    #[tokio::test]
    async fn test_read_memory_file_not_found() {
        assert!(read_memory_file(Path::new("/nonexistent/file.md")).await.is_none());
    }
}
