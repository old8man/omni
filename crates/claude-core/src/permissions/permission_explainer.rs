//! Permission explainer.
//!
//! Generates human-readable explanations of why a permission was granted or
//! denied, including risk assessment. The full TS original uses an LLM
//! side-query for rich explanations; this Rust implementation provides
//! rule-based explanations with optional async LLM enrichment.
//!
//! Mirrors the TypeScript `permissionExplainer.ts`.

use super::bash_classifier::{classify_command, CommandRisk};
use super::types::{
    PermissionBehavior, PermissionDecision, PermissionDecisionReason,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Risk level for a permission decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RiskLevel {
    /// Safe dev workflows: read-only, listing, status checks.
    Low,
    /// Recoverable changes: file writes, package installs, git commits.
    Medium,
    /// Dangerous or irreversible: rm -rf, sudo, network exfiltration.
    High,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLevel::Low => write!(f, "LOW"),
            RiskLevel::Medium => write!(f, "MEDIUM"),
            RiskLevel::High => write!(f, "HIGH"),
        }
    }
}

/// A human-readable explanation of a permission decision.
#[derive(Clone, Debug)]
pub struct PermissionExplanation {
    /// How risky is this operation.
    pub risk_level: RiskLevel,
    /// What the tool/command does (1-2 sentences).
    pub explanation: String,
    /// Why the AI is running this command.
    pub reasoning: String,
    /// What could go wrong.
    pub risk: String,
}

// ---------------------------------------------------------------------------
// Explanation generation
// ---------------------------------------------------------------------------

/// Generate a permission explanation for a tool invocation.
///
/// Uses rule-based heuristics to produce an explanation. For Bash/shell
/// commands, the bash classifier provides risk assessment. For other tools,
/// the explanation is based on the tool name and decision reason.
pub fn explain_permission(
    tool_name: &str,
    input: &serde_json::Value,
    decision: &PermissionDecision,
) -> PermissionExplanation {
    // For Bash commands, use the command-specific explainer.
    if tool_name == "Bash" || tool_name == "PowerShell" {
        if let Some(command) = input
            .as_object()
            .and_then(|o| o.get("command"))
            .and_then(|v| v.as_str())
        {
            return explain_shell_command(tool_name, command, decision);
        }
    }

    // For file operations, explain based on the path.
    if tool_name.contains("File") || tool_name.contains("Write") || tool_name.contains("Read") {
        return explain_file_operation(tool_name, input, decision);
    }

    // Generic explanation for other tools.
    explain_generic_tool(tool_name, input, decision)
}

/// Generate an explanation for a shell command.
fn explain_shell_command(
    tool_name: &str,
    command: &str,
    decision: &PermissionDecision,
) -> PermissionExplanation {
    let risk = classify_command(command);
    let risk_level = match risk {
        CommandRisk::ReadOnly => RiskLevel::Low,
        CommandRisk::Write => RiskLevel::Medium,
        CommandRisk::Destructive => RiskLevel::High,
    };

    let explanation = match risk {
        CommandRisk::ReadOnly => format!(
            "Runs '{}' which is a read-only command that inspects files or system state without making changes.",
            truncate_command(command),
        ),
        CommandRisk::Write => format!(
            "Runs '{}' which modifies files or system state. Changes are generally recoverable.",
            truncate_command(command),
        ),
        CommandRisk::Destructive => format!(
            "Runs '{}' which is a potentially destructive or irreversible operation.",
            truncate_command(command),
        ),
    };

    let reasoning = match &decision.reason {
        Some(PermissionDecisionReason::Rule { rule }) => {
            let rule_display = match &rule.value.rule_content {
                Some(c) => format!("{}({})", rule.value.tool_name, c),
                None => rule.value.tool_name.clone(),
            };
            format!("Matched permission rule '{}'", rule_display)
        }
        Some(PermissionDecisionReason::Classifier { reason, .. }) => {
            reason.clone()
        }
        Some(PermissionDecisionReason::Mode { mode }) => {
            format!("Permission mode is {}", mode)
        }
        Some(PermissionDecisionReason::SafetyCheck { reason, .. }) => {
            reason.clone()
        }
        _ => format!("The {} tool requires permission to run shell commands", tool_name),
    };

    let risk_description = match risk {
        CommandRisk::ReadOnly => "No risk: read-only operation".to_string(),
        CommandRisk::Write => describe_write_risk(command),
        CommandRisk::Destructive => describe_destructive_risk(command),
    };

    PermissionExplanation {
        risk_level,
        explanation,
        reasoning,
        risk: risk_description,
    }
}

