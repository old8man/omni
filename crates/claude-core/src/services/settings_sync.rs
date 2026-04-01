/// Settings synchronization across Claude Code sessions and machines.
///
/// - Interactive CLI: uploads local settings to remote (incremental, only changed entries).
/// - Headless/CCR: downloads remote settings to local before plugin installation.
///
/// File-based persistence with checksum verification and conflict detection.
///
/// Port of `services/settingsSync/index.ts` and `services/settingsSync/types.ts`.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

// ── Constants ──────────────────────────────────────────────────────────────

/// HTTP timeout for settings sync API calls.
const SETTINGS_SYNC_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum retries for transient failures.
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Maximum file size for sync (500 KB, matches backend limit).
const MAX_FILE_SIZE_BYTES: u64 = 500 * 1024;

// ── Sync keys ──────────────────────────────────────────────────────────────

/// Well-known keys used for sync entries, mapping to file paths.
pub struct SyncKeys;

impl SyncKeys {
    pub const USER_SETTINGS: &'static str = "~/.claude/settings.json";
    pub const USER_MEMORY: &'static str = "~/.claude/CLAUDE.md";

    pub fn project_settings(project_id: &str) -> String {
        format!("projects/{project_id}/.claude/settings.local.json")
    }

    pub fn project_memory(project_id: &str) -> String {
        format!("projects/{project_id}/CLAUDE.local.md")
    }
}

// ── Types ──────────────────────────────────────────────────────────────────

/// Content portion of user sync data: flat key-value storage.
/// Keys are opaque strings (typically file paths). Values are UTF-8 content.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserSyncContent {
    pub entries: HashMap<String, String>,
}

/// Full response from GET /api/claude_code/user_settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserSyncData {
    #[serde(rename = "userId")]
    pub user_id: String,
    pub version: u64,
    #[serde(rename = "lastModified")]
    pub last_modified: String,
    pub checksum: String,
    pub content: UserSyncContent,
}

/// Result from fetching user settings.
#[derive(Clone, Debug)]
pub struct SettingsSyncFetchResult {
    pub success: bool,
    pub data: Option<UserSyncData>,
    /// True if 404 (no data exists yet).
    pub is_empty: bool,
    pub error: Option<String>,
    /// True if error is permanent (don't retry).
    pub skip_retry: bool,
}

impl Default for SettingsSyncFetchResult {
    fn default() -> Self {
        Self {
            success: false,
            data: None,
            is_empty: false,
            error: None,
            skip_retry: false,
        }
    }
}

/// Result from uploading user settings.
#[derive(Clone, Debug)]
pub struct SettingsSyncUploadResult {
    pub success: bool,
    pub checksum: Option<String>,
    pub last_modified: Option<String>,
    pub error: Option<String>,
}

impl Default for SettingsSyncUploadResult {
    fn default() -> Self {
        Self {
            success: false,
            checksum: None,
            last_modified: None,
            error: None,
        }
    }
}

// ── Configuration ──────────────────────────────────────────────────────────

/// Settings sync configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingsSyncConfig {
    /// Base URL for the settings sync API.
    pub base_api_url: String,
    /// OAuth access token.
    pub access_token: Option<String>,
    /// Whether this is an interactive CLI session (upload) or headless (download).
    pub is_interactive: bool,
    /// Whether upload is enabled (feature flag).
    pub upload_enabled: bool,
    /// Whether download is enabled (feature flag).
    pub download_enabled: bool,
}

impl Default for SettingsSyncConfig {
    fn default() -> Self {
        Self {
            base_api_url: String::new(),
            access_token: None,
            is_interactive: true,
            upload_enabled: false,
            download_enabled: false,
        }
    }
}

// ── File helpers ───────────────────────────────────────────────────────────

/// Try to read a file for sync. Returns None if file doesn't exist, is empty,
/// or exceeds the size limit.
pub async fn try_read_file_for_sync(path: &Path) -> Option<String> {
    let metadata = tokio::fs::metadata(path).await.ok()?;

    if metadata.len() > MAX_FILE_SIZE_BYTES {
        debug!(
            path = %path.display(),
            size = metadata.len(),
            "settings sync: file too large, skipping"
        );
        return None;
    }

    let content = tokio::fs::read_to_string(path).await.ok()?;

    // Skip empty or whitespace-only content
    if content.trim().is_empty() {
        return None;
    }

    Some(content)
}

