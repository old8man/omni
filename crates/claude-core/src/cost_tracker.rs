use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::types::usage::Usage;
use crate::utils::model::get_canonical_name;

// ---------------------------------------------------------------------------
// Model pricing
// ---------------------------------------------------------------------------

/// Per-token pricing for a model (costs in USD per million tokens).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_cost_per_mtok: f64,
    pub output_cost_per_mtok: f64,
    pub cache_read_cost_per_mtok: f64,
    pub cache_write_cost_per_mtok: f64,
    pub web_search_cost_per_request: f64,
}

/// Standard pricing tier: Sonnet models ($3/$15 per Mtok).
const TIER_3_15: ModelPricing = ModelPricing {
    input_cost_per_mtok: 3.0,
    output_cost_per_mtok: 15.0,
    cache_write_cost_per_mtok: 3.75,
    cache_read_cost_per_mtok: 0.3,
    web_search_cost_per_request: 0.01,
};

/// Opus 4/4.1 tier ($15/$75 per Mtok).
const TIER_15_75: ModelPricing = ModelPricing {
    input_cost_per_mtok: 15.0,
    output_cost_per_mtok: 75.0,
    cache_write_cost_per_mtok: 18.75,
    cache_read_cost_per_mtok: 1.5,
    web_search_cost_per_request: 0.01,
};

/// Opus 4.5/4.6 normal tier ($5/$25 per Mtok).
const TIER_5_25: ModelPricing = ModelPricing {
    input_cost_per_mtok: 5.0,
    output_cost_per_mtok: 25.0,
    cache_write_cost_per_mtok: 6.25,
    cache_read_cost_per_mtok: 0.5,
    web_search_cost_per_request: 0.01,
};

/// Opus 4.6 fast mode tier ($30/$150 per Mtok).
const TIER_30_150: ModelPricing = ModelPricing {
    input_cost_per_mtok: 30.0,
    output_cost_per_mtok: 150.0,
    cache_write_cost_per_mtok: 37.5,
    cache_read_cost_per_mtok: 3.0,
    web_search_cost_per_request: 0.01,
};

/// Haiku 3.5 tier ($0.80/$4 per Mtok).
const TIER_HAIKU_35: ModelPricing = ModelPricing {
    input_cost_per_mtok: 0.8,
    output_cost_per_mtok: 4.0,
    cache_write_cost_per_mtok: 1.0,
    cache_read_cost_per_mtok: 0.08,
    web_search_cost_per_request: 0.01,
};

/// Haiku 4.5 tier ($1/$5 per Mtok).
const TIER_HAIKU_45: ModelPricing = ModelPricing {
    input_cost_per_mtok: 1.0,
    output_cost_per_mtok: 5.0,
    cache_write_cost_per_mtok: 1.25,
    cache_read_cost_per_mtok: 0.1,
    web_search_cost_per_request: 0.01,
};

/// Fallback pricing for unknown models (Opus 4.5 $5/$25 tier).
const DEFAULT_UNKNOWN_PRICING: ModelPricing = TIER_5_25;

fn build_model_pricing() -> HashMap<&'static str, ModelPricing> {
    let mut m = HashMap::new();

    // Haiku family
    m.insert("claude-3-5-haiku", TIER_HAIKU_35);
    m.insert("claude-haiku-4-5", TIER_HAIKU_45);

    // Sonnet family (all $3/$15 tier)
    m.insert("claude-3-5-sonnet", TIER_3_15);
    m.insert("claude-3-7-sonnet", TIER_3_15);
    m.insert("claude-sonnet-4", TIER_3_15);
    m.insert("claude-sonnet-4-5", TIER_3_15);
    m.insert("claude-sonnet-4-6", TIER_3_15);

    // Opus 4/4.1 ($15/$75 tier)
    m.insert("claude-opus-4", TIER_15_75);
    m.insert("claude-opus-4-1", TIER_15_75);

    // Opus 4.5/4.6 ($5/$25 normal tier — fast mode handled separately)
    m.insert("claude-opus-4-5", TIER_5_25);
    m.insert("claude-opus-4-6", TIER_5_25);

    m
}

