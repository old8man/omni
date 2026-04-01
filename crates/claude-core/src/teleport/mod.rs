//! Teleport -- Session Migration System
//!
//! Teleport enables migrating Claude sessions between environments (local <-> remote).
//! It bundles the git repo state, uploads it, and creates a remote session that can
//! continue where the local session left off.

pub mod api;
pub mod environments;
pub mod git_bundle;
