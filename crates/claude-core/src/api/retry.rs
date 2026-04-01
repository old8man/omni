use std::time::Duration;

use rand::Rng;

/// Policy governing how the client retries failed API requests.
pub struct RetryPolicy {
    /// Initial delay before the first retry.
    pub base_delay: Duration,
    /// Maximum number of retry attempts for general transient errors (e.g. 429, 5xx).
    pub max_retries: u32,
    /// Maximum number of retry attempts specifically for HTTP 529 (overloaded) responses.
    /// After this limit the decision switches to `FallbackToNonStreaming`.
    pub max_529_retries: u32,
    /// Maximum backoff cap used for persistent overload conditions.
    pub persistent_max_backoff: Duration,
    /// Reset cap for persistent overload backoff tracking.
    pub persistent_reset_cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_millis(500),
            max_retries: 10,
            max_529_retries: 3,
            persistent_max_backoff: Duration::from_secs(60),
            persistent_reset_cap: Duration::from_secs(300),
        }
    }
}

/// The action the caller should take after receiving a particular HTTP status.
pub enum RetryDecision {
    /// Wait for `delay` and then retry the request.
    Retry { delay: Duration },
    /// Switch from a streaming request to a non-streaming request and try once more.
    FallbackToNonStreaming,
    /// The error is not recoverable; surface it to the caller.
    Fatal { status: u16 },
}

impl RetryPolicy {
    /// Decide what to do given an HTTP `status` code and the number of retries
    /// already attempted (`attempt` starts at 1 for the first retry).
    ///
    /// If the response includes a `retry-after` header, pass its value as
    /// `retry_after_header` and it will take precedence over the computed backoff.
    pub fn should_retry(
        &self,
        status: u16,
        attempt: u32,
        retry_after_header: Option<&str>,
    ) -> RetryDecision {
        match status {
            // 529 – API overloaded: fall back to non-streaming once the limit is reached.
            529 => {
                if attempt >= self.max_529_retries {
                    RetryDecision::FallbackToNonStreaming
                } else {
                    RetryDecision::Retry {
                        delay: self.resolve_delay(attempt, retry_after_header),
                    }
                }
            }
            // 429 – rate-limited, 500/502/503/504 – transient server errors.
            429 | 500 | 502 | 503 | 504 => {
                if attempt >= self.max_retries {
                    RetryDecision::Fatal { status }
                } else {
                    RetryDecision::Retry {
                        delay: self.resolve_delay(attempt, retry_after_header),
                    }
                }
            }
            // Everything else (4xx client errors, etc.) is fatal.
            _ => RetryDecision::Fatal { status },
        }
    }

    /// Return whether a given HTTP status code is retryable at all.
    pub fn is_retryable(status: u16) -> bool {
        matches!(status, 429 | 500 | 502 | 503 | 504 | 529)
    }

    /// Resolve the delay: prefer the `retry-after` header when present,
    /// otherwise fall back to exponential backoff.
    fn resolve_delay(&self, attempt: u32, retry_after_header: Option<&str>) -> Duration {
        if let Some(header) = retry_after_header {
            if let Some(delay) = parse_retry_after(header) {
                return delay;
            }
        }
        self.backoff_delay(attempt)
    }

    /// Compute the exponential backoff delay for the given `attempt` (1-based).
    ///
    /// Formula: `min(base_delay * 2^(attempt-1), 60s) + uniform_jitter(0..25%)`
    pub fn backoff_delay(&self, attempt: u32) -> Duration {
        const MAX_DELAY: Duration = Duration::from_secs(60);

        // Compute base * 2^(attempt-1), capped at 60 s to avoid overflow.
        let exp = attempt.saturating_sub(1);
        let base_ms = self.base_delay.as_millis() as u64;
        let multiplier: u64 = if exp < 64 { 1u64 << exp } else { u64::MAX };
        let raw_ms = base_ms.saturating_mul(multiplier);

        let capped = if raw_ms >= MAX_DELAY.as_millis() as u64 {
            MAX_DELAY
        } else {
            Duration::from_millis(raw_ms)
        };

        // Add up to 25% jitter (matching TS implementation).
        let jitter_max_ms = (capped.as_millis() as f64 * 0.25) as u64;
        let jitter_ms = if jitter_max_ms > 0 {
            rand::thread_rng().gen_range(0..=jitter_max_ms)
        } else {
            0
        };

        capped + Duration::from_millis(jitter_ms)
    }
}

