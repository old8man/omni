//! Git diff generation, parsing, and hunk representation.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Statistics for an entire diff.
#[derive(Clone, Debug, Default)]
pub struct GitDiffStats {
    pub files_count: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
}

/// Per-file diff statistics.
#[derive(Clone, Debug)]
pub struct PerFileStats {
    pub added: usize,
    pub removed: usize,
    pub is_binary: bool,
    pub is_untracked: bool,
}

/// A parsed hunk from a unified diff.
#[derive(Clone, Debug)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<String>,
}

/// Full diff result.
#[derive(Clone, Debug)]
pub struct GitDiffResult {
    pub stats: GitDiffStats,
    pub per_file_stats: HashMap<String, PerFileStats>,
    pub hunks: HashMap<String, Vec<DiffHunk>>,
}

/// Diff result for a single file (tool-use context).
#[derive(Clone, Debug)]
pub struct ToolUseDiff {
    pub filename: String,
    pub status: DiffStatus,
    pub additions: usize,
    pub deletions: usize,
    pub changes: usize,
    pub patch: String,
    pub repository: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffStatus {
    Modified,
    Added,
}

const MAX_FILES: usize = 50;
const MAX_DIFF_SIZE_BYTES: usize = 1_000_000;
const MAX_LINES_PER_FILE: usize = 400;

// ---------------------------------------------------------------------------
// Diff generation
// ---------------------------------------------------------------------------

/// Generate a unified diff string for a git repository at `repo_path`.
/// Diffs the working tree against HEAD.
pub fn generate_diff(repo_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["--no-optional-locks", "diff", "HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let diff = String::from_utf8_lossy(&output.stdout).to_string();
    if diff.trim().is_empty() {
        return None;
    }
    Some(diff)
}

/// Generate a numstat diff for the repo.
pub fn generate_numstat(repo_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["--no-optional-locks", "diff", "HEAD", "--numstat"])
        .current_dir(repo_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

// ---------------------------------------------------------------------------
// Numstat parsing
// ---------------------------------------------------------------------------

/// Parse `git diff --numstat` output into stats.
pub fn parse_git_numstat(stdout: &str) -> (GitDiffStats, HashMap<String, PerFileStats>) {
    let mut added_total = 0usize;
    let mut removed_total = 0usize;
    let mut file_count = 0usize;
    let mut per_file = HashMap::new();

    for line in stdout.trim().lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() < 3 {
            continue;
        }

        file_count += 1;
        let is_binary = parts[0] == "-" || parts[1] == "-";
        let file_added = if is_binary {
            0
        } else {
            parts[0].parse::<usize>().unwrap_or(0)
        };
        let file_removed = if is_binary {
            0
        } else {
            parts[1].parse::<usize>().unwrap_or(0)
        };

        added_total += file_added;
        removed_total += file_removed;

        if per_file.len() < MAX_FILES {
            per_file.insert(
                parts[2].to_string(),
                PerFileStats {
                    added: file_added,
                    removed: file_removed,
                    is_binary,
                    is_untracked: false,
                },
            );
        }
    }

    let stats = GitDiffStats {
        files_count: file_count,
        lines_added: added_total,
        lines_removed: removed_total,
    };

    (stats, per_file)
}

// ---------------------------------------------------------------------------
// Shortstat parsing
// ---------------------------------------------------------------------------

/// Parse `git diff --shortstat` output.
pub fn parse_shortstat(stdout: &str) -> Option<GitDiffStats> {
    let re = lazy_regex::regex!(
        r"(\d+)\s+files?\s+changed(?:,\s+(\d+)\s+insertions?\(\+\))?(?:,\s+(\d+)\s+deletions?\(-\))?"
    );
    let caps = re.captures(stdout)?;

    Some(GitDiffStats {
        files_count: caps.get(1)?.as_str().parse().ok()?,
        lines_added: caps
            .get(2)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0),
        lines_removed: caps
            .get(3)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0),
    })
}

// ---------------------------------------------------------------------------
// Unified diff parsing
// ---------------------------------------------------------------------------

/// Parse unified diff output into per-file hunks.
pub fn parse_diff(stdout: &str) -> HashMap<String, Vec<DiffHunk>> {
    let mut result: HashMap<String, Vec<DiffHunk>> = HashMap::new();
    if stdout.trim().is_empty() {
        return result;
    }

    // Split by file diffs.
    let file_diffs: Vec<&str> = stdout
        .split("diff --git ")
        .filter(|s| !s.is_empty())
        .collect();

    for file_diff in file_diffs {
        if result.len() >= MAX_FILES {
            break;
        }
        if file_diff.len() > MAX_DIFF_SIZE_BYTES {
            continue;
        }

        let mut lines_iter = file_diff.lines();
        let header_line = match lines_iter.next() {
            Some(l) => l,
            None => continue,
        };

        // Extract filename from "a/path b/path".
        let file_path = match parse_diff_header_path(header_line) {
            Some(p) => p,
            None => continue,
        };

        let mut file_hunks: Vec<DiffHunk> = Vec::new();
        let mut current_hunk: Option<DiffHunk> = None;
        let mut line_count = 0usize;

        for line in lines_iter {
            // Hunk header.
            if let Some(hunk) = parse_hunk_header(line) {
                if let Some(h) = current_hunk.take() {
                    file_hunks.push(h);
                }
                current_hunk = Some(hunk);
                continue;
            }

            // Skip metadata lines.
            if line.starts_with("index ")
                || line.starts_with("---")
                || line.starts_with("+++")
                || line.starts_with("new file")
                || line.starts_with("deleted file")
                || line.starts_with("old mode")
                || line.starts_with("new mode")
                || line.starts_with("Binary files")
            {
                continue;
            }

            // Diff content lines.
            if let Some(ref mut hunk) = current_hunk {
                if (line.starts_with('+')
                    || line.starts_with('-')
                    || line.starts_with(' ')
                    || line.is_empty())
                    && line_count < MAX_LINES_PER_FILE
                {
                    hunk.lines.push(line.to_string());
                    line_count += 1;
                }
            }
        }

        if let Some(h) = current_hunk {
            file_hunks.push(h);
        }

        if !file_hunks.is_empty() {
            result.insert(file_path, file_hunks);
        }
    }

    result
}

