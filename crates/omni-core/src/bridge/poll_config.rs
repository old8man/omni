//! Bridge polling configuration.
//!
//! Defines the poll interval parameters used by both the standalone bridge
//! (bridgeMain) and the REPL bridge. Defaults are provided here and can
//! be overridden via runtime configuration if needed.

use std::time::Duration;

/// Poll interval when actively seeking work (no transport / below max_sessions).
/// Governs user-visible "connecting..." latency on initial work pickup and
/// recovery speed after the server re-dispatches a work item.
const POLL_INTERVAL_MS_NOT_AT_CAPACITY: u64 = 2_000;

/// Poll interval when the transport is connected. Runs independently of
/// heartbeat. 10 minutes gives 24x headroom on the Redis TTL while still
/// picking up server-initiated token-rotation redispatches within one poll
/// cycle.
const POLL_INTERVAL_MS_AT_CAPACITY: u64 = 600_000;

/// Multisession bridge poll intervals default to the single-session values.
const MULTISESSION_POLL_INTERVAL_MS_NOT_AT_CAPACITY: u64 = POLL_INTERVAL_MS_NOT_AT_CAPACITY;
const MULTISESSION_POLL_INTERVAL_MS_PARTIAL_CAPACITY: u64 = POLL_INTERVAL_MS_NOT_AT_CAPACITY;
const MULTISESSION_POLL_INTERVAL_MS_AT_CAPACITY: u64 = POLL_INTERVAL_MS_AT_CAPACITY;

/// Default reclaim_older_than_ms for the poll query parameter.
/// Matches the server's DEFAULT_RECLAIM_OLDER_THAN_MS (work_service.py:24).
const DEFAULT_RECLAIM_OLDER_THAN_MS: u64 = 5_000;

/// Default keepalive interval for session-ingress (2 minutes).
const DEFAULT_SESSION_KEEPALIVE_INTERVAL_V2_MS: u64 = 120_000;

/// Poll interval configuration for the bridge.
///
/// All values are in milliseconds. The validation constraints from the TS
/// implementation are enforced at construction time.
#[derive(Clone, Debug)]
pub struct PollIntervalConfig {
    /// Poll interval when actively seeking work (below capacity).
    pub poll_interval_ms_not_at_capacity: u64,
    /// Poll interval when at capacity. 0 = disabled (heartbeat-only mode).
    pub poll_interval_ms_at_capacity: u64,
    /// Non-exclusive heartbeat interval. 0 = disabled; when > 0, at-capacity
    /// loops send per-work-item heartbeats at this interval. Independent of
    /// poll_interval_ms_at_capacity -- both may run.
    pub non_exclusive_heartbeat_interval_ms: u64,
    /// Multisession poll interval when not at capacity.
    pub multisession_poll_interval_ms_not_at_capacity: u64,
    /// Multisession poll interval when at partial capacity.
    pub multisession_poll_interval_ms_partial_capacity: u64,
    /// Multisession poll interval when at full capacity. 0 = disabled.
    pub multisession_poll_interval_ms_at_capacity: u64,
    /// Reclaim unacknowledged work items older than this (ms).
    pub reclaim_older_than_ms: u64,
    /// Keepalive interval for session-ingress push frames (ms). 0 = disabled.
    pub session_keepalive_interval_v2_ms: u64,
}

impl Default for PollIntervalConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms_not_at_capacity: POLL_INTERVAL_MS_NOT_AT_CAPACITY,
            poll_interval_ms_at_capacity: POLL_INTERVAL_MS_AT_CAPACITY,
            non_exclusive_heartbeat_interval_ms: 0,
            multisession_poll_interval_ms_not_at_capacity:
                MULTISESSION_POLL_INTERVAL_MS_NOT_AT_CAPACITY,
            multisession_poll_interval_ms_partial_capacity:
                MULTISESSION_POLL_INTERVAL_MS_PARTIAL_CAPACITY,
            multisession_poll_interval_ms_at_capacity:
                MULTISESSION_POLL_INTERVAL_MS_AT_CAPACITY,
            reclaim_older_than_ms: DEFAULT_RECLAIM_OLDER_THAN_MS,
            session_keepalive_interval_v2_ms: DEFAULT_SESSION_KEEPALIVE_INTERVAL_V2_MS,
        }
    }
}

