use serde::{Deserialize, Serialize};

/// Where a plugin was loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginSource {
    /// Built-in plugin shipped with the CLI.
    Builtin,
    /// User-installed in `~/.claude/plugins/`.
    User,
    /// Project-level in `.claude/plugins/`.
    Project,
    /// Installed from a marketplace.
    Marketplace(String),
}

/// Parsed plugin manifest (`plugin.json` or `package.json` with `claude` key).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name.
    pub name: String,
    /// One-line description.
    #[serde(default)]
    pub description: String,
    /// Semantic version string.
    #[serde(default)]
    pub version: String,
    /// Tool names this plugin provides.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Command names this plugin provides.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Skill names this plugin provides.
    #[serde(default)]
    pub skills: Vec<String>,
}

/// A loaded plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    /// Parsed manifest.
    pub manifest: PluginManifest,
    /// Where this plugin was discovered.
    pub source: PluginSource,
    /// Directory containing the plugin.
    pub directory: std::path::PathBuf,
    /// Whether the plugin is currently enabled.
    pub enabled: bool,
    /// Whether this is a built-in plugin (ships with the CLI).
    pub is_builtin: bool,
    /// Repository identifier (for marketplace plugins).
    pub repository: Option<String>,
}
