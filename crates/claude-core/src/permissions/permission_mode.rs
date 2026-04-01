//! Permission mode progression.
//!
//! Logic for cycling through permission modes with Shift+Tab and for
//! transitioning between modes with appropriate safety checks.
//!
//! Mirrors the TypeScript `getNextPermissionMode.ts` and the
//! `transitionPermissionMode` logic from `permissionSetup.ts`.

use super::dangerous_patterns::strip_dangerous_bash_rules;
use super::types::{PermissionMode, ToolPermissionContext};

// ---------------------------------------------------------------------------
// Mode progression context
// ---------------------------------------------------------------------------

/// Additional context for mode progression decisions.
#[derive(Clone, Debug, Default)]
pub struct ModeProgressionContext {
    /// Whether bypass-permissions mode is available (e.g. user is authorized).
    pub is_bypass_permissions_available: bool,
    /// Whether auto-mode is available (gate checks passed).
    pub is_auto_mode_available: bool,
    /// Whether auto-mode gate is currently enabled.
    pub is_auto_mode_gate_enabled: bool,
}

impl ModeProgressionContext {
    fn can_cycle_to_auto(&self) -> bool {
        self.is_auto_mode_available && self.is_auto_mode_gate_enabled
    }
}

// ---------------------------------------------------------------------------
// Mode cycling
// ---------------------------------------------------------------------------

/// Determines the next permission mode when cycling through modes with Shift+Tab.
///
/// The cycle order is:
///   Default -> AcceptEdits -> Plan -> [Bypass] -> [Auto] -> Default
///
/// Bypass and Auto are only included if they are available.
pub fn get_next_permission_mode(
    current_mode: &PermissionMode,
    ctx: &ModeProgressionContext,
) -> PermissionMode {
    match current_mode {
        PermissionMode::Default => PermissionMode::AcceptEdits,

        PermissionMode::AcceptEdits => PermissionMode::Plan,

        PermissionMode::Plan => {
            if ctx.is_bypass_permissions_available {
                PermissionMode::Bypass
            } else if ctx.can_cycle_to_auto() {
                PermissionMode::Auto
            } else {
                PermissionMode::Default
            }
        }

        PermissionMode::Bypass => {
            if ctx.can_cycle_to_auto() {
                PermissionMode::Auto
            } else {
                PermissionMode::Default
            }
        }

        PermissionMode::DontAsk => {
            // Not exposed in UI cycle, but return default if reached.
            PermissionMode::Default
        }

        // Auto and any future modes fall back to default.
        PermissionMode::Auto | PermissionMode::InteractiveOnly => PermissionMode::Default,
    }
}

/// Result of a mode cycle operation.
#[derive(Clone, Debug)]
pub struct CycleResult {
    /// The new permission mode.
    pub next_mode: PermissionMode,
    /// The (possibly modified) permission context.
    pub context: ToolPermissionContext,
    /// Any rules that were stripped during the transition (e.g. dangerous
    /// patterns removed when entering auto mode).
    pub stripped_rules: Vec<String>,
}

/// Compute the next permission mode and prepare the context for it.
///
/// Handles any context cleanup needed for the target mode. When entering
/// auto mode, dangerous allow rules that could bypass the classifier are
/// stripped.
pub fn cycle_permission_mode(
    current_ctx: &ToolPermissionContext,
    progression: &ModeProgressionContext,
) -> CycleResult {
    let next_mode = get_next_permission_mode(&current_ctx.mode, progression);
    let (context, stripped_rules) =
        transition_permission_mode(&current_ctx.mode, &next_mode, current_ctx);

    CycleResult {
        next_mode,
        context,
        stripped_rules,
    }
}

