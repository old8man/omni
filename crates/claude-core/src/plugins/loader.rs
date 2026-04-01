//! Plugin filesystem discovery and loading.

use std::path::Path;

use super::types::{Plugin, PluginManifest, PluginSource};

/// Discover plugins from standard locations.
pub fn discover_plugins(project_root: Option<&Path>) -> Vec<Plugin> {
    let mut plugins = Vec::new();

    // User plugins: ~/.claude-omni/plugins/
    if let Some(home) = dirs::home_dir() {
        let user_dir = home.join(crate::config::paths::OMNI_DIR_NAME).join("plugins");
        plugins.extend(load_plugins_dir(&user_dir, PluginSource::User));
    }

    // Project plugins: <root>/.claude-omni/plugins/
    if let Some(root) = project_root {
        let project_dir = root.join(crate::config::paths::PROJECT_DIR_NAME).join("plugins");
        plugins.extend(load_plugins_dir(&project_dir, PluginSource::Project));
    }

    plugins
}

/// Load plugins from a directory. Each subdirectory is checked for a manifest.
fn load_plugins_dir(dir: &Path, source: PluginSource) -> Vec<Plugin> {
    let mut plugins = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return plugins,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(plugin) = load_plugin_from_dir(&path, source.clone()) {
                plugins.push(plugin);
            }
        }
    }

    plugins
}

/// Load a single plugin from its directory.
fn load_plugin_from_dir(dir: &Path, source: PluginSource) -> Option<Plugin> {
    // Try plugin.json first, then package.json
    let manifest_path = dir.join("plugin.json");
    let manifest = if manifest_path.exists() {
        parse_manifest(&manifest_path)?
    } else {
        let pkg_path = dir.join("package.json");
        parse_package_json(&pkg_path)?
    };

    Some(Plugin {
        manifest,
        source,
        directory: dir.to_path_buf(),
        enabled: true,
        is_builtin: false,
        repository: None,
    })
}

/// Parse a `plugin.json` manifest.
fn parse_manifest(path: &Path) -> Option<PluginManifest> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Parse a `package.json` with a `claude` key containing plugin metadata.
fn parse_package_json(path: &Path) -> Option<PluginManifest> {
    let content = std::fs::read_to_string(path).ok()?;
    let pkg: serde_json::Value = serde_json::from_str(&content).ok()?;
    let claude = pkg.get("claude")?;
    serde_json::from_value(claude.clone()).ok()
}
