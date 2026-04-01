use std::sync::LazyLock;
use std::time::Instant;

use regex::Regex;

const COMPLETION_THRESHOLD: f64 = 0.9;
const DIMINISHING_THRESHOLD: u64 = 500;

/// Tracks token budget state across query loop iterations.
pub struct BudgetTracker {
    pub continuation_count: u32,
    pub last_delta_tokens: u64,
    pub last_global_turn_tokens: u64,
    pub started_at: Instant,
}

impl BudgetTracker {
    pub fn new() -> Self {
        Self {
            continuation_count: 0,
            last_delta_tokens: 0,
            last_global_turn_tokens: 0,
            started_at: Instant::now(),
        }
    }
}

impl Default for BudgetTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// The decision returned by `check_token_budget`.
#[derive(Debug)]
pub enum TokenBudgetDecision {
    Continue {
        nudge_message: String,
        continuation_count: u32,
        pct: u64,
        turn_tokens: u64,
        budget: u64,
    },
    Stop {
        completion_event: Option<CompletionEvent>,
    },
}

#[derive(Debug)]
pub struct CompletionEvent {
    pub continuation_count: u32,
    pub pct: u64,
    pub turn_tokens: u64,
    pub budget: u64,
    pub diminishing_returns: bool,
    pub duration_ms: u64,
}

/// Check whether the query should continue running to fill the token budget,
/// or stop because the budget is (nearly) exhausted or progress has stalled.
///
/// Mirrors the TypeScript `checkTokenBudget` in `query/tokenBudget.ts`.
pub fn check_token_budget(
    tracker: &mut BudgetTracker,
    is_subagent: bool,
    budget: Option<u64>,
    global_turn_tokens: u64,
) -> TokenBudgetDecision {
    let budget = match budget {
        Some(b) if !is_subagent && b > 0 => b,
        _ => return TokenBudgetDecision::Stop { completion_event: None },
    };

    let turn_tokens = global_turn_tokens;
    let pct = (turn_tokens as f64 / budget as f64 * 100.0).round() as u64;
    let delta_since_last = global_turn_tokens.saturating_sub(tracker.last_global_turn_tokens);

    let is_diminishing = tracker.continuation_count >= 3
        && delta_since_last < DIMINISHING_THRESHOLD
        && tracker.last_delta_tokens < DIMINISHING_THRESHOLD;

    if !is_diminishing && (turn_tokens as f64) < (budget as f64 * COMPLETION_THRESHOLD) {
        tracker.continuation_count += 1;
        tracker.last_delta_tokens = delta_since_last;
        tracker.last_global_turn_tokens = global_turn_tokens;
        return TokenBudgetDecision::Continue {
            nudge_message: budget_continuation_message(pct, turn_tokens, budget),
            continuation_count: tracker.continuation_count,
            pct,
            turn_tokens,
            budget,
        };
    }

    if is_diminishing || tracker.continuation_count > 0 {
        return TokenBudgetDecision::Stop {
            completion_event: Some(CompletionEvent {
                continuation_count: tracker.continuation_count,
                pct,
                turn_tokens,
                budget,
                diminishing_returns: is_diminishing,
                duration_ms: tracker.started_at.elapsed().as_millis() as u64,
            }),
        };
    }

    TokenBudgetDecision::Stop { completion_event: None }
}

/// Build the nudge message injected as a user turn to keep the model working.
fn budget_continuation_message(pct: u64, turn_tokens: u64, budget: u64) -> String {
    format!(
        "Stopped at {}% of token target ({} / {}). Keep working \u{2014} do not summarize.",
        pct,
        format_number(turn_tokens),
        format_number(budget),
    )
}

fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

// Token budget parsing regexes — compiled once at first use.
static RE_BUDGET_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*\+(\d+(?:\.\d+)?)\s*(k|m|b)\b").expect("static regex")
});
static RE_BUDGET_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s\+(\d+(?:\.\d+)?)\s*(k|m|b)\s*[.!?]?\s*$").expect("static regex")
});
static RE_BUDGET_VERBOSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:use|spend)\s+(\d+(?:\.\d+)?)\s*(k|m|b)\s*tokens?\b")
        .expect("static regex")
});

/// Parse a token budget from user text like "+500k", "use 2M tokens", etc.
pub fn parse_token_budget(text: &str) -> Option<u64> {
    // Shorthand at start: "+500k"
    if let Some(caps) = RE_BUDGET_START.captures(text) {
        return Some(parse_budget_match(&caps[1], &caps[2]));
    }

    // Shorthand at end: "do the work +500k"
    if let Some(caps) = RE_BUDGET_END.captures(text) {
        return Some(parse_budget_match(&caps[1], &caps[2]));
    }

    // Verbose: "use 500k tokens"
    if let Some(caps) = RE_BUDGET_VERBOSE.captures(text) {
        return Some(parse_budget_match(&caps[1], &caps[2]));
    }

    None
}

fn parse_budget_match(value: &str, suffix: &str) -> u64 {
    let multiplier: f64 = match suffix.to_lowercase().as_str() {
        "k" => 1_000.0,
        "m" => 1_000_000.0,
        "b" => 1_000_000_000.0,
        _ => 1.0,
    };
    (value.parse::<f64>().unwrap_or(0.0) * multiplier) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_token_budget() {
        assert_eq!(parse_token_budget("+500k"), Some(500_000));
        assert_eq!(parse_token_budget("+2M"), Some(2_000_000));
        assert_eq!(parse_token_budget("do the work +1.5m"), Some(1_500_000));
        assert_eq!(parse_token_budget("use 500k tokens"), Some(500_000));
        assert_eq!(parse_token_budget("spend 2m tokens"), Some(2_000_000));
        assert_eq!(parse_token_budget("hello world"), None);
    }

    #[test]
    fn test_check_budget_no_budget() {
        let mut tracker = BudgetTracker::new();
        let decision = check_token_budget(&mut tracker, false, None, 100);
        assert!(matches!(decision, TokenBudgetDecision::Stop { completion_event: None }));
    }

    #[test]
    fn test_check_budget_subagent() {
        let mut tracker = BudgetTracker::new();
        let decision = check_token_budget(&mut tracker, true, Some(500_000), 100);
        assert!(matches!(decision, TokenBudgetDecision::Stop { completion_event: None }));
    }

    #[test]
    fn test_check_budget_continue() {
        let mut tracker = BudgetTracker::new();
        let decision = check_token_budget(&mut tracker, false, Some(500_000), 100_000);
        assert!(matches!(decision, TokenBudgetDecision::Continue { .. }));
    }

    #[test]
    fn test_check_budget_near_threshold() {
        let mut tracker = BudgetTracker::new();
        let decision = check_token_budget(&mut tracker, false, Some(500_000), 460_000);
        assert!(matches!(decision, TokenBudgetDecision::Stop { .. }));
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(500_000), "500,000");
        assert_eq!(format_number(1_234_567), "1,234,567");
        assert_eq!(format_number(42), "42");
    }
}
