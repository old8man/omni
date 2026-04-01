//! Bridge main loop.
//!
//! Implements the core event loop for the bridge: register an environment,
//! poll for work, dispatch sessions, send heartbeats, and handle shutdown.
//! Uses exponential backoff for reconnection on transient failures.
//!
//! Supports both single-session and multi-session modes:
//! - Single-session: one session in cwd, bridge tears down when it ends
//! - Multi-session: persistent server with capacity tracking, worktree
//!   support, and session timeout management

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::api::{BridgeApiClient, BridgeFatalError};
use super::capacity::CapacityWake;
use super::jwt::{decode_work_secret, format_duration_ms};
use super::poll_config::PollIntervalConfig;
use super::spawn::SessionCapacity;
use super::types::{
    BackoffConfig, BridgeConfig, BridgeState, SessionDoneStatus, SpawnMode, WorkResponse,
};
use super::work_secret::{same_session_id, to_compat_session_id};

/// How often the bridge polls for new work (used by single-session callers).
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Grace period for in-flight sessions during shutdown.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// Heartbeat interval for active sessions.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Sleep detection threshold: if the time between two consecutive poll
/// iterations exceeds this, we assume the machine went to sleep and
/// reset the error budget. Must exceed the max backoff cap.
const SLEEP_DETECTION_THRESHOLD: Duration = Duration::from_secs(240);

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

    /// Called to update the session count in multi-session mode.
    fn on_session_count_change(&self, _active: usize, _max: u32, _mode: SpawnMode) {}

    /// Called when the bridge enters idle state (no active sessions).
    fn on_idle(&self) {}
}

/// State tracked for each active session in the multi-session loop.
pub struct ActiveSession {
    /// Work item ID associated with this session.
    work_id: String,
    /// Session ingress JWT token.
    ingress_token: String,
    /// Session start time for duration tracking.
    start_time: Instant,
    /// Compat-format session ID (session_*).
    compat_id: String,
    /// Whether this session uses CCR v2 transport.
    pub is_v2: bool,
    /// Timeout timer handle, if set.
    timeout_at: Option<Instant>,
    /// Whether the session was killed by the timeout watchdog.
    timed_out: bool,
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
    let capacity_wake = CapacityWake::new(cancel.clone());
    let poll_config = PollIntervalConfig::default();

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

    // ── Multi-session state ────────────────────────────────────────────────
    let mut active_sessions: HashMap<String, ActiveSession> = HashMap::new();
    let mut session_capacity = SessionCapacity::new(config.max_sessions);
    let completed_work_ids: HashSet<String> = HashSet::new();

    callbacks.on_session_count_change(0, config.max_sessions, config.spawn_mode);

    // ── Poll loop ──────────────────────────────────────────────────────────
    let mut poll_attempt: u32 = 0;
    let mut poll_failure_start: Option<Instant> = None;
    let mut last_poll_time = Instant::now();

    tracing::debug!(
        "[bridge:work] Starting poll loop spawnMode={:?} maxSessions={} environmentId={}",
        config.spawn_mode,
        config.max_sessions,
        environment_id
    );

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Check for session timeouts
        check_session_timeouts(&mut active_sessions, &callbacks);

        // Sleep detection: if time since last poll exceeds threshold, the
        // machine likely went to sleep. Reset the error budget so transient
        // errors from the wake don't trigger premature give-up.
        let now = Instant::now();
        if now.duration_since(last_poll_time) > SLEEP_DETECTION_THRESHOLD {
            tracing::debug!(
                "[bridge:work] Sleep detected ({}ms since last poll), resetting error budget",
                now.duration_since(last_poll_time).as_millis()
            );
            poll_failure_start = None;
            poll_attempt = 0;
        }
        last_poll_time = now;

        // Determine reclaim parameter
        let reclaim_ms = if session_capacity.has_capacity() {
            Some(poll_config.reclaim_older_than_ms())
        } else {
            None
        };

