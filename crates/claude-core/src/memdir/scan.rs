//! Memory file scanning primitives.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use tokio::fs;
use tracing::debug;

use super::memory_types::{parse_frontmatter, MemoryType};

const MAX_MEMORY_FILES: usize = 200;
const FRONTMATTER_MAX_BYTES: usize = 2048;

/// Parsed header information from a single memory file.
#[derive(Clone, Debug)]
pub struct MemoryHeader {
    /// Relative path from memory directory (e.g. "subdir/file.md").
    pub filename: String,
    /// Absolute path to the file.
    pub file_path: PathBuf,
    /// File modification time in milliseconds since epoch.
    pub mtime_ms: u64,
    /// Description from frontmatter.
    pub description: Option<String>,
    /// Memory type from frontmatter.
    pub memory_type: Option<MemoryType>,
    /// Name from frontmatter.
    pub name: Option<String>,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// Scan a memory directory for `.md` files, read their frontmatter, and return
/// a header list sorted newest-first (capped at `MAX_MEMORY_FILES`).
///
/// Single-pass: reads frontmatter and stats in one pass, then sorts.
/// This halves syscalls vs a separate stat round for the common case (N <= 200).
pub async fn scan_memory_files(memory_dir: &Path) -> Vec<MemoryHeader> {
    match scan_inner(memory_dir).await {
        Ok(headers) => headers,
        Err(e) => {
            debug!("failed to scan memory directory {}: {e}", memory_dir.display());
            Vec::new()
        }
    }
}

async fn scan_inner(memory_dir: &Path) -> anyhow::Result<Vec<MemoryHeader>> {
    let mut headers = Vec::new();
    collect_md_files(memory_dir, memory_dir, &mut headers).await?;
    headers.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    headers.truncate(MAX_MEMORY_FILES);
    Ok(headers)
}

fn collect_md_files<'a>(
    base: &'a Path,
    dir: &'a Path,
    headers: &'a mut Vec<MemoryHeader>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;

            if file_type.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if !name.starts_with('.') {
                        collect_md_files(base, &path, headers).await?;
                    }
                }
            } else if file_type.is_file() {
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                if !name.ends_with(".md") || name == "MEMORY.md" { continue; }

                let metadata = fs::metadata(&path).await?;
                let mtime_ms = metadata.modified().ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let size_bytes = metadata.len();

                let content = read_file_head(&path, FRONTMATTER_MAX_BYTES).await;
                let (fm, _) = parse_frontmatter(&content);

                let relative = path.strip_prefix(base).unwrap_or(&path)
                    .to_string_lossy().to_string();

                headers.push(MemoryHeader {
                    filename: relative,
                    file_path: path,
                    mtime_ms,
                    description: fm.description,
                    memory_type: fm.memory_type,
                    name: fm.name,
                    size_bytes,
                });
            }
        }
        Ok(())
    })
}

async fn read_file_head(path: &Path, max_bytes: usize) -> String {
    match fs::read(path).await {
        Ok(bytes) => {
            let len = bytes.len().min(max_bytes);
            String::from_utf8_lossy(&bytes[..len]).to_string()
        }
        Err(_) => String::new(),
    }
}

/// Format memory headers as a text manifest.
///
/// One line per file with `[type] filename (timestamp): description`.
/// Used by both the recall selector prompt and the extraction-agent prompt.
pub fn format_memory_manifest(memories: &[MemoryHeader]) -> String {
    memories.iter().map(|m| {
        let tag = match &m.memory_type {
            Some(t) => format!("[{t}] "),
            None => String::new(),
        };
        let ts = format_timestamp_iso(m.mtime_ms);
        match &m.description {
            Some(desc) => format!("- {tag}{} ({ts}): {desc}", m.filename),
            None => format!("- {tag}{} ({ts})", m.filename),
        }
    }).collect::<Vec<_>>().join("\n")
}

/// Detect duplicate memories by comparing descriptions.
///
/// Returns pairs of indices into the headers array where the descriptions
/// match exactly. The first element of each pair is the older memory.
pub fn detect_duplicates(headers: &[MemoryHeader]) -> Vec<(usize, usize)> {
    let mut duplicates = Vec::new();
    for i in 0..headers.len() {
        for j in (i + 1)..headers.len() {
            if let (Some(a), Some(b)) = (&headers[i].description, &headers[j].description) {
                if !a.is_empty() && descriptions_match(a, b) {
                    duplicates.push((i, j));
                }
            }
        }
    }
    duplicates
}

/// Check if two descriptions are semantically duplicate.
///
/// Exact match, or after normalization (lowercase, whitespace collapse).
fn descriptions_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    normalize_description(a) == normalize_description(b)
}

