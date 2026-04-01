use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Feature categories
// ---------------------------------------------------------------------------

/// Categorization of feature flags by stability / audience.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeatureCategory {
    /// Stable, shipped to all users.
    Production,
    /// Opt-in beta features visible to early adopters.
    Beta,
    /// Internal-only features for Anthropic engineers.
    Internal,
    /// Debugging / diagnostics aids, never shipped.
    Debug,
}

impl std::fmt::Display for FeatureCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

// ---------------------------------------------------------------------------
// Feature flags
// ---------------------------------------------------------------------------

/// Known feature flags in the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeatureFlag {
    // -- Production --
    CoordinatorMode,
    VoiceInput,
    ReactiveCompact,
    SnipCompact,
    AutoMode,
    McpSkills,
    ComputerUse,
    Plugins,
    RemoteSessions,
    AutoUpdater,
    CostTracking,

    // -- Beta --
    BridgeMode,
    BuddyMode,
    Proactive,

    // -- Internal --
    Kairos,
    KairosBrief,
    KairosChannels,
    KairosPushNotification,
    KairosGithubWebhooks,

    // -- Debug --
    AutoDream,
}

impl FeatureFlag {
    /// The environment variable name that controls this flag.
    pub fn env_var(&self) -> &'static str {
        match self {
            Self::CoordinatorMode => "CLAUDE_COORDINATOR_MODE",
            Self::VoiceInput => "CLAUDE_VOICE_INPUT",
            Self::AutoDream => "CLAUDE_AUTO_DREAM",
            Self::ReactiveCompact => "CLAUDE_REACTIVE_COMPACT",
            Self::SnipCompact => "CLAUDE_SNIP_COMPACT",
            Self::AutoMode => "CLAUDE_AUTO_MODE",
            Self::McpSkills => "CLAUDE_MCP_SKILLS",
            Self::ComputerUse => "CLAUDE_COMPUTER_USE",
            Self::Plugins => "CLAUDE_PLUGINS",
            Self::RemoteSessions => "CLAUDE_REMOTE_SESSIONS",
            Self::AutoUpdater => "CLAUDE_AUTO_UPDATER",
            Self::CostTracking => "CLAUDE_COST_TRACKING",
            Self::BridgeMode => "CLAUDE_BRIDGE_MODE",
            Self::BuddyMode => "CLAUDE_BUDDY_MODE",
            Self::Kairos => "CLAUDE_KAIROS",
            Self::KairosBrief => "CLAUDE_KAIROS_BRIEF",
            Self::KairosChannels => "CLAUDE_KAIROS_CHANNELS",
            Self::KairosPushNotification => "CLAUDE_KAIROS_PUSH_NOTIFICATION",
            Self::KairosGithubWebhooks => "CLAUDE_KAIROS_GITHUB_WEBHOOKS",
            Self::Proactive => "CLAUDE_PROACTIVE",
        }
    }

    /// The category this flag belongs to.
    pub fn category(&self) -> FeatureCategory {
        match self {
            Self::CoordinatorMode
            | Self::VoiceInput
            | Self::ReactiveCompact
            | Self::SnipCompact
            | Self::AutoMode
            | Self::McpSkills
            | Self::ComputerUse
            | Self::Plugins
            | Self::RemoteSessions
            | Self::AutoUpdater
            | Self::CostTracking => FeatureCategory::Production,

            Self::BridgeMode | Self::BuddyMode | Self::Proactive => FeatureCategory::Beta,

            Self::Kairos
            | Self::KairosBrief
            | Self::KairosChannels
            | Self::KairosPushNotification
            | Self::KairosGithubWebhooks => FeatureCategory::Internal,

            Self::AutoDream => FeatureCategory::Debug,
        }
    }

    /// All known feature flags.
    pub fn all() -> &'static [FeatureFlag] {
        &[
            Self::CoordinatorMode,
            Self::VoiceInput,
            Self::AutoDream,
            Self::ReactiveCompact,
            Self::SnipCompact,
            Self::AutoMode,
            Self::McpSkills,
            Self::ComputerUse,
            Self::Plugins,
            Self::RemoteSessions,
            Self::AutoUpdater,
            Self::CostTracking,
            Self::BridgeMode,
            Self::BuddyMode,
            Self::Kairos,
            Self::KairosBrief,
            Self::KairosChannels,
            Self::KairosPushNotification,
            Self::KairosGithubWebhooks,
            Self::Proactive,
        ]
    }

    /// All flags in a given category.
    pub fn in_category(category: FeatureCategory) -> Vec<FeatureFlag> {
        Self::all()
            .iter()
            .copied()
            .filter(|f| f.category() == category)
            .collect()
    }
}