/// Transition from one permission mode to another, applying any necessary
/// context transformations.
///
/// When transitioning TO auto mode, dangerous bash/powershell allow rules
/// are stripped so the classifier evaluates each invocation independently.
pub fn transition_permission_mode(
    _from: &PermissionMode,
    to: &PermissionMode,
    ctx: &ToolPermissionContext,
) -> (ToolPermissionContext, Vec<String>) {
    let mut new_ctx = ctx.clone();
    new_ctx.mode = to.clone();

    let mut all_stripped = Vec::new();

    // When entering auto mode, strip dangerous allow rules.
    if *to == PermissionMode::Auto {
        let mut new_allow_rules = std::collections::HashMap::new();
        for (source, rules) in &ctx.allow_rules {
            let (kept, stripped) = strip_dangerous_bash_rules(rules);
            for s in &stripped {
                all_stripped.push(s.rule.clone());
            }
            if !kept.is_empty() {
                new_allow_rules.insert(source.clone(), kept);
            }
        }
        new_ctx.allow_rules = new_allow_rules;
    }

    (new_ctx, all_stripped)
}

// ---------------------------------------------------------------------------
// Mode properties
// ---------------------------------------------------------------------------

/// Whether the given mode allows read-only operations without prompting.
pub fn mode_allows_reads(mode: &PermissionMode) -> bool {
    matches!(
        mode,
        PermissionMode::Default
            | PermissionMode::AcceptEdits
            | PermissionMode::Bypass
            | PermissionMode::Auto
    )
}

/// Whether the given mode allows write operations without prompting.
pub fn mode_allows_writes(mode: &PermissionMode) -> bool {
    matches!(
        mode,
        PermissionMode::Bypass | PermissionMode::Auto | PermissionMode::AcceptEdits
    )
}

/// Whether the mode requires user confirmation for all operations.
pub fn mode_requires_confirmation(mode: &PermissionMode) -> bool {
    matches!(
        mode,
        PermissionMode::InteractiveOnly | PermissionMode::Plan
    )
}

/// Whether the mode suppresses all prompts (headless).
pub fn mode_suppresses_prompts(mode: &PermissionMode) -> bool {
    matches!(mode, PermissionMode::DontAsk)
}

