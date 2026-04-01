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
//! - Multi-session capacity management
//! - Transport abstraction (WebSocket v1, SSE/CCR v2)
//! - Inbound message and attachment handling
//! - Trusted device token management
//! - Git worktree-based session isolation

pub mod api;
pub mod capacity;
pub mod debug;
pub mod inbound;
pub mod jwt;
pub mod main_loop;
pub mod messaging;
pub mod poll_config;
pub mod repl_bridge;
pub mod session;
pub mod spawn;
pub mod transport;
pub mod trusted_device;
pub mod types;
pub mod work_secret;