        // Poll for work (only if we have capacity)
        if session_capacity.has_capacity() {
            match api_client
                .poll_for_work(&environment_id, &environment_secret, reclaim_ms)
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

                    // Skip already-completed work
                    if completed_work_ids.contains(&work.id) {
                        tracing::debug!(
                            "[bridge:work] Skipping already-completed work_id={}",
                            work.id
                        );
                        continue;
                    }

                    // Decode work secret and dispatch
                    let work_id = work.id.clone();
                    let session_id = work.data.id.clone();
                    let compat_id = to_compat_session_id(&session_id);

                    // Check if we already have this session (token refresh re-dispatch)
                    if active_sessions.contains_key(&session_id)
                        || active_sessions
                            .values()
                            .any(|s| same_session_id(&s.compat_id, &compat_id))
                    {
                        tracing::debug!(
                            "[bridge:work] Re-dispatch for existing sessionId={session_id}, updating token"
                        );

                        match decode_work_secret(&work.secret) {
                            Ok(secret) => {
                                // Acknowledge to keep the work item alive
                                let _ = api_client
                                    .acknowledge_work(
                                        &environment_id,
                                        &work_id,
                                        &secret.session_ingress_token,
                                    )
                                    .await;

                                // Update the existing session's token
                                if let Some(session) = active_sessions.get_mut(&session_id) {
                                    session.ingress_token =
                                        secret.session_ingress_token.clone();
                                    session.work_id = work_id;
                                }
                            }
                            Err(e) => {
                                callbacks.on_error(&format!(
                                    "Failed to decode work secret for re-dispatch {session_id}: {e}"
                                ));
                            }
                        }
                        continue;
                    }

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
                                callbacks.on_error(&format!(
                                    "Failed to acknowledge work {work_id}: {e}"
                                ));
                                continue;
                            }

                            // Register in capacity tracker
                            if !session_capacity.register(&session_id) {
                                callbacks.on_error(&format!(
                                    "At capacity, cannot accept session {session_id}"
                                ));
                                continue;
                            }

                            // Track the session
                            let is_v2 = secret.use_code_sessions.unwrap_or(false);
                            let timeout_at = config.session_timeout_ms.map(|ms| {
                                Instant::now() + Duration::from_millis(ms)
                            });

                            active_sessions.insert(
                                session_id.clone(),
                                ActiveSession {
                                    work_id: work_id.clone(),
                                    ingress_token: secret.session_ingress_token.clone(),
                                    start_time: Instant::now(),
                                    compat_id: compat_id.clone(),
                                    is_v2,
                                    timeout_at,
                                    timed_out: false,
                                },
                            );

                            callbacks.on_work_received(
                                &work,
                                &secret.session_ingress_token,
                                &secret.api_base_url,
                            );
                            callbacks.on_session_count_change(
                                active_sessions.len(),
                                config.max_sessions,
                                config.spawn_mode,
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
        } else {
            // At capacity -- send heartbeats instead of polling
            heartbeat_active_sessions(
                &api_client,
                &environment_id,
                &active_sessions,
                &callbacks,
            )
            .await;
        }

        // Choose poll interval based on capacity
        let interval = if active_sessions.is_empty() {
            poll_config.not_at_capacity_interval()
        } else if session_capacity.has_capacity() {
            poll_config.multisession_interval(
                active_sessions.len(),
                config.max_sessions as usize,
            )
        } else {
            poll_config
                .at_capacity_interval()
                .unwrap_or(poll_config.not_at_capacity_interval())
        };

        // Wait before next poll, with capacity-wake interrupt
        let interrupted = capacity_wake.sleep_interruptible(interval).await;
        if cancel.is_cancelled() {
            break;
        }
        if interrupted {
            tracing::debug!("[bridge:work] Capacity wake interrupted sleep");
        }
    }

    // ── Graceful shutdown ──────────────────────────────────────────────────
    callbacks.on_state_change(BridgeState::ShuttingDown);
    tracing::debug!(
        "Bridge main loop shutting down, {} active sessions",
        active_sessions.len()
    );

    // Notify all active sessions to shut down
    for (session_id, session) in &active_sessions {
        callbacks.on_session_done(session_id, "interrupted");

        // Best-effort stop work
        let _ = api_client
            .stop_work(&environment_id, &session.work_id, false)
            .await;
    }

    // Deregister the environment
    if let Err(e) = api_client.deregister_environment(&environment_id).await {
        tracing::warn!("Failed to deregister environment on shutdown: {e}");
    }

    callbacks.on_state_change(BridgeState::Disconnected);
    Ok(())
}

/// Notify the main loop that a session has completed.
///
/// Call this from session completion handlers to update capacity tracking
/// and trigger work re-polling.
pub fn notify_session_done(
    session_id: &str,
    status: SessionDoneStatus,
    active_sessions: &mut HashMap<String, ActiveSession>,
    session_capacity: &mut SessionCapacity,
    completed_work_ids: &mut HashSet<String>,
    capacity_wake: &CapacityWake,
    callbacks: &dyn BridgeCallbacks,
) {
    if let Some(session) = active_sessions.remove(session_id) {
        let duration_ms = session.start_time.elapsed().as_millis() as u64;
        completed_work_ids.insert(session.work_id);
        session_capacity.remove(session_id);

        let status_str = match status {
            SessionDoneStatus::Completed => "completed",
            SessionDoneStatus::Failed => "failed",
            SessionDoneStatus::Interrupted => "interrupted",
        };

        tracing::debug!(
            "[bridge:session] sessionId={} exited status={} duration={}",
            session_id,
            status_str,
            format_duration_ms(duration_ms)
        );

        callbacks.on_session_done(session_id, status_str);
    }

    // Wake the poll loop to accept new work
    capacity_wake.wake();
}