impl PollIntervalConfig {
    /// Validate the configuration, returning an error if constraints are violated.
    ///
    /// Constraints:
    /// - `poll_interval_ms_not_at_capacity` must be >= 100
    /// - `poll_interval_ms_at_capacity` must be 0 (disabled) or >= 100
    /// - At-capacity liveness requires at least one of `non_exclusive_heartbeat_interval_ms > 0`
    ///   or `poll_interval_ms_at_capacity > 0`
    /// - Same liveness constraint for multisession at-capacity
    pub fn validate(&self) -> Result<(), String> {
        if self.poll_interval_ms_not_at_capacity < 100 {
            return Err("poll_interval_ms_not_at_capacity must be >= 100".into());
        }
        if self.poll_interval_ms_at_capacity > 0 && self.poll_interval_ms_at_capacity < 100 {
            return Err("poll_interval_ms_at_capacity must be 0 (disabled) or >= 100".into());
        }
        if self.multisession_poll_interval_ms_not_at_capacity < 100 {
            return Err("multisession_poll_interval_ms_not_at_capacity must be >= 100".into());
        }
        if self.multisession_poll_interval_ms_at_capacity > 0
            && self.multisession_poll_interval_ms_at_capacity < 100
        {
            return Err(
                "multisession_poll_interval_ms_at_capacity must be 0 (disabled) or >= 100".into(),
            );
        }
        // At-capacity liveness: at least one mechanism must be enabled
        if self.non_exclusive_heartbeat_interval_ms == 0
            && self.poll_interval_ms_at_capacity == 0
        {
            return Err(
                "at-capacity liveness requires non_exclusive_heartbeat_interval_ms > 0 or poll_interval_ms_at_capacity > 0".into()
            );
        }
        if self.non_exclusive_heartbeat_interval_ms == 0
            && self.multisession_poll_interval_ms_at_capacity == 0
        {
            return Err(
                "at-capacity liveness requires non_exclusive_heartbeat_interval_ms > 0 or multisession_poll_interval_ms_at_capacity > 0".into()
            );
        }
        Ok(())
    }

    /// Get the poll interval as a [`Duration`] for the not-at-capacity state.
    pub fn not_at_capacity_interval(&self) -> Duration {
        Duration::from_millis(self.poll_interval_ms_not_at_capacity)
    }

    /// Get the poll interval as a [`Duration`] for the at-capacity state.
    /// Returns `None` if at-capacity polling is disabled (interval = 0).
    pub fn at_capacity_interval(&self) -> Option<Duration> {
        if self.poll_interval_ms_at_capacity == 0 {
            None
        } else {
            Some(Duration::from_millis(self.poll_interval_ms_at_capacity))
        }
    }

