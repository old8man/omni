use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// A single release note entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseNote {
    /// Version string (e.g., "1.2.0").
    pub version: String,

    /// Release date as a string (e.g., "2026-03-15").
    pub date: String,

    /// Markdown-formatted release notes body.
    pub body: String,
}

/// Load release notes from a local file.
///
/// The file is expected to contain a JSON array of [`ReleaseNote`] objects,
/// sorted from newest to oldest.
pub fn load_release_notes(path: &Path) -> Result<Vec<ReleaseNote>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading release notes from {}", path.display()))?;
    let notes: Vec<ReleaseNote> = serde_json::from_str(&content)
        .with_context(|| format!("parsing release notes from {}", path.display()))?;
    debug!(count = notes.len(), "loaded release notes");
    Ok(notes)
}

/// Get release notes for versions newer than `since_version`.
///
/// Returns notes in chronological order (newest first).
pub fn notes_since_version<'a>(
    notes: &'a [ReleaseNote],
    since_version: &str,
) -> Vec<&'a ReleaseNote> {
    use super::auto_updater::version_is_newer;

    notes
        .iter()
        .filter(|note| version_is_newer(&note.version, since_version))
        .collect()
}

/// Format release notes for display in the terminal.
pub fn format_release_notes(notes: &[&ReleaseNote]) -> String {
    if notes.is_empty() {
        return "No new release notes.".to_string();
    }

    let mut output = String::new();

    for note in notes {
        output.push_str(&format!("## {} ({})\n\n", note.version, note.date));
        output.push_str(&note.body);
        output.push_str("\n\n");
    }

    output.trim_end().to_string()
}

/// Check if the user should be shown release notes (new version since last seen).
///
/// The "last seen" version is stored in `<config_dir>/last_seen_version.txt`.
pub fn should_show_release_notes(config_dir: &Path, current_version: &str) -> bool {
    let path = config_dir.join("last_seen_version.txt");
    match std::fs::read_to_string(&path) {
        Ok(last_seen) => {
            let last_seen = last_seen.trim();
            if last_seen == current_version {
                return false;
            }
            super::auto_updater::version_is_newer(current_version, last_seen)
        }
        Err(_) => {
            // First run or missing file — show notes
            true
        }
    }
}

/// Mark the current version as "seen" so release notes aren't shown again.
pub fn mark_version_seen(config_dir: &Path, current_version: &str) -> Result<()> {
    let path = config_dir.join("last_seen_version.txt");
    std::fs::write(&path, current_version)
        .with_context(|| format!("writing {}", path.display()))?;
    info!(version = current_version, "marked version as seen");
    Ok(())
}
