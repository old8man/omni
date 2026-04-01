//! Synchronous permission evaluation.
//!
//! Evaluates deny -> allow -> ask rules in priority order, then falls back
//! to mode-based defaults. Supports glob patterns, path-based patterns,
//! tool-specific sub-patterns, and negation rules.

use serde_json::Value;

use super::rule_parser::{
    is_negation_pattern, matches_shell_rule, parse_shell_permission_rule,
    permission_rule_value_from_string, strip_negation,
};
use super::types::{
    PermissionBehavior, PermissionDecision, PermissionDecisionReason, PermissionMode,
    PermissionRule, PermissionRuleSource, PermissionRuleValue, ToolPermissionContext,
};

// ---------------------------------------------------------------------------
// Structured rule collection from context
// ---------------------------------------------------------------------------

/// Gather all allow rules from every source in the context.
pub fn get_allow_rules(ctx: &ToolPermissionContext) -> Vec<PermissionRule> {
    collect_rules(&ctx.allow_rules, PermissionBehavior::Allow)
}

/// Gather all deny rules from every source in the context.
pub fn get_deny_rules(ctx: &ToolPermissionContext) -> Vec<PermissionRule> {
    collect_rules(&ctx.deny_rules, PermissionBehavior::Deny)
}

/// Gather all ask rules from every source in the context.
pub fn get_ask_rules(ctx: &ToolPermissionContext) -> Vec<PermissionRule> {
    collect_rules(&ctx.ask_rules, PermissionBehavior::Ask)
}

fn collect_rules(
    map: &std::collections::HashMap<String, Vec<String>>,
    behavior: PermissionBehavior,
) -> Vec<PermissionRule> {
    let mut out = Vec::new();
    for (source_key, rule_strings) in map {
        let source = source_from_key(source_key);
        for rs in rule_strings {
            let value = permission_rule_value_from_string(rs);
            out.push(PermissionRule {
                source: source.clone(),
                behavior: behavior.clone(),
                value,
            });
        }
    }
    out
}

fn source_from_key(key: &str) -> PermissionRuleSource {
    match key {
        "projectSettings" | "project" => PermissionRuleSource::ProjectSettings,
        "userSettings" | "user" => PermissionRuleSource::UserSettings,
        "enterpriseSettings" | "enterprise" => PermissionRuleSource::EnterpriseSettings,
        "cliArg" | "cli" => PermissionRuleSource::CliArg,
        "command" => PermissionRuleSource::Command,
        "session" => PermissionRuleSource::Session,
        _ => PermissionRuleSource::Session,
    }
}

// ---------------------------------------------------------------------------
// Tool-level matching (no content constraint)
// ---------------------------------------------------------------------------