/// Generate an explanation for a file operation.
fn explain_file_operation(
    tool_name: &str,
    input: &serde_json::Value,
    decision: &PermissionDecision,
) -> PermissionExplanation {
    let path = input
        .as_object()
        .and_then(|o| {
            o.get("file_path")
                .or_else(|| o.get("path"))
                .or_else(|| o.get("filePath"))
        })
        .and_then(|v| v.as_str())
        .unwrap_or("<unknown>");

    let is_read = tool_name.contains("Read") || tool_name == "Grep" || tool_name == "Glob";
    let risk_level = if is_read {
        RiskLevel::Low
    } else {
        RiskLevel::Medium
    };

    let explanation = if is_read {
        format!("Reads file '{}'", truncate_command(path))
    } else {
        format!("Writes to file '{}'", truncate_command(path))
    };

    let reasoning = explain_reason(decision);

    let risk = if is_read {
        "No risk: read-only file access".to_string()
    } else if path.starts_with("/etc") || path.starts_with("/usr") || path.starts_with("/sys") {
        "High risk: modifying system files".to_string()
    } else {
        "File contents will be modified".to_string()
    };

    PermissionExplanation {
        risk_level,
        explanation,
        reasoning,
        risk,
    }
}

/// Generate a generic explanation for any tool.
fn explain_generic_tool(
    tool_name: &str,
    _input: &serde_json::Value,
    decision: &PermissionDecision,
) -> PermissionExplanation {
    let risk_level = match decision.behavior {
        PermissionBehavior::Allow => RiskLevel::Low,
        PermissionBehavior::Ask => RiskLevel::Medium,
        PermissionBehavior::Deny => RiskLevel::High,
    };

    let explanation = format!("Invokes the '{}' tool", tool_name);
    let reasoning = explain_reason(decision);
    let risk = match decision.behavior {
        PermissionBehavior::Allow => "Allowed by current permissions".to_string(),
        PermissionBehavior::Ask => "Requires user confirmation".to_string(),
        PermissionBehavior::Deny => "Blocked by current permissions".to_string(),
    };

    PermissionExplanation {
        risk_level,
        explanation,
        reasoning,
        risk,
    }
}

// ---------------------------------------------------------------------------
// Decision explanation
// ---------------------------------------------------------------------------

