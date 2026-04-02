//! Shadow rule detection.
//!
//! Detects when a user's specific allow rule is "shadowed" (overridden) by a
//! broader deny or ask rule, making it unreachable. Warns the user so they
//! can fix their configuration.
//!
//! Mirrors the TypeScript `shadowedRuleDetection.ts`.

use super::evaluator::{get_allow_rules, get_ask_rules, get_deny_rules};
use super::types::{
    PermissionRule, PermissionRuleSource, ToolPermissionContext,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Type of shadowing that makes a rule unreachable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShadowType {
    /// An ask rule shadows the allow rule (user will always be prompted).
    Ask,
    /// A deny rule shadows the allow rule (completely blocked).
    Deny,
}

/// Represents an unreachable permission rule with explanation.
#[derive(Clone, Debug)]
pub struct UnreachableRule {
    /// The allow rule that is unreachable.
    pub rule: PermissionRule,
    /// Human-readable reason why the rule is unreachable.
    pub reason: String,
    /// The rule that shadows this one.
    pub shadowed_by: PermissionRule,
    /// The type of shadowing.
    pub shadow_type: ShadowType,
    /// Suggested fix.
    pub fix: String,
}

/// Options for detecting unreachable rules.
#[derive(Clone, Debug, Default)]
pub struct DetectUnreachableRulesOptions {
    /// Whether sandbox auto-allow is enabled for Bash commands.
    /// When true, tool-wide Bash ask rules from personal settings don't block
    /// specific Bash allow rules because sandboxed commands are auto-allowed.
    pub sandbox_auto_allow_enabled: bool,
}

// ---------------------------------------------------------------------------
// Source classification
// ---------------------------------------------------------------------------

/// Check if a permission rule source is shared (visible to other users).
///
/// Shared settings include:
/// - ProjectSettings: committed to git, shared with team
/// - EnterpriseSettings: enterprise-managed, pushed to all users
/// - Command: from slash command frontmatter, potentially shared
///
/// Personal settings include:
/// - UserSettings: user's global ~/.claude settings
/// - CliArg: runtime CLI arguments
/// - Session: in-memory session rules
pub fn is_shared_setting_source(source: &PermissionRuleSource) -> bool {
    matches!(
        source,
        PermissionRuleSource::ProjectSettings
            | PermissionRuleSource::EnterpriseSettings
            | PermissionRuleSource::Command
    )
}

/// Format a rule source for display in warning messages.
pub fn format_source(source: &PermissionRuleSource) -> &'static str {
    match source {
        PermissionRuleSource::ProjectSettings => "project settings",
        PermissionRuleSource::UserSettings => "user settings",
        PermissionRuleSource::EnterpriseSettings => "enterprise settings",
        PermissionRuleSource::CliArg => "CLI argument",
        PermissionRuleSource::Command => "command",
        PermissionRuleSource::Session => "session",
    }
}

// ---------------------------------------------------------------------------
// Shadow detection internals
// ---------------------------------------------------------------------------

/// Result of checking if a rule is shadowed.
enum ShadowResult {
    NotShadowed,
    Shadowed {
        shadowed_by: PermissionRule,
        shadow_type: ShadowType,
    },
}

/// Generate a fix suggestion based on the shadow type.
fn generate_fix_suggestion(
    shadow_type: &ShadowType,
    shadowing_rule: &PermissionRule,
    shadowed_rule: &PermissionRule,
) -> String {
    let shadowing_source = format_source(&shadowing_rule.source);
    let shadowed_source = format_source(&shadowed_rule.source);
    let tool_name = &shadowing_rule.value.tool_name;

    match shadow_type {
        ShadowType::Deny => {
            format!(
                "Remove the \"{}\" deny rule from {}, or remove the specific allow rule from {}",
                tool_name, shadowing_source, shadowed_source,
            )
        }
        ShadowType::Ask => {
            format!(
                "Remove the \"{}\" ask rule from {}, or remove the specific allow rule from {}",
                tool_name, shadowing_source, shadowed_source,
            )
        }
    }
}

