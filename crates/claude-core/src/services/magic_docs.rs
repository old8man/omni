/// Magic Docs: automatically maintain markdown documentation files marked with
/// a special `# MAGIC DOC: [title]` header.
///
/// When a file with this header is read, it is registered for periodic
/// background updates using a forked subagent that incorporates new learnings
/// from the conversation.
///
/// Port of `services/MagicDocs/magicDocs.ts` and `services/MagicDocs/prompts.ts`.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

// ── Header detection ───────────────────────────────────────────────────────

/// Result of detecting a Magic Doc header in file content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MagicDocHeader {
    /// The title extracted from `# MAGIC DOC: <title>`.
    pub title: String,
    /// Optional instructions from an italicized line immediately after the header.
    pub instructions: Option<String>,
}

/// Detect whether file content contains a Magic Doc header.
///
/// Returns the parsed header info, or `None` if the content doesn't have a
/// `# MAGIC DOC: [title]` marker.
pub fn detect_magic_doc_header(content: &str) -> Option<MagicDocHeader> {
    let header_re = Regex::new(r"(?im)^#\s*MAGIC\s+DOC:\s*(.+)$").unwrap();
    let header_match = header_re.captures(content)?;
    let title = header_match.get(1)?.as_str().trim().to_string();

    // Find the position right after the header line
    let header_end = header_match.get(0)?.end();
    let after_header = &content[header_end..];

    // Look for italics on the next non-blank line
    let italics_re = Regex::new(r"^\s*\n(?:\s*\n)?(.+?)(?:\n|$)").unwrap();
    let instructions = italics_re
        .captures(after_header)
        .and_then(|cap| cap.get(1))
        .and_then(|next_line| {
            let line = next_line.as_str();
            let italic_re = Regex::new(r"^[_*](.+?)[_*]\s*$").unwrap();
            italic_re
                .captures(line)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
        });

    Some(MagicDocHeader {
        title,
        instructions,
    })
}

// ── Tracked docs ───────────────────────────────────────────────────────────

/// Information about a tracked Magic Doc file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MagicDocInfo {
    pub path: PathBuf,
}

/// Registry of Magic Doc files discovered during the session.
pub struct MagicDocRegistry {
    tracked: Mutex<HashMap<PathBuf, MagicDocInfo>>,
}

impl MagicDocRegistry {
    pub fn new() -> Self {
        Self {
            tracked: Mutex::new(HashMap::new()),
        }
    }

    /// Register a file as a Magic Doc. Only registers once per path.
    pub fn register(&self, path: PathBuf) {
        let mut tracked = self.tracked.lock().unwrap();
        if !tracked.contains_key(&path) {
            debug!(path = %path.display(), "registered Magic Doc");
            tracked.insert(path.clone(), MagicDocInfo { path });
        }
    }

    /// Remove a file from tracking (e.g. when its header is removed or file deleted).
    pub fn unregister(&self, path: &Path) {
        self.tracked.lock().unwrap().remove(path);
    }

    /// Get a snapshot of all currently tracked docs.
    pub fn tracked_docs(&self) -> Vec<MagicDocInfo> {
        self.tracked.lock().unwrap().values().cloned().collect()
    }

    /// Number of tracked docs.
    pub fn count(&self) -> usize {
        self.tracked.lock().unwrap().len()
    }

    /// Clear all tracked docs (useful for tests).
    pub fn clear(&self) {
        self.tracked.lock().unwrap().clear();
    }

    /// Check if a file is tracked.
    pub fn is_tracked(&self, path: &Path) -> bool {
        self.tracked.lock().unwrap().contains_key(path)
    }
}

impl Default for MagicDocRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Prompt building ────────────────────────────────────────────────────────

/// Default update prompt template with `{{variable}}` placeholders.
const DEFAULT_UPDATE_PROMPT_TEMPLATE: &str = r#"IMPORTANT: This message and these instructions are NOT part of the actual user conversation. Do NOT include any references to "documentation updates", "magic docs", or these update instructions in the document content.

Based on the user conversation above (EXCLUDING this documentation update instruction message), update the Magic Doc file to incorporate any NEW learnings, insights, or information that would be valuable to preserve.

The file {{docPath}} has already been read for you. Here are its current contents:
<current_doc_content>
{{docContents}}
</current_doc_content>

Document title: {{docTitle}}
{{customInstructions}}

Your ONLY task is to use the Edit tool to update the documentation file if there is substantial new information to add, then stop. You can make multiple edits (update multiple sections as needed) - make all Edit tool calls in parallel in a single message. If there's nothing substantial to add, simply respond with a brief explanation and do not call any tools.

