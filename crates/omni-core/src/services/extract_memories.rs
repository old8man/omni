/// Extract durable memories from session transcripts and write them to the
/// auto-memory directory (`~/.claude/projects/<path>/memory/`).
///
/// Runs once at the end of each complete query loop (when the model produces
/// a final response with no tool calls). Uses a cursor to track progress and
/// only considers new messages since the last extraction.
///
/// Port of `services/extractMemories/extractMemories.ts` and
/// `services/extractMemories/prompts.ts`.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::message::Message;

// ── Memory types ───────────────────────────────────────────────────────────

/// Classification of a memory for routing to the correct storage scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryType {
    /// User preferences, habits, communication style.
    User,
    /// Explicit feedback about Claude's behavior.
    Feedback,
    /// Project architecture, patterns, conventions.
    Project,
    /// Reference material: API endpoints, config paths, etc.
    Reference,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryType::User => write!(f, "user"),
            MemoryType::Feedback => write!(f, "feedback"),
            MemoryType::Project => write!(f, "project"),
            MemoryType::Reference => write!(f, "reference"),
        }
    }
}

/// A single extracted memory ready for persistence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtractedMemory {
    pub content: String,
    pub memory_type: MemoryType,
    /// Optional file name hint (e.g. `user_role.md`).
    pub suggested_filename: Option<String>,
}

// ── Frontmatter ────────────────────────────────────────────────────────────

/// Example frontmatter block included in extraction prompts.
pub const MEMORY_FRONTMATTER_EXAMPLE: &str = r#"```markdown
---
type: <user|feedback|project|reference>
title: Short descriptive title
---