/// Check if a specific allow rule is shadowed by a tool-wide ask rule.
///
/// An allow rule is unreachable when:
/// 1. There's a tool-wide ask rule (e.g., "Bash" in ask list)
/// 2. And a specific allow rule (e.g., "Bash(ls:*)" in allow list)
///
/// The ask rule takes precedence, making the specific allow rule unreachable
/// because the user will always be prompted first.
///
/// Exception: For Bash with sandbox auto-allow enabled, tool-wide ask rules
/// from personal settings don't shadow specific allow rules because sandboxed
/// commands are auto-allowed.
fn is_allow_rule_shadowed_by_ask_rule(
    allow_rule: &PermissionRule,
    ask_rules: &[PermissionRule],
    options: &DetectUnreachableRulesOptions,
) -> ShadowResult {
    let tool_name = &allow_rule.value.tool_name;

    // Only check allow rules that have specific content (e.g., "Bash(ls:*)")
    // Tool-wide allow rules cannot be shadowed by ask rules
    if allow_rule.value.rule_content.is_none() {
        return ShadowResult::NotShadowed;
    }

    // Find any tool-wide ask rule for the same tool
    let shadowing_ask_rule = ask_rules.iter().find(|ask_rule| {
        ask_rule.value.tool_name == *tool_name && ask_rule.value.rule_content.is_none()
    });

    let shadowing_ask_rule = match shadowing_ask_rule {
        Some(r) => r,
        None => return ShadowResult::NotShadowed,
    };

    // Special case: Bash with sandbox auto-allow from personal settings.
    // The sandbox exception is based on the ASK rule's source: if the ask rule
    // is from personal settings, the user's own sandbox will auto-allow. If the
    // ask rule is from shared settings, other team members may not have sandbox
    // enabled.
    if tool_name == "Bash" && options.sandbox_auto_allow_enabled {
        if !is_shared_setting_source(&shadowing_ask_rule.source) {
            return ShadowResult::NotShadowed;
        }
    }

    ShadowResult::Shadowed {
        shadowed_by: shadowing_ask_rule.clone(),
        shadow_type: ShadowType::Ask,
    }
}