/// Map a full model name to its canonical short form.
///
/// Delegates to `utils::model::get_canonical_name()` which implements
/// the canonical name mapping logic (matching TS `firstPartyNameToCanonical`).
fn to_canonical(model: &str) -> String {
    get_canonical_name(model)
}

fn model_pricing() -> &'static HashMap<&'static str, ModelPricing> {
    use std::sync::OnceLock;
    static PRICING: OnceLock<HashMap<&'static str, ModelPricing>> = OnceLock::new();
    PRICING.get_or_init(build_model_pricing)
}

/// Look up pricing for a model, falling back to the default tier.
///
/// Model names are canonicalized (date suffixes and 3P prefixes stripped)
/// before lookup, matching the TypeScript `firstPartyNameToCanonical()`.
///
/// For Opus 4.6 in fast mode, use [`get_pricing_for_usage`] instead.
pub fn get_pricing(model: &str) -> ModelPricing {
    let canonical = to_canonical(model);
    model_pricing()
        .get(canonical.as_str())
        .cloned()
        .unwrap_or(DEFAULT_UNKNOWN_PRICING)
}

/// Look up pricing for a model + usage combination.
///
/// Handles Opus 4.6 fast mode: if `usage.speed == Some("fast")` and the
/// model is Opus 4.6, returns the $30/$150 fast-mode tier.
pub fn get_pricing_for_usage(model: &str, usage: &Usage) -> ModelPricing {
    let is_opus_46 = model.contains("opus-4-6");
    if is_opus_46 && usage.speed.as_deref() == Some("fast") {
        return TIER_30_150;
    }
    get_pricing(model)
}

/// Returns true if the model is not in the known pricing table.
pub fn is_unknown_model(model: &str) -> bool {
    let canonical = to_canonical(model);
    !model_pricing().contains_key(canonical.as_str())
}

/// Calculate USD cost from usage and pricing.
pub fn calculate_cost(pricing: &ModelPricing, usage: &Usage) -> f64 {
    let input = (usage.input_tokens as f64 / 1_000_000.0) * pricing.input_cost_per_mtok;
    let output = (usage.output_tokens as f64 / 1_000_000.0) * pricing.output_cost_per_mtok;
    let cache_read = (usage.cache_read_input_tokens.unwrap_or(0) as f64 / 1_000_000.0)
        * pricing.cache_read_cost_per_mtok;
    let cache_write = (usage.cache_creation_input_tokens.unwrap_or(0) as f64 / 1_000_000.0)
        * pricing.cache_write_cost_per_mtok;
    let web_search = usage
        .server_tool_use
        .as_ref()
        .map(|s| s.web_search_requests as f64 * pricing.web_search_cost_per_request)
        .unwrap_or(0.0);
    input + output + cache_read + cache_write + web_search
}

/// Calculate USD cost for a model + usage in one call.
pub fn calculate_usd_cost(model: &str, usage: &Usage) -> f64 {
    let pricing = get_pricing_for_usage(model, usage);
    calculate_cost(&pricing, usage)
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Format a cost value as a USD string.
///
/// - Costs > $0.50 are shown with 2 decimal places (e.g. `$1.23`)
/// - Costs <= $0.50 are shown with up to `max_decimal_places` (e.g. `$0.0042`)
pub fn format_cost(cost: f64, max_decimal_places: usize) -> String {
    if cost > 0.5 {
        let rounded = (cost * 100.0).round() / 100.0;
        format!("${rounded:.2}")
    } else {
        format!("${cost:.prec$}", prec = max_decimal_places)
    }
}

/// Format a number with thousands separators (e.g. `1,234,567`).
pub fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

/// Format a duration in human-readable form (e.g. `2m 30s`, `45s`).
///
/// Delegates to [`crate::utils::format::format_duration`] for the actual formatting.
pub fn format_duration(duration: Duration) -> String {
    crate::utils::format::format_duration(duration.as_millis() as u64)
}

/// Format a pricing tier as `$input/$output per Mtok`.
pub fn format_model_pricing(pricing: &ModelPricing) -> String {
    let fmt_price = |p: f64| -> String {
        if p == p.floor() {
            format!("${}", p as u64)
        } else {
            format!("${p:.2}")
        }
    };
    format!(
        "{}/{} per Mtok",
        fmt_price(pricing.input_cost_per_mtok),
        fmt_price(pricing.output_cost_per_mtok)
    )
}

// ---------------------------------------------------------------------------
// Per-model usage tracking
// ---------------------------------------------------------------------------

/// Per-model usage accumulator (matches TS `ModelUsage`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub web_search_requests: u64,
    pub cost_usd: f64,
    #[serde(default)]
    pub context_window: u64,
    #[serde(default)]
    pub max_output_tokens: u64,
}