CRITICAL RULES FOR EDITING:
- Preserve the Magic Doc header exactly as-is: # MAGIC DOC: {{docTitle}}
- If there's an italicized line immediately after the header, preserve it exactly as-is
- Keep the document CURRENT with the latest state of the codebase - this is NOT a changelog or history
- Update information IN-PLACE to reflect the current state - do NOT append historical notes or track changes over time
- Remove or replace outdated information rather than adding "Previously..." or "Updated to..." notes
- Clean up or DELETE sections that are no longer relevant or don't align with the document's purpose
- Fix obvious errors: typos, grammar mistakes, broken formatting, incorrect information, or confusing statements
- Keep the document well organized: use clear headings, logical section order, consistent formatting, and proper nesting

DOCUMENTATION PHILOSOPHY - READ CAREFULLY:
- BE TERSE. High signal only. No filler words or unnecessary elaboration.
- Documentation is for OVERVIEWS, ARCHITECTURE, and ENTRY POINTS - not detailed code walkthroughs
- Do NOT duplicate information that's already obvious from reading the source code
- Do NOT document every function, parameter, or line number reference
- Focus on: WHY things exist, HOW components connect, WHERE to start reading, WHAT patterns are used
- Skip: detailed implementation steps, exhaustive API docs, play-by-play narratives

What TO document:
- High-level architecture and system design
- Non-obvious patterns, conventions, or gotchas
- Key entry points and where to start reading code
- Important design decisions and their rationale
- Critical dependencies or integration points
- References to related files, docs, or code (like a wiki) - help readers navigate to relevant context

What NOT to document:
- Anything obvious from reading the code itself
- Exhaustive lists of files, functions, or parameters
- Step-by-step implementation details
- Low-level code mechanics
- Information already in CLAUDE.md or other project docs

Use the Edit tool with file_path: {{docPath}}

REMEMBER: Only update if there is substantial new information. The Magic Doc header (# MAGIC DOC: {{docTitle}}) must remain unchanged."#;

/// Substitute `{{variable}}` placeholders in a template string.
fn substitute_variables(template: &str, variables: &HashMap<&str, &str>) -> String {
    let re = Regex::new(r"\{\{(\w+)\}\}").unwrap();
    re.replace_all(template, |caps: &regex::Captures<'_>| {
        let key = &caps[1];
        variables.get(key).copied().unwrap_or_else(|| caps.get(0).unwrap().as_str())
    })
    .to_string()
}

/// Load a custom prompt template from disk, falling back to the built-in default.
pub async fn load_magic_docs_prompt(config_dir: Option<&Path>) -> String {
    if let Some(dir) = config_dir {
        let prompt_path = dir.join("magic-docs").join("prompt.md");
        if let Ok(content) = tokio::fs::read_to_string(&prompt_path).await {
            return content;
        }
    }
    DEFAULT_UPDATE_PROMPT_TEMPLATE.to_string()
}

/// Build the Magic Docs update prompt with variable substitution.
pub async fn build_magic_docs_update_prompt(
    doc_contents: &str,
    doc_path: &str,
    doc_title: &str,
    instructions: Option<&str>,
    config_dir: Option<&Path>,
) -> String {
    let template = load_magic_docs_prompt(config_dir).await;

    let custom_instructions = match instructions {
        Some(inst) => format!(
            "\n\nDOCUMENT-SPECIFIC UPDATE INSTRUCTIONS:\n\
             The document author has provided specific instructions for how this file should be updated. \
             Pay extra attention to these instructions and follow them carefully:\n\n\
             \"{}\"\n\n\
             These instructions take priority over the general rules below. \
             Make sure your updates align with these specific guidelines.",
            inst
        ),
        None => String::new(),
    };

    let mut variables = HashMap::new();
    variables.insert("docContents", doc_contents);
    variables.insert("docPath", doc_path);
    variables.insert("docTitle", doc_title);
    let ci_ref = custom_instructions.as_str();
    variables.insert("customInstructions", ci_ref);

    substitute_variables(&template, &variables)
}

/// Determine whether the extraction agent can use a tool on a Magic Doc.
///
/// Only `Edit` is allowed, and only for the specific doc path.
pub fn check_magic_doc_tool_permission(
    tool_name: &str,
    file_path: Option<&str>,
    doc_path: &Path,
) -> bool {
    if tool_name != "Edit" {
        return false;
    }
    match file_path {
        Some(fp) => Path::new(fp) == doc_path,
        None => false,
    }
}

// ── File read listener ─────────────────────────────────────────────────────