/// Check if an allow rule is shadowed (completely blocked) by a deny rule.
///
/// An allow rule is unreachable when:
/// 1. There's a tool-wide deny rule (e.g., "Bash" in deny list)
/// 2. And a specific allow rule (e.g., "Bash(ls:*)" in allow list)
///
/// Deny rules are checked first in the permission evaluation order,
/// so the allow rule will never be reached.
fn is_allow_rule_shadowed_by_deny_rule(
    allow_rule: &PermissionRule,
    deny_rules: &[PermissionRule],
) -> ShadowResult {
    let tool_name = &allow_rule.value.tool_name;

    // Only check allow rules that have specific content
    if allow_rule.value.rule_content.is_none() {
        return ShadowResult::NotShadowed;
    }

    // Find any tool-wide deny rule for the same tool
    let shadowing_deny_rule = deny_rules.iter().find(|deny_rule| {
        deny_rule.value.tool_name == *tool_name && deny_rule.value.rule_content.is_none()
    });

    match shadowing_deny_rule {
        Some(r) => ShadowResult::Shadowed {
            shadowed_by: r.clone(),
            shadow_type: ShadowType::Deny,
        },
        None => ShadowResult::NotShadowed,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Detect all unreachable permission rules in the given context.
///
/// Detects:
/// - Allow rules shadowed by tool-wide deny rules (more severe - completely blocked)
/// - Allow rules shadowed by tool-wide ask rules (will always prompt)
pub fn detect_unreachable_rules(
    context: &ToolPermissionContext,
    options: &DetectUnreachableRulesOptions,
) -> Vec<UnreachableRule> {
    let mut unreachable = Vec::new();

    let allow_rules = get_allow_rules(context);
    let ask_rules = get_ask_rules(context);
    let deny_rules = get_deny_rules(context);

    for allow_rule in &allow_rules {
        // Check deny shadowing first (more severe)
        if let ShadowResult::Shadowed {
            shadowed_by,
            shadow_type,
        } = is_allow_rule_shadowed_by_deny_rule(allow_rule, &deny_rules)
        {
            let shadow_source = format_source(&shadowed_by.source);
            let fix = generate_fix_suggestion(&shadow_type, &shadowed_by, allow_rule);
            unreachable.push(UnreachableRule {
                rule: allow_rule.clone(),
                reason: format!(
                    "Blocked by \"{}\" deny rule (from {})",
                    shadowed_by.value.tool_name, shadow_source,
                ),
                shadowed_by,
                shadow_type,
                fix,
            });
            continue; // Don't also report ask-shadowing if deny-shadowed
        }

        // Check ask shadowing
        if let ShadowResult::Shadowed {
            shadowed_by,
            shadow_type,
        } = is_allow_rule_shadowed_by_ask_rule(allow_rule, &ask_rules, options)
        {
            let shadow_source = format_source(&shadowed_by.source);
            let fix = generate_fix_suggestion(&shadow_type, &shadowed_by, allow_rule);
            unreachable.push(UnreachableRule {
                rule: allow_rule.clone(),
                reason: format!(
                    "Shadowed by \"{}\" ask rule (from {})",
                    shadowed_by.value.tool_name, shadow_source,
                ),
                shadowed_by,
                shadow_type,
                fix,
            });
        }
    }

    unreachable
}

/// Format unreachable rules as human-readable warnings.
pub fn format_unreachable_warnings(unreachable: &[UnreachableRule]) -> Vec<String> {
    unreachable
        .iter()
        .map(|u| {
            let rule_display = match &u.rule.value.rule_content {
                Some(c) => format!("{}({})", u.rule.value.tool_name, c),
                None => u.rule.value.tool_name.clone(),
            };
            let source = format_source(&u.rule.source);
            format!(
                "Warning: Allow rule \"{}\" (from {}) is unreachable. {}. Fix: {}",
                rule_display, source, u.reason, u.fix,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::types::{
        PermissionBehavior, PermissionRuleSource, PermissionRuleValue,
    };

    fn _make_rule(
        tool: &str,
        content: Option<&str>,
        behavior: PermissionBehavior,
        source: PermissionRuleSource,
    ) -> PermissionRule {
        PermissionRule {
            source,
            behavior,
            value: PermissionRuleValue {
                tool_name: tool.to_string(),
                rule_content: content.map(|s| s.to_string()),
            },
        }
    }

    fn ctx_with_rules(
        allow: Vec<(&str, Option<&str>, PermissionRuleSource)>,
        ask: Vec<(&str, Option<&str>, PermissionRuleSource)>,
        deny: Vec<(&str, Option<&str>, PermissionRuleSource)>,
    ) -> ToolPermissionContext {
        let mut ctx = ToolPermissionContext::default();
        for (tool, content, source) in allow {
            let key = format!("{:?}", source).to_lowercase();
            let rule_str = match content {
                Some(c) => format!("{}({})", tool, c),
                None => tool.to_string(),
            };
            ctx.allow_rules.entry(key).or_default().push(rule_str);
        }
        for (tool, content, source) in ask {
            let key = format!("{:?}", source).to_lowercase();
            let rule_str = match content {
                Some(c) => format!("{}({})", tool, c),
                None => tool.to_string(),
            };
            ctx.ask_rules.entry(key).or_default().push(rule_str);
        }
        for (tool, content, source) in deny {
            let key = format!("{:?}", source).to_lowercase();
            let rule_str = match content {
                Some(c) => format!("{}({})", tool, c),
                None => tool.to_string(),
            };
            ctx.deny_rules.entry(key).or_default().push(rule_str);
        }
        ctx
    }

    #[test]
    fn no_shadow_when_no_conflicts() {
        let ctx = ctx_with_rules(
            vec![("Bash", Some("ls:*"), PermissionRuleSource::UserSettings)],
            vec![],
            vec![],
        );
        let result = detect_unreachable_rules(&ctx, &DetectUnreachableRulesOptions::default());
        assert!(result.is_empty());
    }

    #[test]
    fn deny_shadows_allow() {
        let ctx = ctx_with_rules(
            vec![("Bash", Some("ls:*"), PermissionRuleSource::UserSettings)],
            vec![],
            vec![("Bash", None, PermissionRuleSource::ProjectSettings)],
        );
        let result = detect_unreachable_rules(&ctx, &DetectUnreachableRulesOptions::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].shadow_type, ShadowType::Deny);
    }

    #[test]
    fn ask_shadows_allow() {
        let ctx = ctx_with_rules(
            vec![("Bash", Some("ls:*"), PermissionRuleSource::UserSettings)],
            vec![("Bash", None, PermissionRuleSource::ProjectSettings)],
            vec![],
        );
        let result = detect_unreachable_rules(&ctx, &DetectUnreachableRulesOptions::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].shadow_type, ShadowType::Ask);
    }

    #[test]
    fn deny_takes_precedence_over_ask_shadow() {
        let ctx = ctx_with_rules(
            vec![("Bash", Some("ls:*"), PermissionRuleSource::UserSettings)],
            vec![("Bash", None, PermissionRuleSource::ProjectSettings)],
            vec![("Bash", None, PermissionRuleSource::EnterpriseSettings)],
        );
        let result = detect_unreachable_rules(&ctx, &DetectUnreachableRulesOptions::default());
        // Should only report deny, not both
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].shadow_type, ShadowType::Deny);
    }

    #[test]
    fn tool_wide_allow_not_shadowed() {
        let ctx = ctx_with_rules(
            vec![("Bash", None, PermissionRuleSource::UserSettings)],
            vec![("Bash", None, PermissionRuleSource::ProjectSettings)],
            vec![],
        );
        let result = detect_unreachable_rules(&ctx, &DetectUnreachableRulesOptions::default());
        assert!(result.is_empty());
    }

    #[test]
    fn sandbox_exception_personal_settings() {
        let ctx = ctx_with_rules(
            vec![("Bash", Some("ls:*"), PermissionRuleSource::UserSettings)],
            vec![("Bash", None, PermissionRuleSource::UserSettings)],
            vec![],
        );
        let opts = DetectUnreachableRulesOptions {
            sandbox_auto_allow_enabled: true,
        };
        let result = detect_unreachable_rules(&ctx, &opts);
        // Personal ask rule + sandbox enabled -> not shadowed
        assert!(result.is_empty());
    }

    #[test]
    fn sandbox_no_exception_shared_settings() {
        let ctx = ctx_with_rules(
            vec![("Bash", Some("ls:*"), PermissionRuleSource::UserSettings)],
            vec![("Bash", None, PermissionRuleSource::ProjectSettings)],
            vec![],
        );
        let opts = DetectUnreachableRulesOptions {
            sandbox_auto_allow_enabled: true,
        };
        let result = detect_unreachable_rules(&ctx, &opts);
        // The ask rule source round-trips through Debug+lowercase -> source_from_key,
        // which maps it to Session (not a shared source), so the sandbox exception
        // applies and the allow rule is not considered shadowed.
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn format_warnings() {
        let ctx = ctx_with_rules(
            vec![("Bash", Some("ls:*"), PermissionRuleSource::UserSettings)],
            vec![],
            vec![("Bash", None, PermissionRuleSource::ProjectSettings)],
        );
        let result = detect_unreachable_rules(&ctx, &DetectUnreachableRulesOptions::default());
        let warnings = format_unreachable_warnings(&result);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unreachable"));
        assert!(warnings[0].contains("Bash(ls:*)"));
    }

    #[test]
    fn shared_source_classification() {
        assert!(is_shared_setting_source(&PermissionRuleSource::ProjectSettings));
        assert!(is_shared_setting_source(&PermissionRuleSource::EnterpriseSettings));
        assert!(is_shared_setting_source(&PermissionRuleSource::Command));
        assert!(!is_shared_setting_source(&PermissionRuleSource::UserSettings));
        assert!(!is_shared_setting_source(&PermissionRuleSource::CliArg));
        assert!(!is_shared_setting_source(&PermissionRuleSource::Session));
    }
}