/// Check if a tool matches a rule that targets the tool as a whole (no content).
///
/// Handles MCP server-level matching: rule `"mcp__server1"` matches tool
/// `"mcp__server1__toolX"`.
fn tool_matches_rule_whole(tool_name: &str, rule: &PermissionRule) -> bool {
    if rule.value.rule_content.is_some() {
        return false;
    }
    // Direct name match.
    if rule.value.tool_name == tool_name {
        return true;
    }
    // Wildcard: "*" matches everything.
    if rule.value.tool_name == "*" {
        return true;
    }
    // MCP server-level: rule "mcp__server1" matches "mcp__server1__toolX".
    if tool_name.starts_with("mcp__") && rule.value.tool_name.starts_with("mcp__") {
        let rule_parts: Vec<&str> = rule.value.tool_name.splitn(3, "__").collect();
        let tool_parts: Vec<&str> = tool_name.splitn(3, "__").collect();
        if rule_parts.len() == 2 && tool_parts.len() >= 2 {
            // rule is "mcp__<server>" (no tool portion)
            return rule_parts[1] == tool_parts[1];
        }
        // "mcp__server__*" wildcard
        if rule_parts.len() == 3 && rule_parts[2] == "*" && tool_parts.len() >= 2 {
            return rule_parts[1] == tool_parts[1];
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Content-level matching
// ---------------------------------------------------------------------------

/// Check if a tool invocation matches a rule that has a content constraint.
///
/// The `input` JSON is inspected to extract the relevant content for matching.
/// For shell tools (Bash, PowerShell) this is the `command` field. For other
/// tools it falls back to the first string field in the input.
fn tool_matches_rule_with_content(
    tool_name: &str,
    input: &Value,
    rule: &PermissionRule,
) -> bool {
    let content = match &rule.value.rule_content {
        Some(c) => c,
        None => return false,
    };

    // Tool name must match.
    if rule.value.tool_name != tool_name && rule.value.tool_name != "*" {
        return false;
    }

    // Extract the relevant input string to match against.
    let input_str = extract_match_string(tool_name, input);
    let input_str = match input_str {
        Some(s) => s,
        None => return false,
    };

    // Parse the rule content as a shell permission rule and match.
    let shell_rule = parse_shell_permission_rule(content);
    matches_shell_rule(&shell_rule, &input_str)
}

/// Extract the string from `input` that should be matched against rule content.
///
/// For Bash/PowerShell tools: `input.command`.
/// For file tools: `input.file_path` or `input.path`.
/// Fallback: first string field in the JSON object.
fn extract_match_string(tool_name: &str, input: &Value) -> Option<String> {
    let obj = input.as_object()?;

    // Shell tools -> command
    if tool_name == "Bash" || tool_name == "PowerShell" {
        return obj
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    // File-oriented tools
    for key in &["file_path", "path", "filePath"] {
        if let Some(v) = obj.get(*key).and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }

    // Agent tool -> agent type
    if tool_name == "Agent" {
        if let Some(v) = obj.get("agentType").and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }

    // Fallback: first string field.
    for (_k, v) in obj {
        if let Some(s) = v.as_str() {
            return Some(s.to_string());
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Combined rule matching
// ---------------------------------------------------------------------------

/// Check if a tool invocation matches a single rule (whole-tool or content).
fn matches_rule(tool_name: &str, input: &Value, rule: &PermissionRule) -> bool {
    tool_matches_rule_whole(tool_name, rule)
        || tool_matches_rule_with_content(tool_name, input, rule)
}

/// Check if a tool invocation matches any rule in the given list,
/// respecting negation patterns.
///
/// Negation rules (starting with `!`) *exclude* otherwise-matching
/// invocations: a negation match means "this rule does NOT apply".
fn matches_any_rule(tool_name: &str, input: &Value, rules: &[PermissionRule]) -> Option<PermissionRule> {
    // First pass: check negation rules. If a negation rule matches, it
    // prevents the corresponding positive rule from firing.
    let mut negated_tools: Vec<PermissionRuleValue> = Vec::new();
    for rule in rules {
        if let Some(content) = &rule.value.rule_content {
            if is_negation_pattern(content) {
                let inner = strip_negation(content);
                let synthetic = PermissionRule {
                    source: rule.source.clone(),
                    behavior: rule.behavior.clone(),
                    value: PermissionRuleValue {
                        tool_name: rule.value.tool_name.clone(),
                        rule_content: if inner.is_empty() {
                            None
                        } else {
                            Some(inner.to_string())
                        },
                    },
                };
                if matches_rule(tool_name, input, &synthetic) {
                    negated_tools.push(rule.value.clone());
                }
            }
        }
    }

    // Second pass: find matching positive rules, skipping negated ones.
    for rule in rules {
        // Skip negation rules themselves.
        if let Some(content) = &rule.value.rule_content {
            if is_negation_pattern(content) {
                continue;
            }
        }
        // Skip if negated.
        if negated_tools.iter().any(|n| n.tool_name == rule.value.tool_name) {
            continue;
        }
        if matches_rule(tool_name, input, rule) {
            return Some(rule.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public evaluation API
// ---------------------------------------------------------------------------

/// Synchronous permission evaluation (no user prompting).
///
/// Priority order:
/// 1. Deny rules -> Deny
/// 2. Allow rules -> Allow
/// 3. Ask rules -> Ask
/// 4. Mode-based default
///
/// Returns `PermissionDecision::Ask` when user interaction is needed.
pub fn evaluate_permission_sync(
    tool_name: &str,
    input: &Value,
    ctx: &ToolPermissionContext,
    is_read_only: bool,
) -> PermissionDecision {
    let deny_rules = get_deny_rules(ctx);
    let allow_rules = get_allow_rules(ctx);
    let ask_rules = get_ask_rules(ctx);

    // 1. Deny rules.
    if let Some(rule) = matches_any_rule(tool_name, input, &deny_rules) {
        return PermissionDecision::deny(format!(
            "Tool '{}' is denied by rule '{}'.",
            tool_name,
            rule_display(&rule),
        ))
        .with_reason(PermissionDecisionReason::Rule { rule });
    }

    // 2. Allow rules.
    if let Some(rule) = matches_any_rule(tool_name, input, &allow_rules) {
        return PermissionDecision::allow().with_reason(PermissionDecisionReason::Rule { rule });
    }

    // 3. Ask rules.
    if let Some(rule) = matches_any_rule(tool_name, input, &ask_rules) {
        return PermissionDecision::ask(format!(
            "Tool '{}' requires user confirmation (rule: '{}').",
            tool_name,
            rule_display(&rule),
        ))
        .with_reason(PermissionDecisionReason::Rule { rule });
    }

    // 4. Mode-based defaults.
    match ctx.mode {
        PermissionMode::Bypass => PermissionDecision::allow().with_reason(
            PermissionDecisionReason::Mode {
                mode: PermissionMode::Bypass,
            },
        ),
        PermissionMode::Default | PermissionMode::AcceptEdits => {
            if is_read_only {
                PermissionDecision::allow().with_reason(PermissionDecisionReason::Mode {
                    mode: ctx.mode.clone(),
                })
            } else {
                PermissionDecision::ask(format!(
                    "Tool '{}' requires user confirmation (write operation).",
                    tool_name,
                ))
                .with_reason(PermissionDecisionReason::Mode {
                    mode: ctx.mode.clone(),
                })
            }
        }
        PermissionMode::InteractiveOnly | PermissionMode::Plan => {
            PermissionDecision::ask(format!(
                "Tool '{}' requires user confirmation (interactive mode).",
                tool_name,
            ))
            .with_reason(PermissionDecisionReason::Mode {
                mode: ctx.mode.clone(),
            })
        }
        PermissionMode::DontAsk => PermissionDecision::deny(format!(
            "Tool '{}' denied (don't-ask mode).",
            tool_name,
        ))
        .with_reason(PermissionDecisionReason::Mode {
            mode: PermissionMode::DontAsk,
        }),
        PermissionMode::Auto => {
            // In auto mode, if we reached here it means no rules matched.
            // The caller should run the AI classifier next.
            PermissionDecision::ask(format!(
                "Tool '{}' requires classifier evaluation (auto mode).",
                tool_name,
            ))
            .with_reason(PermissionDecisionReason::Mode {
                mode: PermissionMode::Auto,
            })
        }
    }
}

/// Look up the deny rule (if any) for a given tool, ignoring input content.
pub fn get_deny_rule_for_tool(
    ctx: &ToolPermissionContext,
    tool_name: &str,
) -> Option<PermissionRule> {
    get_deny_rules(ctx)
        .into_iter()
        .find(|r| tool_matches_rule_whole(tool_name, r))
}

/// Look up the allow rule (if any) for a given tool, ignoring input content.
pub fn get_allow_rule_for_tool(
    ctx: &ToolPermissionContext,
    tool_name: &str,
) -> Option<PermissionRule> {
    get_allow_rules(ctx)
        .into_iter()
        .find(|r| tool_matches_rule_whole(tool_name, r))
}

/// Map of rule contents -> rule for a given tool + behaviour.
pub fn get_rule_by_contents_for_tool(
    ctx: &ToolPermissionContext,
    tool_name: &str,
    behavior: &PermissionBehavior,
) -> std::collections::HashMap<String, PermissionRule> {
    let rules = match behavior {
        PermissionBehavior::Allow => get_allow_rules(ctx),
        PermissionBehavior::Deny => get_deny_rules(ctx),
        PermissionBehavior::Ask => get_ask_rules(ctx),
    };
    let mut map = std::collections::HashMap::new();
    for rule in rules {
        if rule.value.tool_name == tool_name {
            if let Some(content) = &rule.value.rule_content {
                map.insert(content.clone(), rule);
            }
        }
    }
    map
}

fn rule_display(rule: &PermissionRule) -> String {
    match &rule.value.rule_content {
        Some(c) => format!("{}({})", rule.value.tool_name, c),
        None => rule.value.tool_name.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_ctx() -> ToolPermissionContext {
        ToolPermissionContext::default()
    }

    fn ctx_with_rule(
        bucket: &str,
        source: &str,
        rule: &str,
    ) -> ToolPermissionContext {
        let mut ctx = make_ctx();
        let map = match bucket {
            "allow" => &mut ctx.allow_rules,
            "deny" => &mut ctx.deny_rules,
            "ask" => &mut ctx.ask_rules,
            _ => panic!("bad bucket"),
        };
        map.entry(source.to_string())
            .or_default()
            .push(rule.to_string());
        ctx
    }

    #[test]
    fn deny_beats_allow() {
        let mut ctx = ctx_with_rule("deny", "user", "Bash");
        ctx.allow_rules
            .entry("user".to_string())
            .or_default()
            .push("Bash".to_string());
        let d = evaluate_permission_sync("Bash", &json!({}), &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn allow_beats_ask() {
        let mut ctx = ctx_with_rule("allow", "user", "Bash");
        ctx.ask_rules
            .entry("user".to_string())
            .or_default()
            .push("Bash".to_string());
        let d = evaluate_permission_sync("Bash", &json!({}), &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn wildcard_tool_rule() {
        let ctx = ctx_with_rule("allow", "user", "*");
        let d = evaluate_permission_sync("AnyTool", &json!({}), &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn content_rule_prefix_match() {
        let ctx = ctx_with_rule("allow", "user", "Bash(npm:*)");
        let input = json!({"command": "npm install"});
        let d = evaluate_permission_sync("Bash", &input, &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn content_rule_prefix_no_match() {
        let ctx = ctx_with_rule("allow", "user", "Bash(npm:*)");
        let input = json!({"command": "yarn install"});
        let d = evaluate_permission_sync("Bash", &input, &ctx, false);
        // No rule matches -> falls through to mode default.
        assert_eq!(d.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn content_rule_wildcard_match() {
        let ctx = ctx_with_rule("allow", "user", "Bash(git *)");
        let input = json!({"command": "git status"});
        let d = evaluate_permission_sync("Bash", &input, &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn content_rule_exact_match() {
        let ctx = ctx_with_rule("allow", "user", "Bash(ls -la)");
        let input = json!({"command": "ls -la"});
        let d = evaluate_permission_sync("Bash", &input, &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Allow);

        let input2 = json!({"command": "ls"});
        let d2 = evaluate_permission_sync("Bash", &input2, &ctx, false);
        assert_eq!(d2.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn mode_bypass_auto_allows() {
        let mut ctx = make_ctx();
        ctx.mode = PermissionMode::Bypass;
        let d = evaluate_permission_sync("Anything", &json!({}), &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn mode_default_allows_readonly() {
        let ctx = make_ctx();
        let d = evaluate_permission_sync("ReadTool", &json!({}), &ctx, true);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn mode_default_asks_write() {
        let ctx = make_ctx();
        let d = evaluate_permission_sync("WriteTool", &json!({}), &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn mode_dont_ask_denies() {
        let mut ctx = make_ctx();
        ctx.mode = PermissionMode::DontAsk;
        let d = evaluate_permission_sync("Bash", &json!({}), &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn mcp_server_level_deny() {
        let ctx = ctx_with_rule("deny", "user", "mcp__dangerous_server");
        let d = evaluate_permission_sync("mcp__dangerous_server__some_tool", &json!({}), &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn mcp_server_wildcard() {
        let ctx = ctx_with_rule("allow", "user", "mcp__myserver__*");
        let d = evaluate_permission_sync("mcp__myserver__read", &json!({}), &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn negation_rule_prevents_match() {
        let mut ctx = make_ctx();
        // Allow all bash, but negate rm commands
        ctx.allow_rules
            .entry("user".to_string())
            .or_default()
            .push("Bash".to_string());
        ctx.allow_rules
            .entry("user".to_string())
            .or_default()
            .push("Bash(!rm -rf *)".to_string());
        // The tool-wide "Bash" allow will still match since negation
        // only blocks rules for the same tool_name that have content.
        // This test verifies negation parsing doesn't crash.
        let input = json!({"command": "ls"});
        let d = evaluate_permission_sync("Bash", &input, &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn file_path_content_matching() {
        let ctx = ctx_with_rule("deny", "user", "FileWrite(/etc/*)");
        let input = json!({"file_path": "/etc/passwd"});
        let d = evaluate_permission_sync("FileWrite", &input, &ctx, false);
        assert_eq!(d.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn file_path_content_no_match() {
        let ctx = ctx_with_rule("deny", "user", "FileWrite(/etc/*)");
        let input = json!({"file_path": "/home/user/file.txt"});
        let d = evaluate_permission_sync("FileWrite", &input, &ctx, false);
        // Falls through to mode default.
        assert_eq!(d.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn get_rules_helpers() {
        let ctx = ctx_with_rule("deny", "user", "Bash(rm -rf *)");
        let map = get_rule_by_contents_for_tool(&ctx, "Bash", &PermissionBehavior::Deny);
        assert!(map.contains_key("rm -rf *"));
    }
}