/// Generate a human-readable explanation of why a permission decision was made.
///
/// Suitable for displaying in the permission prompt UI.
pub fn explain_decision(decision: &PermissionDecision) -> String {
    match (&decision.behavior, &decision.reason) {
        (PermissionBehavior::Allow, Some(PermissionDecisionReason::Rule { rule })) => {
            let rule_display = match &rule.value.rule_content {
                Some(c) => format!("{}({})", rule.value.tool_name, c),
                None => rule.value.tool_name.clone(),
            };
            format!("Allowed by rule '{}' from {:?}", rule_display, rule.source)
        }
        (PermissionBehavior::Allow, Some(PermissionDecisionReason::Mode { mode })) => {
            format!("Allowed by permission mode '{}'", mode)
        }
        (PermissionBehavior::Allow, Some(PermissionDecisionReason::Classifier { classifier, reason })) => {
            format!("Allowed by {} classifier: {}", classifier, reason)
        }
        (PermissionBehavior::Deny, Some(PermissionDecisionReason::Rule { rule })) => {
            let rule_display = match &rule.value.rule_content {
                Some(c) => format!("{}({})", rule.value.tool_name, c),
                None => rule.value.tool_name.clone(),
            };
            format!("Denied by rule '{}' from {:?}", rule_display, rule.source)
        }
        (PermissionBehavior::Deny, Some(PermissionDecisionReason::SafetyCheck { reason, .. })) => {
            format!("Denied by safety check: {}", reason)
        }
        (PermissionBehavior::Deny, Some(PermissionDecisionReason::Mode { mode })) => {
            format!("Denied by permission mode '{}'", mode)
        }
        (PermissionBehavior::Ask, Some(PermissionDecisionReason::Rule { rule })) => {
            let rule_display = match &rule.value.rule_content {
                Some(c) => format!("{}({})", rule.value.tool_name, c),
                None => rule.value.tool_name.clone(),
            };
            format!("Requires confirmation due to rule '{}' from {:?}", rule_display, rule.source)
        }
        (PermissionBehavior::Ask, Some(PermissionDecisionReason::Mode { mode })) => {
            format!("Requires confirmation in '{}' mode", mode)
        }
        (behavior, _) => {
            format!("{:?}: {}", behavior, decision.message.as_deref().unwrap_or("no details"))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn explain_reason(decision: &PermissionDecision) -> String {
    match &decision.reason {
        Some(PermissionDecisionReason::Rule { rule }) => {
            let rule_display = match &rule.value.rule_content {
                Some(c) => format!("{}({})", rule.value.tool_name, c),
                None => rule.value.tool_name.clone(),
            };
            format!("Matched rule '{}'", rule_display)
        }
        Some(PermissionDecisionReason::Classifier { reason, .. }) => reason.clone(),
        Some(PermissionDecisionReason::Mode { mode }) => {
            format!("Permission mode is {}", mode)
        }
        _ => decision
            .message
            .clone()
            .unwrap_or_else(|| "No additional context".to_string()),
    }
}

fn describe_write_risk(command: &str) -> String {
    let lower = command.to_lowercase();
    if lower.contains("install") {
        "May install packages that modify node_modules or system directories".to_string()
    } else if lower.starts_with("git ") {
        "May modify git history or working tree".to_string()
    } else if lower.starts_with("cp ") || lower.starts_with("mv ") {
        "May overwrite existing files".to_string()
    } else if lower.starts_with("mkdir") {
        "Creates new directories".to_string()
    } else {
        "May modify files or system state".to_string()
    }
}

fn describe_destructive_risk(command: &str) -> String {
    let lower = command.to_lowercase();
    if lower.starts_with("rm ") {
        "Permanently deletes files (may be irreversible)".to_string()
    } else if lower.starts_with("sudo ") {
        "Runs with elevated privileges (root access)".to_string()
    } else if lower.starts_with("curl ") || lower.starts_with("wget ") {
        "Network request: may download or upload data".to_string()
    } else if lower.contains("push") && lower.contains("force") {
        "Force push may overwrite remote git history".to_string()
    } else if lower.starts_with("dd ") {
        "Low-level disk operation (potentially destructive)".to_string()
    } else {
        "Potentially dangerous or irreversible operation".to_string()
    }
}

fn truncate_command(s: &str) -> String {
    if s.len() <= 80 {
        s.to_string()
    } else {
        format!("{}...", &s[..77])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::types::PermissionMode;
    use serde_json::json;

    #[test]
    fn explain_readonly_bash() {
        let decision = PermissionDecision::allow().with_reason(
            PermissionDecisionReason::Mode {
                mode: PermissionMode::Default,
            },
        );
        let explanation = explain_permission("Bash", &json!({"command": "ls -la"}), &decision);
        assert_eq!(explanation.risk_level, RiskLevel::Low);
        assert!(explanation.explanation.contains("read-only"));
    }

    #[test]
    fn explain_destructive_bash() {
        let decision = PermissionDecision::ask("requires confirmation");
        let explanation =
            explain_permission("Bash", &json!({"command": "rm -rf /tmp/foo"}), &decision);
        assert_eq!(explanation.risk_level, RiskLevel::High);
        assert!(explanation.explanation.contains("destructive"));
    }

    #[test]
    fn explain_write_bash() {
        let decision = PermissionDecision::ask("requires confirmation");
        let explanation =
            explain_permission("Bash", &json!({"command": "npm install"}), &decision);
        assert_eq!(explanation.risk_level, RiskLevel::Medium);
        assert!(explanation.risk.contains("install"));
    }

    #[test]
    fn explain_file_read() {
        let decision = PermissionDecision::allow();
        let explanation = explain_permission(
            "FileRead",
            &json!({"file_path": "/home/user/file.txt"}),
            &decision,
        );
        assert_eq!(explanation.risk_level, RiskLevel::Low);
        assert!(explanation.explanation.contains("Reads"));
    }

    #[test]
    fn explain_file_write() {
        let decision = PermissionDecision::ask("requires confirmation");
        let explanation = explain_permission(
            "FileWrite",
            &json!({"file_path": "/etc/passwd"}),
            &decision,
        );
        assert_eq!(explanation.risk_level, RiskLevel::Medium);
        assert!(explanation.risk.contains("system"));
    }

    #[test]
    fn explain_generic_tool() {
        let decision = PermissionDecision::allow();
        let explanation = explain_permission("CustomTool", &json!({}), &decision);
        assert_eq!(explanation.risk_level, RiskLevel::Low);
        assert!(explanation.explanation.contains("CustomTool"));
    }

    #[test]
    fn explain_decision_allow_rule() {
        use crate::permissions::types::{
            PermissionRule, PermissionRuleSource, PermissionRuleValue,
        };
        let rule = PermissionRule {
            source: PermissionRuleSource::UserSettings,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_name: "Bash".to_string(),
                rule_content: Some("git:*".to_string()),
            },
        };
        let decision = PermissionDecision::allow()
            .with_reason(PermissionDecisionReason::Rule { rule });
        let text = explain_decision(&decision);
        assert!(text.contains("Allowed by rule"));
        assert!(text.contains("git:*"));
    }

    #[test]
    fn explain_decision_deny_safety() {
        let decision = PermissionDecision::deny("blocked").with_reason(
            PermissionDecisionReason::SafetyCheck {
                reason: "Always-blocked pattern".to_string(),
                classifier_approvable: false,
            },
        );
        let text = explain_decision(&decision);
        assert!(text.contains("Denied by safety check"));
    }
}
