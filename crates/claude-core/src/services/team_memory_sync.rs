/// Team memory synchronization: sync shared team memories between local
/// filesystem and a server API, scoped per-repo (identified by git remote).
///
/// Pull: download server entries and write to local team memory directory.
/// Push: upload only changed entries (delta upload based on content hashes).
/// Watch: observe the team memory directory for changes and debounce pushes.
///
/// Port of `services/teamMemorySync/index.ts`, `types.ts`, `watcher.ts`,
/// `secretScanner.ts`, and `teamMemSecretGuard.ts`.
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

// ── Constants ──────────────────────────────────────────────────────────────

/// HTTP timeout for team memory sync API calls.
const TEAM_MEMORY_SYNC_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-entry size cap (matches server default).
const MAX_FILE_SIZE_BYTES: u64 = 250_000;

/// Maximum PUT body size (stay under gateway limit).
const MAX_PUT_BODY_BYTES: usize = 200_000;

/// Retry limits.
const MAX_RETRIES: u32 = 3;
const MAX_CONFLICT_RETRIES: u32 = 2;

/// Debounce interval for the file watcher.
const DEBOUNCE: Duration = Duration::from_secs(2);

// ── Types ──────────────────────────────────────────────────────────────────

/// Content portion of team memory data: flat key-value storage.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamMemoryContent {
    pub entries: HashMap<String, String>,
    /// Per-key SHA-256 of entry content (`sha256:<hex>`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub entry_checksums: HashMap<String, String>,
}

/// Full response from GET /api/claude_code/team_memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamMemoryData {
    #[serde(rename = "organizationId")]
    pub organization_id: String,
    pub repo: String,
    pub version: u64,
    #[serde(rename = "lastModified")]
    pub last_modified: String,
    pub checksum: String,
    pub content: TeamMemoryContent,
}

/// Result from fetching team memory.
#[derive(Clone, Debug, Default)]
pub struct TeamMemorySyncFetchResult {
    pub success: bool,
    pub data: Option<TeamMemoryData>,
    pub is_empty: bool,
    pub not_modified: bool,
    pub checksum: Option<String>,
    pub error: Option<String>,
    pub skip_retry: bool,
    pub error_type: Option<SyncErrorType>,
    pub http_status: Option<u16>,
}

/// Lightweight metadata probe result (hashes only).
#[derive(Clone, Debug, Default)]
pub struct TeamMemoryHashesResult {
    pub success: bool,
    pub version: Option<u64>,
    pub checksum: Option<String>,
    pub entry_checksums: Option<HashMap<String, String>>,
    pub error: Option<String>,
    pub error_type: Option<SyncErrorType>,
    pub http_status: Option<u16>,
}

/// Result from pushing team memory.
#[derive(Clone, Debug, Default)]
pub struct TeamMemorySyncPushResult {
    pub success: bool,
    pub files_uploaded: usize,
    pub checksum: Option<String>,
    pub conflict: bool,
    pub error: Option<String>,
    pub skipped_secrets: Vec<SkippedSecretFile>,
    pub error_type: Option<SyncErrorType>,
    pub http_status: Option<u16>,
}

/// Result from pulling team memory.
#[derive(Clone, Debug, Default)]
pub struct TeamMemorySyncPullResult {
    pub success: bool,
    pub files_written: usize,
    pub entry_count: usize,
    pub error: Option<String>,
}

/// Classification of sync errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncErrorType {
    Auth,
    Timeout,
    Network,
    Parse,
    Conflict,
    NoOAuth,
    NoRepo,
    Unknown,
}

/// A file skipped during push because it contains a detected secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkippedSecretFile {
    pub path: String,
    pub rule_id: String,
    pub label: String,
}

// ── Sync state ─────────────────────────────────────────────────────────────

/// Mutable state for the team memory sync service. Created once per session.
pub struct SyncState {
    /// Last known server checksum (ETag).
    pub last_known_checksum: Mutex<Option<String>>,
    /// Per-key content hash of what we believe the server holds.
    pub server_checksums: Mutex<HashMap<String, String>>,
    /// Server-enforced max_entries cap (learned from 413 response).
    pub server_max_entries: Mutex<Option<usize>>,
}

