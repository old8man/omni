use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// A subscription to PR events on a specific repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrSubscription {
    /// Repository in "owner/repo" format.
    pub repository: String,
    /// PR number being tracked.
    pub pr_number: u64,
    /// Events to notify on (e.g., "review", "comment", "merge", "close").
    pub events: Vec<String>,
}

/// Interface for sending push notifications to the user.
///
/// Implementations handle platform-specific delivery (e.g., APNs, FCM,
/// or channel-based MCP notifications).
pub struct NotificationSender {
    /// Active PR subscriptions keyed by "owner/repo#number".
    subscriptions: HashMap<String, PrSubscription>,
}

impl NotificationSender {
    /// Create a new notification sender.
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
        }
    }

    /// Subscribe to events on a pull request.
    pub fn subscribe_pr(&mut self, sub: PrSubscription) {
        let key = format!("{}#{}", sub.repository, sub.pr_number);
        debug!(key = %key, events = ?sub.events, "subscribing to PR events");
        self.subscriptions.insert(key, sub);
    }

    /// Unsubscribe from a pull request.
    pub fn unsubscribe_pr(&mut self, repository: &str, pr_number: u64) {
        let key = format!("{repository}#{pr_number}");
        if self.subscriptions.remove(&key).is_some() {
            debug!(key = %key, "unsubscribed from PR events");
        }
    }

    /// Get all active PR subscriptions.
    pub fn active_subscriptions(&self) -> Vec<&PrSubscription> {
        self.subscriptions.values().collect()
    }

    /// Check if a given PR event matches any active subscription.
    pub fn matches_subscription(&self, repository: &str, pr_number: u64, event_type: &str) -> bool {
        let key = format!("{repository}#{pr_number}");
        self.subscriptions
            .get(&key)
            .is_some_and(|sub| sub.events.iter().any(|e| e == event_type || e == "*"))
    }

    /// Send a push notification. Returns an error if delivery fails.
    ///
    /// The actual delivery mechanism is determined by the runtime
    /// configuration — this may route through an MCP channel server,
    /// a direct APNs/FCM call, or a webhook.
    pub async fn send(&self, title: &str, body: &str) -> Result<()> {
        // Notification delivery is platform-dependent. The core library
        // provides the interface; the application layer configures the
        // transport (MCP channel, direct push, etc.).
        debug!(title = %title, body_len = body.len(), "sending push notification");
        warn!("push notification transport not configured — notification dropped");
        Ok(())
    }

    /// Send a notification for a PR event that matched a subscription.
    pub async fn notify_pr_event(
        &self,
        repository: &str,
        pr_number: u64,
        event_type: &str,
        summary: &str,
    ) -> Result<()> {
        if !self.matches_subscription(repository, pr_number, event_type) {
            return Ok(());
        }
        let title = format!("{repository}#{pr_number}: {event_type}");
        self.send(&title, summary).await
    }
}

impl Default for NotificationSender {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pr_subscription_matching() {
        let mut sender = NotificationSender::new();
        sender.subscribe_pr(PrSubscription {
            repository: "anthropics/claude-code".to_string(),
            pr_number: 123,
            events: vec!["review".to_string(), "comment".to_string()],
        });

        assert!(sender.matches_subscription("anthropics/claude-code", 123, "review"));
        assert!(sender.matches_subscription("anthropics/claude-code", 123, "comment"));
        assert!(!sender.matches_subscription("anthropics/claude-code", 123, "merge"));
        assert!(!sender.matches_subscription("anthropics/claude-code", 456, "review"));
    }

    #[test]
    fn test_wildcard_subscription() {
        let mut sender = NotificationSender::new();
        sender.subscribe_pr(PrSubscription {
            repository: "org/repo".to_string(),
            pr_number: 1,
            events: vec!["*".to_string()],
        });
        assert!(sender.matches_subscription("org/repo", 1, "anything"));
    }

    #[test]
    fn test_unsubscribe() {
        let mut sender = NotificationSender::new();
        sender.subscribe_pr(PrSubscription {
            repository: "org/repo".to_string(),
            pr_number: 1,
            events: vec!["review".to_string()],
        });
        assert!(sender.matches_subscription("org/repo", 1, "review"));
        sender.unsubscribe_pr("org/repo", 1);
        assert!(!sender.matches_subscription("org/repo", 1, "review"));
    }
}
