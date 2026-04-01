//! Track files read/written during a session, with snapshot support
//! for post-compact state restoration.

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet, VecDeque};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A backup record for a single tracked file at a point in time.
#[derive(Clone, Debug)]
pub struct FileHistoryBackup {
    /// Name of the backup file on disk, or `None` if the file didn't exist.
    pub backup_file_name: Option<String>,
    /// Monotonically increasing version within a file's history.
    pub version: u32,
    /// When the backup was created.
    pub backup_time: DateTime<Utc>,
}

/// A snapshot captures the state of all tracked files at a specific point.
#[derive(Clone, Debug)]
pub struct FileHistorySnapshot {
    /// The message ID this snapshot is associated with.
    pub message_id: Uuid,
    /// Map from absolute file path to its backup at snapshot time.
    pub tracked_file_backups: HashMap<String, FileHistoryBackup>,
    /// When the snapshot was taken.
    pub timestamp: DateTime<Utc>,
}

/// Mutable session-level file history state.
#[derive(Clone, Debug)]
pub struct FileHistoryState {
    /// Ordered list of snapshots (oldest first), capped at `MAX_SNAPSHOTS`.
    pub snapshots: VecDeque<FileHistorySnapshot>,
    /// Set of all file paths being tracked this session.
    pub tracked_files: HashSet<String>,
    /// Monotonically increasing counter incremented on every snapshot, even
    /// when old snapshots are evicted.
    pub snapshot_sequence: u64,
}

const MAX_SNAPSHOTS: usize = 100;

impl Default for FileHistoryState {
    fn default() -> Self {
        Self::new()
    }
}

impl FileHistoryState {
    pub fn new() -> Self {
        Self {
            snapshots: VecDeque::new(),
            tracked_files: HashSet::new(),
            snapshot_sequence: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Track file edits
    // -----------------------------------------------------------------------

    /// Record that a file is about to be modified. Should be called *before*
    /// the actual write so we capture the pre-edit content.
    ///
    /// If the file is already tracked in the most recent snapshot, this is a no-op.
    pub fn track_edit(&mut self, file_path: &str, backup: FileHistoryBackup) {
        self.tracked_files.insert(file_path.to_string());

        if let Some(latest) = self.snapshots.back_mut() {
            if latest.tracked_file_backups.contains_key(file_path) {
                return;
            }
            latest
                .tracked_file_backups
                .insert(file_path.to_string(), backup);
        }
    }

    // -----------------------------------------------------------------------
    // Snapshots
    // -----------------------------------------------------------------------

    /// Create a new snapshot, optionally including fresh backups for tracked files
    /// that have changed since the last snapshot.
    pub fn make_snapshot(
        &mut self,
        message_id: Uuid,
        backups: HashMap<String, FileHistoryBackup>,
    ) {
        let snapshot = FileHistorySnapshot {
            message_id,
            tracked_file_backups: backups,
            timestamp: Utc::now(),
        };

        self.snapshots.push_back(snapshot);
        self.snapshot_sequence += 1;

        // Evict oldest snapshots if over cap.
        while self.snapshots.len() > MAX_SNAPSHOTS {
            self.snapshots.pop_front();
        }
    }

    /// Find the snapshot associated with a particular message ID.
    pub fn find_snapshot(&self, message_id: &Uuid) -> Option<&FileHistorySnapshot> {
        self.snapshots
            .iter()
            .rev()
            .find(|s| s.message_id == *message_id)
    }

    // -----------------------------------------------------------------------
    // Recently accessed files
    // -----------------------------------------------------------------------

    /// Get the set of all files that have been tracked (read or written) this session.
    pub fn tracked_file_paths(&self) -> &HashSet<String> {
        &self.tracked_files
    }

    /// Get the list of files from the most recent snapshot (recently touched).
    pub fn recently_accessed_files(&self) -> Vec<String> {
        match self.snapshots.back() {
            Some(snap) => snap.tracked_file_backups.keys().cloned().collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state() {
        let state = FileHistoryState::new();
        assert!(state.snapshots.is_empty());
        assert!(state.tracked_files.is_empty());
        assert_eq!(state.snapshot_sequence, 0);
    }

    #[test]
    fn test_make_snapshot() {
        let mut state = FileHistoryState::new();
        let id = Uuid::new_v4();
        let mut backups = HashMap::new();
        backups.insert(
            "/tmp/foo.rs".to_string(),
            FileHistoryBackup {
                backup_file_name: Some("foo_v1".to_string()),
                version: 1,
                backup_time: Utc::now(),
            },
        );
        state.make_snapshot(id, backups);
        assert_eq!(state.snapshots.len(), 1);
        assert_eq!(state.snapshot_sequence, 1);
        assert!(state.find_snapshot(&id).is_some());
    }

    #[test]
    fn test_eviction() {
        let mut state = FileHistoryState::new();
        for i in 0..150 {
            let id = Uuid::new_v4();
            let mut backups = HashMap::new();
            backups.insert(
                format!("/tmp/file_{i}.rs"),
                FileHistoryBackup {
                    backup_file_name: None,
                    version: 1,
                    backup_time: Utc::now(),
                },
            );
            state.make_snapshot(id, backups);
        }
        assert_eq!(state.snapshots.len(), MAX_SNAPSHOTS);
        assert_eq!(state.snapshot_sequence, 150);
    }

    #[test]
    fn test_track_edit() {
        let mut state = FileHistoryState::new();
        let id = Uuid::new_v4();
        state.make_snapshot(id, HashMap::new());

        state.track_edit(
            "/tmp/bar.rs",
            FileHistoryBackup {
                backup_file_name: Some("bar_v1".to_string()),
                version: 1,
                backup_time: Utc::now(),
            },
        );

        assert!(state.tracked_files.contains("/tmp/bar.rs"));
        let snap = state.snapshots.back().unwrap();
        assert!(snap.tracked_file_backups.contains_key("/tmp/bar.rs"));
    }

    #[test]
    fn test_recently_accessed_files() {
        let mut state = FileHistoryState::new();
        let mut backups = HashMap::new();
        backups.insert(
            "/a.rs".to_string(),
            FileHistoryBackup {
                backup_file_name: None,
                version: 1,
                backup_time: Utc::now(),
            },
        );
        backups.insert(
            "/b.rs".to_string(),
            FileHistoryBackup {
                backup_file_name: None,
                version: 1,
                backup_time: Utc::now(),
            },
        );
        state.make_snapshot(Uuid::new_v4(), backups);

        let recent = state.recently_accessed_files();
        assert_eq!(recent.len(), 2);
    }
}