/// Get a human-readable description of what the mode does.
pub fn describe_mode(mode: &PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => {
            "Normal mode: read-only operations auto-allowed, writes require confirmation"
        }
        PermissionMode::Bypass => {
            "Bypass mode: all operations auto-allowed without confirmation"
        }
        PermissionMode::InteractiveOnly => {
            "Interactive mode: every tool call requires explicit approval"
        }
        PermissionMode::Auto => {
            "Auto mode: AI classifier decides whether to allow or prompt for each operation"
        }
        PermissionMode::AcceptEdits => {
            "Accept Edits mode: file edits in working directory auto-allowed, other writes prompt"
        }
        PermissionMode::DontAsk => {
            "Don't Ask mode: never prompt, convert asks to denials (headless/CI)"
        }
        PermissionMode::Plan => {
            "Plan mode: all operations require confirmation, can enter auto within plan"
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_progression() -> ModeProgressionContext {
        ModeProgressionContext::default()
    }

    fn full_progression() -> ModeProgressionContext {
        ModeProgressionContext {
            is_bypass_permissions_available: true,
            is_auto_mode_available: true,
            is_auto_mode_gate_enabled: true,
        }
    }

    #[test]
    fn basic_cycle_default_to_accept_edits() {
        let next = get_next_permission_mode(&PermissionMode::Default, &default_progression());
        assert_eq!(next, PermissionMode::AcceptEdits);
    }

    #[test]
    fn cycle_accept_edits_to_plan() {
        let next = get_next_permission_mode(&PermissionMode::AcceptEdits, &default_progression());
        assert_eq!(next, PermissionMode::Plan);
    }

    #[test]
    fn cycle_plan_wraps_to_default() {
        let next = get_next_permission_mode(&PermissionMode::Plan, &default_progression());
        assert_eq!(next, PermissionMode::Default);
    }

    #[test]
    fn cycle_plan_to_bypass_when_available() {
        let ctx = ModeProgressionContext {
            is_bypass_permissions_available: true,
            ..Default::default()
        };
        let next = get_next_permission_mode(&PermissionMode::Plan, &ctx);
        assert_eq!(next, PermissionMode::Bypass);
    }

    #[test]
    fn cycle_bypass_to_auto_when_available() {
        let next = get_next_permission_mode(&PermissionMode::Bypass, &full_progression());
        assert_eq!(next, PermissionMode::Auto);
    }

    #[test]
    fn cycle_auto_to_default() {
        let next = get_next_permission_mode(&PermissionMode::Auto, &full_progression());
        assert_eq!(next, PermissionMode::Default);
    }

    #[test]
    fn full_cycle() {
        let ctx = full_progression();
        let mut mode = PermissionMode::Default;
        let expected = vec![
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::Bypass,
            PermissionMode::Auto,
            PermissionMode::Default,
        ];
        for expected_mode in expected {
            mode = get_next_permission_mode(&mode, &ctx);
            assert_eq!(mode, expected_mode);
        }
    }

    #[test]
    fn dont_ask_goes_to_default() {
        let next = get_next_permission_mode(&PermissionMode::DontAsk, &full_progression());
        assert_eq!(next, PermissionMode::Default);
    }

    #[test]
    fn transition_to_auto_strips_dangerous_rules() {
        let mut ctx = ToolPermissionContext::default();
        ctx.allow_rules
            .entry("user".to_string())
            .or_default()
            .extend(vec![
                "Bash(python:*)".to_string(),
                "Bash(ls -la)".to_string(),
                "Bash(node *)".to_string(),
            ]);

        let (new_ctx, stripped) =
            transition_permission_mode(&PermissionMode::Default, &PermissionMode::Auto, &ctx);

        // Dangerous rules should be stripped
        let user_rules = new_ctx.allow_rules.get("user").unwrap();
        assert_eq!(user_rules.len(), 1);
        assert!(user_rules.contains(&"Bash(ls -la)".to_string()));
        assert_eq!(stripped.len(), 2);
    }

    #[test]
    fn transition_to_non_auto_preserves_rules() {
        let mut ctx = ToolPermissionContext::default();
        ctx.allow_rules
            .entry("user".to_string())
            .or_default()
            .push("Bash(python:*)".to_string());

        let (new_ctx, stripped) = transition_permission_mode(
            &PermissionMode::Default,
            &PermissionMode::AcceptEdits,
            &ctx,
        );

        let user_rules = new_ctx.allow_rules.get("user").unwrap();
        assert_eq!(user_rules.len(), 1);
        assert!(stripped.is_empty());
    }

    #[test]
    fn mode_properties() {
        assert!(mode_allows_reads(&PermissionMode::Default));
        assert!(!mode_allows_writes(&PermissionMode::Default));
        assert!(mode_allows_writes(&PermissionMode::Bypass));
        assert!(mode_requires_confirmation(&PermissionMode::InteractiveOnly));
        assert!(mode_suppresses_prompts(&PermissionMode::DontAsk));
    }

    #[test]
    fn cycle_permission_mode_integration() {
        let mut ctx = ToolPermissionContext::default();
        ctx.mode = PermissionMode::Plan;
        ctx.allow_rules
            .entry("user".to_string())
            .or_default()
            .push("Bash(eval:*)".to_string());

        let progression = ModeProgressionContext {
            is_bypass_permissions_available: false,
            is_auto_mode_available: true,
            is_auto_mode_gate_enabled: true,
        };

        let result = cycle_permission_mode(&ctx, &progression);
        assert_eq!(result.next_mode, PermissionMode::Auto);
        // Dangerous rule should be stripped
        assert_eq!(result.stripped_rules.len(), 1);
        assert!(result.stripped_rules[0].contains("eval"));
    }

    #[test]
    fn describe_mode_returns_description() {
        let desc = describe_mode(&PermissionMode::Default);
        assert!(desc.contains("read-only"));

        let desc = describe_mode(&PermissionMode::Auto);
        assert!(desc.contains("classifier"));
    }
}
