//! Skills system: discovery, loading, and registry.
//!
//! Skills are markdown-based prompt templates that extend Claude's capabilities.
//! They can be bundled with the application, defined per-user, per-project, or
//! provided by plugins.

pub mod bundled;
pub mod loader;
pub mod registry;
pub mod types;

pub use registry::SkillRegistry;
pub use types::{Skill, SkillSource};
