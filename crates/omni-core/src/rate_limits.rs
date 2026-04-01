//! Rate limit tracking, early-warning detection, and user-facing messages.
//!
//! This module is the Rust equivalent of the TypeScript services:
//! - `claudeAiLimits.ts`  -- core quota status tracking
//! - `rateLimitMessages.ts` -- user-facing message generation
//! - `mockRateLimits.ts` -- mock support for testing
//! - `policyLimits/` -- organizational policy restrictions
//!
//! # Architecture
//!
//! `RateLimitTracker` is the central, thread-safe data structure. It:
//!
//! 1. Parses `anthropic-ratelimit-*` HTTP headers from API responses.
//! 2. Maintains current `ClaudeAiLimits` (quota status, utilization, etc.).
//! 3. Fires early-warning checks (header-based and time-relative fallback).
//! 4. Emits status-change notifications via registered listeners.
//! 5. Exposes raw per-window utilization for the status line.
//!
//! `RateLimitMessages` converts the limits state into user-facing text.
//!
//! `MockRateLimits` allows injecting synthetic headers and scenarios for
//! internal testing without hitting real API limits.
//!
//! `PolicyLimits` handles organizational policy restrictions fetched from the
//! API, with file caching, background polling, and fail-open semantics.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Datelike;
use serde::{Deserialize, Serialize};
use sha2::Digest;

// ---------------------------------------------------------------------------
// Quota status
// ---------------------------------------------------------------------------

/// Overall quota decision returned by the unified rate limiter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaStatus {
    /// Usage is within limits.
    Allowed,
    /// Usage is within limits but approaching a threshold.
    AllowedWarning,
    /// Usage has exceeded limits.
    Rejected,
}

impl QuotaStatus {
    /// Parse from the header string value.
    pub fn from_header(s: &str) -> Self {
        match s {
            "allowed_warning" => Self::AllowedWarning,
            "rejected" => Self::Rejected,
            _ => Self::Allowed,
        }
    }
}

impl fmt::Display for QuotaStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allowed => write!(f, "allowed"),
            Self::AllowedWarning => write!(f, "allowed_warning"),
            Self::Rejected => write!(f, "rejected"),
        }
    }
}

// ---------------------------------------------------------------------------
// Rate limit type
// ---------------------------------------------------------------------------

/// The type of rate limit that is active / exhausted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitType {
    FiveHour,
    SevenDay,
    SevenDayOpus,
    SevenDaySonnet,
    Overage,
}

impl RateLimitType {
    /// Parse from the representative-claim header value.
    pub fn from_header(s: &str) -> Option<Self> {
        match s {
            "five_hour" => Some(Self::FiveHour),
            "seven_day" => Some(Self::SevenDay),
            "seven_day_opus" => Some(Self::SevenDayOpus),
            "seven_day_sonnet" => Some(Self::SevenDaySonnet),
            "overage" => Some(Self::Overage),
            _ => None,
        }
    }

    /// Human-readable display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::FiveHour => "session limit",
            Self::SevenDay => "weekly limit",
            Self::SevenDayOpus => "Opus limit",
            Self::SevenDaySonnet => "Sonnet limit",
            Self::Overage => "extra usage limit",
        }
    }

    /// The header abbreviation used in `anthropic-ratelimit-unified-{abbrev}-*`.
    pub fn claim_abbrev(&self) -> Option<&'static str> {
        match self {
            Self::FiveHour => Some("5h"),
            Self::SevenDay => Some("7d"),
            Self::Overage => Some("overage"),
            _ => None,
        }
    }
}

impl fmt::Display for RateLimitType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ---------------------------------------------------------------------------
// Overage disabled reason
// ---------------------------------------------------------------------------

/// Why overage is disabled/rejected. Values come from the API's unified limiter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverageDisabledReason {
    OverageNotProvisioned,
    OrgLevelDisabled,
    OrgLevelDisabledUntil,
    OutOfCredits,
    SeatTierLevelDisabled,
    MemberLevelDisabled,
    SeatTierZeroCreditLimit,
    GroupZeroCreditLimit,
    MemberZeroCreditLimit,
    OrgServiceLevelDisabled,
    OrgServiceZeroCreditLimit,
    NoLimitsConfigured,
    Unknown,
}

impl OverageDisabledReason {
    /// Parse from the header string value.
    pub fn from_header(s: &str) -> Self {
        match s {
            "overage_not_provisioned" => Self::OverageNotProvisioned,
            "org_level_disabled" => Self::OrgLevelDisabled,
            "org_level_disabled_until" => Self::OrgLevelDisabledUntil,
            "out_of_credits" => Self::OutOfCredits,
            "seat_tier_level_disabled" => Self::SeatTierLevelDisabled,
            "member_level_disabled" => Self::MemberLevelDisabled,
            "seat_tier_zero_credit_limit" => Self::SeatTierZeroCreditLimit,
            "group_zero_credit_limit" => Self::GroupZeroCreditLimit,
            "member_zero_credit_limit" => Self::MemberZeroCreditLimit,
            "org_service_level_disabled" => Self::OrgServiceLevelDisabled,
            "org_service_zero_credit_limit" => Self::OrgServiceZeroCreditLimit,
            "no_limits_configured" => Self::NoLimitsConfigured,
            _ => Self::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// ClaudeAiLimits
// ---------------------------------------------------------------------------

/// Current rate-limit state extracted from API response headers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaudeAiLimits {
    pub status: QuotaStatus,
    /// Whether the unified rate limiter offered a fallback model.
    pub unified_rate_limit_fallback_available: bool,
    /// Unix epoch seconds when the limit resets.
    pub resets_at: Option<f64>,
    /// Which limit is the representative claim.
    pub rate_limit_type: Option<RateLimitType>,
    /// Current utilization as a fraction (0.0 - 1.0).
    pub utilization: Option<f64>,
    /// Overage quota status.
    pub overage_status: Option<QuotaStatus>,
    /// Unix epoch seconds when overage resets.
    pub overage_resets_at: Option<f64>,
    /// Why overage is disabled.
    pub overage_disabled_reason: Option<OverageDisabledReason>,
    /// Whether the user is currently consuming overage quota.
    pub is_using_overage: bool,
    /// The surpassed threshold value from the server header.
    pub surpassed_threshold: Option<f64>,
}

impl Default for ClaudeAiLimits {
    fn default() -> Self {
        Self {
            status: QuotaStatus::Allowed,
            unified_rate_limit_fallback_available: false,
            resets_at: None,
            rate_limit_type: None,
            utilization: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            is_using_overage: false,
            surpassed_threshold: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Raw per-window utilization (for status line)
// ---------------------------------------------------------------------------

/// Utilization snapshot for a single time window.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RawWindowUtilization {
    /// Fraction (0.0 - 1.0) of the window consumed.
    pub utilization: f64,
    /// Unix epoch seconds when the window resets.
    pub resets_at: f64,
}

/// Per-window raw utilization extracted from every API response.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RawUtilization {
    pub five_hour: Option<RawWindowUtilization>,
    pub seven_day: Option<RawWindowUtilization>,
}

// ---------------------------------------------------------------------------
// Early warning configuration
// ---------------------------------------------------------------------------

/// A single early-warning threshold.
#[derive(Clone, Debug)]
struct EarlyWarningThreshold {
    /// Trigger warning when usage >= this fraction (0.0 - 1.0).
    utilization: f64,
    /// Trigger warning when elapsed time fraction <= this (0.0 - 1.0).
    time_pct: f64,
}

/// Configuration for a rate-limit window's early warning.
#[derive(Clone, Debug)]
struct EarlyWarningConfig {
    rate_limit_type: RateLimitType,
    claim_abbrev: &'static str,
    window_seconds: f64,
    thresholds: Vec<EarlyWarningThreshold>,
}

/// Early warning configs checked in priority order.
fn early_warning_configs() -> Vec<EarlyWarningConfig> {
    vec![
        EarlyWarningConfig {
            rate_limit_type: RateLimitType::FiveHour,
            claim_abbrev: "5h",
            window_seconds: 5.0 * 60.0 * 60.0,
            thresholds: vec![EarlyWarningThreshold {
                utilization: 0.9,
                time_pct: 0.72,
            }],
        },
        EarlyWarningConfig {
            rate_limit_type: RateLimitType::SevenDay,
            claim_abbrev: "7d",
            window_seconds: 7.0 * 24.0 * 60.0 * 60.0,
            thresholds: vec![
                EarlyWarningThreshold {
                    utilization: 0.75,
                    time_pct: 0.6,
                },
                EarlyWarningThreshold {
                    utilization: 0.5,
                    time_pct: 0.35,
                },
                EarlyWarningThreshold {
                    utilization: 0.25,
                    time_pct: 0.15,
                },
            ],
        },
    ]
}

/// Maps claim abbreviations to rate limit types for header-based detection.
fn claim_abbrev_to_type(abbrev: &str) -> Option<RateLimitType> {
    match abbrev {
        "5h" => Some(RateLimitType::FiveHour),
        "7d" => Some(RateLimitType::SevenDay),
        "overage" => Some(RateLimitType::Overage),
        _ => None,
    }
}

/// Calculate fraction of a time window that has elapsed.
fn compute_time_progress(resets_at: f64, window_seconds: f64) -> f64 {
    let now_seconds = now_unix_seconds();
    let window_start = resets_at - window_seconds;
    let elapsed = now_seconds - window_start;
    (elapsed / window_seconds).clamp(0.0, 1.0)
}

/// Current time as Unix epoch seconds.
fn now_unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// Header abstraction
// ---------------------------------------------------------------------------

/// Trait for abstracting over HTTP header access, so the core logic is not
/// tied to a specific HTTP client library.
pub trait HeaderMap {
    /// Get a header value by name (case-insensitive). Returns `None` if absent.
    fn get_header(&self, name: &str) -> Option<String>;
}

/// A simple in-memory header map (used by mock limits and tests).
#[derive(Clone, Debug, Default)]
pub struct SimpleHeaderMap {
    headers: HashMap<String, String>,
}

impl SimpleHeaderMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.headers.insert(key.into().to_lowercase(), value.into());
    }

    pub fn remove(&mut self, key: &str) {
        self.headers.remove(&key.to_lowercase());
    }

    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.headers.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.headers.iter()
    }

