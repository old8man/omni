//! IDE integration module for Claude Code.
//!
//! Provides detection, connection, and communication with IDE extensions
//! (VS Code, Cursor, JetBrains, etc.) via MCP-based WebSocket or SSE
//! transports. The IDE module handles:
//!
//! - IDE instance detection via lockfiles and environment variables
//! - MCP server configuration generation
//! - JSON-based RPC protocol for file operations and diagnostics

pub mod integration;
pub mod protocol;
pub mod types;