// ---------------------------------------------------------------------------
// Stored cost state (session save/restore)
// ---------------------------------------------------------------------------

/// Serializable snapshot of session cost state for persistence.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCostState {
    pub total_cost_usd: f64,
    pub total_api_duration_ms: f64,
    pub total_api_duration_without_retries_ms: f64,
    pub total_tool_duration_ms: f64,
    pub total_lines_added: u64,
    pub total_lines_removed: u64,
    pub last_duration_ms: Option<f64>,
    pub model_usage: Option<HashMap<String, ModelUsage>>,
}

// ---------------------------------------------------------------------------
// CostTracker
// ---------------------------------------------------------------------------

/// Tracks cumulative cost and usage for a session.
///
/// Thread-safe via internal `Mutex`. Tracks per-model usage, API/tool
/// durations, lines changed, and web search requests.
#[derive(Debug, Clone)]
pub struct CostTracker {
    inner: Arc<Mutex<CostTrackerInner>>,
}

#[derive(Debug)]
struct CostTrackerInner {
    total_cost_usd: f64,
    has_unknown_model_cost: bool,
    model_usage: HashMap<String, ModelUsage>,
    // Duration tracking
    total_api_duration: Duration,
    total_api_duration_without_retries: Duration,
    total_tool_duration: Duration,
    session_start: Instant,
    // Lines changed tracking
    total_lines_added: u64,
    total_lines_removed: u64,
    // Web search
    total_web_search_requests: u64,
}

