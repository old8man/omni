//! Bridge main loop.
//!
//! Implements the core event loop for the bridge: register an environment,
//! poll for work, dispatch sessions, send heartbeats, and handle shutdown.
//! Uses exponential backoff for reconnection on transient failures.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::api::{BridgeApiClient, BridgeFatalError};
use super::jwt::decode_work_secret;
use super::types::{BackoffConfig, BridgeConfig, BridgeState, WorkResponse};

/// Polling interval when no work is available.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Heartbeat interval for active sessions.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Callbacks invoked by the bridge main loop.
pub trait BridgeCallbacks: Send + Sync {
    /// Called when the bridge successfully registers with the backend.
    fn on_registered(&self, environment_id: &str);

    /// Called when a new work item (session) is received.
    fn on_work_received(&self, work: &WorkResponse, session_token: &str, api_base_url: &str);

    /// Called when a session completes.
    fn on_session_done(&self, session_id: &str, status: &str);

    /// Called on state transitions (for UI updates).
    fn on_state_change(&self, state: BridgeState);

    /// Called on recoverable errors (logged, retried).
    fn on_error(&self, error: &str);

    /// Called on fatal errors (bridge shuts down).
    fn on_fatal_error(&self, error: &str);

    /// Called when successfully reconnected after a transient failure.
    fn on_reconnected(&self, disconnected_ms: u64);
}

/// Run the bridge main loop.
///
/// This function registers the bridge environment, then enters a poll loop
/// that dispatches work items to sessions. It handles transient failures
/// with exponential backoff and shuts down cleanly on cancellation or
/// fatal errors.
pub async fn bridge_main_loop(
    config: BridgeConfig,
    api_client: Arc<BridgeApiClient>,
    callbacks: Arc<dyn BridgeCallbacks>,
    cancel: CancellationToken,
) -> Result<()> {
    let conn_backoff = BackoffConfig::connection();
    let general_backoff = BackoffConfig::general();
    let shutdown_notify = Arc::new(Notify::new());

    callbacks.on_state_change(BridgeState::Connecting);

    // ── Registration with backoff ──────────────────────────────────────────
    let registration = {
        let mut attempt: u32 = 0;
        let start = Instant::now();

        loop {
            if cancel.is_cancelled() {
                callbacks.on_state_change(BridgeState::ShuttingDown);
                return Ok(());
            }

            match api_client.register_environment(&config).await {
                Ok(reg) => break reg,
                Err(e) => {
                    // Fatal errors (auth failures) should not be retried
                    if e.downcast_ref::<BridgeFatalError>().is_some() {
                        callbacks.on_fatal_error(&e.to_string());
                        return Err(e);
                    }

                    let elapsed = start.elapsed().as_millis() as u64;
                    if elapsed > conn_backoff.give_up_after_ms {
                        callbacks.on_fatal_error(&format!(
                            "Failed to register after {}ms: {e}",
                            elapsed
                        ));
                        return Err(e);
                    }

                    let delay = conn_backoff.delay_for_attempt(attempt);
                    callbacks.on_error(&format!(
                        "Registration failed (attempt {}), retrying in {}ms: {e}",
                        attempt + 1,
                        delay
                    ));

                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                        _ = cancel.cancelled() => {
                            callbacks.on_state_change(BridgeState::ShuttingDown);
                            return Ok(());
                        }
                    }
                    attempt += 1;
                }
            }
        }
    };

    let environment_id = registration.environment_id.clone();
    let environment_secret = registration.environment_secret.clone();

    callbacks.on_registered(&environment_id);
    callbacks.on_state_change(BridgeState::Connected);

    // ── Poll loop ──────────────────────────────────────────────────────────
    let mut poll_attempt: u32 = 0;
    let mut poll_failure_start: Option<Instant> = None;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Poll for work
        match api_client
            .poll_for_work(&environment_id, &environment_secret, None)
            .await
        {
            Ok(Some(work)) => {
                // Reset backoff on success
                if poll_failure_start.is_some() {
                    let disconnected_ms = poll_failure_start
                        .take()
                        .map(|s| s.elapsed().as_millis() as u64)
                        .unwrap_or(0);
                    callbacks.on_reconnected(disconnected_ms);
                }
                poll_attempt = 0;

                // Decode work secret and dispatch
                let work_id = work.id.clone();
                let session_id = work.data.id.clone();

                match decode_work_secret(&work.secret) {
                    Ok(secret) => {
                        // Acknowledge the work item
                        if let Err(e) = api_client
                            .acknowledge_work(
                                &environment_id,
                                &work_id,
                                &secret.session_ingress_token,
                            )
                            .await
                        {
                            callbacks
                                .on_error(&format!("Failed to acknowledge work {work_id}: {e}"));
                            continue;
                        }

                        callbacks.on_work_received(
                            &work,
                            &secret.session_ingress_token,
                            &secret.api_base_url,
                        );

                        // Spawn heartbeat task for this session
                        let hb_api = Arc::clone(&api_client);
                        let hb_env_id = environment_id.clone();
                        let hb_work_id = work_id.clone();
                        let hb_token = secret.session_ingress_token.clone();
                        let hb_cancel = cancel.clone();
                        let hb_callbacks = Arc::clone(&callbacks);

                        tokio::spawn(async move {
                            run_heartbeat_loop(
                                &hb_api,
                                &hb_env_id,
                                &hb_work_id,
                                &hb_token,
                                &hb_cancel,
                                hb_callbacks.as_ref(),
                            )
                            .await;
                        });
                    }
                    Err(e) => {
                        callbacks.on_error(&format!(
                            "Failed to decode work secret for {session_id}: {e}"
                        ));
                    }
                }
            }
            Ok(None) => {
                // No work available — reset backoff and wait
                if poll_failure_start.is_some() {
                    let disconnected_ms = poll_failure_start
                        .take()
                        .map(|s| s.elapsed().as_millis() as u64)
                        .unwrap_or(0);
                    callbacks.on_reconnected(disconnected_ms);
                }
                poll_attempt = 0;
            }
            Err(e) => {
                // Fatal errors should not be retried
                if e.downcast_ref::<BridgeFatalError>().is_some() {
                    callbacks.on_fatal_error(&e.to_string());
                    break;
                }

                if poll_failure_start.is_none() {
                    poll_failure_start = Some(Instant::now());
                }

                let elapsed = poll_failure_start
                    .as_ref()
                    .map(|s| s.elapsed().as_millis() as u64)
                    .unwrap_or(0);

                if elapsed > general_backoff.give_up_after_ms {
                    callbacks.on_fatal_error(&format!(
                        "Polling failed for {}ms, giving up: {e}",
                        elapsed
                    ));
                    break;
                }

                let delay = general_backoff.delay_for_attempt(poll_attempt);
                callbacks.on_error(&format!(
                    "Poll failed (attempt {}), retrying in {}ms: {e}",
                    poll_attempt + 1,
                    delay
                ));
                poll_attempt += 1;

                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                    _ = cancel.cancelled() => break,
                }
                continue;
            }
        }

        // Wait before next poll
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = cancel.cancelled() => break,
            _ = shutdown_notify.notified() => break,
        }
    }

    // ── Graceful shutdown ──────────────────────────────────────────────────
    callbacks.on_state_change(BridgeState::ShuttingDown);
    tracing::debug!("Bridge main loop shutting down, deregistering environment");

    if let Err(e) = api_client.deregister_environment(&environment_id).await {
        tracing::warn!("Failed to deregister environment on shutdown: {e}");
    }

    callbacks.on_state_change(BridgeState::Disconnected);
    Ok(())
}