/// Check session timeouts and mark timed-out sessions.
fn check_session_timeouts(
    active_sessions: &mut HashMap<String, ActiveSession>,
    callbacks: &Arc<dyn BridgeCallbacks>,
) {
    let now = Instant::now();
    let mut timed_out = Vec::new();

    for (session_id, session) in active_sessions.iter() {
        if let Some(timeout_at) = session.timeout_at {
            if now >= timeout_at && !session.timed_out {
                timed_out.push(session_id.clone());
            }
        }
    }

    for session_id in timed_out {
        if let Some(session) = active_sessions.get_mut(&session_id) {
            session.timed_out = true;
            let duration_ms = session.start_time.elapsed().as_millis() as u64;
            callbacks.on_error(&format!(
                "Session {session_id} timed out after {}",
                format_duration_ms(duration_ms)
            ));
        }
    }
}

/// Send heartbeats for all active sessions.
async fn heartbeat_active_sessions(
    api: &BridgeApiClient,
    environment_id: &str,
    active_sessions: &HashMap<String, ActiveSession>,
    callbacks: &Arc<dyn BridgeCallbacks>,
) {
    for (session_id, session) in active_sessions {
        match api
            .heartbeat_work(environment_id, &session.work_id, &session.ingress_token)
            .await
        {
            Ok(resp) => {
                if !resp.lease_extended {
                    tracing::debug!(
                        "[bridge:heartbeat] sessionId={} work_id={}: lease not extended (state={})",
                        session_id,
                        session.work_id,
                        resp.state
                    );
                }
            }
            Err(e) => {
                // Check for auth failures (JWT expired)
                if let Some(fatal) = e.downcast_ref::<BridgeFatalError>() {
                    if fatal.status == 401 || fatal.status == 403 {
                        tracing::debug!(
                            "[bridge:heartbeat] Auth failed for sessionId={}, will re-queue via reconnect",
                            session_id
                        );
                        // The next poll cycle will pick up the re-dispatched work
                        let _ = api
                            .reconnect_session(environment_id, session_id)
                            .await;
                        continue;
                    }
                }
                callbacks.on_error(&format!(
                    "Heartbeat failed for sessionId={}: {e}",
                    session_id
                ));
            }
        }
    }
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
    fn test_heartbeat_interval_is_reasonable() {
        assert!(HEARTBEAT_INTERVAL.as_secs() >= 10);
        assert!(HEARTBEAT_INTERVAL.as_secs() <= 60);
    }

    #[test]
    fn test_sleep_detection_threshold() {
        // Should be > max backoff cap (120s)
        assert!(SLEEP_DETECTION_THRESHOLD.as_secs() > 120);
    }

    #[test]
    fn test_session_timeout_tracking() {
        let callbacks: Arc<dyn BridgeCallbacks> = Arc::new(NoopCallbacks);
        let mut sessions = HashMap::new();

        // Session without timeout -- should not time out
        sessions.insert(
            "session_1".to_string(),
            ActiveSession {
                work_id: "w1".to_string(),
                ingress_token: "tok1".to_string(),
                start_time: Instant::now() - Duration::from_secs(100),
                compat_id: "session_1".to_string(),
                is_v2: false,
                timeout_at: None,
                timed_out: false,
            },
        );

        // Session with future timeout -- should not time out
        sessions.insert(
            "session_2".to_string(),
            ActiveSession {
                work_id: "w2".to_string(),
                ingress_token: "tok2".to_string(),
                start_time: Instant::now(),
                compat_id: "session_2".to_string(),
                is_v2: false,
                timeout_at: Some(Instant::now() + Duration::from_secs(3600)),
                timed_out: false,
            },
        );

        // Session with past timeout -- should time out
        sessions.insert(
            "session_3".to_string(),
            ActiveSession {
                work_id: "w3".to_string(),
                ingress_token: "tok3".to_string(),
                start_time: Instant::now() - Duration::from_secs(100),
                compat_id: "session_3".to_string(),
                is_v2: false,
                timeout_at: Some(Instant::now() - Duration::from_secs(1)),
                timed_out: false,
            },
        );

        check_session_timeouts(&mut sessions, &callbacks);

        assert!(!sessions["session_1"].timed_out);
        assert!(!sessions["session_2"].timed_out);
        assert!(sessions["session_3"].timed_out);
    }
}
