use omni_core::permissions::evaluator::*;
use omni_core::permissions::types::*;

#[test]
fn test_bypass_mode_always_allows() {
    let ctx = ToolPermissionContext {
        mode: PermissionMode::Bypass,
        ..Default::default()
    };
    let decision = evaluate_permission_sync("Bash", &serde_json::json!({}), &ctx, false);
    assert_eq!(decision.behavior, PermissionBehavior::Allow);
}

#[test]
fn test_default_mode_allows_readonly() {
    let ctx = ToolPermissionContext {
        mode: PermissionMode::Default,
        ..Default::default()
    };
    let decision = evaluate_permission_sync("Read", &serde_json::json!({}), &ctx, true);
    assert_eq!(decision.behavior, PermissionBehavior::Allow);
}

#[test]
fn test_default_mode_asks_for_write() {
    let ctx = ToolPermissionContext {
        mode: PermissionMode::Default,
        ..Default::default()
    };
    let decision = evaluate_permission_sync("Bash", &serde_json::json!({}), &ctx, false);
    assert_eq!(decision.behavior, PermissionBehavior::Ask);
}

#[test]
fn test_deny_rule_blocks() {
    let mut ctx = ToolPermissionContext::default();
    ctx.deny_rules.insert(
        "manual".into(),
        vec!["Bash".to_string()],
    );
    let decision = evaluate_permission_sync("Bash", &serde_json::json!({}), &ctx, false);
    assert_eq!(decision.behavior, PermissionBehavior::Deny);
}

#[test]
fn test_allow_rule_permits() {
    let mut ctx = ToolPermissionContext::default();
    ctx.mode = PermissionMode::Default;
    ctx.allow_rules.insert(
        "manual".into(),
        vec!["Bash".to_string()],
    );
    let decision = evaluate_permission_sync("Bash", &serde_json::json!({}), &ctx, false);
    assert_eq!(decision.behavior, PermissionBehavior::Allow);
}

#[test]
fn test_wildcard_deny_rule() {
    let mut ctx = ToolPermissionContext::default();
    ctx.deny_rules.insert(
        "manual".into(),
        vec!["*".to_string()],
    );
    let decision = evaluate_permission_sync("Read", &serde_json::json!({}), &ctx, true);
    assert_eq!(decision.behavior, PermissionBehavior::Deny);
}