    /// Merge another header map on top of this one (overwrites existing keys).
    pub fn merge(&mut self, other: &SimpleHeaderMap) {
        for (k, v) in &other.headers {
            self.headers.insert(k.clone(), v.clone());
        }
    }
}

impl HeaderMap for SimpleHeaderMap {
    fn get_header(&self, name: &str) -> Option<String> {
        self.headers.get(&name.to_lowercase()).cloned()
    }
}

/// Blanket implementation for `reqwest::header::HeaderMap` so callers can pass
/// real HTTP response headers directly.
impl HeaderMap for reqwest::header::HeaderMap {
    fn get_header(&self, name: &str) -> Option<String> {
        self.get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }
}

// ---------------------------------------------------------------------------
// Header extraction helpers
// ---------------------------------------------------------------------------

/// Extract raw per-window utilization from response headers.
fn extract_raw_utilization(headers: &dyn HeaderMap) -> RawUtilization {
    let mut result = RawUtilization::default();

    for (field, abbrev) in &[("five_hour", "5h"), ("seven_day", "7d")] {
        let util_key = format!("anthropic-ratelimit-unified-{abbrev}-utilization");
        let reset_key = format!("anthropic-ratelimit-unified-{abbrev}-reset");

        if let (Some(util_str), Some(reset_str)) =
            (headers.get_header(&util_key), headers.get_header(&reset_key))
        {
            if let (Ok(util), Ok(reset)) = (util_str.parse::<f64>(), reset_str.parse::<f64>()) {
                let window = RawWindowUtilization {
                    utilization: util,
                    resets_at: reset,
                };
                match *field {
                    "five_hour" => result.five_hour = Some(window),
                    "seven_day" => result.seven_day = Some(window),
                    _ => {}
                }
            }
        }
    }

    result
}

/// Check for early warning based on surpassed-threshold headers.
fn get_header_based_early_warning(
    headers: &dyn HeaderMap,
    fallback_available: bool,
) -> Option<ClaudeAiLimits> {
    let claim_abbrevs = ["5h", "7d", "overage"];

    for abbrev in &claim_abbrevs {
        let threshold_key =
            format!("anthropic-ratelimit-unified-{abbrev}-surpassed-threshold");

        if let Some(threshold_str) = headers.get_header(&threshold_key) {
            let rate_limit_type = claim_abbrev_to_type(abbrev)?;

            let util_key = format!("anthropic-ratelimit-unified-{abbrev}-utilization");
            let reset_key = format!("anthropic-ratelimit-unified-{abbrev}-reset");

            let utilization = headers
                .get_header(&util_key)
                .and_then(|s| s.parse::<f64>().ok());
            let resets_at = headers
                .get_header(&reset_key)
                .and_then(|s| s.parse::<f64>().ok());

            return Some(ClaudeAiLimits {
                status: QuotaStatus::AllowedWarning,
                resets_at,
                rate_limit_type: Some(rate_limit_type),
                utilization,
                unified_rate_limit_fallback_available: fallback_available,
                is_using_overage: false,
                surpassed_threshold: threshold_str.parse::<f64>().ok(),
                ..Default::default()
            });
        }
    }

    None
}

/// Check time-relative early warning for a single config.
fn get_time_relative_early_warning(
    headers: &dyn HeaderMap,
    config: &EarlyWarningConfig,
    fallback_available: bool,
) -> Option<ClaudeAiLimits> {
    let util_key = format!(
        "anthropic-ratelimit-unified-{}-utilization",
        config.claim_abbrev
    );
    let reset_key = format!(
        "anthropic-ratelimit-unified-{}-reset",
        config.claim_abbrev
    );

    let utilization = headers
        .get_header(&util_key)
        .and_then(|s| s.parse::<f64>().ok())?;
    let resets_at = headers
        .get_header(&reset_key)
        .and_then(|s| s.parse::<f64>().ok())?;

    let time_progress = compute_time_progress(resets_at, config.window_seconds);

    let should_warn = config
        .thresholds
        .iter()
        .any(|t| utilization >= t.utilization && time_progress <= t.time_pct);

    if !should_warn {
        return None;
    }

    Some(ClaudeAiLimits {
        status: QuotaStatus::AllowedWarning,
        resets_at: Some(resets_at),
        rate_limit_type: Some(config.rate_limit_type),
        utilization: Some(utilization),
        unified_rate_limit_fallback_available: fallback_available,
        is_using_overage: false,
        ..Default::default()
    })
}

/// Get early warning using header-based detection with time-relative fallback.
fn get_early_warning_from_headers(
    headers: &dyn HeaderMap,
    fallback_available: bool,
) -> Option<ClaudeAiLimits> {
    // Try header-based detection first (preferred)
    if let Some(warning) = get_header_based_early_warning(headers, fallback_available) {
        return Some(warning);
    }

    // Fallback: time-relative thresholds (client-side)
    for config in &early_warning_configs() {
        if let Some(warning) =
            get_time_relative_early_warning(headers, config, fallback_available)
        {
            return Some(warning);
        }
    }

    None
}

