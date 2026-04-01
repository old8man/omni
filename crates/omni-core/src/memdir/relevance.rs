//! Memory relevance finding.

use std::collections::HashSet;
use std::path::Path;

use tracing::debug;

use super::scan::{scan_memory_files, MemoryHeader};

/// A memory selected as relevant to the current context.
#[derive(Clone, Debug)]
pub struct RelevantMemory {
    pub path: String,
    pub mtime_ms: u64,
    pub score: f64,
}

const MAX_RELEVANT: usize = 5;
const MIN_SCORE_THRESHOLD: f64 = 0.1;

/// Find memory files relevant to a query by keyword overlap scoring.
pub async fn find_relevant_memories(
    query: &str,
    memory_dir: &Path,
    already_surfaced: &HashSet<String>,
) -> Vec<RelevantMemory> {
    let memories = scan_memory_files(memory_dir).await;
    let filtered: Vec<&MemoryHeader> = memories.iter()
        .filter(|m| !already_surfaced.contains(&m.file_path.to_string_lossy().to_string()))
        .collect();

    if filtered.is_empty() { return Vec::new(); }

    let query_tokens = tokenize(query);
    if query_tokens.is_empty() { return Vec::new(); }

    let mut scored: Vec<RelevantMemory> = filtered.iter().filter_map(|m| {
        let score = score_memory(m, &query_tokens);
        if score >= MIN_SCORE_THRESHOLD {
            Some(RelevantMemory {
                path: m.file_path.to_string_lossy().to_string(),
                mtime_ms: m.mtime_ms,
                score,
            })
        } else { None }
    }).collect();

    scored.sort_by(|a, b| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.mtime_ms.cmp(&a.mtime_ms))
    });
    scored.truncate(MAX_RELEVANT);

    debug!(count = scored.len(), "found relevant memories for query");
    scored
}

fn score_memory(header: &MemoryHeader, query_tokens: &HashSet<String>) -> f64 {
    let mut score = 0.0;

    if let Some(desc) = &header.description {
        let desc_tokens = tokenize(desc);
        let overlap = query_tokens.intersection(&desc_tokens).count();
        if !desc_tokens.is_empty() {
            let union = query_tokens.union(&desc_tokens).count();
            score += overlap as f64 / union as f64;
        }
    }

    let filename_tokens = tokenize(&header.filename);
    let fn_overlap = query_tokens.intersection(&filename_tokens).count();
    if !filename_tokens.is_empty() {
        let fn_union = query_tokens.union(&filename_tokens).count();
        score += 0.3 * (fn_overlap as f64 / fn_union as f64);
    }

    score
}

fn tokenize(text: &str) -> HashSet<String> {
    static STOP_WORDS: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could",
        "should", "may", "might", "shall", "can", "to", "of", "in", "for",
        "on", "with", "at", "by", "from", "as", "into", "about", "that",
        "this", "it", "its", "and", "or", "but", "not", "no", "if", "then",
        "than", "so", "up", "out", "just", "also", "very", "too", "only",
    ];
    let stop: HashSet<&str> = STOP_WORDS.iter().copied().collect();

    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 2 && !stop.contains(w.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("hello world test");
        assert!(tokens.contains("hello"));
        assert!(tokens.contains("world"));
        assert!(tokens.contains("test"));
    }

    #[test]
    fn test_tokenize_filters_stop_words() {
        let tokens = tokenize("the quick brown fox is a test");
        assert!(!tokens.contains("the"));
        assert!(tokens.contains("quick"));
        assert!(tokens.contains("brown"));
    }

    #[test]
    fn test_score_memory_with_description() {
        let query_tokens = tokenize("database testing migration");
        let header = MemoryHeader {
            filename: "db_testing.md".into(),
            file_path: PathBuf::from("/tmp/memory/db_testing.md"),
            mtime_ms: 1_700_000_000_000,
            description: Some("integration tests must hit a real database".into()),
            memory_type: None,
            name: None,
            size_bytes: 0,
        };
        assert!(score_memory(&header, &query_tokens) > 0.0);
    }

    #[test]
    fn test_score_memory_no_match() {
        let query_tokens = tokenize("kubernetes deployment helm");
        let header = MemoryHeader {
            filename: "user_role.md".into(),
            file_path: PathBuf::from("/tmp/memory/user_role.md"),
            mtime_ms: 1_700_000_000_000,
            description: Some("user is a data scientist focused on logging".into()),
            memory_type: None,
            name: None,
            size_bytes: 0,
        };
        assert!(score_memory(&header, &query_tokens) < MIN_SCORE_THRESHOLD);
    }

    #[tokio::test]
    async fn test_find_relevant_memories_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let results = find_relevant_memories("test query", tmp.path(), &HashSet::new()).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_find_relevant_memories_filters_surfaced() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.md");
        tokio::fs::write(&file_path,
            "---\nname: test\ndescription: database testing memory\ntype: feedback\n---\nBody")
            .await.unwrap();

        let mut surfaced = HashSet::new();
        surfaced.insert(file_path.to_string_lossy().to_string());
        let results = find_relevant_memories("database testing", tmp.path(), &surfaced).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_find_relevant_memories_returns_matches() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("db_testing.md"),
            "---\nname: db testing\ndescription: integration tests must hit real database\ntype: feedback\n---\nBody")
            .await.unwrap();
        tokio::fs::write(tmp.path().join("user_role.md"),
            "---\nname: user role\ndescription: user is a frontend designer\ntype: user\n---\nBody")
            .await.unwrap();

        let results = find_relevant_memories("database testing integration", tmp.path(), &HashSet::new()).await;
        assert!(!results.is_empty());
        assert!(results[0].path.contains("db_testing"));
    }
}