    /// Get the heartbeat interval as a [`Duration`].
    /// Returns `None` if heartbeats are disabled (interval = 0).
    pub fn heartbeat_interval(&self) -> Option<Duration> {
        if self.non_exclusive_heartbeat_interval_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(
                self.non_exclusive_heartbeat_interval_ms,
            ))
        }
    }

    /// Choose the correct poll interval for a multisession bridge given the
    /// current number of active sessions and the maximum.
    pub fn multisession_interval(&self, active: usize, max: usize) -> Duration {
        if active == 0 {
            Duration::from_millis(self.multisession_poll_interval_ms_not_at_capacity)
        } else if active < max {
            Duration::from_millis(self.multisession_poll_interval_ms_partial_capacity)
        } else {
            // At capacity -- may be 0 (disabled)
            Duration::from_millis(
                self.multisession_poll_interval_ms_at_capacity
                    .max(self.poll_interval_ms_not_at_capacity),
            )
        }
    }

    /// Get the reclaim_older_than_ms value for the poll query parameter.
    pub fn reclaim_older_than_ms(&self) -> u64 {
        self.reclaim_older_than_ms
    }

    /// Get the session keepalive interval as a [`Duration`].
    /// Returns `None` if keepalives are disabled (interval = 0).
    pub fn keepalive_interval(&self) -> Option<Duration> {
        if self.session_keepalive_interval_v2_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(self.session_keepalive_interval_v2_ms))
        }
    }

    /// Parse a [`PollIntervalConfig`] from a JSON value, falling back to
    /// defaults for missing or invalid fields.
    pub fn from_json(value: &serde_json::Value) -> Self {
        let default = Self::default();
        let obj = match value.as_object() {
            Some(o) => o,
            None => return default,
        };

        let get_u64 = |key: &str, fallback: u64| -> u64 {
            obj.get(key)
                .and_then(|v| v.as_u64())
                .unwrap_or(fallback)
        };

        let candidate = Self {
            poll_interval_ms_not_at_capacity: get_u64(
                "poll_interval_ms_not_at_capacity",
                default.poll_interval_ms_not_at_capacity,
            ),
            poll_interval_ms_at_capacity: get_u64(
                "poll_interval_ms_at_capacity",
                default.poll_interval_ms_at_capacity,
            ),
            non_exclusive_heartbeat_interval_ms: get_u64(
                "non_exclusive_heartbeat_interval_ms",
                default.non_exclusive_heartbeat_interval_ms,
            ),
            multisession_poll_interval_ms_not_at_capacity: get_u64(
                "multisession_poll_interval_ms_not_at_capacity",
                default.multisession_poll_interval_ms_not_at_capacity,
            ),
            multisession_poll_interval_ms_partial_capacity: get_u64(
                "multisession_poll_interval_ms_partial_capacity",
                default.multisession_poll_interval_ms_partial_capacity,
            ),
            multisession_poll_interval_ms_at_capacity: get_u64(
                "multisession_poll_interval_ms_at_capacity",
                default.multisession_poll_interval_ms_at_capacity,
            ),
            reclaim_older_than_ms: get_u64(
                "reclaim_older_than_ms",
                default.reclaim_older_than_ms,
            ),
            session_keepalive_interval_v2_ms: get_u64(
                "session_keepalive_interval_v2_ms",
                default.session_keepalive_interval_v2_ms,
            ),
        };

        if candidate.validate().is_ok() {
            candidate
        } else {
            default
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = PollIntervalConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_default_values() {
        let config = PollIntervalConfig::default();
        assert_eq!(config.poll_interval_ms_not_at_capacity, 2_000);
        assert_eq!(config.poll_interval_ms_at_capacity, 600_000);
        assert_eq!(config.non_exclusive_heartbeat_interval_ms, 0);
        assert_eq!(config.reclaim_older_than_ms, 5_000);
        assert_eq!(config.session_keepalive_interval_v2_ms, 120_000);
    }

    #[test]
    fn test_validate_too_small_not_at_capacity() {
        let mut config = PollIntervalConfig::default();
        config.poll_interval_ms_not_at_capacity = 50;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_at_capacity_zero_is_ok() {
        let mut config = PollIntervalConfig::default();
        config.poll_interval_ms_at_capacity = 0;
        // Need heartbeat enabled for liveness
        config.non_exclusive_heartbeat_interval_ms = 60_000;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_at_capacity_too_small() {
        let mut config = PollIntervalConfig::default();
        config.poll_interval_ms_at_capacity = 50;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_no_liveness_mechanism() {
        let mut config = PollIntervalConfig::default();
        config.poll_interval_ms_at_capacity = 0;
        config.non_exclusive_heartbeat_interval_ms = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_not_at_capacity_interval() {
        let config = PollIntervalConfig::default();
        assert_eq!(
            config.not_at_capacity_interval(),
            Duration::from_millis(2_000)
        );
    }

    #[test]
    fn test_at_capacity_interval_some() {
        let config = PollIntervalConfig::default();
        assert_eq!(
            config.at_capacity_interval(),
            Some(Duration::from_millis(600_000))
        );
    }

    #[test]
    fn test_at_capacity_interval_disabled() {
        let mut config = PollIntervalConfig::default();
        config.poll_interval_ms_at_capacity = 0;
        config.non_exclusive_heartbeat_interval_ms = 60_000;
        assert_eq!(config.at_capacity_interval(), None);
    }

    #[test]
    fn test_multisession_interval_no_active() {
        let config = PollIntervalConfig::default();
        assert_eq!(
            config.multisession_interval(0, 4),
            Duration::from_millis(2_000)
        );
    }

    #[test]
    fn test_multisession_interval_partial() {
        let config = PollIntervalConfig::default();
        assert_eq!(
            config.multisession_interval(2, 4),
            Duration::from_millis(2_000)
        );
    }

    #[test]
    fn test_multisession_interval_full() {
        let config = PollIntervalConfig::default();
        let interval = config.multisession_interval(4, 4);
        assert!(interval.as_millis() >= 2_000);
    }

    #[test]
    fn test_from_json_valid() {
        let json = serde_json::json!({
            "poll_interval_ms_not_at_capacity": 3000,
            "poll_interval_ms_at_capacity": 300000,
            "reclaim_older_than_ms": 10000
        });
        let config = PollIntervalConfig::from_json(&json);
        assert_eq!(config.poll_interval_ms_not_at_capacity, 3000);
        assert_eq!(config.poll_interval_ms_at_capacity, 300000);
        assert_eq!(config.reclaim_older_than_ms, 10000);
    }

    #[test]
    fn test_from_json_invalid_falls_back() {
        let json = serde_json::json!({
            "poll_interval_ms_not_at_capacity": 10,
            "poll_interval_ms_at_capacity": 0,
            "non_exclusive_heartbeat_interval_ms": 0
        });
        let config = PollIntervalConfig::from_json(&json);
        // Should fall back to defaults since validation fails
        assert_eq!(config.poll_interval_ms_not_at_capacity, 2_000);
    }

    #[test]
    fn test_from_json_not_object() {
        let json = serde_json::json!(42);
        let config = PollIntervalConfig::from_json(&json);
        assert_eq!(config.poll_interval_ms_not_at_capacity, 2_000);
    }
}