/// Compute new limits from API response headers.
fn compute_new_limits_from_headers(headers: &dyn HeaderMap) -> ClaudeAiLimits {
    let status = headers
        .get_header("anthropic-ratelimit-unified-status")
        .map(|s| QuotaStatus::from_header(&s))
        .unwrap_or(QuotaStatus::Allowed);

    let resets_at = headers
        .get_header("anthropic-ratelimit-unified-reset")
        .and_then(|s| s.parse::<f64>().ok());

    let fallback_available = headers
        .get_header("anthropic-ratelimit-unified-fallback")
        .map(|s| s == "available")
        .unwrap_or(false);

    let rate_limit_type = headers
        .get_header("anthropic-ratelimit-unified-representative-claim")
        .and_then(|s| RateLimitType::from_header(&s));

    let overage_status = headers
        .get_header("anthropic-ratelimit-unified-overage-status")
        .map(|s| QuotaStatus::from_header(&s));

    let overage_resets_at = headers
        .get_header("anthropic-ratelimit-unified-overage-reset")
        .and_then(|s| s.parse::<f64>().ok());

    let overage_disabled_reason = headers
        .get_header("anthropic-ratelimit-unified-overage-disabled-reason")
        .map(|s| OverageDisabledReason::from_header(&s));

    // Using overage when standard limits rejected but overage allowed
    let is_using_overage = status == QuotaStatus::Rejected
        && matches!(
            overage_status,
            Some(QuotaStatus::Allowed) | Some(QuotaStatus::AllowedWarning)
        );

    // Check for early warning when status is allowed or allowed_warning
    if status == QuotaStatus::Allowed || status == QuotaStatus::AllowedWarning {
        if let Some(early_warning) = get_early_warning_from_headers(headers, fallback_available) {
            return early_warning;
        }
        // No early warning -- return as plain allowed
        return ClaudeAiLimits {
            status: QuotaStatus::Allowed,
            resets_at,
            unified_rate_limit_fallback_available: fallback_available,
            rate_limit_type,
            overage_status,
            overage_resets_at,
            overage_disabled_reason,
            is_using_overage,
            ..Default::default()
        };
    }

    ClaudeAiLimits {
        status,
        resets_at,
        unified_rate_limit_fallback_available: fallback_available,
        rate_limit_type,
        overage_status,
        overage_resets_at,
        overage_disabled_reason,
        is_using_overage,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Status change listener
// ---------------------------------------------------------------------------

/// Listener callback invoked when rate-limit status changes.
pub type StatusChangeListener = Box<dyn Fn(&ClaudeAiLimits) + Send + Sync>;

// ---------------------------------------------------------------------------
// RateLimitTracker
// ---------------------------------------------------------------------------

/// Thread-safe rate-limit tracking for a session.
///
/// Processes API response headers and maintains current quota state.
/// Supports listener registration for reactive UI updates.
#[derive(Clone)]
pub struct RateLimitTracker {
    inner: Arc<Mutex<RateLimitTrackerInner>>,
}

struct RateLimitTrackerInner {
    current_limits: ClaudeAiLimits,
    raw_utilization: RawUtilization,
    listeners: Vec<Arc<StatusChangeListener>>,
    /// Whether the user is a Claude.ai subscriber (determines if we process limits).
    is_subscriber: bool,
    /// Mock state (if active).
    mock: Option<MockRateLimits>,
}

impl RateLimitTracker {
    /// Create a new tracker.
    pub fn new(is_subscriber: bool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RateLimitTrackerInner {
                current_limits: ClaudeAiLimits::default(),
                raw_utilization: RawUtilization::default(),
                listeners: Vec::new(),
                is_subscriber,
                mock: None,
            })),
        }
    }

    /// Get the current rate-limit state.
    pub fn current_limits(&self) -> ClaudeAiLimits {
        self.inner
            .lock()
            .map(|i| i.current_limits.clone())
            .unwrap_or_default()
    }

    /// Get raw per-window utilization (for status line display).
    pub fn raw_utilization(&self) -> RawUtilization {
        self.inner
            .lock()
            .map(|i| i.raw_utilization.clone())
            .unwrap_or_default()
    }

    /// Register a status-change listener.
    pub fn on_status_change(&self, listener: StatusChangeListener) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.listeners.push(Arc::new(listener));
        }
    }

    /// Update subscriber status (e.g. after login).
    pub fn set_subscriber(&self, is_subscriber: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.is_subscriber = is_subscriber;
        }
    }

    /// Whether rate limits should be processed (subscriber or mock active).
    pub fn should_process(&self) -> bool {
        self.inner
            .lock()
            .map(|i| i.is_subscriber || i.mock.is_some())
            .unwrap_or(false)
    }

    /// Extract and update quota status from API response headers.
    ///
    /// This is the primary entry point, called after every API response.
    pub fn extract_quota_status_from_headers(&self, headers: &dyn HeaderMap) {
        let mut inner = match self.inner.lock() {
            Ok(i) => i,
            Err(_) => return,
        };

        if !inner.is_subscriber && inner.mock.is_none() {
            // Not subscribed and no mock -- clear any stale state
            inner.raw_utilization = RawUtilization::default();
            if inner.current_limits.status != QuotaStatus::Allowed
                || inner.current_limits.resets_at.is_some()
            {
                let default = ClaudeAiLimits::default();
                inner.current_limits = default.clone();
                let listeners: Vec<_> = inner.listeners.iter().cloned().collect();
                drop(inner);
                for listener in listeners {
                    listener(&default);
                }
            }
            return;
        }

        // Build effective header map: copy known headers, overlay mock if active
        let effective = if let Some(ref mock) = inner.mock {
            let mut merged = SimpleHeaderMap::new();
            for key in KNOWN_RATE_LIMIT_HEADERS {
                if let Some(val) = headers.get_header(key) {
                    merged.set(key.to_string(), val);
                }
            }
            merged.merge(&mock.headers);
            merged
        } else {
            let mut snapshot = SimpleHeaderMap::new();
            for key in KNOWN_RATE_LIMIT_HEADERS {
                if let Some(val) = headers.get_header(key) {
                    snapshot.set(key.to_string(), val);
                }
            }
            snapshot
        };

        inner.raw_utilization = extract_raw_utilization(&effective);
        let new_limits = compute_new_limits_from_headers(&effective);

        if inner.current_limits != new_limits {
            inner.current_limits = new_limits.clone();
            let listeners: Vec<_> = inner.listeners.iter().cloned().collect();
            drop(inner);
            for listener in listeners {
                listener(&new_limits);
            }
        }
    }

    /// Extract and update quota status from a 429 API error.
    ///
    /// If headers are present on the error, they are processed. The status is
    /// always forced to `Rejected`.
    pub fn extract_quota_status_from_error(
        &self,
        status_code: u16,
        error_headers: Option<&dyn HeaderMap>,
    ) {
        if status_code != 429 {
            return;
        }

        let mut inner = match self.inner.lock() {
            Ok(i) => i,
            Err(_) => return,
        };

        if !inner.is_subscriber && inner.mock.is_none() {
            return;
        }

        let mut new_limits = inner.current_limits.clone();

        if let Some(headers) = error_headers {
            let effective = if let Some(ref mock) = inner.mock {
                let mut merged = SimpleHeaderMap::new();
                for key in KNOWN_RATE_LIMIT_HEADERS {
                    if let Some(val) = headers.get_header(key) {
                        merged.set(key.to_string(), val);
                    }
                }
                merged.merge(&mock.headers);
                merged
            } else {
                let mut snapshot = SimpleHeaderMap::new();
                for key in KNOWN_RATE_LIMIT_HEADERS {
                    if let Some(val) = headers.get_header(key) {
                        snapshot.set(key.to_string(), val);
                    }
                }
                snapshot
            };

            inner.raw_utilization = extract_raw_utilization(&effective);
            new_limits = compute_new_limits_from_headers(&effective);
        }

        // Always set to rejected for 429 errors
        new_limits.status = QuotaStatus::Rejected;

        if inner.current_limits != new_limits {
            inner.current_limits = new_limits.clone();
            let listeners: Vec<_> = inner.listeners.iter().cloned().collect();
            drop(inner);
            for listener in listeners {
                listener(&new_limits);
            }
        }
    }

    /// Set mock rate limits for testing.
    pub fn set_mock(&self, mock: Option<MockRateLimits>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.mock = mock;
        }
    }

    /// Get a reference to the current mock state (if any).
    pub fn mock_state(&self) -> Option<MockRateLimits> {
        self.inner.lock().ok().and_then(|i| i.mock.clone())
    }

    /// Reset limits to default (e.g. on logout).
    pub fn reset(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.current_limits = ClaudeAiLimits::default();
            inner.raw_utilization = RawUtilization::default();
        }
    }
}

impl fmt::Debug for RateLimitTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimitTracker")
            .field("current_limits", &self.current_limits())
            .finish()
    }
}

/// Known rate-limit header names we may need to probe when building the merged
/// header map for mock testing.
const KNOWN_RATE_LIMIT_HEADERS: &[&str] = &[
    "anthropic-ratelimit-unified-status",
    "anthropic-ratelimit-unified-reset",
    "anthropic-ratelimit-unified-fallback",
    "anthropic-ratelimit-unified-fallback-percentage",
    "anthropic-ratelimit-unified-representative-claim",
    "anthropic-ratelimit-unified-overage-status",
    "anthropic-ratelimit-unified-overage-reset",
    "anthropic-ratelimit-unified-overage-disabled-reason",
    "anthropic-ratelimit-unified-5h-utilization",
    "anthropic-ratelimit-unified-5h-reset",
    "anthropic-ratelimit-unified-5h-surpassed-threshold",
    "anthropic-ratelimit-unified-7d-utilization",
    "anthropic-ratelimit-unified-7d-reset",
    "anthropic-ratelimit-unified-7d-surpassed-threshold",
    "anthropic-ratelimit-unified-overage-utilization",
    "anthropic-ratelimit-unified-overage-surpassed-threshold",
    "retry-after",
];

// ===========================================================================
// Rate limit messages
// ===========================================================================

/// Severity of a rate-limit message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RateLimitSeverity {
    Error,
    Warning,
}

/// A user-facing rate-limit message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateLimitMessage {
    pub message: String,
    pub severity: RateLimitSeverity,
}

/// All possible rate-limit error message prefixes.
/// Export for UI components that need to detect rate-limit messages.
pub const RATE_LIMIT_ERROR_PREFIXES: &[&str] = &[
    "You've hit your",
    "You've used",
    "You're now using extra usage",
    "You're close to",
    "You're out of extra usage",
];

