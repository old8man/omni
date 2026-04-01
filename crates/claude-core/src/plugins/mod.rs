//! Plugin system: discovery, loading, and registry.
//!
//! Plugins extend Claude with additional tools, commands, and skills via
//! `plugin.json` or `package.json` manifests.

pub mod loader;
pub mod registry;
pub mod types;

pub use registry::PluginRegistry;
pub use types::Plugin;
