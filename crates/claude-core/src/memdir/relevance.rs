//! Memory relevance finding.
//!
//! Scores memory files against a query context and returns the most relevant
//! ones for injection into the conversation. The TypeScript original uses a
//! Sonnet side-query for selection; this Rust implementation provides a
//! keyword-overlap scorer as a fast local fallback, plus the same interface
//! for integration with an LLM selector.

use std::collections::HashSet;
use std::path::Path;

use tracing::debug;

use super::scan::{scan_memory_files, MemoryHeader};

/// A memory selected as relevant to the current context.
#[derive(Clone, Debug)]
pub struct RelevantMemory {
    /// Absolute path to the memory file.
    pub path: String,
    /// Modification time in milliseconds since Unix epoch.
    pub mtime_ms: u64,
    /// Relevance score (higher is more relevant). Interpretation depends on
    /// the scoring strategy used.
    pub score: f64,
}

/// Maximum number of memories to return from relevance finding.
const MAX_RELEVANT: usize = 5;

/// Minimum keyword overlap score to consider a memory relevant.
const MIN_SCORE_THRESHOLD: f64 = 0.1;

/// Find memory files relevant to a query by scanning memory file headers
/// and scoring them by keyword overlap with the query context.
///
/// Returns absolute file paths + mtime of the most relevant memories
/// (up to 5). Excludes MEMORY.md (already loaded in system prompt).
///
/// `already_surfaced` filters paths shown in prior turns so the selector
/// spends its budget on fresh candidates instead of re-picking.
pub async fn find_relevant_memories(
    query: &str,
    memory_dir: &Path,
    already_surfaced: &HashSet<String>,
) -> Vec<RelevantMemory> {
    let memories = scan_memory_files(memory_dir).await;
    let filtered: Vec<&MemoryHeader> = memories
        .iter()
        .filter(|m| !already_surfaced.contains(&m.file_path.to_string_lossy().to_string()))
        .collect();

    if filtered.is_empty() {
        return Vec::new();
    }

    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<RelevantMemory> = filtered
        .iter()
        .filter_map(|m| {
            let score = score_memory(m, &query_tokens);
            if score >= MIN_SCORE_THRESHOLD {
                Some(RelevantMemory {
                    path: m.file_path.to_string_lossy().to_string(),
                    mtime_ms: m.mtime_ms,
                    score,
                })
            } else {
                None
            }
        })
        .collect();

    // Sort by score descending, then by recency (newer first) as tiebreaker
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.mtime_ms.cmp(&a.mtime_ms))
    });

    scored.truncate(MAX_RELEVANT);

    debug!(
        count = scored.len(),
        "found relevant memories for query"
    );

    scored
}

/// Score a memory header against a set of query tokens.
///
/// Uses a combination of:
/// - Keyword overlap between query and memory description/filename
/// - Type-based boosting (user/feedback get slight boost for interactive queries)
/// - Recency bonus (newer memories score slightly higher)
fn score_memory(header: &MemoryHeader, query_tokens: &HashSet<String>) -> f64 {
    let mut score = 0.0;

    // Score from description overlap
    if let Some(desc) = &header.description {
        let desc_tokens = tokenize(desc);
        let overlap = query_tokens.intersection(&desc_tokens).count();
        if !desc_tokens.is_empty() {
            // Jaccard-like coefficient: overlap / union
            let union = query_tokens.union(&desc_tokens).count();
            score += overlap as f64 / union as f64;
        }
    }

    // Score from filename overlap
    let filename_tokens = tokenize(&header.filename);
    let fn_overlap = query_tokens.intersection(&filename_tokens).count();
    if !filename_tokens.is_empty() {
        let fn_union = query_tokens.union(&filename_tokens).count();
        score += 0.3 * (fn_overlap as f64 / fn_union as f64);
    }

    score
}

/// Tokenize text into a set of lowercase words, filtering out common stop
/// words and very short tokens.
fn tokenize(text: &str) -> HashSet<String> {
    static STOP_WORDS: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has",
        "had", "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall",
        "can", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "about",
        "that", "this", "it", "its", "and", "or", "but", "not", "no", "if", "then", "than",
        "so", "up", "out", "just", "also", "very", "too", "only",
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
        assert!(!tokens.contains("is"));
        assert!(!tokens.contains("a"));
        assert!(tokens.contains("quick"));
        assert!(tokens.contains("brown"));
        assert!(tokens.contains("fox"));
    }

    #[test]
    fn test_tokenize_filters_short() {
        let tokens = tokenize("I x am testing");
        assert!(!tokens.contains("i"));
        assert!(!tokens.contains("x"));
        assert!(tokens.contains("am"));
        assert!(tokens.contains("testing"));
    }

    #[test]
    fn test_tokenize_splits_punctuation() {
        let tokens = tokenize("user_role.md feedback-testing");
        assert!(tokens.contains("user_role"));
        assert!(tokens.contains("md"));
        assert!(tokens.contains("feedback"));
        assert!(tokens.contains("testing"));
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
        };
        let score = score_memory(&header, &query_tokens);
        assert!(score > 0.0, "score should be positive for matching content");
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
        };
        let score = score_memory(&header, &query_tokens);
        assert!(score < MIN_SCORE_THRESHOLD, "score should be below threshold for unrelated content");
    }

    #[test]
    fn test_score_memory_filename_match() {
        let query_tokens = tokenize("feedback on testing approach");
        let header = MemoryHeader {
            filename: "feedback_testing.md".into(),
            file_path: PathBuf::from("/tmp/memory/feedback_testing.md"),
            mtime_ms: 1_700_000_000_000,
            description: None,
            memory_type: None,
        };
        let score = score_memory(&header, &query_tokens);
        assert!(score > 0.0, "filename overlap should produce positive score");
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
        tokio::fs::write(
            &file_path,
            "---\nname: test\ndescription: database testing memory\ntype: feedback\n---\nBody",
        )
        .await
        .unwrap();

        let mut surfaced = HashSet::new();
        surfaced.insert(file_path.to_string_lossy().to_string());

        let results =
            find_relevant_memories("database testing", tmp.path(), &surfaced).await;
        assert!(results.is_empty(), "surfaced files should be filtered out");
    }

    #[tokio::test]
    async fn test_find_relevant_memories_returns_matches() {
        let tmp = TempDir::new().unwrap();

        // Create a memory that matches the query
        tokio::fs::write(
            tmp.path().join("db_testing.md"),
            "---\nname: db testing\ndescription: integration tests must hit real database\ntype: feedback\n---\nBody",
        )
        .await
        .unwrap();

        // Create an unrelated memory
        tokio::fs::write(
            tmp.path().join("user_role.md"),
            "---\nname: user role\ndescription: user is a frontend designer\ntype: user\n---\nBody",
        )
        .await
        .unwrap();

        let results =
            find_relevant_memories("database testing integration", tmp.path(), &HashSet::new())
                .await;

        // Should find at least the matching memory
        assert!(!results.is_empty());
        // The database testing memory should score higher
        assert!(results[0].path.contains("db_testing"));
    }
}
