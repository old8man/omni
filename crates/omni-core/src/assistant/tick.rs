use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// A tick message sent to the engine to keep the assistant alive.
#[derive(Debug, Clone)]
pub struct TickMessage {
    /// The user's local time when the tick was generated.
    pub local_time: String,
    /// Monotonic tick sequence number within this session.
    pub sequence: u64,
}

impl TickMessage {
    /// Format this tick as the XML tag the model expects.
    pub fn to_xml(&self) -> String {
        format!(
            "<tick timestamp=\"{}\" tick_number=\"{}\"/>",
            self.local_time, self.sequence
        )
    }
}

/// Periodic tick scheduler that sends tick prompts to the engine at a
/// configurable interval. The engine treats each tick as a "you're awake,
/// what now?" prompt.
pub struct TickScheduler {
    interval: Duration,
    cancel: CancellationToken,
    sender: mpsc::Sender<TickMessage>,
}

impl TickScheduler {
    /// Create a new tick scheduler.
    ///
    /// `interval` controls the time between ticks. `sender` receives tick
    /// messages that the caller forwards to the engine as user-turn prompts.
    pub fn new(interval: Duration, sender: mpsc::Sender<TickMessage>) -> Self {
        Self {
            interval,
            cancel: CancellationToken::new(),
            sender,
        }
    }

    /// Update the tick interval. Takes effect on the next sleep cycle.
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
        debug!(interval_ms = interval.as_millis(), "tick interval updated");
    }

    /// Get the current tick interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Start the tick loop. Returns a handle that runs until the cancellation
    /// token is triggered or the receiver is dropped.
    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let cancel = self.cancel.clone();
        let sender = self.sender.clone();

        tokio::spawn(async move {
            let mut sequence: u64 = 0;
            info!(
                interval_ms = self.interval.as_millis(),
                "tick scheduler started"
            );

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("tick scheduler stopped");
                        break;
                    }
                    _ = tokio::time::sleep(self.interval) => {
                        sequence += 1;
                        let tick = TickMessage {
                            local_time: Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string(),
                            sequence,
                        };
                        debug!(seq = sequence, time = %tick.local_time, "sending tick");
                        if sender.send(tick).await.is_err() {
                            info!("tick receiver dropped, stopping scheduler");
                            break;
                        }
                    }
                }
            }
        })
    }

    /// Stop the tick scheduler.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Get a clone of the cancellation token.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tick_message_xml() {
        let tick = TickMessage {
            local_time: "2026-04-01 14:30:00 PDT".to_string(),
            sequence: 42,
        };
        assert_eq!(
            tick.to_xml(),
            "<tick timestamp=\"2026-04-01 14:30:00 PDT\" tick_number=\"42\"/>"
        );
    }
}