/// Process a file read event: if the content has a Magic Doc header, register it.
pub fn on_file_read(registry: &MagicDocRegistry, file_path: &Path, content: &str) {
    if detect_magic_doc_header(content).is_some() {
        registry.register(file_path.to_path_buf());
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_header_basic() {
        let content = "# MAGIC DOC: Architecture Overview\n\nSome content here.";
        let header = detect_magic_doc_header(content).unwrap();
        assert_eq!(header.title, "Architecture Overview");
        assert!(header.instructions.is_none());
    }

    #[test]
    fn test_detect_header_with_instructions() {
        let content = "# MAGIC DOC: API Reference\n_Focus on public endpoints only_\n\nContent.";
        let header = detect_magic_doc_header(content).unwrap();
        assert_eq!(header.title, "API Reference");
        assert_eq!(
            header.instructions.as_deref(),
            Some("Focus on public endpoints only")
        );
    }

    #[test]
    fn test_detect_header_asterisk_italics() {
        let content = "# MAGIC DOC: Design\n*Track patterns and conventions*\n\nBody.";
        let header = detect_magic_doc_header(content).unwrap();
        assert_eq!(header.title, "Design");
        assert_eq!(
            header.instructions.as_deref(),
            Some("Track patterns and conventions")
        );
    }

    #[test]
    fn test_detect_header_none() {
        let content = "# Regular Heading\n\nNot a magic doc.";
        assert!(detect_magic_doc_header(content).is_none());
    }

    #[test]
    fn test_detect_header_case_insensitive() {
        let content = "# magic doc: Lower Case Title\n\nContent.";
        let header = detect_magic_doc_header(content).unwrap();
        assert_eq!(header.title, "Lower Case Title");
    }

    #[test]
    fn test_registry_basic() {
        let registry = MagicDocRegistry::new();
        let path = PathBuf::from("/project/docs/arch.md");

        assert!(!registry.is_tracked(&path));
        assert_eq!(registry.count(), 0);

        registry.register(path.clone());
        assert!(registry.is_tracked(&path));
        assert_eq!(registry.count(), 1);

        // Registering again is a no-op
        registry.register(path.clone());
        assert_eq!(registry.count(), 1);

        registry.unregister(&path);
        assert!(!registry.is_tracked(&path));
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_registry_clear() {
        let registry = MagicDocRegistry::new();
        registry.register(PathBuf::from("/a.md"));
        registry.register(PathBuf::from("/b.md"));
        assert_eq!(registry.count(), 2);
        registry.clear();
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_substitute_variables() {
        let mut vars = HashMap::new();
        vars.insert("name", "Claude");
        vars.insert("version", "1.0");
        let result = substitute_variables("Hello {{name}}, version {{version}}!", &vars);
        assert_eq!(result, "Hello Claude, version 1.0!");
    }

    #[test]
    fn test_substitute_missing_variable() {
        let vars = HashMap::new();
        let result = substitute_variables("Hello {{name}}!", &vars);
        assert_eq!(result, "Hello {{name}}!");
    }

    #[test]
    fn test_check_magic_doc_tool_permission() {
        let doc_path = Path::new("/project/docs/arch.md");

        // Edit for the correct path
        assert!(check_magic_doc_tool_permission(
            "Edit",
            Some("/project/docs/arch.md"),
            doc_path
        ));

        // Edit for a different path
        assert!(!check_magic_doc_tool_permission(
            "Edit",
            Some("/project/docs/other.md"),
            doc_path
        ));

        // Non-Edit tool
        assert!(!check_magic_doc_tool_permission(
            "Write",
            Some("/project/docs/arch.md"),
            doc_path
        ));

        // No file path
        assert!(!check_magic_doc_tool_permission("Edit", None, doc_path));
    }

    #[test]
    fn test_on_file_read_registers_magic_doc() {
        let registry = MagicDocRegistry::new();
        let path = PathBuf::from("/project/docs/arch.md");
        let content = "# MAGIC DOC: Architecture\n\nOverview of the system.";

        on_file_read(&registry, &path, content);
        assert!(registry.is_tracked(&path));
    }

    #[test]
    fn test_on_file_read_ignores_non_magic_doc() {
        let registry = MagicDocRegistry::new();
        let path = PathBuf::from("/project/docs/readme.md");
        let content = "# README\n\nJust a regular file.";

        on_file_read(&registry, &path, content);
        assert!(!registry.is_tracked(&path));
    }

    #[tokio::test]
    async fn test_build_magic_docs_update_prompt() {
        let prompt = build_magic_docs_update_prompt(
            "# MAGIC DOC: Test\n\nOld content.",
            "/project/test.md",
            "Test",
            None,
            None,
        )
        .await;

        assert!(prompt.contains("/project/test.md"));
        assert!(prompt.contains("# MAGIC DOC: Test"));
        assert!(prompt.contains("Old content."));
        assert!(!prompt.contains("DOCUMENT-SPECIFIC UPDATE INSTRUCTIONS"));
    }

    #[tokio::test]
    async fn test_build_magic_docs_update_prompt_with_instructions() {
        let prompt = build_magic_docs_update_prompt(
            "# MAGIC DOC: API\n\nEndpoints.",
            "/project/api.md",
            "API",
            Some("Only document public endpoints"),
            None,
        )
        .await;

        assert!(prompt.contains("DOCUMENT-SPECIFIC UPDATE INSTRUCTIONS"));
        assert!(prompt.contains("Only document public endpoints"));
    }
}