/// Normalize a description for comparison.
fn normalize_description(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Merge duplicate memories by keeping the newer file and deleting the older.
///
/// Returns the number of files deleted. Only merges exact description duplicates.
/// The newer file (higher mtime_ms) is kept; the older is removed from disk.
pub async fn merge_duplicate_memories(memory_dir: &Path) -> usize {
    let headers = scan_memory_files(memory_dir).await;
    let duplicates = detect_duplicates(&headers);
    let mut deleted = 0;

    for (i, j) in &duplicates {
        let (older_idx, _newer_idx) = if headers[*i].mtime_ms < headers[*j].mtime_ms {
            (*i, *j)
        } else {
            (*j, *i)
        };

        let older_path = &headers[older_idx].file_path;
        match fs::remove_file(older_path).await {
            Ok(()) => {
                debug!("merged duplicate memory, removed: {}", older_path.display());
                deleted += 1;
            }
            Err(e) => {
                debug!("failed to remove duplicate memory {}: {e}", older_path.display());
            }
        }
    }

    deleted
}

fn format_timestamp_iso(mtime_ms: u64) -> String {
    let secs = (mtime_ms / 1000) as i64;
    let nanos = ((mtime_ms % 1000) * 1_000_000) as u32;
    match chrono::DateTime::from_timestamp(secs, nanos) {
        Some(dt) => dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    fn now_ms() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
    }

    #[tokio::test]
    async fn test_scan_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let headers = scan_memory_files(tmp.path()).await;
        assert!(headers.is_empty());
    }

    #[tokio::test]
    async fn test_scan_nonexistent_directory() {
        let headers = scan_memory_files(Path::new("/nonexistent/path/memory")).await;
        assert!(headers.is_empty());
    }

    #[tokio::test]
    async fn test_scan_with_memory_files() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("test.md"),
            "---\nname: test\ndescription: a test memory\ntype: user\n---\n\nBody").await.unwrap();
        tokio::fs::write(tmp.path().join("MEMORY.md"), "# Index").await.unwrap();
        tokio::fs::write(tmp.path().join("notes.txt"), "notes").await.unwrap();

        let headers = scan_memory_files(tmp.path()).await;
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].filename, "test.md");
        assert_eq!(headers[0].description.as_deref(), Some("a test memory"));
        assert_eq!(headers[0].memory_type, Some(MemoryType::User));
        assert_eq!(headers[0].name.as_deref(), Some("test"));
    }

    #[tokio::test]
    async fn test_scan_recursive() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("subdir");
        tokio::fs::create_dir(&sub).await.unwrap();
        tokio::fs::write(tmp.path().join("root.md"),
            "---\nname: root\ndescription: root\ntype: project\n---\n").await.unwrap();
        tokio::fs::write(sub.join("nested.md"),
            "---\nname: nested\ndescription: nested\ntype: feedback\n---\n").await.unwrap();

        let headers = scan_memory_files(tmp.path()).await;
        assert_eq!(headers.len(), 2);
    }

    #[tokio::test]
    async fn test_scan_skips_hidden_dirs() {
        let tmp = TempDir::new().unwrap();
        let hidden = tmp.path().join(".hidden");
        tokio::fs::create_dir(&hidden).await.unwrap();
        tokio::fs::write(hidden.join("secret.md"), "---\nname: secret\n---\n").await.unwrap();

        let headers = scan_memory_files(tmp.path()).await;
        assert!(headers.is_empty());
    }

    #[test]
    fn test_format_memory_manifest() {
        let headers = vec![
            MemoryHeader {
                filename: "user_role.md".into(),
                file_path: PathBuf::from("/tmp/memory/user_role.md"),
                mtime_ms: 1_700_000_000_000,
                description: Some("User's role".into()),
                memory_type: Some(MemoryType::User),
                name: Some("User Role".into()),
                size_bytes: 512,
            },
            MemoryHeader {
                filename: "no_desc.md".into(),
                file_path: PathBuf::from("/tmp/memory/no_desc.md"),
                mtime_ms: 1_700_000_000_000,
                description: None,
                memory_type: None,
                name: None,
                size_bytes: 128,
            },
        ];
        let manifest = format_memory_manifest(&headers);
        assert!(manifest.contains("[user] user_role.md"));
        assert!(manifest.contains("User's role"));
        assert!(manifest.contains("no_desc.md"));
    }

    #[test]
    fn test_detect_duplicates() {
        let headers = vec![
            MemoryHeader { filename: "a.md".into(), file_path: PathBuf::from("/a.md"), mtime_ms: now_ms(), description: Some("same".into()), memory_type: None, name: None, size_bytes: 0 },
            MemoryHeader { filename: "b.md".into(), file_path: PathBuf::from("/b.md"), mtime_ms: now_ms(), description: Some("diff".into()), memory_type: None, name: None, size_bytes: 0 },
            MemoryHeader { filename: "c.md".into(), file_path: PathBuf::from("/c.md"), mtime_ms: now_ms(), description: Some("same".into()), memory_type: None, name: None, size_bytes: 0 },
        ];
        let dupes = detect_duplicates(&headers);
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0], (0, 2));
    }

    #[test]
    fn test_detect_duplicates_normalized() {
        let headers = vec![
            MemoryHeader { filename: "a.md".into(), file_path: PathBuf::from("/a.md"), mtime_ms: now_ms(), description: Some("User  prefers  Rust".into()), memory_type: None, name: None, size_bytes: 0 },
            MemoryHeader { filename: "b.md".into(), file_path: PathBuf::from("/b.md"), mtime_ms: now_ms(), description: Some("user prefers rust".into()), memory_type: None, name: None, size_bytes: 0 },
        ];
        let dupes = detect_duplicates(&headers);
        assert_eq!(dupes.len(), 1);
    }

    #[tokio::test]
    async fn test_merge_duplicate_memories() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(
            tmp.path().join("old.md"),
            "---\nname: old\ndescription: same description\ntype: user\n---\nOld content",
        )
        .await
        .unwrap();

        // Small delay to ensure different mtime
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        tokio::fs::write(
            tmp.path().join("new.md"),
            "---\nname: new\ndescription: same description\ntype: user\n---\nNew content",
        )
        .await
        .unwrap();

        let deleted = merge_duplicate_memories(tmp.path()).await;
        assert_eq!(deleted, 1);

        let remaining = scan_memory_files(tmp.path()).await;
        assert_eq!(remaining.len(), 1);
        // The newer file should be kept
        assert_eq!(remaining[0].filename, "new.md");
    }
}