/// Check if a message string is a rate-limit error message.
pub fn is_rate_limit_error_message(text: &str) -> bool {
    RATE_LIMIT_ERROR_PREFIXES
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

/// Format a Unix-epoch timestamp as a human-readable reset time.
///
/// Returns a relative description like "in 3 hours" or "in 5 days".
pub fn format_reset_time(resets_at: f64) -> String {
    let now = now_unix_seconds();
    let diff_seconds = (resets_at - now).max(0.0);

    let hours = (diff_seconds / 3600.0).floor() as u64;
    let days = hours / 24;
    let remaining_hours = hours % 24;

    if days > 0 {
        if remaining_hours > 0 {
            format!("in {days}d {remaining_hours}h")
        } else {
            format!("in {days}d")
        }
    } else if hours > 0 {
        format!("in {hours}h")
    } else {
        let minutes = (diff_seconds / 60.0).ceil() as u64;
        if minutes > 1 {
            format!("in {minutes}m")
        } else {
            "in ~1m".to_string()
        }
    }
}

/// Get the appropriate rate-limit message based on limit state.
///
/// Returns `None` if no message should be shown.
pub fn get_rate_limit_message(
    limits: &ClaudeAiLimits,
    model: &str,
    subscription_type: Option<&str>,
    has_extra_usage_enabled: bool,
    has_billing_access: bool,
) -> Option<RateLimitMessage> {
    // Overage scenarios first
    if limits.is_using_overage {
        if limits.overage_status == Some(QuotaStatus::AllowedWarning) {
            return Some(RateLimitMessage {
                message: "You're close to your extra usage spending limit".to_string(),
                severity: RateLimitSeverity::Warning,
            });
        }
        return None;
    }

    // Error states
    if limits.status == QuotaStatus::Rejected {
        return Some(RateLimitMessage {
            message: get_limit_reached_text(limits, model, subscription_type),
            severity: RateLimitSeverity::Error,
        });
    }

    // Warning states
    if limits.status == QuotaStatus::AllowedWarning {
        const WARNING_THRESHOLD: f64 = 0.7;
        if let Some(util) = limits.utilization {
            if util < WARNING_THRESHOLD {
                return None;
            }
        }

        // Don't warn non-billing Team/Enterprise users if overages are enabled
        let is_team_or_enterprise = matches!(subscription_type, Some("team") | Some("enterprise"));
        if is_team_or_enterprise && has_extra_usage_enabled && !has_billing_access {
            return None;
        }

        if let Some(text) = get_early_warning_text(limits) {
            return Some(RateLimitMessage {
                message: text,
                severity: RateLimitSeverity::Warning,
            });
        }
    }

    None
}

/// Get error message for API errors (only error severity, not warnings).
pub fn get_rate_limit_error_message(
    limits: &ClaudeAiLimits,
    model: &str,
    subscription_type: Option<&str>,
) -> Option<String> {
    let msg = get_rate_limit_message(limits, model, subscription_type, false, false)?;
    if msg.severity == RateLimitSeverity::Error {
        Some(msg.message)
    } else {
        None
    }
}

/// Get warning message for UI footer (only warning severity).
pub fn get_rate_limit_warning(
    limits: &ClaudeAiLimits,
    model: &str,
    subscription_type: Option<&str>,
    has_extra_usage_enabled: bool,
    has_billing_access: bool,
) -> Option<String> {
    let msg = get_rate_limit_message(
        limits,
        model,
        subscription_type,
        has_extra_usage_enabled,
        has_billing_access,
    )?;
    if msg.severity == RateLimitSeverity::Warning {
        Some(msg.message)
    } else {
        None
    }
}

/// Get overage transition notification text.
pub fn get_using_overage_text(
    limits: &ClaudeAiLimits,
    subscription_type: Option<&str>,
) -> String {
    let limit_name = match limits.rate_limit_type {
        Some(RateLimitType::FiveHour) => Some("session limit"),
        Some(RateLimitType::SevenDay) => Some("weekly limit"),
        Some(RateLimitType::SevenDayOpus) => Some("Opus limit"),
        Some(RateLimitType::SevenDaySonnet) => {
            let is_pro_or_enterprise =
                matches!(subscription_type, Some("pro") | Some("enterprise"));
            if is_pro_or_enterprise {
                Some("weekly limit")
            } else {
                Some("Sonnet limit")
            }
        }
        _ => None,
    };

    let Some(name) = limit_name else {
        return "Now using extra usage".to_string();
    };

    let reset_msg = limits
        .resets_at
        .map(|r| format!(" · Your {name} resets {}", format_reset_time(r)))
        .unwrap_or_default();

    format!("You're now using extra usage{reset_msg}")
}

fn get_limit_reached_text(
    limits: &ClaudeAiLimits,
    _model: &str,
    subscription_type: Option<&str>,
) -> String {
    let reset_time = limits.resets_at.map(format_reset_time);
    let overage_reset_time = limits.overage_resets_at.map(format_reset_time);
    let reset_message = reset_time
        .as_ref()
        .map(|t| format!(" · resets {t}"))
        .unwrap_or_default();

    // Both subscription and overage exhausted
    if limits.overage_status == Some(QuotaStatus::Rejected) {
        let overage_reset_message = match (&limits.resets_at, &limits.overage_resets_at) {
            (Some(sub), Some(ovr)) => {
                if sub < ovr {
                    reset_message.clone()
                } else {
                    overage_reset_time
                        .as_ref()
                        .map(|t| format!(" · resets {t}"))
                        .unwrap_or_default()
                }
            }
            _ => {
                if reset_time.is_some() {
                    reset_message.clone()
                } else {
                    overage_reset_time
                        .as_ref()
                        .map(|t| format!(" · resets {t}"))
                        .unwrap_or_default()
                }
            }
        };

        if limits.overage_disabled_reason == Some(OverageDisabledReason::OutOfCredits) {
            return format!("You're out of extra usage{overage_reset_message}");
        }

        return format!("You've hit your limit{overage_reset_message}");
    }

    match limits.rate_limit_type {
        Some(RateLimitType::SevenDaySonnet) => {
            let is_pro_or_enterprise =
                matches!(subscription_type, Some("pro") | Some("enterprise"));
            let limit = if is_pro_or_enterprise {
                "weekly limit"
            } else {
                "Sonnet limit"
            };
            format!("You've hit your {limit}{reset_message}")
        }
        Some(RateLimitType::SevenDayOpus) => {
            format!("You've hit your Opus limit{reset_message}")
        }
        Some(RateLimitType::SevenDay) => {
            format!("You've hit your weekly limit{reset_message}")
        }
        Some(RateLimitType::FiveHour) => {
            format!("You've hit your session limit{reset_message}")
        }
        _ => format!("You've hit your usage limit{reset_message}"),
    }
}

fn get_early_warning_text(limits: &ClaudeAiLimits) -> Option<String> {
    let limit_name = match limits.rate_limit_type {
        Some(RateLimitType::SevenDay) => "weekly limit",
        Some(RateLimitType::FiveHour) => "session limit",
        Some(RateLimitType::SevenDayOpus) => "Opus limit",
        Some(RateLimitType::SevenDaySonnet) => "Sonnet limit",
        Some(RateLimitType::Overage) => "extra usage",
        None => return None,
    };

    let used = limits
        .utilization
        .map(|u| (u * 100.0).floor() as u64);
    let reset_time = limits.resets_at.map(format_reset_time);

    match (used, &reset_time) {
        (Some(pct), Some(time)) => Some(format!(
            "You've used {pct}% of your {limit_name} · resets {time}"
        )),
        (Some(pct), None) => Some(format!("You've used {pct}% of your {limit_name}")),
        (None, Some(time)) => {
            let display_name = if limits.rate_limit_type == Some(RateLimitType::Overage) {
                "extra usage limit"
            } else {
                limit_name
            };
            Some(format!("Approaching {display_name} · resets {time}"))
        }
        (None, None) => {
            let display_name = if limits.rate_limit_type == Some(RateLimitType::Overage) {
                "extra usage limit"
            } else {
                limit_name
            };
            Some(format!("Approaching {display_name}"))
        }
    }
}

// ===========================================================================
// Mock rate limits
// ===========================================================================

/// Predefined mock scenarios for testing rate-limit behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MockScenario {
    Normal,
    SessionLimitReached,
    ApproachingWeeklyLimit,
    WeeklyLimitReached,
    OverageActive,
    OverageWarning,
    OverageExhausted,
    OutOfCredits,
    OrgZeroCreditLimit,
    OrgSpendCapHit,
    MemberZeroCreditLimit,
    SeatTierZeroCreditLimit,
    OpusLimit,
    OpusWarning,
    SonnetLimit,
    SonnetWarning,
    FastModeLimit,
    FastModeShortLimit,
    ExtraUsageRequired,
    Clear,
}

