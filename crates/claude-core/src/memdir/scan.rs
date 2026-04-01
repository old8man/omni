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
    pub filename: String,
    pub file_path: PathBuf,
    pub mtime_ms: u64,
    pub description: Option<String>,
    pub memory_type: Option<MemoryType>,
}

/// Scan a memory directory for `.md` files, read their frontmatter, and return
/// a header list sorted newest-first (capped at `MAX_MEMORY_FILES`).
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
pub fn detect_duplicates(headers: &[MemoryHeader]) -> Vec<(usize, usize)> {
    let mut duplicates = Vec::new();
    for i in 0..headers.len() {
        for j in (i + 1)..headers.len() {
            if let (Some(a), Some(b)) = (&headers[i].description, &headers[j].description) {
                if !a.is_empty() && a == b { duplicates.push((i, j)); }
            }
        }
    }
    duplicates
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
            },
            MemoryHeader {
                filename: "no_desc.md".into(),
                file_path: PathBuf::from("/tmp/memory/no_desc.md"),
                mtime_ms: 1_700_000_000_000,
                description: None,
                memory_type: None,
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
            MemoryHeader { filename: "a.md".into(), file_path: PathBuf::from("/a.md"), mtime_ms: now_ms(), description: Some("same".into()), memory_type: None },
            MemoryHeader { filename: "b.md".into(), file_path: PathBuf::from("/b.md"), mtime_ms: now_ms(), description: Some("diff".into()), memory_type: None },
            MemoryHeader { filename: "c.md".into(), file_path: PathBuf::from("/c.md"), mtime_ms: now_ms(), description: Some("same".into()), memory_type: None },
        ];
        let dupes = detect_duplicates(&headers);
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0], (0, 2));
    }
}
