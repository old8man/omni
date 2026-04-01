//! Hook registry: stores and queries registered hooks by event.

use std::collections::HashMap;

use super::types::{HookEvent, HookSource, IndividualHookConfig};

/// Registry of hooks organized by event.
pub struct HookRegistry {
    hooks: HashMap<HookEvent, Vec<IndividualHookConfig>>,
    /// Whether only managed hooks are allowed (policy setting).
    managed_only: bool,
    /// Whether all hooks are disabled (policy setting).
    all_disabled: bool,
}

impl HookRegistry {
    /// Create a new, empty hook registry.
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
            managed_only: false,
            all_disabled: false,
        }
    }

    /// Register a hook for the given event.
    pub fn register(&mut self, hook: IndividualHookConfig) {
        self.hooks.entry(hook.event).or_default().push(hook);
    }

    /// Register multiple hooks at once.
    pub fn register_all(&mut self, hooks: Vec<IndividualHookConfig>) {
        for hook in hooks {
            self.register(hook);
        }
    }

    /// Get all hooks for a specific event, respecting managed-only and disabled policies.
    pub fn get_hooks(&self, event: HookEvent) -> Vec<&IndividualHookConfig> {
        if self.all_disabled {
            return Vec::new();
        }

        let hooks = match self.hooks.get(&event) {
            Some(v) => v,
            None => return Vec::new(),
        };

        if self.managed_only {
            hooks.iter().filter(|h| h.source.is_managed()).collect()
        } else {
            hooks.iter().collect()
        }
    }

    /// Get all hooks for a specific event without any policy filtering.
    /// Used internally by config snapshot and listing code.
    pub fn get_hooks_unfiltered(&self, event: HookEvent) -> &[IndividualHookConfig] {
        self.hooks.get(&event).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all registered hooks (respecting policies).
    pub fn all_hooks(&self) -> Vec<&IndividualHookConfig> {
        if self.all_disabled {
            return Vec::new();
        }

        let iter = self.hooks.values().flat_map(|v| v.iter());
        if self.managed_only {
            iter.filter(|h| h.source.is_managed()).collect()
        } else {
            iter.collect()
        }
    }

    /// Remove all hooks from a specific source.
    pub fn remove_by_source(&mut self, source: &HookSource) {
        for hooks in self.hooks.values_mut() {
            hooks.retain(|h| &h.source != source);
        }
    }

    /// Clear all hooks.
    pub fn clear(&mut self) {
        self.hooks.clear();
    }

    /// Check if any hooks are registered for the given event (respecting policies).
    pub fn has_hooks(&self, event: HookEvent) -> bool {
        if self.all_disabled {
            return false;
        }
        self.hooks.get(&event).is_some_and(|v| {
            if self.managed_only {
                v.iter().any(|h| h.source.is_managed())
            } else {
                !v.is_empty()
            }
        })
    }

    /// Set whether only managed hooks are allowed.
    pub fn set_managed_only(&mut self, managed_only: bool) {
        self.managed_only = managed_only;
    }

    /// Check whether only managed hooks are allowed.
    pub fn is_managed_only(&self) -> bool {
        self.managed_only
    }

    /// Set whether all hooks (including managed) are disabled.
    pub fn set_all_disabled(&mut self, disabled: bool) {
        self.all_disabled = disabled;
    }

    /// Check whether all hooks are disabled.
    pub fn is_all_disabled(&self) -> bool {
        self.all_disabled
    }

    /// Get the number of registered hooks (ignoring policies).
    pub fn len(&self) -> usize {
        self.hooks.values().map(|v| v.len()).sum()
    }

    /// Check if the registry is empty (ignoring policies).
    pub fn is_empty(&self) -> bool {
        self.hooks.values().all(|v| v.is_empty())
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::types::{HookCommand, HookEvent, HookSource, IndividualHookConfig};

    fn make_hook(event: HookEvent, source: HookSource) -> IndividualHookConfig {
        IndividualHookConfig {
            event,
            config: HookCommand::Command {
                command: "echo test".to_string(),
                shell: "bash".to_string(),
                condition: None,
                timeout: None,
            },
            matcher: None,
            source,
            plugin_name: None,
        }
    }

    #[test]
    fn test_register_and_get() {
        let mut reg = HookRegistry::new();
        reg.register(make_hook(HookEvent::PreToolUse, HookSource::UserSettings));
        assert_eq!(reg.get_hooks(HookEvent::PreToolUse).len(), 1);
        assert_eq!(reg.get_hooks(HookEvent::PostToolUse).len(), 0);
    }

    #[test]
    fn test_managed_only() {
        let mut reg = HookRegistry::new();
        reg.register(make_hook(HookEvent::PreToolUse, HookSource::UserSettings));
        reg.register(make_hook(
            HookEvent::PreToolUse,
            HookSource::PolicySettings,
        ));
        assert_eq!(reg.get_hooks(HookEvent::PreToolUse).len(), 2);

        reg.set_managed_only(true);
        assert_eq!(reg.get_hooks(HookEvent::PreToolUse).len(), 1);
        assert_eq!(
            reg.get_hooks(HookEvent::PreToolUse)[0].source,
            HookSource::PolicySettings
        );
    }

    #[test]
    fn test_all_disabled() {
        let mut reg = HookRegistry::new();
        reg.register(make_hook(
            HookEvent::PreToolUse,
            HookSource::PolicySettings,
        ));
        assert!(reg.has_hooks(HookEvent::PreToolUse));

        reg.set_all_disabled(true);
        assert!(!reg.has_hooks(HookEvent::PreToolUse));
        assert!(reg.get_hooks(HookEvent::PreToolUse).is_empty());
    }

    #[test]
    fn test_remove_by_source() {
        let mut reg = HookRegistry::new();
        reg.register(make_hook(HookEvent::PreToolUse, HookSource::UserSettings));
        reg.register(make_hook(
            HookEvent::PreToolUse,
            HookSource::ProjectSettings,
        ));
        assert_eq!(reg.get_hooks(HookEvent::PreToolUse).len(), 2);

        reg.remove_by_source(&HookSource::UserSettings);
        assert_eq!(reg.get_hooks(HookEvent::PreToolUse).len(), 1);
    }
}
