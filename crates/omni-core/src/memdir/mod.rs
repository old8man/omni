//! Directory-based memory system.
//!
//! Provides persistent, file-based memory storage organized into typed memory
//! files with YAML frontmatter. Mirrors the TypeScript `memdir/` module.
//!
//! # Architecture
//!
//! - **MEMORY.md** is the entrypoint index (max 200 lines / 25 KB).
//! - Individual memory files live alongside it in the memory directory.
//! - Each file has YAML frontmatter with `name`, `description`, and `type`.
//! - Four memory types: `user`, `feedback`, `project`, `reference`.
//! - Memory scopes: `global`, `project`, `session`, `team`.
//! - Team memory lives in a `team/` subdirectory of the auto-memory dir.
//! - Memory deduplication detects and merges duplicate memories.
//! - Memory prompt injection builds system prompt sections from stored memories.

pub mod age;
pub mod core;
pub mod memory_types;
pub mod paths;
pub mod relevance;
pub mod scan;
pub mod team_mem;

pub use age::{
    is_stale, memory_age, memory_age_days, memory_freshness_note, memory_freshness_text,
};
pub use core::{
    build_memory_lines, build_memory_prompt, build_searching_past_context_section,
    ensure_memory_dir_exists, load_memory_prompt, read_memory_file,
    truncate_entrypoint_content, EntrypointTruncation, ENTRYPOINT_NAME, MAX_ENTRYPOINT_BYTES,
    MAX_ENTRYPOINT_LINES,
};
pub use memory_types::{
    memory_drift_caveat, memory_frontmatter_example, parse_frontmatter, parse_memory_type,
    trusting_recall_section, types_section_combined, types_section_individual,
    what_not_to_save_section, when_to_access_section, MemoryFrontmatter, MemoryType,
    MEMORY_TYPES,
};
pub use paths::{
    compute_auto_mem_path, get_auto_mem_daily_log_path, get_auto_mem_entrypoint,
    get_auto_mem_path, get_memory_base_dir, is_auto_mem_path, is_auto_memory_enabled,
    sanitize_path_for_key,
};
pub use relevance::{find_relevant_memories, RelevantMemory};
pub use scan::{
    detect_duplicates, format_memory_manifest, merge_duplicate_memories, scan_memory_files,
    MemoryHeader,
};
pub use team_mem::{
    build_combined_memory_prompt, get_team_mem_entrypoint, get_team_mem_path, is_team_mem_file,
    is_team_mem_path, is_team_memory_enabled, sync_team_memories, TeamMemorySyncResult,
};
