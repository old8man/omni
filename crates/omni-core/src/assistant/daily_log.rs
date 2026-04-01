use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Category for a daily log entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogCategory {
    /// An observation about the codebase, user, or environment.
    Observation,
    /// A decision made by the assistant.
    Decision,
    /// An action taken (commit, PR, file change, etc.).
    Action,
    /// An error or failure encountered during operation.
    Error,
}

impl std::fmt::Display for LogCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Observation => write!(f, "observation"),
            Self::Decision => write!(f, "decision"),
            Self::Action => write!(f, "action"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// A single entry in the daily log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// ISO timestamp of when the entry was created.
    pub timestamp: String,
    /// Category of the entry.
    pub category: LogCategory,
    /// The log content.
    pub content: String,
}

impl LogEntry {
    /// Create a new log entry with the current timestamp.
    pub fn new(category: LogCategory, content: impl Into<String>) -> Self {
        Self {
            timestamp: Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            category,
            content: content.into(),
        }
    }

    /// Format this entry as a markdown bullet for the daily log file.
    pub fn to_markdown(&self) -> String {
        format!(
            "- [{}] [{}] {}",
            self.timestamp, self.category, self.content
        )
    }
}

/// Append-only daily log manager for KAIROS assistant mode.
///
/// Logs are stored as date-named markdown files under the memory directory:
/// `<memory_dir>/logs/YYYY/MM/YYYY-MM-DD.md`
///
/// A separate nightly process (the /dream skill) distills these logs
/// into MEMORY.md and topic files.
pub struct DailyLog {
    /// Root directory for memory files (e.g., `~/.claude/projects/<id>/memory/`).
    memory_dir: PathBuf,
}

impl DailyLog {
    /// Create a new daily log manager.
    pub fn new(memory_dir: impl Into<PathBuf>) -> Self {
        Self {
            memory_dir: memory_dir.into(),
        }
    }

    /// Get the path to the daily log file for a given date.
    pub fn log_path_for_date(&self, date: NaiveDate) -> PathBuf {
        let yyyy = date.format("%Y").to_string();
        let mm = date.format("%m").to_string();
        let filename = date.format("%Y-%m-%d.md").to_string();
        self.memory_dir
            .join("logs")
            .join(&yyyy)
            .join(&mm)
            .join(filename)
    }

    /// Get the path to today's daily log file.
    pub fn today_log_path(&self) -> PathBuf {
        self.log_path_for_date(Local::now().date_naive())
    }

    /// Get the path pattern string for the daily log (with YYYY/MM/DD placeholders).
    pub fn log_path_pattern(&self) -> String {
        self.memory_dir
            .join("logs")
            .join("YYYY")
            .join("MM")
            .join("YYYY-MM-DD.md")
            .to_string_lossy()
            .to_string()
    }

    /// Append a log entry to today's daily log file.
    ///
    /// Creates the file and parent directories if they don't exist.
    pub fn append(&self, entry: &LogEntry) -> Result<()> {
        let path = self.today_log_path();
        self.append_to_path(&path, entry)
    }

    /// Append a log entry to a specific log file path.
    fn append_to_path(&self, path: &Path, entry: &LogEntry) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create log directory {}", parent.display()))?;
        }
        let line = format!("{}\n", entry.to_markdown());
        debug!(path = %path.display(), category = %entry.category, "appending to daily log");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("failed to open daily log {}", path.display()))?
            .write_all(line.as_bytes())
            .context("failed to write log entry")?;
        Ok(())
    }

    /// Read all entries from a specific date's log file.
    pub fn read_entries(&self, date: NaiveDate) -> Result<Vec<String>> {
        let path = self.log_path_for_date(date);
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read daily log {}", path.display()))?;
        Ok(content.lines().map(String::from).collect())
    }

    /// Load all log lines from today's log file.
    pub fn load_today(&self) -> Result<Vec<String>> {
        self.read_entries(Local::now().date_naive())
    }

    /// Load all log lines from a specific date's log file.
    pub fn load_date(&self, date: NaiveDate) -> Result<Vec<String>> {
        self.read_entries(date)
    }

    /// List all available log dates by scanning the logs directory.
    pub fn available_dates(&self) -> Result<Vec<NaiveDate>> {
        let logs_dir = self.memory_dir.join("logs");
        if !logs_dir.exists() {
            return Ok(vec![]);
        }
        let mut dates = Vec::new();
        for year_entry in std::fs::read_dir(&logs_dir)? {
            let year_entry = year_entry?;
            if !year_entry.file_type()?.is_dir() {
                continue;
            }
            for month_entry in std::fs::read_dir(year_entry.path())? {
                let month_entry = month_entry?;
                if !month_entry.file_type()?.is_dir() {
                    continue;
                }
                for file_entry in std::fs::read_dir(month_entry.path())? {
                    let file_entry = file_entry?;
                    let name = file_entry.file_name();
                    let name_str = name.to_string_lossy();
                    if let Some(date_str) = name_str.strip_suffix(".md") {
                        if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                            dates.push(date);
                        }
                    }
                }
            }
        }
        dates.sort();
        Ok(dates)
    }
}

use std::io::Write;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_entry_markdown() {
        let entry = LogEntry {
            timestamp: "2026-04-01T14:30:00".to_string(),
            category: LogCategory::Decision,
            content: "Using async_trait for the SttProvider".to_string(),
        };
        assert_eq!(
            entry.to_markdown(),
            "- [2026-04-01T14:30:00] [decision] Using async_trait for the SttProvider"
        );
    }

    #[test]
    fn test_log_path_for_date() {
        let log = DailyLog::new("/tmp/memory");
        let date = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        let path = log.log_path_for_date(date);
        assert_eq!(
            path.to_string_lossy(),
            "/tmp/memory/logs/2026/04/2026-04-01.md"
        );
    }

    #[test]
    fn test_log_path_pattern() {
        let log = DailyLog::new("/tmp/memory");
        let pattern = log.log_path_pattern();
        assert!(pattern.contains("YYYY"));
        assert!(pattern.contains("MM"));
        assert!(pattern.contains("YYYY-MM-DD.md"));
    }

    #[test]
    fn test_append_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let log = DailyLog::new(dir.path());
        let entry = LogEntry::new(LogCategory::Action, "created a test file");

        log.append(&entry).unwrap();
        let today = Local::now().date_naive();
        let entries = log.read_entries(today).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].contains("created a test file"));
    }
}
