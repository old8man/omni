//! Team memory paths, validation, and prompt generation.

use std::path::{Path, PathBuf};

use super::memdir::{ENTRYPOINT_NAME, MAX_ENTRYPOINT_LINES};
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
    path.starts_with(&get_team_mem_path())
}

/// Check if a file path is within team memory AND team memory is enabled.
pub fn is_team_mem_file(path: &Path) -> bool {
    is_team_memory_enabled() && is_team_mem_path(path)
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

    lines.join("\n")
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
    }

    #[test]
    fn test_build_combined_with_extra() {
        let extra = vec!["Custom guideline.".to_string()];
        let prompt = build_combined_memory_prompt(Some(&extra));
        assert!(prompt.contains("Custom guideline."));
    }
}