impl MockScenario {
    /// Human-readable description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Normal => "Normal usage, no limits",
            Self::SessionLimitReached => "Session rate limit exceeded",
            Self::ApproachingWeeklyLimit => "Approaching weekly aggregate limit",
            Self::WeeklyLimitReached => "Weekly aggregate limit exceeded",
            Self::OverageActive => "Using extra usage (overage active)",
            Self::OverageWarning => "Approaching extra usage limit",
            Self::OverageExhausted => "Both subscription and extra usage limits exhausted",
            Self::OutOfCredits => "Out of extra usage credits (wallet empty)",
            Self::OrgZeroCreditLimit => "Org spend cap is zero (no extra usage budget)",
            Self::OrgSpendCapHit => "Org spend cap hit for the month",
            Self::MemberZeroCreditLimit => "Member limit is zero (admin can allocate more)",
            Self::SeatTierZeroCreditLimit => "Seat tier limit is zero (admin can allocate more)",
            Self::OpusLimit => "Opus limit reached",
            Self::OpusWarning => "Approaching Opus limit",
            Self::SonnetLimit => "Sonnet limit reached",
            Self::SonnetWarning => "Approaching Sonnet limit",
            Self::FastModeLimit => "Fast mode rate limit",
            Self::FastModeShortLimit => "Fast mode rate limit (short)",
            Self::ExtraUsageRequired => "Headerless 429: Extra usage required for 1M context",
            Self::Clear => "Clear mock headers (use real limits)",
        }
    }
}

/// Exceeded limit tracking for mock scenarios.
#[derive(Clone, Debug)]
struct ExceededLimit {
    limit_type: RateLimitType,
    resets_at: f64,
}

/// Mock rate-limit state for testing.
///
/// Allows injecting synthetic headers and scenarios without hitting real limits.
#[derive(Clone, Debug)]
pub struct MockRateLimits {
    headers: SimpleHeaderMap,
    exceeded_limits: Vec<ExceededLimit>,
    headerless_429_message: Option<String>,
    fast_mode_rate_limit_duration_ms: Option<u64>,
    fast_mode_rate_limit_expires_at: Option<f64>,
}

impl MockRateLimits {
    /// Create a new empty mock state.
    pub fn new() -> Self {
        Self {
            headers: SimpleHeaderMap::new(),
            exceeded_limits: Vec::new(),
            headerless_429_message: None,
            fast_mode_rate_limit_duration_ms: None,
            fast_mode_rate_limit_expires_at: None,
        }
    }

    /// Set a mock header value. Pass `None` to remove.
    pub fn set_header(&mut self, key: &str, value: Option<&str>) {
        let full_key = if key == "retry-after" {
            "retry-after".to_string()
        } else {
            format!("anthropic-ratelimit-unified-{key}")
        };

        match value {
            None => {
                self.headers.remove(&full_key);
                if key == "claim" {
                    self.exceeded_limits.clear();
                }
                if key == "status" || key == "overage-status" {
                    self.update_retry_after();
                }
            }
            Some(val) => {
                let mut final_val = val.to_string();

                // Handle reset times as hours from now
                if key == "reset" || key == "overage-reset" {
                    if let Ok(hours) = val.parse::<f64>() {
                        final_val = format!("{}", (now_unix_seconds() + hours * 3600.0).floor());
                    }
                }

                // Handle claims -- add to exceeded limits
                if key == "claim" {
                    let valid_types = [
                        ("five_hour", RateLimitType::FiveHour),
                        ("seven_day", RateLimitType::SevenDay),
                        ("seven_day_opus", RateLimitType::SevenDayOpus),
                        ("seven_day_sonnet", RateLimitType::SevenDaySonnet),
                    ];

                    for (name, lt) in &valid_types {
                        if val == *name {
                            let resets_at = match lt {
                                RateLimitType::FiveHour => {
                                    now_unix_seconds() + 5.0 * 3600.0
                                }
                                _ => now_unix_seconds() + 7.0 * 24.0 * 3600.0,
                            };

                            self.exceeded_limits.retain(|l| l.limit_type != *lt);
                            self.exceeded_limits.push(ExceededLimit {
                                limit_type: *lt,
                                resets_at,
                            });
                            self.update_representative_claim();
                            return;
                        }
                    }
                }

                self.headers.set(full_key, final_val);

                if key == "status" || key == "overage-status" {
                    self.update_retry_after();
                }
            }
        }
    }

    /// Add an exceeded limit with a custom reset time.
    pub fn add_exceeded_limit(&mut self, limit_type: RateLimitType, hours_from_now: f64) {
        let resets_at = now_unix_seconds() + hours_from_now * 3600.0;
        self.exceeded_limits.retain(|l| l.limit_type != limit_type);
        self.exceeded_limits.push(ExceededLimit {
            limit_type,
            resets_at,
        });

        if !self.exceeded_limits.is_empty() {
            self.headers.set(
                "anthropic-ratelimit-unified-status",
                "rejected",
            );
        }
        self.update_representative_claim();
    }

    /// Set mock early warning utilization.
    pub fn set_early_warning(
        &mut self,
        claim_abbrev: &str, // "5h", "7d", or "overage"
        utilization: f64,
        hours_from_now: Option<f64>,
    ) {
        // Clear all early warning headers first
        self.clear_early_warning();

        let default_hours = if claim_abbrev == "5h" { 4.0 } else { 120.0 };
        let hours = hours_from_now.unwrap_or(default_hours);
        let resets_at = now_unix_seconds() + hours * 3600.0;

        self.headers.set(
            format!("anthropic-ratelimit-unified-{claim_abbrev}-utilization"),
            utilization.to_string(),
        );
        self.headers.set(
            format!("anthropic-ratelimit-unified-{claim_abbrev}-reset"),
            format!("{}", resets_at.floor()),
        );
        self.headers.set(
            format!("anthropic-ratelimit-unified-{claim_abbrev}-surpassed-threshold"),
            utilization.to_string(),
        );

        // Set status to allowed if not already set
        if self
            .headers
            .get_header("anthropic-ratelimit-unified-status")
            .is_none()
        {
            self.headers
                .set("anthropic-ratelimit-unified-status", "allowed");
        }
    }

    /// Clear mock early warning headers.
    pub fn clear_early_warning(&mut self) {
        for abbrev in &["5h", "7d"] {
            self.headers
                .remove(&format!("anthropic-ratelimit-unified-{abbrev}-utilization"));
            self.headers
                .remove(&format!("anthropic-ratelimit-unified-{abbrev}-reset"));
            self.headers.remove(&format!(
                "anthropic-ratelimit-unified-{abbrev}-surpassed-threshold"
            ));
        }
    }

    /// Apply a predefined mock scenario.
    pub fn set_scenario(&mut self, scenario: MockScenario) {
        if scenario == MockScenario::Clear {
            *self = Self::new();
            return;
        }

        let five_hours_from_now = now_unix_seconds() + 5.0 * 3600.0;
        let seven_days_from_now = now_unix_seconds() + 7.0 * 24.0 * 3600.0;

        // End-of-month timestamp
        let end_of_month = {
            let now = chrono::Utc::now();
            let next_month = if now.month() == 12 {
                chrono::NaiveDate::from_ymd_opt(now.year() + 1, 1, 1)
            } else {
                chrono::NaiveDate::from_ymd_opt(now.year(), now.month() + 1, 1)
            };
            next_month
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp() as f64)
                .unwrap_or(now_unix_seconds() + 30.0 * 24.0 * 3600.0)
        };

        // Clear existing state for most scenarios
        let preserve_exceeded = matches!(
            scenario,
            MockScenario::OverageActive | MockScenario::OverageWarning | MockScenario::OverageExhausted
        );
        self.headers = SimpleHeaderMap::new();
        self.headerless_429_message = None;
        if !preserve_exceeded {
            self.exceeded_limits.clear();
        }

        match scenario {
            MockScenario::Normal => {
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "allowed",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-reset",
                    format!("{}", five_hours_from_now.floor()),
                );
            }

            MockScenario::SessionLimitReached => {
                self.exceeded_limits = vec![ExceededLimit {
                    limit_type: RateLimitType::FiveHour,
                    resets_at: five_hours_from_now,
                }];
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
            }

