use claude_core::api::retry::*;
use std::time::Duration;

#[test]
fn test_retry_policy_defaults() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.base_delay, Duration::from_millis(1000));
    assert_eq!(policy.max_retries, 3);
    assert_eq!(policy.max_529_retries, 2);
}

#[test]
fn test_should_retry_429() {
    // 429 is NOT retried (matches "stop retrying on 429" behavior)
    let policy = RetryPolicy::default();
    let decision = policy.should_retry(429, 1, None);
    assert!(matches!(decision, RetryDecision::Fatal { status: 429 }));
}

#[test]
fn test_should_retry_529_within_limit() {
    let policy = RetryPolicy::default();
    let decision = policy.should_retry(529, 1, None);
    assert!(matches!(decision, RetryDecision::Retry { .. }));
}

#[test]
fn test_should_fallback_529_exhausted() {
    let policy = RetryPolicy::default();
    let decision = policy.should_retry(529, 3, None);
    assert!(matches!(decision, RetryDecision::FallbackToNonStreaming));
}

#[test]
fn test_fatal_400() {
    let policy = RetryPolicy::default();
    let decision = policy.should_retry(400, 1, None);
    assert!(matches!(decision, RetryDecision::Fatal { .. }));
}

#[test]
fn test_fatal_403() {
    let policy = RetryPolicy::default();
    let decision = policy.should_retry(403, 1, None);
    assert!(matches!(decision, RetryDecision::Fatal { .. }));
}

#[test]
fn test_backoff_exponential() {
    let policy = RetryPolicy::default();
    let d1 = policy.backoff_delay(1);
    let d2 = policy.backoff_delay(2);
    let d3 = policy.backoff_delay(3);
    // With 25% jitter: base * 2^(attempt-1) * [1.0, 1.25]
    // base is 1000ms now
    assert!(d1 >= Duration::from_millis(1000) && d1 <= Duration::from_millis(1250 + 10));
    assert!(d2 >= Duration::from_millis(2000) && d2 <= Duration::from_millis(2500 + 10));
    assert!(d3 >= Duration::from_millis(4000) && d3 <= Duration::from_millis(5000 + 10));
}

#[test]
fn test_backoff_caps_at_60s() {
    let policy = RetryPolicy::default();
    let d = policy.backoff_delay(20);
    assert!(d <= Duration::from_secs(75 + 1)); // 60s + 25% jitter
}