impl std::fmt::Display for FeatureFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

// ---------------------------------------------------------------------------
// Convenience: standalone runtime check
// ---------------------------------------------------------------------------

/// Check whether a feature flag is enabled at runtime via environment variables.
///
/// This is a standalone convenience function that does not require a
/// `FeatureGates` instance. The flag is considered enabled unless its
/// environment variable is explicitly set to a falsy value (`0`, `false`,
/// `no`).
pub fn is_feature_enabled(flag: FeatureFlag) -> bool {
    match std::env::var(flag.env_var()) {
        Ok(val) => !matches!(val.to_lowercase().as_str(), "0" | "false" | "no"),
        Err(_) => true, // absent env var => enabled by default
    }
}

// ---------------------------------------------------------------------------
// Compile-time feature helpers
// ---------------------------------------------------------------------------

/// Check whether the `coordinator` Cargo feature is compiled in.
pub fn is_coordinator_compiled() -> bool {
    cfg!(feature = "coordinator")
}

/// Check whether the `bridge` Cargo feature is compiled in.
pub fn is_bridge_compiled() -> bool {
    cfg!(feature = "bridge")
}

/// Check whether the `voice` Cargo feature is compiled in.
pub fn is_voice_compiled() -> bool {
    cfg!(feature = "voice")
}

/// Check whether the `kairos` Cargo feature is compiled in.
pub fn is_kairos_compiled() -> bool {
    cfg!(feature = "kairos")
}

/// Check whether the `buddy` Cargo feature is compiled in.
pub fn is_buddy_compiled() -> bool {
    cfg!(feature = "buddy")
}

// ---------------------------------------------------------------------------
// FeatureGates
// ---------------------------------------------------------------------------

/// Feature gate configuration.
///
/// In the Rust version, all features are enabled by default.
/// The env-var mechanism is retained only as a kill-switch for debugging.
#[derive(Debug, Clone)]
pub struct FeatureGates {
    flags: HashMap<FeatureFlag, bool>,
}

impl FeatureGates {
    /// Create feature gates with all flags set to the given default.
    pub fn new(default: bool) -> Self {
        Self {
            flags: FeatureFlag::all().iter().map(|f| (*f, default)).collect(),
        }
    }

    /// Check if a feature flag is enabled.
    ///
    /// All flags are enabled by default. An env var set to "0" / "false" / "no"
    /// can disable a flag at runtime for debugging.
    pub fn is_enabled(&self, flag: FeatureFlag) -> bool {
        self.flags.get(&flag).copied().unwrap_or(true)
    }

    /// Set a feature flag.
    pub fn set(&mut self, flag: FeatureFlag, enabled: bool) {
        self.flags.insert(flag, enabled);
    }

    /// Get all enabled features.
    pub fn enabled_features(&self) -> Vec<FeatureFlag> {
        self.flags
            .iter()
            .filter_map(|(k, v)| if *v { Some(*k) } else { None })
            .collect()
    }

    /// Get all enabled features in a specific category.
    pub fn enabled_in_category(&self, category: FeatureCategory) -> Vec<FeatureFlag> {
        self.enabled_features()
            .into_iter()
            .filter(|f| f.category() == category)
            .collect()
    }
}