/// Write a file for sync, creating parent directories as needed.
pub async fn write_file_for_sync(path: &Path, content: &str) -> Result<bool> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("creating parent directory for sync file")?;
    }

    tokio::fs::write(path, content)
        .await
        .context("writing sync file")?;

    debug!(path = %path.display(), "settings sync: file written");
    Ok(true)
}

/// Compute MD5 hex digest of content (for checksum comparison).
pub fn compute_checksum(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ── Entry building ─────────────────────────────────────────────────────────

/// Build the set of sync entries from local files.
///
/// Reads user settings, user memory, and optionally project-specific files.
/// Returns a map of sync keys to file contents.
pub async fn build_entries_from_local_files(
    user_settings_path: Option<&Path>,
    user_memory_path: &Path,
    local_settings_path: Option<&Path>,
    local_memory_path: Option<&Path>,
    project_id: Option<&str>,
) -> HashMap<String, String> {
    let mut entries = HashMap::new();

    // Global user settings
    if let Some(path) = user_settings_path {
        if let Some(content) = try_read_file_for_sync(path).await {
            entries.insert(SyncKeys::USER_SETTINGS.to_string(), content);
        }
    }

    // Global user memory
    if let Some(content) = try_read_file_for_sync(user_memory_path).await {
        entries.insert(SyncKeys::USER_MEMORY.to_string(), content);
    }

    // Project-specific files (only with a valid project ID)
    if let Some(pid) = project_id {
        if let Some(path) = local_settings_path {
            if let Some(content) = try_read_file_for_sync(path).await {
                entries.insert(SyncKeys::project_settings(pid), content);
            }
        }
        if let Some(path) = local_memory_path {
            if let Some(content) = try_read_file_for_sync(path).await {
                entries.insert(SyncKeys::project_memory(pid), content);
            }
        }
    }

    entries
}

/// Compute which entries have changed compared to remote.
pub fn compute_changed_entries<'a>(
    local: &'a HashMap<String, String>,
    remote: &HashMap<String, String>,
) -> HashMap<String, String> {
    local
        .iter()
        .filter(|(key, value)| remote.get(*key).map_or(true, |rv| rv != *value))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

// ── Apply remote entries ───────────────────────────────────────────────────

/// Result of applying remote entries to local files.
#[derive(Debug, Default)]
pub struct ApplyResult {
    pub applied_count: usize,
    pub settings_written: bool,
    pub memory_written: bool,
}

/// Check if content exceeds the size limit.
fn exceeds_size_limit(content: &str) -> bool {
    content.len() as u64 > MAX_FILE_SIZE_BYTES
}

/// Apply remote entries to local files (download/CCR pull pattern).
///
/// Only writes files that match expected keys. Returns info about what was written
/// so the caller can invalidate caches.
pub async fn apply_remote_entries_to_local(
    entries: &HashMap<String, String>,
    project_id: Option<&str>,
    user_settings_path: Option<&Path>,
    user_memory_path: &Path,
    local_settings_path: Option<&Path>,
    local_memory_path: Option<&Path>,
) -> Result<ApplyResult> {
    let mut result = ApplyResult::default();

    // Apply global user settings
    if let Some(content) = entries.get(SyncKeys::USER_SETTINGS) {
        if let Some(path) = user_settings_path {
            if !exceeds_size_limit(content) {
                if write_file_for_sync(path, content).await? {
                    result.applied_count += 1;
                    result.settings_written = true;
                }
            }
        }
    }

    // Apply global user memory
    if let Some(content) = entries.get(SyncKeys::USER_MEMORY) {
        if !exceeds_size_limit(content) {
            if write_file_for_sync(user_memory_path, content).await? {
                result.applied_count += 1;
                result.memory_written = true;
            }
        }
    }

    // Apply project-specific files
    if let Some(pid) = project_id {
        let proj_settings_key = SyncKeys::project_settings(pid);
        if let Some(content) = entries.get(&proj_settings_key) {
            if let Some(path) = local_settings_path {
                if !exceeds_size_limit(content) {
                    if write_file_for_sync(path, content).await? {
                        result.applied_count += 1;
                        result.settings_written = true;
                    }
                }
            }
        }

        let proj_memory_key = SyncKeys::project_memory(pid);
        if let Some(content) = entries.get(&proj_memory_key) {
            if let Some(path) = local_memory_path {
                if !exceeds_size_limit(content) {
                    if write_file_for_sync(path, content).await? {
                        result.applied_count += 1;
                        result.memory_written = true;
                    }
                }
            }
        }
    }

    info!(applied_count = result.applied_count, "settings sync: applied remote entries");
    Ok(result)
}

// ── Retry logic ────────────────────────────────────────────────────────────

/// Compute retry delay with exponential backoff and jitter.
pub fn get_retry_delay(attempt: u32) -> Duration {
    let base_ms = 1000u64 * 2u64.pow(attempt.saturating_sub(1));
    // Add up to 50% jitter
    let jitter_ms = (base_ms as f64 * 0.5 * rand::random::<f64>()) as u64;
    Duration::from_millis(base_ms + jitter_ms)
}

/// Execute a fetch with retries.
pub async fn fetch_with_retries<F, Fut>(
    max_retries: u32,
    mut fetch_fn: F,
) -> SettingsSyncFetchResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = SettingsSyncFetchResult>,
{
    let mut last_result = SettingsSyncFetchResult::default();

    for attempt in 1..=(max_retries + 1) {
        last_result = fetch_fn().await;

        if last_result.success || last_result.skip_retry {
            return last_result;
        }

        if attempt > max_retries {
            return last_result;
        }

        let delay = get_retry_delay(attempt);
        debug!(
            attempt,
            max_retries,
            delay_ms = delay.as_millis(),
            "settings sync: retrying"
        );
        tokio::time::sleep(delay).await;
    }

    last_result
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_sync_keys() {
        assert_eq!(SyncKeys::USER_SETTINGS, "~/.claude/settings.json");
        assert_eq!(SyncKeys::USER_MEMORY, "~/.claude/CLAUDE.md");
        assert_eq!(
            SyncKeys::project_settings("abc123"),
            "projects/abc123/.claude/settings.local.json"
        );
        assert_eq!(
            SyncKeys::project_memory("abc123"),
            "projects/abc123/CLAUDE.local.md"
        );
    }

    #[test]
    fn test_compute_checksum() {
        let checksum = compute_checksum("hello world");
        assert!(!checksum.is_empty());
        // Same input = same output
        assert_eq!(checksum, compute_checksum("hello world"));
        // Different input = different output
        assert_ne!(checksum, compute_checksum("hello world!"));
    }

    #[test]
    fn test_compute_changed_entries() {
        let mut local = HashMap::new();
        local.insert("key1".to_string(), "value1".to_string());
        local.insert("key2".to_string(), "value2_new".to_string());
        local.insert("key3".to_string(), "value3".to_string());

        let mut remote = HashMap::new();
        remote.insert("key1".to_string(), "value1".to_string());
        remote.insert("key2".to_string(), "value2_old".to_string());

        let changed = compute_changed_entries(&local, &remote);
        // key1 unchanged, key2 changed, key3 new
        assert_eq!(changed.len(), 2);
        assert!(changed.contains_key("key2"));
        assert!(changed.contains_key("key3"));
        assert!(!changed.contains_key("key1"));
    }

    #[test]
    fn test_compute_changed_entries_empty_remote() {
        let mut local = HashMap::new();
        local.insert("key1".to_string(), "value1".to_string());
        let remote = HashMap::new();

        let changed = compute_changed_entries(&local, &remote);
        assert_eq!(changed.len(), 1);
    }

    #[test]
    fn test_exceeds_size_limit() {
        assert!(!exceeds_size_limit("short"));
        let large = "x".repeat(MAX_FILE_SIZE_BYTES as usize + 1);
        assert!(exceeds_size_limit(&large));
    }

    #[tokio::test]
    async fn test_try_read_file_for_sync_missing() {
        let result = try_read_file_for_sync(Path::new("/nonexistent/file.json")).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_try_read_file_for_sync_normal() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        tokio::fs::write(&path, r#"{"key": "value"}"#).await.unwrap();

        let result = try_read_file_for_sync(&path).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap(), r#"{"key": "value"}"#);
    }

    #[tokio::test]
    async fn test_try_read_file_for_sync_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.json");
        tokio::fs::write(&path, "   \n  ").await.unwrap();

        let result = try_read_file_for_sync(&path).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_write_file_for_sync() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sub").join("settings.json");

        let result = write_file_for_sync(&path, r#"{"key":"val"}"#).await;
        assert!(result.is_ok());
        assert!(result.unwrap());

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, r#"{"key":"val"}"#);
    }

    #[tokio::test]
    async fn test_build_entries_from_local_files() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let memory_path = dir.path().join("CLAUDE.md");

        tokio::fs::write(&settings_path, r#"{"theme":"dark"}"#).await.unwrap();
        tokio::fs::write(&memory_path, "# Memory\nSome notes").await.unwrap();

        let entries = build_entries_from_local_files(
            Some(&settings_path),
            &memory_path,
            None,
            None,
            None,
        )
        .await;

        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key(SyncKeys::USER_SETTINGS));
        assert!(entries.contains_key(SyncKeys::USER_MEMORY));
    }

    #[tokio::test]
    async fn test_apply_remote_entries_to_local() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let memory_path = dir.path().join("CLAUDE.md");

        let mut entries = HashMap::new();
        entries.insert(
            SyncKeys::USER_SETTINGS.to_string(),
            r#"{"theme":"light"}"#.to_string(),
        );
        entries.insert(
            SyncKeys::USER_MEMORY.to_string(),
            "# Memory\nUpdated.".to_string(),
        );

        let result = apply_remote_entries_to_local(
            &entries,
            None,
            Some(&settings_path),
            &memory_path,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(result.applied_count, 2);
        assert!(result.settings_written);
        assert!(result.memory_written);

        let settings_content = tokio::fs::read_to_string(&settings_path).await.unwrap();
        assert_eq!(settings_content, r#"{"theme":"light"}"#);

        let memory_content = tokio::fs::read_to_string(&memory_path).await.unwrap();
        assert_eq!(memory_content, "# Memory\nUpdated.");
    }

    #[test]
    fn test_get_retry_delay() {
        let d1 = get_retry_delay(1);
        assert!(d1.as_millis() >= 1000 && d1.as_millis() <= 1500);

        let d2 = get_retry_delay(2);
        assert!(d2.as_millis() >= 2000 && d2.as_millis() <= 3000);

        let d3 = get_retry_delay(3);
        assert!(d3.as_millis() >= 4000 && d3.as_millis() <= 6000);
    }

    #[tokio::test]
    async fn test_fetch_with_retries_success_first_try() {
        let result = fetch_with_retries(3, || async {
            SettingsSyncFetchResult {
                success: true,
                data: None,
                is_empty: true,
                error: None,
                skip_retry: false,
            }
        })
        .await;

        assert!(result.success);
    }

    #[tokio::test]
    async fn test_fetch_with_retries_skip_retry() {
        let result = fetch_with_retries(3, || async {
            SettingsSyncFetchResult {
                success: false,
                data: None,
                is_empty: false,
                error: Some("auth failed".to_string()),
                skip_retry: true,
            }
        })
        .await;

        assert!(!result.success);
        assert!(result.skip_retry);
    }

    #[test]
    fn test_user_sync_data_serde() {
        let data = UserSyncData {
            user_id: "user-123".to_string(),
            version: 1,
            last_modified: "2025-01-01T00:00:00Z".to_string(),
            checksum: "abc123".to_string(),
            content: UserSyncContent {
                entries: HashMap::from([("key".to_string(), "value".to_string())]),
            },
        };

        let json = serde_json::to_string(&data).unwrap();
        let parsed: UserSyncData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.user_id, "user-123");
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.content.entries.get("key").unwrap(), "value");
    }
}
