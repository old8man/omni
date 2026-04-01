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
//! - Team memory lives in a `team/` subdirectory of the auto-memory dir.

pub mod age;
pub mod core;
pub mod memory_types;
pub mod paths;
pub mod relevance;
pub mod scan;
pub mod team_mem;

pub use age::{memory_age, memory_age_days, memory_freshness_note, memory_freshness_text};
pub use core::{
    build_memory_lines, build_memory_prompt, ensure_memory_dir_exists, load_memory_prompt,
    truncate_entrypoint_content, EntrypointTruncation, ENTRYPOINT_NAME, MAX_ENTRYPOINT_BYTES,
    MAX_ENTRYPOINT_LINES,
};
pub use memory_types::{parse_memory_type, MemoryFrontmatter, MemoryType, MEMORY_TYPES};
pub use paths::{
    get_auto_mem_entrypoint, get_auto_mem_path, get_memory_base_dir, is_auto_mem_path,
    is_auto_memory_enabled,
};
pub use relevance::{find_relevant_memories, RelevantMemory};
pub use scan::{format_memory_manifest, scan_memory_files, MemoryHeader};
pub use team_mem::{
    build_combined_memory_prompt, get_team_mem_path, is_team_mem_path, is_team_memory_enabled,
};