/// Run heartbeat pings for an active work item until cancelled.
async fn run_heartbeat_loop(
    api: &BridgeApiClient,
    environment_id: &str,
    work_id: &str,
    session_token: &str,
    cancel: &CancellationToken,
    callbacks: &dyn BridgeCallbacks,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {}
            _ = cancel.cancelled() => return,
        }

        match api
            .heartbeat_work(environment_id, work_id, session_token)
            .await
        {
            Ok(resp) => {
                if !resp.lease_extended {
                    tracing::debug!(
                        "Heartbeat for work {work_id}: lease not extended (state={})",
                        resp.state
                    );
                    return;
                }
            }
            Err(e) => {
                callbacks.on_error(&format!("Heartbeat failed for work {work_id}: {e}"));
                // Don't return — keep trying until cancelled
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopCallbacks;
    impl BridgeCallbacks for NoopCallbacks {
        fn on_registered(&self, _: &str) {}
        fn on_work_received(&self, _: &WorkResponse, _: &str, _: &str) {}
        fn on_session_done(&self, _: &str, _: &str) {}
        fn on_state_change(&self, _: BridgeState) {}
        fn on_error(&self, _: &str) {}
        fn on_fatal_error(&self, _: &str) {}
        fn on_reconnected(&self, _: u64) {}
    }

    #[test]
    fn test_poll_interval_is_reasonable() {
        assert!(POLL_INTERVAL.as_secs() >= 1);
        assert!(POLL_INTERVAL.as_secs() <= 10);
    }

    #[test]
    fn test_heartbeat_interval_is_reasonable() {
        assert!(HEARTBEAT_INTERVAL.as_secs() >= 10);
        assert!(HEARTBEAT_INTERVAL.as_secs() <= 60);
    }
}
