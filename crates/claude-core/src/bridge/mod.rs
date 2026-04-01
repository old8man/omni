//! Bridge module for Claude Code Remote Control.
//!
//! The bridge connects a local machine to the Anthropic backend, allowing
//! remote session management via the environments API. It handles:
//!
//! - Environment registration and deregistration
//! - Polling for work (sessions, healthchecks)
//! - Session lifecycle (create, acknowledge, heartbeat, archive)
//! - JWT token parsing and refresh scheduling
//! - Message routing and echo deduplication

pub mod api;
pub mod jwt;
pub mod main_loop;
pub mod messaging;
pub mod session;
pub mod types;