/// Extract filename from diff header line: `"a/path/to/file b/path/to/file"`.
fn parse_diff_header_path(header: &str) -> Option<String> {
    let re = lazy_regex::regex!(r"^a/(.+?) b/(.+)$");
    let caps = re.captures(header)?;
    caps.get(2)
        .or_else(|| caps.get(1))
        .map(|m| m.as_str().to_string())
}

/// Parse a hunk header line: `"@@ -1,3 +1,4 @@"`.
fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let re = lazy_regex::regex!(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@");
    let caps = re.captures(line)?;
    Some(DiffHunk {
        old_start: caps.get(1)?.as_str().parse().ok()?,
        old_lines: caps
            .get(2)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(1),
        new_start: caps.get(3)?.as_str().parse().ok()?,
        new_lines: caps
            .get(4)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(1),
        lines: Vec::new(),
    })
}

/// Parse a raw unified diff for a single file into a `ToolUseDiff`.
pub fn parse_raw_diff_to_tool_use_diff(
    filename: &str,
    raw_diff: &str,
    status: DiffStatus,
) -> ToolUseDiff {
    let mut patch_lines = Vec::new();
    let mut in_hunks = false;
    let mut additions = 0usize;
    let mut deletions = 0usize;

    for line in raw_diff.lines() {
        if line.starts_with("@@") {
            in_hunks = true;
        }
        if in_hunks {
            patch_lines.push(line.to_string());
            if line.starts_with('+') && !line.starts_with("+++") {
                additions += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                deletions += 1;
            }
        }
    }

    ToolUseDiff {
        filename: filename.to_string(),
        status,
        additions,
        deletions,
        changes: additions + deletions,
        patch: patch_lines.join("\n"),
        repository: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_numstat() {
        let input = "10\t5\tsrc/main.rs\n-\t-\timage.png\n";
        let (stats, per_file) = parse_git_numstat(input);
        assert_eq!(stats.files_count, 2);
        assert_eq!(stats.lines_added, 10);
        assert_eq!(stats.lines_removed, 5);
        assert_eq!(per_file["src/main.rs"].added, 10);
        assert!(per_file["image.png"].is_binary);
    }

    #[test]
    fn test_parse_shortstat() {
        let input = " 3 files changed, 100 insertions(+), 50 deletions(-)\n";
        let stats = parse_shortstat(input).unwrap();
        assert_eq!(stats.files_count, 3);
        assert_eq!(stats.lines_added, 100);
        assert_eq!(stats.lines_removed, 50);
    }

    #[test]
    fn test_parse_shortstat_no_deletions() {
        let input = " 1 file changed, 42 insertions(+)\n";
        let stats = parse_shortstat(input).unwrap();
        assert_eq!(stats.files_count, 1);
        assert_eq!(stats.lines_added, 42);
        assert_eq!(stats.lines_removed, 0);
    }

    #[test]
    fn test_parse_diff_basic() {
        let diff = r#"diff --git a/foo.rs b/foo.rs
index abc..def 100644
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,4 @@
 line1
+added
 line2
 line3
"#;
        let hunks = parse_diff(diff);
        assert!(hunks.contains_key("foo.rs"));
        let file_hunks = &hunks["foo.rs"];
        assert_eq!(file_hunks.len(), 1);
        assert_eq!(file_hunks[0].old_start, 1);
        assert_eq!(file_hunks[0].new_start, 1);
        assert_eq!(file_hunks[0].new_lines, 4);
    }

    #[test]
    fn test_parse_diff_empty() {
        let hunks = parse_diff("");
        assert!(hunks.is_empty());
    }

    #[test]
    fn test_parse_raw_diff_to_tool_use_diff() {
        let raw = "--- a/foo.rs\n+++ b/foo.rs\n@@ -1,2 +1,3 @@\n line1\n+added\n line2\n";
        let result = parse_raw_diff_to_tool_use_diff("foo.rs", raw, DiffStatus::Modified);
        assert_eq!(result.additions, 1);
        assert_eq!(result.deletions, 0);
        assert_eq!(result.changes, 1);
        assert_eq!(result.status, DiffStatus::Modified);
    }
}