            MockScenario::ApproachingWeeklyLimit => {
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "allowed_warning",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-reset",
                    format!("{}", seven_days_from_now.floor()),
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-representative-claim",
                    "seven_day",
                );
            }

            MockScenario::WeeklyLimitReached => {
                self.exceeded_limits = vec![ExceededLimit {
                    limit_type: RateLimitType::SevenDay,
                    resets_at: seven_days_from_now,
                }];
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
            }

            MockScenario::OverageActive => {
                if self.exceeded_limits.is_empty() {
                    self.exceeded_limits.push(ExceededLimit {
                        limit_type: RateLimitType::FiveHour,
                        resets_at: five_hours_from_now,
                    });
                }
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-status",
                    "allowed",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-reset",
                    format!("{}", end_of_month.floor()),
                );
            }

            MockScenario::OverageWarning => {
                if self.exceeded_limits.is_empty() {
                    self.exceeded_limits.push(ExceededLimit {
                        limit_type: RateLimitType::FiveHour,
                        resets_at: five_hours_from_now,
                    });
                }
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-status",
                    "allowed_warning",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-reset",
                    format!("{}", end_of_month.floor()),
                );
            }

            MockScenario::OverageExhausted => {
                if self.exceeded_limits.is_empty() {
                    self.exceeded_limits.push(ExceededLimit {
                        limit_type: RateLimitType::FiveHour,
                        resets_at: five_hours_from_now,
                    });
                }
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-reset",
                    format!("{}", end_of_month.floor()),
                );
            }

            MockScenario::OutOfCredits => {
                if self.exceeded_limits.is_empty() {
                    self.exceeded_limits.push(ExceededLimit {
                        limit_type: RateLimitType::FiveHour,
                        resets_at: five_hours_from_now,
                    });
                }
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-disabled-reason",
                    "out_of_credits",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-reset",
                    format!("{}", end_of_month.floor()),
                );
            }

            MockScenario::OrgZeroCreditLimit => {
                if self.exceeded_limits.is_empty() {
                    self.exceeded_limits.push(ExceededLimit {
                        limit_type: RateLimitType::FiveHour,
                        resets_at: five_hours_from_now,
                    });
                }
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-disabled-reason",
                    "org_service_zero_credit_limit",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-reset",
                    format!("{}", end_of_month.floor()),
                );
            }

            MockScenario::OrgSpendCapHit => {
                if self.exceeded_limits.is_empty() {
                    self.exceeded_limits.push(ExceededLimit {
                        limit_type: RateLimitType::FiveHour,
                        resets_at: five_hours_from_now,
                    });
                }
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-disabled-reason",
                    "org_level_disabled_until",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-reset",
                    format!("{}", end_of_month.floor()),
                );
            }

            MockScenario::MemberZeroCreditLimit => {
                if self.exceeded_limits.is_empty() {
                    self.exceeded_limits.push(ExceededLimit {
                        limit_type: RateLimitType::FiveHour,
                        resets_at: five_hours_from_now,
                    });
                }
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-disabled-reason",
                    "member_zero_credit_limit",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-reset",
                    format!("{}", end_of_month.floor()),
                );
            }

            MockScenario::SeatTierZeroCreditLimit => {
                if self.exceeded_limits.is_empty() {
                    self.exceeded_limits.push(ExceededLimit {
                        limit_type: RateLimitType::FiveHour,
                        resets_at: five_hours_from_now,
                    });
                }
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-status",
                    "rejected",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-disabled-reason",
                    "seat_tier_zero_credit_limit",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-overage-reset",
                    format!("{}", end_of_month.floor()),
                );
            }

            MockScenario::OpusLimit => {
                self.exceeded_limits = vec![ExceededLimit {
                    limit_type: RateLimitType::SevenDayOpus,
                    resets_at: seven_days_from_now,
                }];
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
            }

            MockScenario::OpusWarning => {
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "allowed_warning",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-reset",
                    format!("{}", seven_days_from_now.floor()),
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-representative-claim",
                    "seven_day_opus",
                );
            }

            MockScenario::SonnetLimit => {
                self.exceeded_limits = vec![ExceededLimit {
                    limit_type: RateLimitType::SevenDaySonnet,
                    resets_at: seven_days_from_now,
                }];
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
            }

            MockScenario::SonnetWarning => {
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "allowed_warning",
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-reset",
                    format!("{}", seven_days_from_now.floor()),
                );
                self.headers.set(
                    "anthropic-ratelimit-unified-representative-claim",
                    "seven_day_sonnet",
                );
            }

            MockScenario::FastModeLimit => {
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.fast_mode_rate_limit_duration_ms = Some(10 * 60 * 1000);
            }

            MockScenario::FastModeShortLimit => {
                self.update_representative_claim();
                self.headers.set(
                    "anthropic-ratelimit-unified-status",
                    "rejected",
                );
                self.fast_mode_rate_limit_duration_ms = Some(10 * 1000);
            }

            MockScenario::ExtraUsageRequired => {
                self.headerless_429_message =
                    Some("Extra usage is required for long context requests.".to_string());
            }

            MockScenario::Clear => unreachable!(),
        }
    }

    /// Get the headerless 429 message (if set).
    pub fn headerless_429_message(&self) -> Option<&str> {
        self.headerless_429_message.as_deref()
    }

    /// Whether mock headers are active.
    pub fn is_active(&self) -> bool {
        !self.headers.is_empty() || self.headerless_429_message.is_some()
    }

    /// Get a status description of current mock state.
    pub fn status_description(&self) -> String {
        if self.headers.is_empty() {
            return "No mock headers active (using real limits)".to_string();
        }

        let mut lines = vec!["Active mock headers:".to_string()];

        for (key, value) in self.headers.iter() {
            let formatted_key = key
                .replace("anthropic-ratelimit-unified-", "")
                .replace('-', " ");
            if key.contains("reset") {
                if let Ok(ts) = value.parse::<f64>() {
                    lines.push(format!(
                        "  {formatted_key}: {value} (resets {})",
                        format_reset_time(ts)
                    ));
                    continue;
                }
            }
            lines.push(format!("  {formatted_key}: {value}"));
        }

        if !self.exceeded_limits.is_empty() {
            lines.push(String::new());
            lines.push("Exceeded limits (contributing to representative claim):".to_string());
            for limit in &self.exceeded_limits {
                lines.push(format!(
                    "  {:?}: resets {}",
                    limit.limit_type,
                    format_reset_time(limit.resets_at)
                ));
            }
        }

        lines.join("\n")
    }

    /// Check fast-mode rate limit for mock scenarios.
    pub fn check_fast_mode_rate_limit(&mut self, is_fast_mode_active: bool) -> bool {
        let Some(duration_ms) = self.fast_mode_rate_limit_duration_ms else {
            return false;
        };

        if !is_fast_mode_active {
            return false;
        }

        // Check if the rate limit has expired
        if let Some(expires_at) = self.fast_mode_rate_limit_expires_at {
            let now_ms = now_unix_seconds() * 1000.0;
            if now_ms >= expires_at {
                self.fast_mode_rate_limit_duration_ms = None;
                self.fast_mode_rate_limit_expires_at = None;
                return false;
            }
        }

        // Set expiry on first error
        if self.fast_mode_rate_limit_expires_at.is_none() {
            self.fast_mode_rate_limit_expires_at =
                Some(now_unix_seconds() * 1000.0 + duration_ms as f64);
        }

        true
    }

    // -- Internal helpers -----------------------------------------------

    fn update_retry_after(&mut self) {
        let status = self
            .headers
            .get_header("anthropic-ratelimit-unified-status");
        let overage_status = self
            .headers
            .get_header("anthropic-ratelimit-unified-overage-status");
        let reset = self
            .headers
            .get_header("anthropic-ratelimit-unified-reset");

        if status.as_deref() == Some("rejected")
            && (overage_status.is_none() || overage_status.as_deref() == Some("rejected"))
        {
            if let Some(reset_str) = reset {
                if let Ok(reset_ts) = reset_str.parse::<f64>() {
                    let secs = (reset_ts - now_unix_seconds()).max(0.0).ceil() as u64;
                    self.headers.set("retry-after", secs.to_string());
                    return;
                }
            }
        }
        self.headers.remove("retry-after");
    }

    fn update_representative_claim(&mut self) {
        if self.exceeded_limits.is_empty() {
            self.headers
                .remove("anthropic-ratelimit-unified-representative-claim");
            self.headers
                .remove("anthropic-ratelimit-unified-reset");
            self.headers.remove("retry-after");
            return;
        }

        // Find the limit with the furthest reset time
        let Some(furthest) = self
            .exceeded_limits
            .iter()
            .max_by(|a, b| a.resets_at.partial_cmp(&b.resets_at).unwrap_or(std::cmp::Ordering::Equal))
        else {
            return; // empty after is_empty check above — unreachable
        };

        let claim_str = match furthest.limit_type {
            RateLimitType::FiveHour => "five_hour",
            RateLimitType::SevenDay => "seven_day",
            RateLimitType::SevenDayOpus => "seven_day_opus",
            RateLimitType::SevenDaySonnet => "seven_day_sonnet",
            RateLimitType::Overage => "overage",
        };

        self.headers.set(
            "anthropic-ratelimit-unified-representative-claim",
            claim_str,
        );
        self.headers.set(
            "anthropic-ratelimit-unified-reset",
            format!("{}", furthest.resets_at.floor()),
        );

        self.update_retry_after();
    }
}

impl Default for MockRateLimits {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Policy limits
// ===========================================================================

/// A single policy restriction from the API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyRestriction {
    pub allowed: bool,
}

/// Policy limits response from the API.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PolicyLimitsResponse {
    pub restrictions: HashMap<String, PolicyRestriction>,
}

/// Result of fetching policy limits.
#[derive(Clone, Debug)]
pub struct PolicyLimitsFetchResult {
    pub success: bool,
    /// `None` means 304 Not Modified (cache is valid).
    pub restrictions: Option<HashMap<String, PolicyRestriction>>,
    pub etag: Option<String>,
    pub error: Option<String>,
    pub skip_retry: bool,
}

