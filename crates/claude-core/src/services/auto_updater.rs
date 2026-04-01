use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Information about an available update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// The new version available.
    pub version: String,
    /// Release notes / changelog (if available).
    pub release_notes: Option<String>,
    /// Download URL for the update.
    pub download_url: Option<String>,
}

/// Current update status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// No update check has been performed.
    Unknown,
    /// An update is available.
    Available(String),
    /// Already running the latest version.
    UpToDate,
    /// Update check failed.
    CheckFailed(String),
}

/// Check for available updates by comparing the current version
/// against the latest release.
///
/// This performs an HTTP request to the configured update endpoint.
/// Returns `None` if already up to date, or `Some(UpdateInfo)` if
/// an update is available.
pub async fn check_for_updates(
    current_version: &str,
    update_url: &str,
) -> Result<Option<UpdateInfo>> {
    debug!(
        current = current_version,
        url = update_url,
        "checking for updates"
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let response = client
        .get(update_url)
        .header("User-Agent", format!("claude-rs/{current_version}"))
        .send()
        .await
        .context("failed to check for updates")?;

    if !response.status().is_success() {
        warn!(
            status = %response.status(),
            "update check returned non-success status"
        );
        return Ok(None);
    }

    let info: LatestVersionResponse = response
        .json()
        .await
        .context("failed to parse update response")?;

    if version_is_newer(&info.version, current_version) {
        info!(
            current = current_version,
            latest = %info.version,
            "update available"
        );
        Ok(Some(UpdateInfo {
            version: info.version,
            release_notes: info.release_notes,
            download_url: info.download_url,
        }))
    } else {
        debug!(current = current_version, "already up to date");
        Ok(None)
    }
}

/// Response from the update check endpoint.
#[derive(Debug, Deserialize)]
struct LatestVersionResponse {
    version: String,
    release_notes: Option<String>,
    download_url: Option<String>,
}

/// Compare two semver-like version strings.
///
/// Returns `true` if `latest` is strictly newer than `current`.
/// Both versions should be in `MAJOR.MINOR.PATCH` format.
pub fn version_is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<(u32, u32, u32)> {
        // Strip leading 'v' if present
        let v = v.strip_prefix('v').unwrap_or(v);
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() < 3 {
            return None;
        }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            // Handle pre-release suffixes like "1.0.0-beta"
            parts[2].split('-').next().and_then(|p| p.parse().ok())?,
        ))
    };

    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        assert!(version_is_newer("1.0.1", "1.0.0"));
        assert!(version_is_newer("1.1.0", "1.0.9"));
        assert!(version_is_newer("2.0.0", "1.9.9"));
        assert!(!version_is_newer("1.0.0", "1.0.0"));
        assert!(!version_is_newer("1.0.0", "1.0.1"));
        assert!(version_is_newer("v1.1.0", "v1.0.0"));
        assert!(version_is_newer("1.0.1-beta", "1.0.0"));
    }
}
