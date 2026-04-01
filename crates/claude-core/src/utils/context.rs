//! Context window calculation utilities — model-aware, 1M detection, env overrides.

use crate::utils::model::{
    get_model_capabilities, has_1m_context, is_1m_context_disabled,
    model_supports_1m,
};
use std::env;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default context window for all models (200k tokens).
pub const MODEL_CONTEXT_WINDOW_DEFAULT: u64 = 200_000;

/// Max output tokens for compact operations.
pub const COMPACT_MAX_OUTPUT_TOKENS: u64 = 20_000;

/// Capped default for slot-reservation optimisation.
pub const CAPPED_DEFAULT_MAX_TOKENS: u64 = 8_000;

/// Escalated max tokens after a retry.
pub const ESCALATED_MAX_TOKENS: u64 = 64_000;

/// Auto-compact threshold as a fraction of the context window (80%).
const AUTO_COMPACT_FRACTION: f64 = 0.80;

// ---------------------------------------------------------------------------
// Context window resolution
// ---------------------------------------------------------------------------

/// Get the effective context window for a model, respecting:
/// 1. `CLAUDE_CODE_MAX_CONTEXT_TOKENS` env override
/// 2. `[1m]` suffix on the model string
/// 3. `model_supports_1m` canonical check
/// 4. Static default (200k)
pub fn get_context_window_for_model(model: &str) -> u64 {
    // Environment override takes highest precedence.
    if let Ok(val) = env::var("CLAUDE_CODE_MAX_CONTEXT_TOKENS") {
        if let Ok(n) = val.parse::<u64>() {
            if n > 0 {
                return n;
            }
        }
    }

    // [1m] suffix — explicit client-side opt-in.
    if has_1m_context(model) {
        if is_1m_context_disabled() {
            return MODEL_CONTEXT_WINDOW_DEFAULT;
        }
        return 1_000_000;
    }

    // Check if canonical model supports 1M (for beta-header based upgrades etc.).
    // The static capabilities table currently uses 200k as the base, so we rely
    // on explicit opt-in via [1m] suffix or beta headers for 1M access.
    MODEL_CONTEXT_WINDOW_DEFAULT
}

/// Alias for `get_context_window_for_model`.
pub fn get_context_window(model: &str) -> u64 {
    get_context_window_for_model(model)
}

/// Get the effective context window capped to the model's known maximum.
pub fn get_effective_context_window(model: &str) -> u64 {
    get_context_window_for_model(model)
}

// ---------------------------------------------------------------------------
// Max output tokens
// ---------------------------------------------------------------------------

/// Returns (default, upper_limit) for max output tokens given a model.
pub fn get_max_output_tokens(model: &str) -> (u64, u64) {
    let caps = get_model_capabilities(model);
    (caps.max_output_tokens_default, caps.max_output_tokens_upper)
}

/// Returns the max thinking budget tokens (upper_limit - 1).
pub fn get_max_thinking_tokens_for_model(model: &str) -> u64 {
    let (_, upper) = get_max_output_tokens(model);
    upper.saturating_sub(1)
}

// ---------------------------------------------------------------------------
// Auto-compact threshold
// ---------------------------------------------------------------------------

/// Calculate the token count at which auto-compact should trigger.
pub fn get_auto_compact_threshold(model: &str) -> u64 {
    let window = get_context_window_for_model(model);
    (window as f64 * AUTO_COMPACT_FRACTION) as u64
}

// ---------------------------------------------------------------------------
// Context usage percentages
// ---------------------------------------------------------------------------

/// Percentage breakdown of context window usage.
#[derive(Clone, Debug)]
pub struct ContextPercentages {
    /// Percentage of the context window used (0–100), or `None` if no usage data.
    pub used: Option<u8>,
    /// Percentage remaining (0–100), or `None` if no usage data.
    pub remaining: Option<u8>,
}

/// Calculate context window usage percentages from token counts.
pub fn calculate_context_percentages(
    current_usage: Option<&crate::utils::tokens::CurrentUsage>,
    context_window_size: u64,
) -> ContextPercentages {
    match current_usage {
        None => ContextPercentages {
            used: None,
            remaining: None,
        },
        Some(usage) => {
            let total_input = usage.input_tokens
                + usage.cache_creation_input_tokens
                + usage.cache_read_input_tokens;
            let pct = ((total_input as f64 / context_window_size as f64) * 100.0).round() as i64;
            let clamped = pct.clamp(0, 100) as u8;
            ContextPercentages {
                used: Some(clamped),
                remaining: Some(100 - clamped),
            }
        }
    }
}

/// Detect if a model string indicates 1M context.
pub fn is_1m_context_model(model: &str) -> bool {
    has_1m_context(model)
        || (!is_1m_context_disabled() && model_supports_1m(model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_context_window() {
        assert_eq!(
            get_context_window_for_model("claude-opus-4-6-20260401"),
            200_000
        );
    }

    #[test]
    fn test_1m_context_window() {
        assert_eq!(
            get_context_window_for_model("claude-opus-4-6[1m]"),
            1_000_000
        );
    }

    #[test]
    fn test_auto_compact_threshold() {
        let t = get_auto_compact_threshold("claude-opus-4-6");
        assert_eq!(t, 160_000); // 80% of 200k
    }

    #[test]
    fn test_max_output_tokens() {
        let (default, upper) = get_max_output_tokens("claude-opus-4-6-20260401");
        assert_eq!(default, 64_000);
        assert_eq!(upper, 128_000);
    }

    #[test]
    fn test_context_percentages_none() {
        let pct = calculate_context_percentages(None, 200_000);
        assert!(pct.used.is_none());
    }

    #[test]
    fn test_context_percentages() {
        let usage = crate::utils::tokens::CurrentUsage {
            input_tokens: 100_000,
            output_tokens: 0,
            cache_creation_input_tokens: 20_000,
            cache_read_input_tokens: 30_000,
        };
        let pct = calculate_context_percentages(Some(&usage), 200_000);
        assert_eq!(pct.used, Some(75));
        assert_eq!(pct.remaining, Some(25));
    }
}