/// Organizational policy limits manager.
///
/// Fetches restrictions from the API, caches them locally, and provides
/// synchronous `is_policy_allowed` checks. Follows fail-open semantics:
/// if the fetch fails and no cache exists, all policies are allowed.
#[derive(Clone)]
pub struct PolicyLimits {
    inner: Arc<Mutex<PolicyLimitsInner>>,
}

struct PolicyLimitsInner {
    session_cache: Option<HashMap<String, PolicyRestriction>>,
    is_eligible: bool,
    is_essential_traffic_only: bool,
}

/// Policies that default to denied when essential-traffic-only mode is active
/// and the policy cache is unavailable.
const ESSENTIAL_TRAFFIC_DENY_ON_MISS: &[&str] = &["allow_product_feedback"];

impl PolicyLimits {
    /// Create a new policy limits manager.
    pub fn new(is_eligible: bool, is_essential_traffic_only: bool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PolicyLimitsInner {
                session_cache: None,
                is_eligible,
                is_essential_traffic_only,
            })),
        }
    }

    /// Check if a specific policy is allowed.
    ///
    /// Returns `true` if the policy is unknown, unavailable, or explicitly allowed
    /// (fail open), except for policies in `ESSENTIAL_TRAFFIC_DENY_ON_MISS` when
    /// essential-traffic-only mode is active and the cache is unavailable.
    pub fn is_policy_allowed(&self, policy: &str) -> bool {
        let inner = match self.inner.lock() {
            Ok(i) => i,
            Err(_) => return true, // fail open on poisoned lock
        };

        if !inner.is_eligible {
            return true;
        }

        let Some(ref restrictions) = inner.session_cache else {
            // No cache available
            if inner.is_essential_traffic_only
                && ESSENTIAL_TRAFFIC_DENY_ON_MISS.contains(&policy)
            {
                return false;
            }
            return true; // fail open
        };

        match restrictions.get(policy) {
            Some(r) => r.allowed,
            None => true, // unknown policy = allowed
        }
    }

    /// Update the session cache with new restrictions.
    pub fn update_restrictions(&self, restrictions: HashMap<String, PolicyRestriction>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.session_cache = Some(restrictions);
        }
    }

    /// Clear the session cache.
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.session_cache = None;
        }
    }

    /// Whether any restrictions are cached.
    pub fn has_cached_restrictions(&self) -> bool {
        self.inner
            .lock()
            .map(|i| i.session_cache.is_some())
            .unwrap_or(false)
    }

    /// Get a clone of the current restrictions (for persistence).
    pub fn cached_restrictions(&self) -> Option<HashMap<String, PolicyRestriction>> {
        self.inner
            .lock()
            .ok()
            .and_then(|i| i.session_cache.clone())
    }

    /// Set eligibility (e.g. after auth state change).
    pub fn set_eligible(&self, eligible: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.is_eligible = eligible;
        }
    }

    /// Compute a SHA-256 checksum of the current restrictions for HTTP caching.
    pub fn compute_checksum(&self) -> Option<String> {
        let inner = self.inner.lock().ok()?;
        let restrictions = inner.session_cache.as_ref()?;

        // Sort keys for deterministic hashing
        let mut sorted: Vec<_> = restrictions.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());

        let json = serde_json::to_string(&sorted).ok()?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(json.as_bytes());
        let hash = hasher.finalize();
        Some(format!("sha256:{}", hex::encode(hash)))
    }
}

