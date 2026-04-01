//! Hook configuration: loading hooks from settings, snapshot management.
//!
//! Mirrors `hooksConfigSnapshot.ts`, `hooksSettings.ts`, and the settings-loading
//! portions of `hooks.ts`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tracing::warn;

use super::registry::HookRegistry;
use super::types::*;

/// Thread-safe hook configuration snapshot.
///
/// Captures the hooks configuration at startup and allows atomic updates.
/// Falls back to fresh settings reads if no snapshot has been captured.
pub struct HooksConfigSnapshot {
    config: Arc<RwLock<Option<HooksSettings>>>,
    /// Whether only managed hooks should run.
    managed_only: Arc<RwLock<bool>>,
    /// Whether all hooks are disabled.
    all_disabled: Arc<RwLock<bool>>,
}

impl HooksConfigSnapshot {
    /// Create a new, empty snapshot.
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(None)),
            managed_only: Arc::new(RwLock::new(false)),
            all_disabled: Arc::new(RwLock::new(false)),
        }
    }

    /// Capture a snapshot from the provided settings.
    ///
    /// Should be called once during application startup.
    pub fn capture(&self, settings: &HooksSettings) {
        let mut config = self.config.write().unwrap();
        *config = Some(settings.clone());
    }

    /// Update the snapshot with new settings.
    ///
    /// Called when hooks are modified through the settings UI.
    pub fn update(&self, settings: &HooksSettings) {
        let mut config = self.config.write().unwrap();
        *config = Some(settings.clone());
    }

    /// Get the current hooks configuration from the snapshot.
    pub fn get(&self) -> Option<HooksSettings> {
        self.config.read().unwrap().clone()
    }

    /// Get hooks for a specific event from the snapshot.
    pub fn get_for_event(&self, event: &str) -> Option<Vec<HookMatcher>> {
        let config = self.config.read().unwrap();
        config.as_ref()?.get(event).cloned()
    }

    /// Check whether only managed hooks should run.
    pub fn should_allow_managed_hooks_only(&self) -> bool {
        *self.managed_only.read().unwrap()
    }

    /// Set whether only managed hooks should run.
    pub fn set_managed_only(&self, value: bool) {
        *self.managed_only.write().unwrap() = value;
    }

    /// Check whether all hooks are disabled.
    pub fn should_disable_all_hooks(&self) -> bool {
        *self.all_disabled.read().unwrap()
    }

    /// Set whether all hooks are disabled.
    pub fn set_all_disabled(&self, value: bool) {
        *self.all_disabled.write().unwrap() = value;
    }

    /// Reset the snapshot (for testing).
    pub fn reset(&self) {
        *self.config.write().unwrap() = None;
        *self.managed_only.write().unwrap() = false;
        *self.all_disabled.write().unwrap() = false;
    }
}

impl Default for HooksConfigSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

/// Load hooks from a settings map (event name -> matchers) into `IndividualHookConfig` entries.
///
/// This is the primary way to convert JSON settings into structured hook configs.
pub fn load_hooks_from_settings(
    hooks_settings: &HooksSettings,
    source: HookSource,
) -> Vec<IndividualHookConfig> {
    let mut result = Vec::new();
    for (event_name, matchers) in hooks_settings {
        let event = match event_name.parse::<HookEvent>() {
            Ok(e) => e,
            Err(err) => {
                warn!("ignoring unknown hook event {event_name:?}: {err}");
                continue;
            }
        };
        for matcher in matchers {
            for hook_cmd in &matcher.hooks {
                result.push(IndividualHookConfig {
                    event,
                    config: hook_cmd.clone(),
                    matcher: matcher.matcher.clone(),
                    source: source.clone(),
                    plugin_name: None,
                });
            }
        }
    }
    result
}

/// Load hooks from a settings map directly into a registry.
pub fn load_hooks_into_registry(
    registry: &mut HookRegistry,
    hooks_settings: &HooksSettings,
    source: HookSource,
) {
    let hooks = load_hooks_from_settings(hooks_settings, source);
    registry.register_all(hooks);
}

/// Validate a hooks configuration, returning a list of warnings.
///
/// Checks for:
/// - Unknown event names
/// - Empty hooks arrays
/// - Invalid matcher patterns
/// - Missing required fields on hook commands
pub fn validate_hooks_config(config: &HooksSettings) -> Vec<String> {
    let mut warnings = Vec::new();

    for (event_name, matchers) in config {
        // Check event name
        if event_name.parse::<HookEvent>().is_err() {
            warnings.push(format!("unknown hook event: {event_name:?}"));
        }

        for (i, matcher) in matchers.iter().enumerate() {
            // Check for empty hooks arrays
            if matcher.hooks.is_empty() {
                warnings.push(format!(
                    "{event_name}[{i}]: empty hooks array (no commands to execute)"
                ));
            }

            // Validate matcher pattern if it looks like a regex
            if let Some(pattern) = &matcher.matcher {
                if pattern.contains('^')
                    || pattern.contains('$')
                    || pattern.contains('(')
                    || pattern.contains('\\')
                {
                    if regex::Regex::new(pattern).is_err() {
                        warnings.push(format!(
                            "{event_name}[{i}]: invalid regex in matcher: {pattern:?}"
                        ));
                    }
                }
            }

            // Validate individual hook commands
            for (j, hook) in matcher.hooks.iter().enumerate() {
                match hook {
                    HookCommand::Command { command, .. } => {
                        if command.trim().is_empty() {
                            warnings.push(format!(
                                "{event_name}[{i}].hooks[{j}]: empty command string"
                            ));
                        }
                    }
                    HookCommand::Http { url, .. } => {
                        if url.trim().is_empty() {
                            warnings.push(format!(
                                "{event_name}[{i}].hooks[{j}]: empty URL"
                            ));
                        }
                        if !url.starts_with("http://") && !url.starts_with("https://") {
                            warnings.push(format!(
                                "{event_name}[{i}].hooks[{j}]: URL should start with http:// or https://"
                            ));
                        }
                    }
                    HookCommand::Prompt { prompt, .. }
                    | HookCommand::Agent { prompt, .. } => {
                        if prompt.trim().is_empty() {
                            warnings.push(format!(
                                "{event_name}[{i}].hooks[{j}]: empty prompt"
                            ));
                        }
                    }
                }
            }
        }
    }

    warnings
}