impl CostTracker {
    /// Create a new cost tracker.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CostTrackerInner {
                total_cost_usd: 0.0,
                has_unknown_model_cost: false,
                model_usage: HashMap::new(),
                total_api_duration: Duration::ZERO,
                total_api_duration_without_retries: Duration::ZERO,
                total_tool_duration: Duration::ZERO,
                session_start: Instant::now(),
                total_lines_added: 0,
                total_lines_removed: 0,
                total_web_search_requests: 0,
            })),
        }
    }

    /// Record usage from a single API call for a given model.
    pub fn add_usage(&self, model: &str, usage: &Usage) {
        if let Ok(mut inner) = self.inner.lock() {
            let cost = calculate_usd_cost(model, usage);
            inner.total_cost_usd += cost;

            if is_unknown_model(model) {
                inner.has_unknown_model_cost = true;
            }

            let web_search = usage
                .server_tool_use
                .as_ref()
                .map(|s| s.web_search_requests)
                .unwrap_or(0);
            inner.total_web_search_requests += web_search;

            let entry = inner.model_usage.entry(model.to_string()).or_default();
            entry.input_tokens += usage.input_tokens;
            entry.output_tokens += usage.output_tokens;
            entry.cache_read_input_tokens += usage.cache_read_input_tokens.unwrap_or(0);
            entry.cache_creation_input_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
            entry.web_search_requests += web_search;
            entry.cost_usd += cost;
        }
    }

    /// Record API call duration (total including retries).
    pub fn add_api_duration(&self, duration: Duration) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.total_api_duration += duration;
        }
    }

    /// Record API call duration (without retries).
    pub fn add_api_duration_without_retries(&self, duration: Duration) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.total_api_duration_without_retries += duration;
        }
    }

    /// Record tool execution duration.
    pub fn add_tool_duration(&self, duration: Duration) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.total_tool_duration += duration;
        }
    }

    /// Record lines added and removed.
    pub fn add_lines_changed(&self, added: u64, removed: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.total_lines_added += added;
            inner.total_lines_removed += removed;
        }
    }

    /// Get total cost in USD.
    pub fn total_cost_usd(&self) -> f64 {
        self.inner.lock().map(|i| i.total_cost_usd).unwrap_or(0.0)
    }

    /// Whether any unknown model was seen (costs may be inaccurate).
    pub fn has_unknown_model_cost(&self) -> bool {
        self.inner
            .lock()
            .map(|i| i.has_unknown_model_cost)
            .unwrap_or(false)
    }

    /// Get per-model usage map.
    pub fn model_usage(&self) -> HashMap<String, ModelUsage> {
        self.inner
            .lock()
            .map(|i| i.model_usage.clone())
            .unwrap_or_default()
    }

    /// Get total input tokens across all models.
    pub fn total_input_tokens(&self) -> u64 {
        self.inner
            .lock()
            .map(|i| i.model_usage.values().map(|u| u.input_tokens).sum())
            .unwrap_or(0)
    }

    /// Get total output tokens across all models.
    pub fn total_output_tokens(&self) -> u64 {
        self.inner
            .lock()
            .map(|i| i.model_usage.values().map(|u| u.output_tokens).sum())
            .unwrap_or(0)
    }

    /// Get total cache read input tokens.
    pub fn total_cache_read_input_tokens(&self) -> u64 {
        self.inner
            .lock()
            .map(|i| {
                i.model_usage
                    .values()
                    .map(|u| u.cache_read_input_tokens)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Get total cache creation input tokens.
    pub fn total_cache_creation_input_tokens(&self) -> u64 {
        self.inner
            .lock()
            .map(|i| {
                i.model_usage
                    .values()
                    .map(|u| u.cache_creation_input_tokens)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Get total web search requests.
    pub fn total_web_search_requests(&self) -> u64 {
        self.inner
            .lock()
            .map(|i| i.total_web_search_requests)
            .unwrap_or(0)
    }

    /// Get total API duration.
    pub fn total_api_duration(&self) -> Duration {
        self.inner
            .lock()
            .map(|i| i.total_api_duration)
            .unwrap_or(Duration::ZERO)
    }

    /// Get total wall-clock duration since tracker creation.
    pub fn total_duration(&self) -> Duration {
        self.inner
            .lock()
            .map(|i| i.session_start.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    /// Get total lines added.
    pub fn total_lines_added(&self) -> u64 {
        self.inner.lock().map(|i| i.total_lines_added).unwrap_or(0)
    }

    /// Get total lines removed.
    pub fn total_lines_removed(&self) -> u64 {
        self.inner
            .lock()
            .map(|i| i.total_lines_removed)
            .unwrap_or(0)
    }

    /// Restore cost state from a stored snapshot (session resume).
    pub fn restore(&self, state: &StoredCostState) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.total_cost_usd = state.total_cost_usd;
            inner.total_api_duration = Duration::from_millis(state.total_api_duration_ms as u64);
            inner.total_api_duration_without_retries =
                Duration::from_millis(state.total_api_duration_without_retries_ms as u64);
            inner.total_tool_duration = Duration::from_millis(state.total_tool_duration_ms as u64);
            inner.total_lines_added = state.total_lines_added;
            inner.total_lines_removed = state.total_lines_removed;
            if let Some(usage) = &state.model_usage {
                inner.model_usage = usage.clone();
            }
        }
    }

    /// Snapshot current cost state for persistence.
    pub fn snapshot(&self) -> StoredCostState {
        let inner = self.inner.lock().expect("cost tracker mutex poisoned");
        StoredCostState {
            total_cost_usd: inner.total_cost_usd,
            total_api_duration_ms: inner.total_api_duration.as_millis() as f64,
            total_api_duration_without_retries_ms: inner
                .total_api_duration_without_retries
                .as_millis() as f64,
            total_tool_duration_ms: inner.total_tool_duration.as_millis() as f64,
            total_lines_added: inner.total_lines_added,
            total_lines_removed: inner.total_lines_removed,
            last_duration_ms: Some(inner.session_start.elapsed().as_millis() as f64),
            model_usage: Some(inner.model_usage.clone()),
        }
    }

    /// Reset all tracked state (for tests).
    pub fn reset(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.total_cost_usd = 0.0;
            inner.has_unknown_model_cost = false;
            inner.model_usage.clear();
            inner.total_api_duration = Duration::ZERO;
            inner.total_api_duration_without_retries = Duration::ZERO;
            inner.total_tool_duration = Duration::ZERO;
            inner.session_start = Instant::now();
            inner.total_lines_added = 0;
            inner.total_lines_removed = 0;
            inner.total_web_search_requests = 0;
        }
    }

    /// Format total cost summary for display (matches TS `formatTotalCost`).
    pub fn format_total_cost(&self) -> String {
        let cost = self.total_cost_usd();
        let cost_display = if self.has_unknown_model_cost() {
            format!(
                "{} (costs may be inaccurate due to usage of unknown models)",
                format_cost(cost, 4)
            )
        } else {
            format_cost(cost, 4)
        };

        let model_usage_display = self.format_model_usage();
        let api_dur = self.total_api_duration();
        let wall_dur = self.total_duration();
        let added = self.total_lines_added();
        let removed = self.total_lines_removed();
        let added_label = if added == 1 { "line" } else { "lines" };
        let removed_label = if removed == 1 { "line" } else { "lines" };

        format!(
            "Total cost:            {cost_display}\n\
             Total duration (API):  {}\n\
             Total duration (wall): {}\n\
             Total code changes:    {added} {added_label} added, {removed} {removed_label} removed\n\
             {model_usage_display}",
            format_duration(api_dur),
            format_duration(wall_dur),
        )
    }

    /// Format per-model usage breakdown.
    fn format_model_usage(&self) -> String {
        let usage_map = self.model_usage();
        if usage_map.is_empty() {
            return "Usage:                 0 input, 0 output, 0 cache read, 0 cache write"
                .to_string();
        }

        let mut result = String::from("Usage by model:");
        for (model, usage) in &usage_map {
            let mut line = format!(
                "  {} input, {} output, {} cache read, {} cache write",
                format_number(usage.input_tokens),
                format_number(usage.output_tokens),
                format_number(usage.cache_read_input_tokens),
                format_number(usage.cache_creation_input_tokens),
            );
            if usage.web_search_requests > 0 {
                line.push_str(&format!(
                    ", {} web search",
                    format_number(usage.web_search_requests)
                ));
            }
            line.push_str(&format!(" ({})", format_cost(usage.cost_usd, 4)));
            result.push_str(&format!("\n{:>21}:{line}", model));
        }
        result
    }
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Legacy single-model API (kept for backwards compatibility)
// ---------------------------------------------------------------------------

/// Record usage for a single API call. Convenience wrapper.
pub fn record_usage(tracker: &CostTracker, model: &str, usage: &Usage) {
    tracker.add_usage(model, usage);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::usage::ServerToolUse;

    #[test]
    fn test_sonnet_pricing() {
        // Short canonical name
        let pricing = get_pricing("claude-sonnet-4-6");
        assert!((pricing.input_cost_per_mtok - 3.0).abs() < f64::EPSILON);
        assert!((pricing.output_cost_per_mtok - 15.0).abs() < f64::EPSILON);
        // Full date-suffixed name (canonicalized)
        let pricing2 = get_pricing("claude-sonnet-4-6-20260401");
        assert!((pricing2.input_cost_per_mtok - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_opus_46_normal_pricing() {
        let pricing = get_pricing("claude-opus-4-6");
        assert!((pricing.input_cost_per_mtok - 5.0).abs() < f64::EPSILON);
        assert!((pricing.output_cost_per_mtok - 25.0).abs() < f64::EPSILON);
        // Bedrock-style name should also resolve
        let pricing2 = get_pricing("us.anthropic.claude-opus-4-6-v1");
        assert!((pricing2.input_cost_per_mtok - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_opus_46_fast_mode_pricing() {
        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            speed: Some("fast".to_string()),
            ..Default::default()
        };
        let pricing = get_pricing_for_usage("claude-opus-4-6-20260401", &usage);
        assert!((pricing.input_cost_per_mtok - 30.0).abs() < f64::EPSILON);
        assert!((pricing.output_cost_per_mtok - 150.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_opus_4_pricing() {
        let pricing = get_pricing("claude-opus-4-20250514");
        assert!((pricing.input_cost_per_mtok - 15.0).abs() < f64::EPSILON);
        assert!((pricing.output_cost_per_mtok - 75.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_haiku_45_pricing() {
        let pricing = get_pricing("claude-haiku-4-5-20251001");
        assert!((pricing.input_cost_per_mtok - 1.0).abs() < f64::EPSILON);
        assert!((pricing.output_cost_per_mtok - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_haiku_35_pricing() {
        let pricing = get_pricing("claude-3-5-haiku-20241022");
        assert!((pricing.input_cost_per_mtok - 0.8).abs() < f64::EPSILON);
        assert!((pricing.output_cost_per_mtok - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_unknown_model_fallback() {
        let pricing = get_pricing("some-future-model");
        // Falls back to $5/$25 tier
        assert!((pricing.input_cost_per_mtok - 5.0).abs() < f64::EPSILON);
        assert!((pricing.output_cost_per_mtok - 25.0).abs() < f64::EPSILON);
        assert!(is_unknown_model("some-future-model"));
    }

    #[test]
    fn test_canonical_name_resolution() {
        // Date-suffixed
        assert_eq!(to_canonical("claude-opus-4-6-20260401"), "claude-opus-4-6");
        assert_eq!(to_canonical("claude-sonnet-4-20250514"), "claude-sonnet-4");
        // Bedrock format
        assert_eq!(
            to_canonical("us.anthropic.claude-opus-4-6-v1"),
            "claude-opus-4-6"
        );
        // Vertex format
        assert_eq!(
            to_canonical("claude-sonnet-4-5@20250929"),
            "claude-sonnet-4-5"
        );
        // Already canonical
        assert_eq!(to_canonical("claude-opus-4-6"), "claude-opus-4-6");
        // Unknown passes through
        assert_eq!(to_canonical("my-custom-model"), "my-custom-model");
    }

    #[test]
    fn test_calculate_cost_with_web_search() {
        let pricing = TIER_3_15;
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            server_tool_use: Some(ServerToolUse {
                web_search_requests: 5,
            }),
            speed: None,
        };
        let cost = calculate_cost(&pricing, &usage);
        // $3 input + $1.5 output + $0.05 web search = $4.55
        assert!((cost - 4.55).abs() < 0.001);
    }

    #[test]
    fn test_format_cost_small() {
        assert_eq!(format_cost(0.0042, 4), "$0.0042");
        assert_eq!(format_cost(0.1, 4), "$0.1000");
    }

    #[test]
    fn test_format_cost_large() {
        assert_eq!(format_cost(1.234, 4), "$1.23");
        assert_eq!(format_cost(99.999, 4), "$100.00");
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1_234_567), "1,234,567");
    }

    #[test]
    fn test_tracker_per_model_usage() {
        let tracker = CostTracker::new();
        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        tracker.add_usage("claude-sonnet-4-6", &usage);
        tracker.add_usage("claude-opus-4-6", &usage);
        tracker.add_usage("claude-sonnet-4-6", &usage);

        let model_usage = tracker.model_usage();
        assert_eq!(model_usage.len(), 2);
        assert_eq!(model_usage["claude-sonnet-4-6"].input_tokens, 2000);
        assert_eq!(model_usage["claude-opus-4-6"].input_tokens, 1000);
    }

    #[test]
    fn test_tracker_snapshot_restore() {
        let tracker = CostTracker::new();
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            ..Default::default()
        };
        tracker.add_usage("claude-sonnet-4-6", &usage);
        tracker.add_lines_changed(50, 10);

        let snapshot = tracker.snapshot();
        assert!(snapshot.total_cost_usd > 0.0);
        assert_eq!(snapshot.total_lines_added, 50);
        assert_eq!(snapshot.total_lines_removed, 10);

        let tracker2 = CostTracker::new();
        tracker2.restore(&snapshot);
        assert!((tracker2.total_cost_usd() - tracker.total_cost_usd()).abs() < 0.001);
        assert_eq!(tracker2.total_lines_added(), 50);
    }

    #[test]
    fn test_format_model_pricing() {
        assert_eq!(format_model_pricing(&TIER_3_15), "$3/$15 per Mtok");
        assert_eq!(format_model_pricing(&TIER_HAIKU_35), "$0.80/$4 per Mtok");
    }
}