Memory content here. Be concise and specific.
```"#;

// ── Memory type taxonomy ───────────────────────────────────────────────────

/// Description of each memory type for the extraction prompt.
pub const TYPES_SECTION: &str = r#"## Memory types

- **user** — Personal preferences, habits, communication style, role, expertise.
  Examples: "prefers concise answers", "uses vim keybindings", "senior backend engineer"
- **feedback** — Explicit feedback about Claude's behavior that should persist.
  Examples: "don't add comments to obvious code", "always run tests after changes"
- **project** — Architecture decisions, conventions, patterns specific to this project.
  Examples: "uses hexagonal architecture", "error handling via Result<T, AppError>"
- **reference** — Frequently referenced facts: API endpoints, config paths, credentials locations.
  Examples: "CI config at .github/workflows/ci.yml", "staging API at https://api.staging.example.com"
"#;

/// Things the extraction agent should NOT save.
pub const WHAT_NOT_TO_SAVE: &str = r#"## What NOT to save

- Transient debugging steps or one-off fixes
- Information already obvious from the codebase (e.g. "uses TypeScript")
- Exact code snippets — summarize patterns instead
- Secrets, API keys, passwords, or credentials
- Conversation-specific context that won't be useful next session
"#;

// ── Prompt builders ────────────────────────────────────────────────────────

/// Build the opener shared by all extraction prompt variants.
fn opener(new_message_count: usize, existing_memories: &str) -> String {
    let manifest = if !existing_memories.is_empty() {
        format!(
            "\n\n## Existing memory files\n\n{}\n\nCheck this list before writing \
             — update an existing file rather than creating a duplicate.",
            existing_memories
        )
    } else {
        String::new()
    };

    format!(
        "You are now acting as the memory extraction subagent. Analyze the most \
         recent ~{count} messages above and use them to update your persistent memory systems.\n\n\
         Available tools: Read, Grep, Glob, read-only Bash (ls/find/cat/stat/wc/head/tail \
         and similar), and Edit/Write for paths inside the memory directory only. \
         Bash rm is not permitted. All other tools will be denied.\n\n\
         You have a limited turn budget. Edit requires a prior Read of the same file, \
         so the efficient strategy is: turn 1 — issue all Read calls in parallel for every \
         file you might update; turn 2 — issue all Write/Edit calls in parallel. \
         Do not interleave reads and writes across multiple turns.\n\n\
         You MUST only use content from the last ~{count} messages to update your \
         persistent memories. Do not waste any turns attempting to investigate or verify \
         that content further.{manifest}",
        count = new_message_count,
        manifest = manifest,
    )
}

/// Build the extraction prompt for auto-only memory (no team memory).
pub fn build_extract_auto_only_prompt(
    new_message_count: usize,
    existing_memories: &str,
    skip_index: bool,
) -> String {
    let how_to_save = if skip_index {
        format!(
            "## How to save memories\n\n\
             Write each memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) \
             using this frontmatter format:\n\n{}\n\n\
             - Organize memory semantically by topic, not chronologically\n\
             - Update or remove memories that turn out to be wrong or outdated\n\
             - Do not write duplicate memories. First check if there is an existing memory \
             you can update before writing a new one.",
            MEMORY_FRONTMATTER_EXAMPLE
        )
    } else {
        format!(
            "## How to save memories\n\n\
             Saving a memory is a two-step process:\n\n\
             **Step 1** — write the memory to its own file (e.g., `user_role.md`, \
             `feedback_testing.md`) using this frontmatter format:\n\n{}\n\n\
             **Step 2** — add a pointer to that file in `MEMORY.md`. `MEMORY.md` is an \
             index, not a memory — each entry should be one line, under ~150 characters: \
             `- [Title](file.md) — one-line hook`. It has no frontmatter. Never write \
             memory content directly into `MEMORY.md`.\n\n\
             - `MEMORY.md` is always loaded into your system prompt — lines after 200 \
             will be truncated, so keep the index concise\n\
             - Organize memory semantically by topic, not chronologically\n\
             - Update or remove memories that turn out to be wrong or outdated\n\
             - Do not write duplicate memories. First check if there is an existing memory \
             you can update before writing a new one.",
            MEMORY_FRONTMATTER_EXAMPLE
        )
    };

    format!(
        "{opener}\n\n\
         If the user explicitly asks you to remember something, save it immediately as \
         whichever type fits best. If they ask you to forget something, find and remove \
         the relevant entry.\n\n\
         {types}\n{not_save}\n\n{how_to_save}",
        opener = opener(new_message_count, existing_memories),
        types = TYPES_SECTION,
        not_save = WHAT_NOT_TO_SAVE,
        how_to_save = how_to_save,
    )
}

/// Build the extraction prompt for combined auto + team memory.
pub fn build_extract_combined_prompt(
    new_message_count: usize,
    existing_memories: &str,
    skip_index: bool,
) -> String {
    let how_to_save = if skip_index {
        format!(
            "## How to save memories\n\n\
             Write each memory to its own file in the chosen directory (private or team, \
             per the type's scope guidance) using this frontmatter format:\n\n{}\n\n\
             - Organize memory semantically by topic, not chronologically\n\
             - Update or remove memories that turn out to be wrong or outdated\n\
             - Do not write duplicate memories. First check if there is an existing memory \
             you can update before writing a new one.",
            MEMORY_FRONTMATTER_EXAMPLE
        )
    } else {
        format!(
            "## How to save memories\n\n\
             Saving a memory is a two-step process:\n\n\
             **Step 1** — write the memory to its own file in the chosen directory (private \
             or team, per the type's scope guidance) using this frontmatter format:\n\n{}\n\n\
             **Step 2** — add a pointer to that file in the same directory's `MEMORY.md`. \
             Each directory (private and team) has its own `MEMORY.md` index — each entry \
             should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. \
             They have no frontmatter. Never write memory content directly into a `MEMORY.md`.\n\n\
             - Both `MEMORY.md` indexes are loaded into your system prompt — lines after 200 \
             will be truncated, so keep them concise\n\
             - Organize memory semantically by topic, not chronologically\n\
             - Update or remove memories that turn out to be wrong or outdated\n\
             - Do not write duplicate memories. First check if there is an existing memory \
             you can update before writing a new one.",
            MEMORY_FRONTMATTER_EXAMPLE
        )
    };

    format!(
        "{opener}\n\n\
         If the user explicitly asks you to remember something, save it immediately as \
         whichever type fits best. If they ask you to forget something, find and remove \
         the relevant entry.\n\n\
         {types}\n{not_save}\n\
         - You MUST avoid saving sensitive data within shared team memories. For example, \
         never save API keys or user credentials.\n\n{how_to_save}",
        opener = opener(new_message_count, existing_memories),
        types = TYPES_SECTION,
        not_save = WHAT_NOT_TO_SAVE,
        how_to_save = how_to_save,
    )
}

// ── Tool permission helpers ────────────────────────────────────────────────

/// Allowed tools for the extraction agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutoMemToolDecision {
    Allow,
    Deny(String),
}

/// Tool names the extraction agent may use.
const ALLOWED_READONLY_TOOLS: &[&str] = &["Read", "Grep", "Glob"];
const WRITE_TOOLS: &[&str] = &["Edit", "Write"];

/// Determine whether the extraction agent may use a given tool.
///
/// - `Read`, `Grep`, `Glob`: always allowed.
/// - `Bash`: only if read-only (caller must validate).
/// - `Edit`, `Write`: only if `file_path` is within `memory_dir`.
/// - Everything else: denied.
pub fn check_auto_mem_tool_permission(
    tool_name: &str,
    file_path: Option<&str>,
    memory_dir: &Path,
) -> AutoMemToolDecision {
    // Read-only tools — always allowed
    if ALLOWED_READONLY_TOOLS.contains(&tool_name) {
        return AutoMemToolDecision::Allow;
    }

    // Write tools — only within memory dir
    if WRITE_TOOLS.contains(&tool_name) {
        if let Some(fp) = file_path {
            let fp_path = Path::new(fp);
            if fp_path.starts_with(memory_dir) {
                return AutoMemToolDecision::Allow;
            }
        }
        return AutoMemToolDecision::Deny(format!(
            "{} only allowed for paths within {}",
            tool_name,
            memory_dir.display()
        ));
    }

    AutoMemToolDecision::Deny(format!(
        "only Read, Grep, Glob, read-only Bash, and Edit/Write within {} are allowed",
        memory_dir.display()
    ))
}

// ── Visibility helpers ─────────────────────────────────────────────────────

/// Returns true if a message is model-visible (sent in API calls).
pub fn is_model_visible_message(msg: &Message) -> bool {
    matches!(msg, Message::User(_) | Message::Assistant(_))
}

/// Count model-visible messages since the given UUID (exclusive).
/// If `since_uuid` is `None`, counts all model-visible messages.
pub fn count_model_visible_messages_since(
    messages: &[Message],
    since_uuid: Option<Uuid>,
) -> usize {
    if since_uuid.is_none() {
        return messages.iter().filter(|m| is_model_visible_message(m)).count();
    }

    let target = since_uuid.unwrap();
    let mut found_start = false;
    let mut count = 0;

    for msg in messages {
        if !found_start {
            let msg_uuid = match msg {
                Message::User(u) => Some(u.uuid),
                Message::Assistant(a) => Some(a.uuid),
                Message::System(_) => None,
            };
            if msg_uuid == Some(target) {
                found_start = true;
            }
            continue;
        }
        if is_model_visible_message(msg) {
            count += 1;
        }
    }

    // If UUID was not found (e.g. removed by compaction), fall back to counting all.
    if !found_start {
        return messages.iter().filter(|m| is_model_visible_message(m)).count();
    }

    count
}

/// Check if any assistant message after `since_uuid` contains a Write/Edit
/// tool_use block targeting a path within the auto-memory directory.
pub fn has_memory_writes_since(
    messages: &[Message],
    since_uuid: Option<Uuid>,
    memory_dir: &Path,
) -> bool {
    let mut found_start = since_uuid.is_none();

    for msg in messages {
        if !found_start {
            let msg_uuid = match msg {
                Message::User(u) => Some(u.uuid),
                Message::Assistant(a) => Some(a.uuid),
                Message::System(_) => None,
            };
            if msg_uuid == since_uuid {
                found_start = true;
            }
            continue;
        }

        if let Message::Assistant(a) = msg {
            for block in &a.message.content {
                if let crate::types::content::ContentBlock::ToolUse { name, input, .. } = block {
                    if name == "Edit" || name == "Write" {
                        if let Some(fp) = input.get("file_path").and_then(|v| v.as_str()) {
                            if Path::new(fp).starts_with(memory_dir) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }

    false
}

/// Extract unique file paths written by Write/Edit tool calls in agent messages.
pub fn extract_written_paths(messages: &[Message]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();

    for msg in messages {
        if let Message::Assistant(a) = msg {
            for block in &a.message.content {
                if let crate::types::content::ContentBlock::ToolUse { name, input, .. } = block {
                    if name == "Edit" || name == "Write" {
                        if let Some(fp) = input.get("file_path").and_then(|v| v.as_str()) {
                            if seen.insert(fp.to_string()) {
                                paths.push(fp.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    paths
}

// ── Extraction state ───────────────────────────────────────────────────────

/// Closure-scoped mutable state for the memory extraction system.
pub struct ExtractMemoriesState {
    /// UUID of the last message processed — cursor for incremental extraction.
    pub last_memory_message_uuid: Mutex<Option<Uuid>>,
    /// True while extraction is running — prevents overlapping runs.
    in_progress: AtomicBool,
    /// Eligible turns since the last extraction run. Resets after each run.
    turns_since_last_extraction: AtomicUsize,
    /// Minimum turns between extractions (configurable, default 1).
    pub min_turns_between_extractions: AtomicUsize,
    /// Feature gate: whether auto-memory is enabled.
    pub enabled: AtomicBool,
}

impl ExtractMemoriesState {
    pub fn new() -> Self {
        Self {
            last_memory_message_uuid: Mutex::new(None),
            in_progress: AtomicBool::new(false),
            turns_since_last_extraction: AtomicUsize::new(0),
            min_turns_between_extractions: AtomicUsize::new(1),
            enabled: AtomicBool::new(true),
        }
    }

    /// Check if extraction is currently in progress.
    pub fn is_in_progress(&self) -> bool {
        self.in_progress.load(Ordering::Acquire)
    }

    /// Try to start an extraction. Returns false if one is already running.
    pub fn try_start(&self) -> bool {
        self.in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Mark extraction as complete.
    pub fn finish(&self) {
        self.in_progress.store(false, Ordering::Release);
    }

    /// Increment turn counter and check if enough turns have elapsed.
    /// Returns true if extraction should proceed. Resets the counter on true.
    pub fn check_and_reset_turn_gate(&self) -> bool {
        let count = self.turns_since_last_extraction.fetch_add(1, Ordering::Relaxed) + 1;
        let min = self.min_turns_between_extractions.load(Ordering::Relaxed);
        if count >= min {
            self.turns_since_last_extraction.store(0, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Advance the cursor to the last message in the slice.
    pub fn advance_cursor(&self, messages: &[Message]) {
        if let Some(last) = messages.last() {
            let uuid = match last {
                Message::User(u) => Some(u.uuid),
                Message::Assistant(a) => Some(a.uuid),
                Message::System(_) => None,
            };
            if let Some(u) = uuid {
                *self.last_memory_message_uuid.lock().unwrap() = Some(u);
            }
        }
    }

    /// Get the current cursor UUID.
    pub fn cursor(&self) -> Option<Uuid> {
        *self.last_memory_message_uuid.lock().unwrap()
    }

    /// Reset all state (for tests).
    pub fn reset(&self) {
        *self.last_memory_message_uuid.lock().unwrap() = None;
        self.in_progress.store(false, Ordering::Release);
        self.turns_since_last_extraction.store(0, Ordering::Relaxed);
    }
}

impl Default for ExtractMemoriesState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Memory directory helpers ───────────────────────────────────────────────

/// Default auto-memory directory, relative to the project config.
pub fn default_auto_mem_path() -> PathBuf {
    crate::config::paths::claude_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("memory")
}

/// Check whether a file path is within the auto-memory directory.
pub fn is_auto_mem_path(path: &Path, memory_dir: &Path) -> bool {
    path.starts_with(memory_dir)
}

/// The entrypoint index file name.
pub const ENTRYPOINT_NAME: &str = "MEMORY.md";

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::content::ContentBlock;
    use crate::types::message::{ApiMessage, AssistantMessage, Role, UserMessage};
    use crate::types::usage::Usage;
    use uuid::Uuid;

    fn make_user_msg() -> Message {
        Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            content: vec![],
            timestamp: chrono::Utc::now(),
        })
    }

    fn make_assistant_msg() -> Message {
        Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            message: ApiMessage {
                id: String::new(),
                model: String::new(),
                role: Role::Assistant,
                content: vec![],
                stop_reason: None,
                usage: Usage::default(),
            },
            request_id: None,
            timestamp: chrono::Utc::now(),
        })
    }

    #[test]
    fn test_count_model_visible_all() {
        let msgs = vec![make_user_msg(), make_assistant_msg(), make_user_msg()];
        assert_eq!(count_model_visible_messages_since(&msgs, None), 3);
    }

    #[test]
    fn test_count_model_visible_since_uuid() {
        let m1 = make_user_msg();
        let uuid1 = match &m1 {
            Message::User(u) => u.uuid,
            _ => unreachable!(),
        };
        let m2 = make_assistant_msg();
        let m3 = make_user_msg();
        let msgs = vec![m1, m2, m3];
        // Should count messages AFTER uuid1
        assert_eq!(count_model_visible_messages_since(&msgs, Some(uuid1)), 2);
    }

    #[test]
    fn test_count_model_visible_uuid_not_found() {
        let msgs = vec![make_user_msg(), make_assistant_msg()];
        // UUID not in messages — falls back to counting all
        assert_eq!(
            count_model_visible_messages_since(&msgs, Some(Uuid::new_v4())),
            2
        );
    }

    #[test]
    fn test_is_model_visible() {
        assert!(is_model_visible_message(&make_user_msg()));
        assert!(is_model_visible_message(&make_assistant_msg()));
    }

    #[test]
    fn test_tool_permission_read() {
        let mem_dir = PathBuf::from("/home/user/.claude/memory");
        assert_eq!(
            check_auto_mem_tool_permission("Read", None, &mem_dir),
            AutoMemToolDecision::Allow
        );
        assert_eq!(
            check_auto_mem_tool_permission("Grep", None, &mem_dir),
            AutoMemToolDecision::Allow
        );
        assert_eq!(
            check_auto_mem_tool_permission("Glob", None, &mem_dir),
            AutoMemToolDecision::Allow
        );
    }

    #[test]
    fn test_tool_permission_write_inside_memdir() {
        let mem_dir = PathBuf::from("/home/user/.claude/memory");
        assert_eq!(
            check_auto_mem_tool_permission(
                "Write",
                Some("/home/user/.claude/memory/user_role.md"),
                &mem_dir
            ),
            AutoMemToolDecision::Allow
        );
    }

    #[test]
    fn test_tool_permission_write_outside_memdir() {
        let mem_dir = PathBuf::from("/home/user/.claude/memory");
        let result = check_auto_mem_tool_permission(
            "Write",
            Some("/home/user/project/src/main.rs"),
            &mem_dir,
        );
        assert!(matches!(result, AutoMemToolDecision::Deny(_)));
    }

    #[test]
    fn test_tool_permission_denied() {
        let mem_dir = PathBuf::from("/home/user/.claude/memory");
        let result = check_auto_mem_tool_permission("Agent", None, &mem_dir);
        assert!(matches!(result, AutoMemToolDecision::Deny(_)));
    }

    #[test]
    fn test_extract_state_turn_gate() {
        let state = ExtractMemoriesState::new();
        // min_turns = 1 (default), so first call returns true
        assert!(state.check_and_reset_turn_gate());
        // After reset, first call again returns true
        assert!(state.check_and_reset_turn_gate());

        // Set min_turns to 3
        state.min_turns_between_extractions.store(3, Ordering::Relaxed);
        assert!(!state.check_and_reset_turn_gate()); // 1 < 3
        assert!(!state.check_and_reset_turn_gate()); // 2 < 3
        assert!(state.check_and_reset_turn_gate());  // 3 >= 3
    }

    #[test]
    fn test_extract_state_try_start() {
        let state = ExtractMemoriesState::new();
        assert!(state.try_start());
        assert!(!state.try_start()); // Already running
        state.finish();
        assert!(state.try_start()); // Can start again
    }

    #[test]
    fn test_build_extract_auto_only_prompt() {
        let prompt = build_extract_auto_only_prompt(10, "", false);
        assert!(prompt.contains("memory extraction subagent"));
        assert!(prompt.contains("~10 messages"));
        assert!(prompt.contains("MEMORY.md"));
    }

    #[test]
    fn test_build_extract_auto_only_prompt_skip_index() {
        let prompt = build_extract_auto_only_prompt(5, "", true);
        assert!(prompt.contains("memory extraction subagent"));
        assert!(!prompt.contains("Step 2"));
        assert!(!prompt.contains("add a pointer"));
    }

    #[test]
    fn test_build_extract_combined_prompt() {
        let prompt = build_extract_combined_prompt(8, "existing.md", false);
        assert!(prompt.contains("memory extraction subagent"));
        assert!(prompt.contains("existing.md"));
        assert!(prompt.contains("team memories"));
    }

    #[test]
    fn test_is_auto_mem_path() {
        let mem_dir = PathBuf::from("/home/user/.claude/memory");
        assert!(is_auto_mem_path(
            Path::new("/home/user/.claude/memory/test.md"),
            &mem_dir
        ));
        assert!(!is_auto_mem_path(
            Path::new("/home/user/project/test.md"),
            &mem_dir
        ));
    }
}