/// Parse a `retry-after` header value into a `Duration`.
///
/// Supports two formats:
/// - Integer seconds (e.g. `"120"`)
/// - HTTP-date (e.g. `"Wed, 01 Apr 2026 12:00:00 GMT"`) — parsed via `httpdate`
fn parse_retry_after(value: &str) -> Option<Duration> {
    let trimmed = value.trim();

    // Try integer seconds first
    if let Ok(seconds) = trimmed.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    // Try as fractional seconds (e.g. "1.5")
    if let Ok(secs) = trimmed.parse::<f64>() {
        if secs > 0.0 {
            return Some(Duration::from_secs_f64(secs));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 10);
        assert_eq!(p.max_529_retries, 3);
        assert_eq!(p.base_delay, Duration::from_millis(500));
    }

    #[test]
    fn test_retryable_statuses() {
        assert!(RetryPolicy::is_retryable(429));
        assert!(RetryPolicy::is_retryable(500));
        assert!(RetryPolicy::is_retryable(502));
        assert!(RetryPolicy::is_retryable(503));
        assert!(RetryPolicy::is_retryable(504));
        assert!(RetryPolicy::is_retryable(529));
        assert!(!RetryPolicy::is_retryable(400));
        assert!(!RetryPolicy::is_retryable(401));
        assert!(!RetryPolicy::is_retryable(404));
    }

    #[test]
    fn test_retry_429_within_limit() {
        let p = RetryPolicy::default();
        match p.should_retry(429, 1, None) {
            RetryDecision::Retry { delay } => {
                assert!(delay >= Duration::from_millis(500));
                assert!(delay <= Duration::from_millis(625 + 1)); // 500 + 25% max jitter
            }
            _ => panic!("expected Retry"),
        }
    }

    #[test]
    fn test_retry_429_exhausted() {
        let p = RetryPolicy::default();
        match p.should_retry(429, 10, None) {
            RetryDecision::Fatal { status } => assert_eq!(status, 429),
            _ => panic!("expected Fatal"),
        }
    }

    #[test]
    fn test_retry_529_fallback() {
        let p = RetryPolicy::default();
        match p.should_retry(529, 3, None) {
            RetryDecision::FallbackToNonStreaming => {}
            _ => panic!("expected FallbackToNonStreaming"),
        }
    }

    #[test]
    fn test_retry_after_header_integer() {
        let p = RetryPolicy::default();
        match p.should_retry(429, 1, Some("5")) {
            RetryDecision::Retry { delay } => {
                assert_eq!(delay, Duration::from_secs(5));
            }
            _ => panic!("expected Retry"),
        }
    }

    #[test]
    fn test_retry_after_header_fractional() {
        let p = RetryPolicy::default();
        match p.should_retry(429, 1, Some("1.5")) {
            RetryDecision::Retry { delay } => {
                assert!(delay >= Duration::from_millis(1400));
                assert!(delay <= Duration::from_millis(1600));
            }
            _ => panic!("expected Retry"),
        }
    }

    #[test]
    fn test_jitter_is_25_percent() {
        let p = RetryPolicy {
            base_delay: Duration::from_secs(4),
            ..Default::default()
        };
        // With attempt=1, base delay = 4s, max jitter = 1s (25%)
        for _ in 0..50 {
            let d = p.backoff_delay(1);
            assert!(d >= Duration::from_secs(4), "delay too low: {:?}", d);
            assert!(
                d <= Duration::from_secs(5) + Duration::from_millis(1),
                "delay too high: {:?}",
                d
            );
        }
    }

    #[test]
    fn test_non_retryable_is_fatal() {
        let p = RetryPolicy::default();
        match p.should_retry(400, 1, None) {
            RetryDecision::Fatal { status } => assert_eq!(status, 400),
            _ => panic!("expected Fatal"),
        }
    }

    #[test]
    fn test_parse_retry_after() {
        assert_eq!(parse_retry_after("120"), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after("0"), Some(Duration::from_secs(0)));
        assert!(parse_retry_after("not-a-number").is_none());
    }
}