/// Build a complete hook registry from multiple settings sources.
///
/// Sources are loaded in order: user settings, project settings, local settings.
/// Hooks from policy settings are always included.
pub fn build_hook_registry(
    user_hooks: Option<&HooksSettings>,
    project_hooks: Option<&HooksSettings>,
    local_hooks: Option<&HooksSettings>,
    policy_hooks: Option<&HooksSettings>,
    managed_only: bool,
    all_disabled: bool,
) -> HookRegistry {
    let mut registry = HookRegistry::new();
    registry.set_managed_only(managed_only);
    registry.set_all_disabled(all_disabled);

    if let Some(hooks) = policy_hooks {
        load_hooks_into_registry(&mut registry, hooks, HookSource::PolicySettings);
    }
    if let Some(hooks) = user_hooks {
        load_hooks_into_registry(&mut registry, hooks, HookSource::UserSettings);
    }
    if let Some(hooks) = project_hooks {
        load_hooks_into_registry(&mut registry, hooks, HookSource::ProjectSettings);
    }
    if let Some(hooks) = local_hooks {
        load_hooks_into_registry(&mut registry, hooks, HookSource::LocalSettings);
    }

    registry
}

/// Get all hooks organized by event and matcher for display purposes.
pub fn get_hooks_by_event_and_matcher(
    registry: &HookRegistry,
) -> HashMap<HookEvent, HashMap<String, Vec<&IndividualHookConfig>>> {
    let mut result: HashMap<HookEvent, HashMap<String, Vec<&IndividualHookConfig>>> =
        HashMap::new();

    for hook in registry.all_hooks() {
        let matcher_key = hook.matcher.clone().unwrap_or_default();
        result
            .entry(hook.event)
            .or_default()
            .entry(matcher_key)
            .or_default()
            .push(hook);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_settings() -> HooksSettings {
        let mut settings = HooksSettings::new();
        settings.insert(
            "PreToolUse".to_string(),
            vec![HookMatcher {
                matcher: Some("Write".to_string()),
                hooks: vec![HookCommand::Command {
                    command: "echo write guard".to_string(),
                    shell: "bash".to_string(),
                    condition: None,
                    timeout: None,
                }],
            }],
        );
        settings.insert(
            "Stop".to_string(),
            vec![HookMatcher {
                matcher: None,
                hooks: vec![HookCommand::Command {
                    command: "check_plan.sh".to_string(),
                    shell: "bash".to_string(),
                    condition: None,
                    timeout: Some(30),
                }],
            }],
        );
        settings
    }

    #[test]
    fn test_load_hooks_from_settings() {
        let settings = make_settings();
        let hooks = load_hooks_from_settings(&settings, HookSource::UserSettings);
        assert_eq!(hooks.len(), 2);
    }

    #[test]
    fn test_validate_hooks_config_valid() {
        let settings = make_settings();
        let warnings = validate_hooks_config(&settings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn test_validate_hooks_config_bad_event() {
        let mut settings = HooksSettings::new();
        settings.insert(
            "FakeEvent".to_string(),
            vec![HookMatcher {
                matcher: None,
                hooks: vec![],
            }],
        );
        let warnings = validate_hooks_config(&settings);
        assert!(warnings.iter().any(|w| w.contains("unknown hook event")));
    }

    #[test]
    fn test_validate_hooks_config_bad_regex() {
        let mut settings = HooksSettings::new();
        settings.insert(
            "PreToolUse".to_string(),
            vec![HookMatcher {
                matcher: Some("^[invalid".to_string()),
                hooks: vec![HookCommand::Command {
                    command: "echo test".to_string(),
                    shell: "bash".to_string(),
                    condition: None,
                    timeout: None,
                }],
            }],
        );
        let warnings = validate_hooks_config(&settings);
        assert!(warnings.iter().any(|w| w.contains("invalid regex")));
    }

    #[test]
    fn test_snapshot_capture_and_get() {
        let snap = HooksConfigSnapshot::new();
        assert!(snap.get().is_none());

        let settings = make_settings();
        snap.capture(&settings);
        let retrieved = snap.get().unwrap();
        assert!(retrieved.contains_key("PreToolUse"));
        assert!(retrieved.contains_key("Stop"));
    }

    #[test]
    fn test_snapshot_managed_only() {
        let snap = HooksConfigSnapshot::new();
        assert!(!snap.should_allow_managed_hooks_only());
        snap.set_managed_only(true);
        assert!(snap.should_allow_managed_hooks_only());
    }

    #[test]
    fn test_build_hook_registry() {
        let user = make_settings();
        let registry = build_hook_registry(
            Some(&user),
            None,
            None,
            None,
            false,
            false,
        );
        assert!(registry.has_hooks(HookEvent::PreToolUse));
        assert!(registry.has_hooks(HookEvent::Stop));
        assert!(!registry.has_hooks(HookEvent::SessionStart));
    }
}
