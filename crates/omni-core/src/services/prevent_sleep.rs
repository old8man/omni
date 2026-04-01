use std::process::Child;
use std::sync::Mutex;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

/// Global sleep prevention state.
///
/// On macOS, we spawn `caffeinate -i` to prevent idle sleep while
/// a long-running session is active.
static CAFFEINATE_PROCESS: Mutex<Option<Child>> = Mutex::new(None);

/// Prevent the system from sleeping.
///
/// On macOS, spawns `caffeinate -i` to inhibit idle sleep. On other
/// platforms, this is a no-op (logged as a debug message).
///
/// Calling this multiple times is safe; only the first call spawns a process.
pub fn prevent_sleep() -> Result<()> {
    let mut guard = CAFFEINATE_PROCESS
        .lock()
        .map_err(|e| anyhow::anyhow!("failed to lock caffeinate mutex: {e}"))?;

    if guard.is_some() {
        debug!("sleep prevention already active");
        return Ok(());
    }

    if cfg!(target_os = "macos") {
        let child = std::process::Command::new("caffeinate")
            .arg("-i") // prevent idle sleep
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("failed to spawn caffeinate")?;

        info!(pid = child.id(), "started caffeinate to prevent sleep");
        *guard = Some(child);
    } else {
        debug!("sleep prevention not available on this platform");
    }

    Ok(())
}

/// Allow the system to sleep again.
///
/// Kills the `caffeinate` process if one is running. Safe to call
/// even if `prevent_sleep` was never called.
pub fn allow_sleep() {
    let mut guard = match CAFFEINATE_PROCESS.lock() {
        Ok(g) => g,
        Err(e) => {
            warn!("failed to lock caffeinate mutex: {e}");
            return;
        }
    };

    if let Some(mut child) = guard.take() {
        match child.kill() {
            Ok(()) => {
                let _ = child.wait();
                info!("stopped caffeinate, sleep allowed");
            }
            Err(e) => {
                warn!("failed to kill caffeinate process: {e}");
            }
        }
    }
}

/// Check if sleep prevention is currently active.
pub fn is_sleep_prevented() -> bool {
    CAFFEINATE_PROCESS
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
}