impl fmt::Debug for PolicyLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PolicyLimits")
            .field("has_cache", &self.has_cached_restrictions())
            .finish()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- QuotaStatus --

    #[test]
    fn test_quota_status_from_header() {
        assert_eq!(QuotaStatus::from_header("allowed"), QuotaStatus::Allowed);
        assert_eq!(
            QuotaStatus::from_header("allowed_warning"),
            QuotaStatus::AllowedWarning
        );
        assert_eq!(QuotaStatus::from_header("rejected"), QuotaStatus::Rejected);
        assert_eq!(QuotaStatus::from_header("unknown"), QuotaStatus::Allowed);
    }

    // -- RateLimitType --

    #[test]
    fn test_rate_limit_type_display_names() {
        assert_eq!(RateLimitType::FiveHour.display_name(), "session limit");
        assert_eq!(RateLimitType::SevenDay.display_name(), "weekly limit");
        assert_eq!(RateLimitType::SevenDayOpus.display_name(), "Opus limit");
        assert_eq!(RateLimitType::SevenDaySonnet.display_name(), "Sonnet limit");
        assert_eq!(RateLimitType::Overage.display_name(), "extra usage limit");
    }

    // -- Header extraction --

    #[test]
    fn test_extract_basic_allowed_status() {
        let mut headers = SimpleHeaderMap::new();
        headers.set("anthropic-ratelimit-unified-status", "allowed");

        let limits = compute_new_limits_from_headers(&headers);
        assert_eq!(limits.status, QuotaStatus::Allowed);
        assert!(!limits.is_using_overage);
    }

    #[test]
    fn test_extract_rejected_with_overage() {
        let mut headers = SimpleHeaderMap::new();
        headers.set("anthropic-ratelimit-unified-status", "rejected");
        headers.set("anthropic-ratelimit-unified-overage-status", "allowed");
        headers.set(
            "anthropic-ratelimit-unified-representative-claim",
            "five_hour",
        );
        headers.set("anthropic-ratelimit-unified-reset", "1700000000");

        let limits = compute_new_limits_from_headers(&headers);
        assert_eq!(limits.status, QuotaStatus::Rejected);
        assert!(limits.is_using_overage);
        assert_eq!(limits.rate_limit_type, Some(RateLimitType::FiveHour));
    }

    #[test]
    fn test_extract_raw_utilization() {
        let mut headers = SimpleHeaderMap::new();
        headers.set("anthropic-ratelimit-unified-5h-utilization", "0.75");
        headers.set("anthropic-ratelimit-unified-5h-reset", "1700000000");
        headers.set("anthropic-ratelimit-unified-7d-utilization", "0.3");
        headers.set("anthropic-ratelimit-unified-7d-reset", "1700500000");

        let raw = extract_raw_utilization(&headers);
        assert!(raw.five_hour.is_some());
        assert!((raw.five_hour.as_ref().unwrap().utilization - 0.75).abs() < f64::EPSILON);
        assert!(raw.seven_day.is_some());
        assert!((raw.seven_day.as_ref().unwrap().utilization - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_header_based_early_warning() {
        let mut headers = SimpleHeaderMap::new();
        headers.set("anthropic-ratelimit-unified-status", "allowed");
        headers.set("anthropic-ratelimit-unified-5h-surpassed-threshold", "0.9");
        headers.set("anthropic-ratelimit-unified-5h-utilization", "0.92");
        headers.set("anthropic-ratelimit-unified-5h-reset", "1700000000");

        let warning = get_header_based_early_warning(&headers, false);
        assert!(warning.is_some());
        let w = warning.unwrap();
        assert_eq!(w.status, QuotaStatus::AllowedWarning);
        assert_eq!(w.rate_limit_type, Some(RateLimitType::FiveHour));
        assert!((w.utilization.unwrap() - 0.92).abs() < f64::EPSILON);
    }

    // -- RateLimitTracker --

    #[test]
    fn test_tracker_default_state() {
        let tracker = RateLimitTracker::new(false);
        let limits = tracker.current_limits();
        assert_eq!(limits.status, QuotaStatus::Allowed);
        assert!(!limits.is_using_overage);
    }

    #[test]
    fn test_tracker_subscriber_processes_headers() {
        let tracker = RateLimitTracker::new(true);

        let mut headers = SimpleHeaderMap::new();
        headers.set("anthropic-ratelimit-unified-status", "rejected");
        headers.set(
            "anthropic-ratelimit-unified-representative-claim",
            "five_hour",
        );
        headers.set("anthropic-ratelimit-unified-reset", "1700000000");

        tracker.extract_quota_status_from_headers(&headers);

        let limits = tracker.current_limits();
        assert_eq!(limits.status, QuotaStatus::Rejected);
        assert_eq!(limits.rate_limit_type, Some(RateLimitType::FiveHour));
    }

    #[test]
    fn test_tracker_non_subscriber_ignores_headers() {
        let tracker = RateLimitTracker::new(false);

        let mut headers = SimpleHeaderMap::new();
        headers.set("anthropic-ratelimit-unified-status", "rejected");

        tracker.extract_quota_status_from_headers(&headers);

        let limits = tracker.current_limits();
        assert_eq!(limits.status, QuotaStatus::Allowed);
    }

    #[test]
    fn test_tracker_listener_fires() {
        let tracker = RateLimitTracker::new(true);
        let fired = Arc::new(Mutex::new(false));
        let fired_clone = fired.clone();

        tracker.on_status_change(Box::new(move |_limits| {
            *fired_clone.lock().unwrap() = true;
        }));

        let mut headers = SimpleHeaderMap::new();
        headers.set("anthropic-ratelimit-unified-status", "rejected");
        tracker.extract_quota_status_from_headers(&headers);

        assert!(*fired.lock().unwrap());
    }

    #[test]
    fn test_tracker_error_extraction() {
        let tracker = RateLimitTracker::new(true);

        let mut headers = SimpleHeaderMap::new();
        headers.set("anthropic-ratelimit-unified-status", "allowed");
        headers.set(
            "anthropic-ratelimit-unified-representative-claim",
            "seven_day",
        );

        tracker.extract_quota_status_from_error(429, Some(&headers));

        let limits = tracker.current_limits();
        // Status should be forced to rejected even though header says allowed
        assert_eq!(limits.status, QuotaStatus::Rejected);
    }

    #[test]
    fn test_tracker_non_429_ignored() {
        let tracker = RateLimitTracker::new(true);
        tracker.extract_quota_status_from_error(500, None);
        assert_eq!(tracker.current_limits().status, QuotaStatus::Allowed);
    }

    // -- Messages --

    #[test]
    fn test_rate_limit_message_rejected() {
        let limits = ClaudeAiLimits {
            status: QuotaStatus::Rejected,
            rate_limit_type: Some(RateLimitType::FiveHour),
            ..Default::default()
        };

        let msg = get_rate_limit_message(&limits, "claude-sonnet-4-6", None, false, false);
        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert_eq!(msg.severity, RateLimitSeverity::Error);
        assert!(msg.message.contains("session limit"));
    }

    #[test]
    fn test_rate_limit_message_warning() {
        let limits = ClaudeAiLimits {
            status: QuotaStatus::AllowedWarning,
            rate_limit_type: Some(RateLimitType::SevenDay),
            utilization: Some(0.85),
            resets_at: Some(now_unix_seconds() + 7.0 * 24.0 * 3600.0),
            ..Default::default()
        };

        let msg = get_rate_limit_message(&limits, "claude-sonnet-4-6", None, false, false);
        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert_eq!(msg.severity, RateLimitSeverity::Warning);
        assert!(msg.message.contains("85%"));
        assert!(msg.message.contains("weekly limit"));
    }

    #[test]
    fn test_rate_limit_message_low_utilization_suppressed() {
        let limits = ClaudeAiLimits {
            status: QuotaStatus::AllowedWarning,
            rate_limit_type: Some(RateLimitType::SevenDay),
            utilization: Some(0.5),
            ..Default::default()
        };

        let msg = get_rate_limit_message(&limits, "claude-sonnet-4-6", None, false, false);
        assert!(msg.is_none());
    }

    #[test]
    fn test_overage_using_text() {
        let limits = ClaudeAiLimits {
            rate_limit_type: Some(RateLimitType::FiveHour),
            resets_at: Some(now_unix_seconds() + 3600.0),
            ..Default::default()
        };

        let text = get_using_overage_text(&limits, None);
        assert!(text.starts_with("You're now using extra usage"));
        assert!(text.contains("session limit"));
    }

    #[test]
    fn test_is_rate_limit_error_message() {
        assert!(is_rate_limit_error_message(
            "You've hit your session limit"
        ));
        assert!(is_rate_limit_error_message("You've used 85% of your weekly limit"));
        assert!(!is_rate_limit_error_message("Some other message"));
    }

    // -- Mock rate limits --

    #[test]
    fn test_mock_scenario_session_limit() {
        let mut mock = MockRateLimits::new();
        mock.set_scenario(MockScenario::SessionLimitReached);
        assert!(mock.is_active());

        let status = mock
            .headers
            .get_header("anthropic-ratelimit-unified-status");
        assert_eq!(status.as_deref(), Some("rejected"));

        let claim = mock
            .headers
            .get_header("anthropic-ratelimit-unified-representative-claim");
        assert_eq!(claim.as_deref(), Some("five_hour"));
    }

    #[test]
    fn test_mock_scenario_overage() {
        let mut mock = MockRateLimits::new();
        mock.set_scenario(MockScenario::OverageActive);
        assert!(mock.is_active());

        let overage = mock
            .headers
            .get_header("anthropic-ratelimit-unified-overage-status");
        assert_eq!(overage.as_deref(), Some("allowed"));
    }

    #[test]
    fn test_mock_clear() {
        let mut mock = MockRateLimits::new();
        mock.set_scenario(MockScenario::SessionLimitReached);
        assert!(mock.is_active());

        mock.set_scenario(MockScenario::Clear);
        assert!(!mock.is_active());
    }

    #[test]
    fn test_mock_early_warning() {
        let mut mock = MockRateLimits::new();
        mock.set_early_warning("5h", 0.92, Some(4.0));
        assert!(mock.is_active());

        let threshold = mock
            .headers
            .get_header("anthropic-ratelimit-unified-5h-surpassed-threshold");
        assert!(threshold.is_some());
    }

    #[test]
    fn test_mock_add_exceeded_limit() {
        let mut mock = MockRateLimits::new();
        mock.add_exceeded_limit(RateLimitType::FiveHour, 5.0);
        mock.add_exceeded_limit(RateLimitType::SevenDay, 168.0);

        assert_eq!(mock.exceeded_limits.len(), 2);

        let claim = mock
            .headers
            .get_header("anthropic-ratelimit-unified-representative-claim");
        // Seven day has the furthest reset time
        assert_eq!(claim.as_deref(), Some("seven_day"));
    }

    #[test]
    fn test_mock_with_tracker() {
        let tracker = RateLimitTracker::new(true);

        let mut mock = MockRateLimits::new();
        mock.set_scenario(MockScenario::SessionLimitReached);
        tracker.set_mock(Some(mock));

        // Even with "allowed" real headers, mock overrides
        let mut headers = SimpleHeaderMap::new();
        headers.set("anthropic-ratelimit-unified-status", "allowed");
        tracker.extract_quota_status_from_headers(&headers);

        let limits = tracker.current_limits();
        assert_eq!(limits.status, QuotaStatus::Rejected);
    }

    // -- Policy limits --

    #[test]
    fn test_policy_allowed_by_default() {
        let policy = PolicyLimits::new(true, false);
        assert!(policy.is_policy_allowed("any_policy"));
    }

    #[test]
    fn test_policy_blocked() {
        let policy = PolicyLimits::new(true, false);
        let mut restrictions = HashMap::new();
        restrictions.insert(
            "allow_remote_sessions".to_string(),
            PolicyRestriction { allowed: false },
        );
        policy.update_restrictions(restrictions);

        assert!(!policy.is_policy_allowed("allow_remote_sessions"));
        assert!(policy.is_policy_allowed("unknown_policy"));
    }

    #[test]
    fn test_policy_essential_traffic_deny_on_miss() {
        let policy = PolicyLimits::new(true, true);
        // No cache -- essential traffic policies should be denied
        assert!(!policy.is_policy_allowed("allow_product_feedback"));
        // Non-essential policies still fail open
        assert!(policy.is_policy_allowed("other_policy"));
    }

    #[test]
    fn test_policy_ineligible_always_allowed() {
        let policy = PolicyLimits::new(false, false);
        let mut restrictions = HashMap::new();
        restrictions.insert(
            "some_policy".to_string(),
            PolicyRestriction { allowed: false },
        );
        policy.update_restrictions(restrictions);

        // Even with restrictions cached, ineligible users bypass them
        assert!(policy.is_policy_allowed("some_policy"));
    }

    #[test]
    fn test_policy_clear() {
        let policy = PolicyLimits::new(true, false);
        let mut restrictions = HashMap::new();
        restrictions.insert(
            "test_policy".to_string(),
            PolicyRestriction { allowed: false },
        );
        policy.update_restrictions(restrictions);
        assert!(!policy.is_policy_allowed("test_policy"));

        policy.clear();
        // After clear, fail open again
        assert!(policy.is_policy_allowed("test_policy"));
    }

    // -- Format reset time --

    #[test]
    fn test_format_reset_time_hours() {
        let resets_at = now_unix_seconds() + 3.0 * 3600.0;
        let text = format_reset_time(resets_at);
        assert!(text.starts_with("in "));
        assert!(text.contains('h'));
    }

    #[test]
    fn test_format_reset_time_days() {
        let resets_at = now_unix_seconds() + 3.0 * 24.0 * 3600.0;
        let text = format_reset_time(resets_at);
        assert!(text.contains('d'));
    }

    #[test]
    fn test_format_reset_time_minutes() {
        let resets_at = now_unix_seconds() + 300.0;
        let text = format_reset_time(resets_at);
        assert!(text.contains('m'));
    }
}
