//! Remote session management for Claude Code.
//!
//! Provides WebSocket-based real-time communication with CCR (Claude Code
//! Remote) sessions. The remote module handles:
//!
//! - WebSocket subscription to session event streams
//! - SDK message adaptation (converting wire format to display types)
//! - Permission request/response coordination
//! - Connection lifecycle with automatic reconnection

pub mod message_adapter;
pub mod session_manager;
pub mod types;
pub mod websocket;
