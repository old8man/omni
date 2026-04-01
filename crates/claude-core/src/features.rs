use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Known feature flags in the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeatureFlag {
    CoordinatorMode,
    VoiceInput,
    AutoDream,
    ReactiveCompact,
    SnipCompact,
    AutoMode,
    McpSkills,
    ComputerUse,
    Plugins,
    RemoteSessions,
    AutoUpdater,
    CostTracking,
    Kairos,
    KairosBrief,
    KairosChannels,
    KairosPushNotification,
    KairosGithubWebhooks,
    Proactive,
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
            Self::Kairos => "CLAUDE_KAIROS",
            Self::KairosBrief => "CLAUDE_KAIROS_BRIEF",
            Self::KairosChannels => "CLAUDE_KAIROS_CHANNELS",
            Self::KairosPushNotification => "CLAUDE_KAIROS_PUSH_NOTIFICATION",
            Self::KairosGithubWebhooks => "CLAUDE_KAIROS_GITHUB_WEBHOOKS",
            Self::Proactive => "CLAUDE_PROACTIVE",
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
            Self::Kairos,
            Self::KairosBrief,
            Self::KairosChannels,
            Self::KairosPushNotification,
            Self::KairosGithubWebhooks,
            Self::Proactive,
        ]
    }
}

impl std::fmt::Display for FeatureFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Feature gate configuration.
///
/// In the Rust version, all features are enabled by default — there is no
/// compile-time gating, no GrowthBook checks, and no entitlement enforcement.
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
}

impl Default for FeatureGates {
    fn default() -> Self {
        // All features enabled by default — no gating in the Rust version.
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
}