impl Default for FeatureGates {
    fn default() -> Self {
        // All features enabled by default -- no gating in the Rust version.
        Self::new(true)
    }
}

/// Load feature gates from environment variables and optional overrides.
///
/// All flags start enabled. Env vars can disable specific flags (set to
/// "0" / "false" / "no"), and explicit overrides take highest priority.
pub fn load_feature_gates(overrides: &HashMap<FeatureFlag, bool>) -> FeatureGates {
    let mut gates = FeatureGates::default();
    for flag in FeatureFlag::all() {
        if let Some(&enabled) = overrides.get(flag) {
            gates.set(*flag, enabled);
            continue;
        }
        // Env vars act as a kill-switch: only disable if explicitly set to a
        // falsy value. Absent env vars leave the flag enabled.
        if let Ok(val) = std::env::var(flag.env_var()) {
            if matches!(val.to_lowercase().as_str(), "0" | "false" | "no") {
                gates.set(*flag, false);
            }
        }
    }
    gates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_gates_enabled() {
        let gates = FeatureGates::default();
        for flag in FeatureFlag::all() {
            assert!(
                gates.is_enabled(*flag),
                "{flag:?} should be enabled by default"
            );
        }
    }

    #[test]
    fn test_overrides_can_disable() {
        let mut o = HashMap::new();
        o.insert(FeatureFlag::VoiceInput, false);
        let gates = load_feature_gates(&o);
        assert!(!gates.is_enabled(FeatureFlag::VoiceInput));
        // Other flags remain enabled.
        assert!(gates.is_enabled(FeatureFlag::AutoDream));
    }

    #[test]
    fn test_feature_categories() {
        assert_eq!(
            FeatureFlag::CoordinatorMode.category(),
            FeatureCategory::Production
        );
        assert_eq!(
            FeatureFlag::BridgeMode.category(),
            FeatureCategory::Beta
        );
        assert_eq!(
            FeatureFlag::Kairos.category(),
            FeatureCategory::Internal
        );
        assert_eq!(
            FeatureFlag::AutoDream.category(),
            FeatureCategory::Debug
        );
    }

    #[test]
    fn test_in_category() {
        let production = FeatureFlag::in_category(FeatureCategory::Production);
        assert!(production.contains(&FeatureFlag::CoordinatorMode));
        assert!(!production.contains(&FeatureFlag::Kairos));
    }

    #[test]
    fn test_enabled_in_category() {
        let mut o = HashMap::new();
        o.insert(FeatureFlag::VoiceInput, false);
        let gates = load_feature_gates(&o);
        let production = gates.enabled_in_category(FeatureCategory::Production);
        assert!(!production.contains(&FeatureFlag::VoiceInput));
        assert!(production.contains(&FeatureFlag::CoordinatorMode));
    }

    #[test]
    fn test_is_feature_enabled_default() {
        // By default (no env var), features are enabled
        // We test with a flag whose env var is unlikely to be set
        assert!(is_feature_enabled(FeatureFlag::AutoDream));
    }

    #[test]
    fn test_compile_time_checks() {
        // These just verify the functions exist and return booleans.
        let _ = is_coordinator_compiled();
        let _ = is_bridge_compiled();
        let _ = is_voice_compiled();
        let _ = is_kairos_compiled();
        let _ = is_buddy_compiled();
    }

    #[test]
    fn test_all_flags_have_env_var() {
        for flag in FeatureFlag::all() {
            let var = flag.env_var();
            assert!(var.starts_with("CLAUDE_"), "{flag:?} env var should start with CLAUDE_");
        }
    }

    #[test]
    fn test_all_flags_have_category() {
        for flag in FeatureFlag::all() {
            // Just ensure it doesn't panic
            let _ = flag.category();
        }
    }
}
