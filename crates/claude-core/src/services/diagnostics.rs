use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
// Diagnostic tracker — no external log dependencies needed.

/// A diagnostic event for tracking errors and notable occurrences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticEvent {
    /// Event category (e.g., "api_error", "tool_failure", "mcp_disconnect").
    pub category: String,

    /// Short description of what happened.
    pub message: String,

    /// When this event occurred (milliseconds since tracker creation).
    pub timestamp_ms: u64,

    /// Optional structured metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Aggregated statistics for a diagnostic category.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CategoryStats {
    /// Total event count.
    pub count: u64,

    /// Time of most recent event (milliseconds since tracker creation).
    pub last_seen_ms: u64,

    /// Most recent event message.
    pub last_message: String,
}

/// Tracks diagnostic events and errors during a session.
///
/// Thread-safe: all state is behind a Mutex. Intended as a lightweight
/// in-process event log for debugging and telemetry.
pub struct DiagnosticTracker {
    inner: Arc<Mutex<TrackerInner>>,
}

struct TrackerInner {
    start: Instant,
    events: Vec<DiagnosticEvent>,
    stats: HashMap<String, CategoryStats>,
    max_events: usize,
}

impl DiagnosticTracker {
    /// Create a new tracker that retains up to `max_events` in memory.
    pub fn new(max_events: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TrackerInner {
                start: Instant::now(),
                events: Vec::new(),
                stats: HashMap::new(),
                max_events,
            })),
        }
    }

    /// Record a diagnostic event.
    pub fn record(
        &self,
        category: impl Into<String>,
        message: impl Into<String>,
        metadata: HashMap<String, String>,
    ) {
        let category = category.into();
        let message = message.into();

        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        let timestamp_ms = inner.start.elapsed().as_millis() as u64;

        // Update category stats
        let stats = inner.stats.entry(category.clone()).or_default();
        stats.count += 1;
        stats.last_seen_ms = timestamp_ms;
        stats.last_message.clone_from(&message);

        // Store event (ring buffer)
        let event = DiagnosticEvent {
            category,
            message,
            timestamp_ms,
            metadata,
        };

        if inner.events.len() >= inner.max_events {
            inner.events.remove(0);
        }
        inner.events.push(event);
    }

    /// Record a simple event with no metadata.
    pub fn record_simple(&self, category: impl Into<String>, message: impl Into<String>) {
        self.record(category, message, HashMap::new());
    }

    /// Get all recorded events.
    pub fn events(&self) -> Vec<DiagnosticEvent> {
        self.inner
            .lock()
            .map(|inner| inner.events.clone())
            .unwrap_or_default()
    }

    /// Get aggregated stats per category.
    pub fn stats(&self) -> HashMap<String, CategoryStats> {
        self.inner
            .lock()
            .map(|inner| inner.stats.clone())
            .unwrap_or_default()
    }

    /// Get events for a specific category.
    pub fn events_for_category(&self, category: &str) -> Vec<DiagnosticEvent> {
        self.inner
            .lock()
            .map(|inner| {
                inner
                    .events
                    .iter()
                    .filter(|e| e.category == category)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get the total event count across all categories.
    pub fn total_count(&self) -> u64 {
        self.inner
            .lock()
            .map(|inner| inner.stats.values().map(|s| s.count).sum())
            .unwrap_or(0)
    }

    /// Get elapsed time since the tracker was created.
    pub fn elapsed(&self) -> Duration {
        self.inner
            .lock()
            .map(|inner| inner.start.elapsed())
            .unwrap_or_default()
    }

    /// Clear all recorded events and stats.
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.events.clear();
            inner.stats.clear();
        }
    }

    /// Generate a diagnostic summary string.
    pub fn summary(&self) -> String {
        let inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return "diagnostic tracker unavailable".to_string(),
        };

        let elapsed = inner.start.elapsed();
        let total: u64 = inner.stats.values().map(|s| s.count).sum();

        let mut lines = vec![format!(
            "Diagnostics: {} events in {:.1}s",
            total,
            elapsed.as_secs_f32()
        )];

        let mut categories: Vec<_> = inner.stats.iter().collect();
        categories.sort_by(|a, b| b.1.count.cmp(&a.1.count));

        for (category, stats) in categories.iter().take(10) {
            lines.push(format!(
                "  {}: {} (last: {})",
                category, stats.count, stats.last_message
            ));
        }

        lines.join("\n")
    }
}

impl Default for DiagnosticTracker {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl Clone for DiagnosticTracker {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_retrieve() {
        let tracker = DiagnosticTracker::new(100);
        tracker.record_simple("api_error", "connection timeout");
        tracker.record_simple("api_error", "rate limited");
        tracker.record_simple("tool_failure", "bash failed");

        assert_eq!(tracker.total_count(), 3);
        assert_eq!(tracker.events().len(), 3);

        let stats = tracker.stats();
        assert_eq!(stats["api_error"].count, 2);
        assert_eq!(stats["tool_failure"].count, 1);
    }

    #[test]
    fn test_ring_buffer() {
        let tracker = DiagnosticTracker::new(2);
        tracker.record_simple("cat", "a");
        tracker.record_simple("cat", "b");
        tracker.record_simple("cat", "c");

        let events = tracker.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].message, "b");
        assert_eq!(events[1].message, "c");
    }

    #[test]
    fn test_category_filter() {
        let tracker = DiagnosticTracker::new(100);
        tracker.record_simple("a", "1");
        tracker.record_simple("b", "2");
        tracker.record_simple("a", "3");

        let events = tracker.events_for_category("a");
        assert_eq!(events.len(), 2);
    }
}
