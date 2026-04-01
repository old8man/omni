//! Capacity wake system for bridge poll loops.
//!
//! Both the REPL bridge and the standalone bridge main loop need to sleep
//! while "at capacity" but wake early when either (a) the outer cancellation
//! fires (shutdown), or (b) capacity frees up (session done / transport lost).
//!
//! This module encapsulates the wake-controller + two-signal merger that both
//! poll loops use, avoiding duplication.

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Handle returned by [`CapacityWake::signal`] for a single sleep cycle.
///
/// The caller awaits `notified` and must call `cleanup` when the sleep
/// resolves normally (without cancellation) to remove internal listeners.
pub struct CapacitySignal {
    /// A [`Notify`] that fires when either the outer cancellation or the
    /// capacity-wake controller triggers.
    pub notify: std::sync::Arc<Notify>,
}

/// Shared capacity-wake primitive for bridge poll loops.
///
/// Calling [`wake`](CapacityWake::wake) aborts the current at-capacity sleep
/// so the poll loop immediately re-checks for new work.
pub struct CapacityWake {
    /// Fires when capacity frees up.
    wake_notify: std::sync::Arc<Notify>,
    /// Outer cancellation token (shutdown).
    cancel: CancellationToken,
}

impl CapacityWake {
    /// Create a new capacity wake primitive linked to the given cancellation token.
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            wake_notify: std::sync::Arc::new(Notify::new()),
            cancel,
        }
    }

    /// Wake the current at-capacity sleep so the poll loop immediately
    /// re-checks for new work. Safe to call multiple times.
    pub fn wake(&self) {
        self.wake_notify.notify_waiters();
    }

    /// Sleep until either the outer cancellation fires, the capacity wake
    /// fires, or the given duration elapses.
    ///
    /// Returns `true` if the sleep was interrupted (wake or cancel),
    /// `false` if the full duration elapsed.
    pub async fn sleep_interruptible(&self, duration: std::time::Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(duration) => false,
            _ = self.wake_notify.notified() => true,
            _ = self.cancel.cancelled() => true,
        }
    }

    /// Check whether the outer cancellation has been triggered.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Get a reference to the underlying cancellation token.
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_wake_interrupts_sleep() {
        let cancel = CancellationToken::new();
        let cw = CapacityWake::new(cancel);

        // Spawn a task that wakes after 10ms
        let wake_notify = cw.wake_notify.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            wake_notify.notify_waiters();
        });

        let interrupted = cw.sleep_interruptible(Duration::from_secs(60)).await;
        assert!(interrupted);
    }

    #[tokio::test]
    async fn test_cancel_interrupts_sleep() {
        let cancel = CancellationToken::new();
        let cw = CapacityWake::new(cancel.clone());

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });

        let interrupted = cw.sleep_interruptible(Duration::from_secs(60)).await;
        assert!(interrupted);
    }

    #[tokio::test]
    async fn test_timeout_completes_normally() {
        let cancel = CancellationToken::new();
        let cw = CapacityWake::new(cancel);

        let interrupted = cw.sleep_interruptible(Duration::from_millis(10)).await;
        assert!(!interrupted);
    }

    #[test]
    fn test_is_cancelled() {
        let cancel = CancellationToken::new();
        let cw = CapacityWake::new(cancel.clone());
        assert!(!cw.is_cancelled());
        cancel.cancel();
        assert!(cw.is_cancelled());
    }
}