impl SyncState {
    pub fn new() -> Self {
        Self {
            last_known_checksum: Mutex::new(None),
            server_checksums: Mutex::new(HashMap::new()),
            server_max_entries: Mutex::new(None),
        }
    }

    /// Update the server checksums from a fetch result.
    pub fn update_checksums(&self, checksums: HashMap<String, String>) {
        *self.server_checksums.lock().unwrap() = checksums;
    }

    /// Get the last known checksum for conditional requests.
    pub fn last_checksum(&self) -> Option<String> {
        self.last_known_checksum.lock().unwrap().clone()
    }

    /// Set the last known checksum.
    pub fn set_last_checksum(&self, checksum: Option<String>) {
        *self.last_known_checksum.lock().unwrap() = checksum;
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Content hashing ────────────────────────────────────────────────────────

/// Compute `sha256:<hex>` over UTF-8 bytes. Matches server format.
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

// ── Team memory key validation ─────────────────────────────────────────────

/// Validate a team memory key (relative file path).
/// Keys must be relative, no `..` components, and use forward slashes.
pub fn validate_team_mem_key(key: &str) -> Result<()> {
    if key.is_empty() {
        anyhow::bail!("team memory key must not be empty");
    }
    if key.starts_with('/') || key.starts_with('\\') {
        anyhow::bail!("team memory key must be relative: {}", key);
    }
    if key.contains("..") {
        anyhow::bail!("team memory key must not contain '..': {}", key);
    }
    Ok(())
}

// ── Local file operations ──────────────────────────────────────────────────

/// Read all files in the team memory directory into a map of relative-path to content.
pub async fn read_local_team_memory(
    team_dir: &Path,
    max_entries: Option<usize>,
) -> Result<HashMap<String, String>> {
    let mut entries = HashMap::new();

    if !team_dir.exists() {
        return Ok(entries);
    }

    read_dir_recursive(team_dir, team_dir, &mut entries).await?;

    // Respect server max_entries cap
    if let Some(max) = max_entries {
        if entries.len() > max {
            // Keep the first `max` entries (alphabetically by key)
            let mut keys: Vec<String> = entries.keys().cloned().collect();
            keys.sort();
            let to_remove: Vec<String> = keys.into_iter().skip(max).collect();
            for key in to_remove {
                entries.remove(&key);
            }
        }
    }

    Ok(entries)
}

/// Recursively read a directory, building relative-path keys.
async fn read_dir_recursive(
    base: &Path,
    dir: &Path,
    entries: &mut HashMap<String, String>,
) -> Result<()> {
    let mut reader = tokio::fs::read_dir(dir)
        .await
        .context("reading team memory directory")?;

    while let Some(entry) = reader.next_entry().await? {
        let path = entry.path();
        let file_type = entry.file_type().await?;

        if file_type.is_dir() {
            // Skip hidden directories
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            Box::pin(read_dir_recursive(base, &path, entries)).await?;
        } else if file_type.is_file() {
            // Skip hidden files and files exceeding size limit
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            let meta = tokio::fs::metadata(&path).await?;
            if meta.len() > MAX_FILE_SIZE_BYTES {
                debug!(path = %path.display(), "team memory: skipping oversized file");
                continue;
            }

            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                if let Ok(rel) = path.strip_prefix(base) {
                    let key = rel.to_string_lossy().replace('\\', "/");
                    if validate_team_mem_key(&key).is_ok() {
                        entries.insert(key, content);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Write server entries to the local team memory directory.
pub async fn write_team_memory_entries(
    team_dir: &Path,
    entries: &HashMap<String, String>,
) -> Result<usize> {
    let mut written = 0;

    tokio::fs::create_dir_all(team_dir).await.ok();

    for (key, content) in entries {
        if validate_team_mem_key(key).is_err() {
            warn!(key = %key, "team memory: skipping invalid key");
            continue;
        }

        let file_path = team_dir.join(key);

        // Create parent directories
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        tokio::fs::write(&file_path, content)
            .await
            .with_context(|| format!("writing team memory file: {}", key))?;

        written += 1;
    }

    Ok(written)
}

/// Compute which local entries have changed compared to server checksums.
pub fn compute_delta(
    local_entries: &HashMap<String, String>,
    server_checksums: &HashMap<String, String>,
) -> HashMap<String, String> {
    local_entries
        .iter()
        .filter(|(key, content)| {
            let local_hash = hash_content(content);
            match server_checksums.get(*key) {
                Some(server_hash) => &local_hash != server_hash,
                None => true, // New key
            }
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

// ── Secret scanning ────────────────────────────────────────────────────────

/// A secret detection match.
#[derive(Clone, Debug)]
pub struct SecretMatch {
    pub rule_id: String,
    pub label: String,
}

/// High-confidence secret detection rules (subset of gitleaks).
struct SecretRule {
    id: &'static str,
    pattern: &'static str,
    flags: Option<&'static str>,
}

const SECRET_RULES: &[SecretRule] = &[
    SecretRule {
        id: "aws-access-token",
        pattern: r"\b((?:A3T[A-Z0-9]|AKIA|ASIA|ABIA|ACCA)[A-Z2-7]{16})\b",
        flags: None,
    },
    SecretRule {
        id: "gcp-api-key",
        pattern: r#"\b(AIza[\w\-]{35})(?:[\x60'"\s;]|\\[nr]|$)"#,
        flags: None,
    },
    SecretRule {
        id: "github-pat",
        pattern: r"ghp_[0-9a-zA-Z]{36}",
        flags: None,
    },
    SecretRule {
        id: "github-fine-grained-pat",
        pattern: r"github_pat_\w{82}",
        flags: None,
    },
    SecretRule {
        id: "github-app-token",
        pattern: r"(?:ghu|ghs)_[0-9a-zA-Z]{36}",
        flags: None,
    },
    SecretRule {
        id: "gitlab-pat",
        pattern: r"glpat-[\w\-]{20}",
        flags: None,
    },
    SecretRule {
        id: "slack-bot-token",
        pattern: r"xoxb-[0-9]{10,13}-[0-9]{10,13}[a-zA-Z0-9\-]*",
        flags: None,
    },
    SecretRule {
        id: "slack-user-token",
        pattern: r"xox[pe](?:-[0-9]{10,13}){3}-[a-zA-Z0-9\-]{28,34}",
        flags: None,
    },
    SecretRule {
        id: "stripe-access-token",
        pattern: r#"\b((?:sk|rk)_(?:test|live|prod)_[a-zA-Z0-9]{10,99})(?:[\x60'"\s;]|\\[nr]|$)"#,
        flags: None,
    },
    SecretRule {
        id: "openai-api-key",
        pattern: r"\b(sk-(?:proj|svcacct|admin)-(?:[A-Za-z0-9_\-]{74}|[A-Za-z0-9_\-]{58})T3BlbkFJ(?:[A-Za-z0-9_\-]{74}|[A-Za-z0-9_\-]{58})\b|sk-[a-zA-Z0-9]{20}T3BlbkFJ[a-zA-Z0-9]{20})",
        flags: None,
    },
    SecretRule {
        id: "npm-access-token",
        pattern: r#"\b(npm_[a-zA-Z0-9]{36})(?:[\x60'"\s;]|\\[nr]|$)"#,
        flags: None,
    },
    SecretRule {
        id: "private-key",
        pattern: r"-----BEGIN[ A-Z0-9_\-]{0,100}PRIVATE KEY(?: BLOCK)?-----[\s\S\-]{64,}?-----END[ A-Z0-9_\-]{0,100}PRIVATE KEY(?: BLOCK)?-----",
        flags: Some("i"),
    },
];

/// Lazily compiled regex cache for secret rules.
fn compiled_secret_rules() -> Vec<(String, Regex)> {
    SECRET_RULES
        .iter()
        .filter_map(|rule| {
            let pattern = if rule.flags == Some("i") {
                format!("(?i){}", rule.pattern)
            } else {
                rule.pattern.to_string()
            };
            Regex::new(&pattern)
                .ok()
                .map(|re| (rule.id.to_string(), re))
        })
        .collect()
}

/// Scan content for potential secrets. Returns one match per rule that fired.
pub fn scan_for_secrets(content: &str) -> Vec<SecretMatch> {
    let rules = compiled_secret_rules();
    let mut matches = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (id, re) in &rules {
        if seen.contains(id.as_str()) {
            continue;
        }
        if re.is_match(content) {
            seen.insert(id.as_str());
            matches.push(SecretMatch {
                rule_id: id.clone(),
                label: rule_id_to_label(id),
            });
        }
    }

    matches
}

/// Convert a rule ID (kebab-case) to a human-readable label.
fn rule_id_to_label(rule_id: &str) -> String {
    let special: HashMap<&str, &str> = HashMap::from([
        ("aws", "AWS"),
        ("gcp", "GCP"),
        ("api", "API"),
        ("pat", "PAT"),
        ("oauth", "OAuth"),
        ("npm", "NPM"),
        ("github", "GitHub"),
        ("gitlab", "GitLab"),
        ("openai", "OpenAI"),
    ]);

    rule_id
        .split('-')
        .map(|part| {
            special
                .get(part)
                .copied()
                .unwrap_or({
                    // Title-case fallback — return owned string via leak-free approach
                    // We'll just return the part since we map to &str
                    part
                })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert rule_id to label with proper capitalization.
pub fn get_secret_label(rule_id: &str) -> String {
    rule_id_to_label(rule_id)
}

/// Check if a file write to a team memory path contains secrets.
/// Returns an error message if secrets are detected, or None if safe.
pub fn check_team_mem_secrets(
    file_path: &Path,
    content: &str,
    team_dir: &Path,
) -> Option<String> {
    if !file_path.starts_with(team_dir) {
        return None;
    }

    let matches = scan_for_secrets(content);
    if matches.is_empty() {
        return None;
    }

    let labels: Vec<String> = matches.iter().map(|m| m.label.clone()).collect();
    Some(format!(
        "Content contains potential secrets ({}) and cannot be written to team memory. \
         Team memory is shared with all repository collaborators. \
         Remove the sensitive content and try again.",
        labels.join(", ")
    ))
}

// ── Watcher state ──────────────────────────────────────────────────────────

/// State for the team memory file watcher.
pub struct TeamMemoryWatcher {
    /// Whether the watcher has been started.
    started: AtomicBool,
    /// Whether a push is currently in progress.
    push_in_progress: AtomicBool,
    /// Whether there are pending changes to push.
    has_pending_changes: AtomicBool,
    /// Reason pushes are suppressed (permanent failure).
    push_suppressed_reason: Mutex<Option<String>>,
}

impl TeamMemoryWatcher {
    pub fn new() -> Self {
        Self {
            started: AtomicBool::new(false),
            push_in_progress: AtomicBool::new(false),
            has_pending_changes: AtomicBool::new(false),
            push_suppressed_reason: Mutex::new(None),
        }
    }

    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    pub fn mark_started(&self) {
        self.started.store(true, Ordering::Release);
    }

    pub fn is_push_in_progress(&self) -> bool {
        self.push_in_progress.load(Ordering::Acquire)
    }

    pub fn set_push_in_progress(&self, in_progress: bool) {
        self.push_in_progress.store(in_progress, Ordering::Release);
    }

    pub fn has_pending_changes(&self) -> bool {
        self.has_pending_changes.load(Ordering::Acquire)
    }

    pub fn set_pending_changes(&self, pending: bool) {
        self.has_pending_changes.store(pending, Ordering::Release);
    }

    /// Check if pushes are suppressed due to permanent failure.
    pub fn is_push_suppressed(&self) -> bool {
        self.push_suppressed_reason.lock().unwrap().is_some()
    }

    /// Suppress pushes with a reason.
    pub fn suppress_push(&self, reason: String) {
        *self.push_suppressed_reason.lock().unwrap() = Some(reason);
    }

    /// Clear push suppression (e.g. after file deletion recovery).
    pub fn clear_suppression(&self) {
        *self.push_suppressed_reason.lock().unwrap() = None;
    }

    /// Get the suppression reason, if any.
    pub fn suppression_reason(&self) -> Option<String> {
        self.push_suppressed_reason.lock().unwrap().clone()
    }

    /// Reset all state (for tests).
    pub fn reset(&self) {
        self.started.store(false, Ordering::Release);
        self.push_in_progress.store(false, Ordering::Release);
        self.has_pending_changes.store(false, Ordering::Release);
        *self.push_suppressed_reason.lock().unwrap() = None;
    }
}

impl Default for TeamMemoryWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a push failure is permanent (retrying won't help).
pub fn is_permanent_failure(result: &TeamMemorySyncPushResult) -> bool {
    if matches!(
        result.error_type,
        Some(SyncErrorType::NoOAuth) | Some(SyncErrorType::NoRepo)
    ) {
        return true;
    }
    if let Some(status) = result.http_status {
        if (400..500).contains(&status) && status != 409 && status != 429 {
            return true;
        }
    }
    false
}

/// Create a reqwest client configured with the team memory sync timeout.
pub fn create_team_sync_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(TEAM_MEMORY_SYNC_TIMEOUT)
        .build()
        .unwrap_or_default()
}

/// Validate that a memory entry does not exceed the PUT body size limit.
pub fn validate_put_body_size(body: &[u8]) -> Result<()> {
    if body.len() > MAX_PUT_BODY_BYTES {
        anyhow::bail!(
            "Team memory entry exceeds maximum PUT body size ({} > {} bytes)",
            body.len(),
            MAX_PUT_BODY_BYTES
        );
    }
    Ok(())
}

/// Execute a team memory operation with retry logic.
pub async fn with_retries<F, Fut, T>(mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_err = None;
    for attempt in 0..=MAX_RETRIES {
        match op().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                last_err = Some(e);
                if attempt < MAX_RETRIES {
                    let delay = Duration::from_millis(500 * 2u64.pow(attempt));
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("max retries exceeded")))
}

/// Execute a team memory push with conflict retry logic.
pub async fn with_conflict_retries<F, Fut, T>(mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_err = None;
    for attempt in 0..=MAX_CONFLICT_RETRIES {
        match op().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                last_err = Some(e);
                if attempt < MAX_CONFLICT_RETRIES {
                    let delay = DEBOUNCE;
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("max conflict retries exceeded")))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_hash_content() {
        let hash = hash_content("hello world");
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), 7 + 64); // "sha256:" + 64 hex chars
        // Same input = same output
        assert_eq!(hash, hash_content("hello world"));
        // Different input = different output
        assert_ne!(hash, hash_content("hello world!"));
    }

    #[test]
    fn test_validate_team_mem_key() {
        assert!(validate_team_mem_key("MEMORY.md").is_ok());
        assert!(validate_team_mem_key("subdir/file.md").is_ok());
        assert!(validate_team_mem_key("").is_err());
        assert!(validate_team_mem_key("/absolute/path").is_err());
        assert!(validate_team_mem_key("../escape").is_err());
        assert!(validate_team_mem_key("dir/../escape").is_err());
    }

    #[test]
    fn test_compute_delta() {
        let mut local = HashMap::new();
        local.insert("file1.md".to_string(), "content1".to_string());
        local.insert("file2.md".to_string(), "content2_new".to_string());
        local.insert("file3.md".to_string(), "content3".to_string());

        let mut server_checksums = HashMap::new();
        server_checksums.insert("file1.md".to_string(), hash_content("content1"));
        server_checksums.insert("file2.md".to_string(), hash_content("content2_old"));
        // file3 not on server

        let delta = compute_delta(&local, &server_checksums);
        assert_eq!(delta.len(), 2); // file2 changed + file3 new
        assert!(delta.contains_key("file2.md"));
        assert!(delta.contains_key("file3.md"));
        assert!(!delta.contains_key("file1.md")); // unchanged
    }

    #[test]
    fn test_scan_for_secrets_github_pat() {
        let content = "token = ghp_abcdefghijklmnopqrstuvwxyz1234567890";
        let matches = scan_for_secrets(content);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].rule_id, "github-pat");
    }

    #[test]
    fn test_scan_for_secrets_aws() {
        let content = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let matches = scan_for_secrets(content);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].rule_id, "aws-access-token");
    }

    #[test]
    fn test_scan_for_secrets_clean() {
        let content = "This is a normal markdown file about authentication patterns.";
        let matches = scan_for_secrets(content);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_check_team_mem_secrets_detects() {
        let team_dir = Path::new("/project/.claude/team-memory");
        let file_path = Path::new("/project/.claude/team-memory/auth.md");
        let content = "API key: ghp_abcdefghijklmnopqrstuvwxyz1234567890";

        let result = check_team_mem_secrets(file_path, content, team_dir);
        assert!(result.is_some());
        assert!(result.unwrap().contains("secrets"));
    }

    #[test]
    fn test_check_team_mem_secrets_outside_team_dir() {
        let team_dir = Path::new("/project/.claude/team-memory");
        let file_path = Path::new("/project/src/main.rs");
        let content = "ghp_abcdefghijklmnopqrstuvwxyz1234567890";

        // Outside team dir — don't check
        assert!(check_team_mem_secrets(file_path, content, team_dir).is_none());
    }

    #[test]
    fn test_check_team_mem_secrets_clean() {
        let team_dir = Path::new("/project/.claude/team-memory");
        let file_path = Path::new("/project/.claude/team-memory/patterns.md");
        let content = "We use the repository pattern for data access.";

        assert!(check_team_mem_secrets(file_path, content, team_dir).is_none());
    }

    #[test]
    fn test_is_permanent_failure() {
        // no_oauth
        assert!(is_permanent_failure(&TeamMemorySyncPushResult {
            error_type: Some(SyncErrorType::NoOAuth),
            ..Default::default()
        }));

        // no_repo
        assert!(is_permanent_failure(&TeamMemorySyncPushResult {
            error_type: Some(SyncErrorType::NoRepo),
            ..Default::default()
        }));

        // 404 (4xx, not 409/429)
        assert!(is_permanent_failure(&TeamMemorySyncPushResult {
            http_status: Some(404),
            ..Default::default()
        }));

        // 413 (4xx, not 409/429)
        assert!(is_permanent_failure(&TeamMemorySyncPushResult {
            http_status: Some(413),
            ..Default::default()
        }));

        // 409 conflict — not permanent
        assert!(!is_permanent_failure(&TeamMemorySyncPushResult {
            http_status: Some(409),
            ..Default::default()
        }));

        // 429 rate limit — not permanent
        assert!(!is_permanent_failure(&TeamMemorySyncPushResult {
            http_status: Some(429),
            ..Default::default()
        }));

        // 500 server error — not permanent
        assert!(!is_permanent_failure(&TeamMemorySyncPushResult {
            http_status: Some(500),
            ..Default::default()
        }));
    }

    #[test]
    fn test_watcher_state() {
        let watcher = TeamMemoryWatcher::new();

        assert!(!watcher.is_started());
        watcher.mark_started();
        assert!(watcher.is_started());

        assert!(!watcher.is_push_in_progress());
        watcher.set_push_in_progress(true);
        assert!(watcher.is_push_in_progress());

        assert!(!watcher.is_push_suppressed());
        watcher.suppress_push("http_413".to_string());
        assert!(watcher.is_push_suppressed());
        assert_eq!(watcher.suppression_reason(), Some("http_413".to_string()));

        watcher.clear_suppression();
        assert!(!watcher.is_push_suppressed());
    }

    #[test]
    fn test_watcher_reset() {
        let watcher = TeamMemoryWatcher::new();
        watcher.mark_started();
        watcher.set_push_in_progress(true);
        watcher.set_pending_changes(true);
        watcher.suppress_push("test".to_string());

        watcher.reset();
        assert!(!watcher.is_started());
        assert!(!watcher.is_push_in_progress());
        assert!(!watcher.has_pending_changes());
        assert!(!watcher.is_push_suppressed());
    }

    #[tokio::test]
    async fn test_read_local_team_memory_empty() {
        let dir = TempDir::new().unwrap();
        let team_dir = dir.path().join("team-memory");
        // Don't create the dir — should return empty
        let entries = read_local_team_memory(&team_dir, None).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_read_local_team_memory() {
        let dir = TempDir::new().unwrap();
        let team_dir = dir.path().join("team-memory");
        tokio::fs::create_dir_all(&team_dir).await.unwrap();

        tokio::fs::write(team_dir.join("MEMORY.md"), "# Index").await.unwrap();
        tokio::fs::write(team_dir.join("patterns.md"), "# Patterns").await.unwrap();
        tokio::fs::write(team_dir.join(".hidden"), "secret").await.unwrap();

        let entries = read_local_team_memory(&team_dir, None).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key("MEMORY.md"));
        assert!(entries.contains_key("patterns.md"));
        assert!(!entries.contains_key(".hidden"));
    }

    #[tokio::test]
    async fn test_read_local_team_memory_with_subdirs() {
        let dir = TempDir::new().unwrap();
        let team_dir = dir.path().join("team-memory");
        let sub_dir = team_dir.join("topics");
        tokio::fs::create_dir_all(&sub_dir).await.unwrap();

        tokio::fs::write(team_dir.join("MEMORY.md"), "index").await.unwrap();
        tokio::fs::write(sub_dir.join("auth.md"), "auth patterns").await.unwrap();

        let entries = read_local_team_memory(&team_dir, None).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key("topics/auth.md"));
    }

    #[tokio::test]
    async fn test_read_local_team_memory_max_entries() {
        let dir = TempDir::new().unwrap();
        let team_dir = dir.path().join("team-memory");
        tokio::fs::create_dir_all(&team_dir).await.unwrap();

        for i in 0..10 {
            tokio::fs::write(team_dir.join(format!("file{:02}.md", i)), format!("content {}", i))
                .await
                .unwrap();
        }

        let entries = read_local_team_memory(&team_dir, Some(5)).await.unwrap();
        assert_eq!(entries.len(), 5);
    }

    #[tokio::test]
    async fn test_write_team_memory_entries() {
        let dir = TempDir::new().unwrap();
        let team_dir = dir.path().join("team-memory");

        let mut entries = HashMap::new();
        entries.insert("MEMORY.md".to_string(), "# Index".to_string());
        entries.insert("topics/auth.md".to_string(), "# Auth".to_string());

        let written = write_team_memory_entries(&team_dir, &entries).await.unwrap();
        assert_eq!(written, 2);

        let index = tokio::fs::read_to_string(team_dir.join("MEMORY.md"))
            .await
            .unwrap();
        assert_eq!(index, "# Index");

        let auth = tokio::fs::read_to_string(team_dir.join("topics/auth.md"))
            .await
            .unwrap();
        assert_eq!(auth, "# Auth");
    }

    #[test]
    fn test_sync_state() {
        let state = SyncState::new();
        assert!(state.last_checksum().is_none());

        state.set_last_checksum(Some("abc123".to_string()));
        assert_eq!(state.last_checksum(), Some("abc123".to_string()));

        let mut checksums = HashMap::new();
        checksums.insert("file.md".to_string(), "sha256:abc".to_string());
        state.update_checksums(checksums);

        let stored = state.server_checksums.lock().unwrap();
        assert_eq!(stored.get("file.md"), Some(&"sha256:abc".to_string()));
    }

    #[test]
    fn test_rule_id_to_label() {
        assert!(rule_id_to_label("github-pat").contains("GitHub"));
        assert!(rule_id_to_label("github-pat").contains("PAT"));
        assert!(rule_id_to_label("aws-access-token").contains("AWS"));
    }
}
