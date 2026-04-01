//! Plugin registry.

use std::collections::HashMap;

use super::types::Plugin;

/// Registry of loaded plugins.
#[derive(Debug, Clone, Default)]
pub struct PluginRegistry {
    plugins: HashMap<String, Plugin>,
    ordered: Vec<String>,
}

impl PluginRegistry {
    /// Build from a list of plugins.
    pub fn from_plugins(plugins: Vec<Plugin>) -> Self {
        let mut reg = Self::default();
        for plugin in plugins {
            reg.register(plugin);
        }
        reg
    }

    /// Register a plugin. Duplicates are silently ignored.
    pub fn register(&mut self, plugin: Plugin) {
        let name = plugin.manifest.name.clone();
        if self.plugins.contains_key(&name) {
            return;
        }
        self.ordered.push(name.clone());
        self.plugins.insert(name, plugin);
    }

    /// Look up by name.
    pub fn find_by_name(&self, name: &str) -> Option<&Plugin> {
        self.plugins.get(name)
    }

    /// Return only enabled plugins.
    pub fn enabled(&self) -> Vec<&Plugin> {
        self.list_all().into_iter().filter(|p| p.enabled).collect()
    }

    /// List all plugins in registration order.
    pub fn list_all(&self) -> Vec<&Plugin> {
        self.ordered
            .iter()
            .filter_map(|name| self.plugins.get(name))
            .collect()
    }

    /// Number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}
