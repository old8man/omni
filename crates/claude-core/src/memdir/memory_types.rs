//! Memory type taxonomy and frontmatter parsing.
//!
//! Memories are constrained to four types capturing context NOT derivable
//! from the current project state. Code patterns, architecture, git history,
//! and file structure are derivable (via grep/git/CLAUDE.md) and should NOT
//! be saved as memories.

use std::fmt;
use std::str::FromStr;

/// The four memory types.
pub const MEMORY_TYPES: &[MemoryType] = &[
    MemoryType::User,
    MemoryType::Feedback,
    MemoryType::Project,
    MemoryType::Reference,
];

/// A memory type from the closed four-type taxonomy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemoryType {
    User,
    Feedback,
    Project,
    Reference,
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryType::User => write!(f, "user"),
            MemoryType::Feedback => write!(f, "feedback"),
            MemoryType::Project => write!(f, "project"),
            MemoryType::Reference => write!(f, "reference"),
        }
    }
}

impl FromStr for MemoryType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(MemoryType::User),
            "feedback" => Ok(MemoryType::Feedback),
            "project" => Ok(MemoryType::Project),
            "reference" => Ok(MemoryType::Reference),
            other => Err(format!("unknown memory type: {other}")),
        }
    }
}

/// Parse a raw frontmatter value into a `MemoryType`.
/// Invalid or missing values return `None` -- legacy files without a
/// `type:` field keep working, files with unknown types degrade gracefully.
pub fn parse_memory_type(raw: Option<&str>) -> Option<MemoryType> {
    raw.and_then(|s| s.parse().ok())
}

/// Parsed YAML frontmatter from a memory file.
#[derive(Clone, Debug, Default)]
pub struct MemoryFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub memory_type: Option<MemoryType>,
}

/// Parse YAML-like frontmatter delimited by `---` lines.
///
/// Returns the parsed frontmatter and the byte offset where body content begins.
/// Handles the simple key: value format used by memory files without pulling in
/// a full YAML parser.
pub fn parse_frontmatter(content: &str) -> (MemoryFrontmatter, usize) {
    let trimmed = content.trim_start();
    let offset_start = content.len() - trimmed.len();

    if !trimmed.starts_with("---") {
        return (MemoryFrontmatter::default(), 0);
    }

    // Find the closing `---`
    let after_open = &trimmed[3..];
    let after_open_trimmed = after_open.trim_start_matches(['\r', '\n']);
    let open_skip = 3 + (after_open.len() - after_open_trimmed.len());

    let close_pos = after_open_trimmed.find("\n---");
    let (fm_block, body_offset) = match close_pos {
        Some(pos) => {
            let block = &after_open_trimmed[..pos];
            // Skip past the closing `---` and the newline after it
            let rest = &after_open_trimmed[pos + 4..];
            let trailing = rest.len() - rest.trim_start_matches(['\r', '\n']).len();
            let total = offset_start + open_skip + pos + 4 + trailing;
            (block, total)
        }
        None => return (MemoryFrontmatter::default(), 0),
    };

    let mut fm = MemoryFrontmatter::default();

    for line in fm_block.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "name" => fm.name = Some(value.to_string()),
                "description" => fm.description = Some(value.to_string()),
                "type" => fm.memory_type = parse_memory_type(Some(value)),
                _ => {} // Ignore unknown fields
            }
        }
    }

    (fm, body_offset)
}

// ── Prompt text sections ───────────────────────────────────────────────────

/// Frontmatter format example with the `type` field.
pub fn memory_frontmatter_example() -> Vec<String> {
    let types_str = MEMORY_TYPES
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        "```markdown".into(),
        "---".into(),
        "name: {{memory name}}".into(),
        "description: {{one-line description — used to decide relevance in future conversations, so be specific}}".into(),
        format!("type: {{{{{types_str}}}}}"),
        "---".into(),
        String::new(),
        "{{memory content — for feedback/project types, structure as: rule/fact, then **Why:** and **How to apply:** lines}}".into(),
        "```".into(),
    ]
}

/// `## Types of memory` section for INDIVIDUAL-ONLY mode (single directory).
pub fn types_section_individual() -> Vec<String> {
    vec![
        "## Types of memory".into(),
        String::new(),
        "There are several discrete types of memory that you can store in your memory system:".into(),
        String::new(),
        "<types>".into(),
        "<type>".into(),
        "    <name>user</name>".into(),
        "    <description>Contain information about the user's role, goals, responsibilities, and knowledge. Great user memories help you tailor your future behavior to the user's preferences and perspective. Your goal in reading and writing these memories is to build up an understanding of who the user is and how you can be most helpful to them specifically. For example, you should collaborate with a senior software engineer differently than a student who is coding for the very first time. Keep in mind, that the aim here is to be helpful to the user. Avoid writing memories about the user that could be viewed as a negative judgement or that are not relevant to the work you're trying to accomplish together.</description>".into(),
        "    <when_to_save>When you learn any details about the user's role, preferences, responsibilities, or knowledge</when_to_save>".into(),
        "    <how_to_use>When your work should be informed by the user's profile or perspective. For example, if the user is asking you to explain a part of the code, you should answer that question in a way that is tailored to the specific details that they will find most valuable or that helps them build their mental model in relation to domain knowledge they already have.</how_to_use>".into(),
        "    <examples>".into(),
        "    user: I'm a data scientist investigating what logging we have in place".into(),
        "    assistant: [saves user memory: user is a data scientist, currently focused on observability/logging]".into(),
        String::new(),
        "    user: I've been writing Go for ten years but this is my first time touching the React side of this repo".into(),
        "    assistant: [saves user memory: deep Go expertise, new to React and this project's frontend — frame frontend explanations in terms of backend analogues]".into(),
        "    </examples>".into(),
        "</type>".into(),
        "<type>".into(),
        "    <name>feedback</name>".into(),
        "    <description>Guidance the user has given you about how to approach work — both what to avoid and what to keep doing. These are a very important type of memory to read and write as they allow you to remain coherent and responsive to the way you should approach work in the project. Record from failure AND success: if you only save corrections, you will avoid past mistakes but drift away from approaches the user has already validated, and may grow overly cautious.</description>".into(),
        r#"    <when_to_save>Any time the user corrects your approach ("no not that", "don't", "stop doing X") OR confirms a non-obvious approach worked ("yes exactly", "perfect, keep doing that", accepting an unusual choice without pushback). Corrections are easy to notice; confirmations are quieter — watch for them. In both cases, save what is applicable to future conversations, especially if surprising or not obvious from the code. Include *why* so you can judge edge cases later.</when_to_save>"#.into(),
        "    <how_to_use>Let these memories guide your behavior so that the user does not need to offer the same guidance twice.</how_to_use>".into(),
        "    <body_structure>Lead with the rule itself, then a **Why:** line (the reason the user gave — often a past incident or strong preference) and a **How to apply:** line (when/where this guidance kicks in). Knowing *why* lets you judge edge cases instead of blindly following the rule.</body_structure>".into(),
        "    <examples>".into(),
        "    user: don't mock the database in these tests — we got burned last quarter when mocked tests passed but the prod migration failed".into(),
        "    assistant: [saves feedback memory: integration tests must hit a real database, not mocks. Reason: prior incident where mock/prod divergence masked a broken migration]".into(),
        String::new(),
        "    user: stop summarizing what you just did at the end of every response, I can read the diff".into(),
        "    assistant: [saves feedback memory: this user wants terse responses with no trailing summaries]".into(),
        String::new(),
        "    user: yeah the single bundled PR was the right call here, splitting this one would've just been churn".into(),
        "    assistant: [saves feedback memory: for refactors in this area, user prefers one bundled PR over many small ones. Confirmed after I chose this approach — a validated judgment call, not a correction]".into(),
        "    </examples>".into(),
        "</type>".into(),
        "<type>".into(),
        "    <name>project</name>".into(),
        "    <description>Information that you learn about ongoing work, goals, initiatives, bugs, or incidents within the project that is not otherwise derivable from the code or git history. Project memories help you understand the broader context and motivation behind the work the user is doing within this working directory.</description>".into(),
        r#"    <when_to_save>When you learn who is doing what, why, or by when. These states change relatively quickly so try to keep your understanding of this up to date. Always convert relative dates in user messages to absolute dates when saving (e.g., "Thursday" → "2026-03-05"), so the memory remains interpretable after time passes.</when_to_save>"#.into(),
        "    <how_to_use>Use these memories to more fully understand the details and nuance behind the user's request and make better informed suggestions.</how_to_use>".into(),
        "    <body_structure>Lead with the fact or decision, then a **Why:** line (the motivation — often a constraint, deadline, or stakeholder ask) and a **How to apply:** line (how this should shape your suggestions). Project memories decay fast, so the why helps future-you judge whether the memory is still load-bearing.</body_structure>".into(),
        "    <examples>".into(),
        "    user: we're freezing all non-critical merges after Thursday — mobile team is cutting a release branch".into(),
        "    assistant: [saves project memory: merge freeze begins 2026-03-05 for mobile release cut. Flag any non-critical PR work scheduled after that date]".into(),
        String::new(),
        "    user: the reason we're ripping out the old auth middleware is that legal flagged it for storing session tokens in a way that doesn't meet the new compliance requirements".into(),
        "    assistant: [saves project memory: auth middleware rewrite is driven by legal/compliance requirements around session token storage, not tech-debt cleanup — scope decisions should favor compliance over ergonomics]".into(),
        "    </examples>".into(),
        "</type>".into(),
        "<type>".into(),
        "    <name>reference</name>".into(),
        "    <description>Stores pointers to where information can be found in external systems. These memories allow you to remember where to look to find up-to-date information outside of the project directory.</description>".into(),
        "    <when_to_save>When you learn about resources in external systems and their purpose. For example, that bugs are tracked in a specific project in Linear or that feedback can be found in a specific Slack channel.</when_to_save>".into(),
        "    <how_to_use>When the user references an external system or information that may be in an external system.</how_to_use>".into(),
        "    <examples>".into(),
        r#"    user: check the Linear project "INGEST" if you want context on these tickets, that's where we track all pipeline bugs"#.into(),
        r#"    assistant: [saves reference memory: pipeline bugs are tracked in Linear project "INGEST"]"#.into(),
        String::new(),
        "    user: the Grafana board at grafana.internal/d/api-latency is what oncall watches — if you're touching request handling, that's the thing that'll page someone".into(),
        "    assistant: [saves reference memory: grafana.internal/d/api-latency is the oncall latency dashboard — check it when editing request-path code]".into(),
        "    </examples>".into(),
        "</type>".into(),
        "</types>".into(),
        String::new(),
    ]
}

/// `## Types of memory` section for COMBINED mode (private + team directories).
pub fn types_section_combined() -> Vec<String> {
    vec![
        "## Types of memory".into(),
        String::new(),
        "There are several discrete types of memory that you can store in your memory system. Each type below declares a <scope> of `private`, `team`, or guidance for choosing between the two.".into(),
        String::new(),
        "<types>".into(),
        "<type>".into(),
        "    <name>user</name>".into(),
        "    <scope>always private</scope>".into(),
        "    <description>Contain information about the user's role, goals, responsibilities, and knowledge. Great user memories help you tailor your future behavior to the user's preferences and perspective. Your goal in reading and writing these memories is to build up an understanding of who the user is and how you can be most helpful to them specifically. For example, you should collaborate with a senior software engineer differently than a student who is coding for the very first time. Keep in mind, that the aim here is to be helpful to the user. Avoid writing memories about the user that could be viewed as a negative judgement or that are not relevant to the work you're trying to accomplish together.</description>".into(),
        "    <when_to_save>When you learn any details about the user's role, preferences, responsibilities, or knowledge</when_to_save>".into(),
        "    <how_to_use>When your work should be informed by the user's profile or perspective. For example, if the user is asking you to explain a part of the code, you should answer that question in a way that is tailored to the specific details that they will find most valuable or that helps them build their mental model in relation to domain knowledge they already have.</how_to_use>".into(),
        "    <examples>".into(),
        "    user: I'm a data scientist investigating what logging we have in place".into(),
        "    assistant: [saves private user memory: user is a data scientist, currently focused on observability/logging]".into(),
        String::new(),
        "    user: I've been writing Go for ten years but this is my first time touching the React side of this repo".into(),
        "    assistant: [saves private user memory: deep Go expertise, new to React and this project's frontend — frame frontend explanations in terms of backend analogues]".into(),
        "    </examples>".into(),
        "</type>".into(),
        "<type>".into(),
        "    <name>feedback</name>".into(),
        "    <scope>default to private. Save as team only when the guidance is clearly a project-wide convention that every contributor should follow (e.g., a testing policy, a build invariant), not a personal style preference.</scope>".into(),
        "    <description>Guidance the user has given you about how to approach work — both what to avoid and what to keep doing. These are a very important type of memory to read and write as they allow you to remain coherent and responsive to the way you should approach work in the project. Record from failure AND success: if you only save corrections, you will avoid past mistakes but drift away from approaches the user has already validated, and may grow overly cautious. Before saving a private feedback memory, check that it doesn't contradict a team feedback memory — if it does, either don't save it or note the override explicitly.</description>".into(),
        r#"    <when_to_save>Any time the user corrects your approach ("no not that", "don't", "stop doing X") OR confirms a non-obvious approach worked ("yes exactly", "perfect, keep doing that", accepting an unusual choice without pushback). Corrections are easy to notice; confirmations are quieter — watch for them. In both cases, save what is applicable to future conversations, especially if surprising or not obvious from the code. Include *why* so you can judge edge cases later.</when_to_save>"#.into(),
        "    <how_to_use>Let these memories guide your behavior so that the user and other users in the project do not need to offer the same guidance twice.</how_to_use>".into(),
        "    <body_structure>Lead with the rule itself, then a **Why:** line (the reason the user gave — often a past incident or strong preference) and a **How to apply:** line (when/where this guidance kicks in). Knowing *why* lets you judge edge cases instead of blindly following the rule.</body_structure>".into(),
        "    <examples>".into(),
        "    user: don't mock the database in these tests — we got burned last quarter when mocked tests passed but the prod migration failed".into(),
        "    assistant: [saves team feedback memory: integration tests must hit a real database, not mocks. Reason: prior incident where mock/prod divergence masked a broken migration. Team scope: this is a project testing policy, not a personal preference]".into(),
        String::new(),
        "    user: stop summarizing what you just did at the end of every response, I can read the diff".into(),
        "    assistant: [saves private feedback memory: this user wants terse responses with no trailing summaries. Private because it's a communication preference, not a project convention]".into(),
        String::new(),
        "    user: yeah the single bundled PR was the right call here, splitting this one would've just been churn".into(),
        "    assistant: [saves private feedback memory: for refactors in this area, user prefers one bundled PR over many small ones. Confirmed after I chose this approach — a validated judgment call, not a correction]".into(),
        "    </examples>".into(),
        "</type>".into(),
        "<type>".into(),
        "    <name>project</name>".into(),
        "    <scope>private or team, but strongly bias toward team</scope>".into(),
        "    <description>Information that you learn about ongoing work, goals, initiatives, bugs, or incidents within the project that is not otherwise derivable from the code or git history. Project memories help you understand the broader context and motivation behind the work users are working on within this working directory.</description>".into(),
        r#"    <when_to_save>When you learn who is doing what, why, or by when. These states change relatively quickly so try to keep your understanding of this up to date. Always convert relative dates in user messages to absolute dates when saving (e.g., "Thursday" → "2026-03-05"), so the memory remains interpretable after time passes.</when_to_save>"#.into(),
        "    <how_to_use>Use these memories to more fully understand the details and nuance behind the user's request, anticipate coordination issues across users, make better informed suggestions.</how_to_use>".into(),
        "    <body_structure>Lead with the fact or decision, then a **Why:** line (the motivation — often a constraint, deadline, or stakeholder ask) and a **How to apply:** line (how this should shape your suggestions). Project memories decay fast, so the why helps future-you judge whether the memory is still load-bearing.</body_structure>".into(),
        "    <examples>".into(),
        "    user: we're freezing all non-critical merges after Thursday — mobile team is cutting a release branch".into(),
        "    assistant: [saves team project memory: merge freeze begins 2026-03-05 for mobile release cut. Flag any non-critical PR work scheduled after that date]".into(),
        String::new(),
        "    user: the reason we're ripping out the old auth middleware is that legal flagged it for storing session tokens in a way that doesn't meet the new compliance requirements".into(),
        "    assistant: [saves team project memory: auth middleware rewrite is driven by legal/compliance requirements around session token storage, not tech-debt cleanup — scope decisions should favor compliance over ergonomics]".into(),
        "    </examples>".into(),
        "</type>".into(),
        "<type>".into(),
        "    <name>reference</name>".into(),
        "    <scope>usually team</scope>".into(),
        "    <description>Stores pointers to where information can be found in external systems. These memories allow you to remember where to look to find up-to-date information outside of the project directory.</description>".into(),
        "    <when_to_save>When you learn about resources in external systems and their purpose. For example, that bugs are tracked in a specific project in Linear or that feedback can be found in a specific Slack channel.</when_to_save>".into(),
        "    <how_to_use>When the user references an external system or information that may be in an external system.</how_to_use>".into(),
        "    <examples>".into(),
        r#"    user: check the Linear project "INGEST" if you want context on these tickets, that's where we track all pipeline bugs"#.into(),
        r#"    assistant: [saves team reference memory: pipeline bugs are tracked in Linear project "INGEST"]"#.into(),
        String::new(),
        "    user: the Grafana board at grafana.internal/d/api-latency is what oncall watches — if you're touching request handling, that's the thing that'll page someone".into(),
        "    assistant: [saves team reference memory: grafana.internal/d/api-latency is the oncall latency dashboard — check it when editing request-path code]".into(),
        "    </examples>".into(),
        "</type>".into(),
        "</types>".into(),
        String::new(),
    ]
}

/// `## What NOT to save in memory` section. Identical across both modes.
pub fn what_not_to_save_section() -> Vec<String> {
    vec![
        "## What NOT to save in memory".into(),
        String::new(),
        "- Code patterns, conventions, architecture, file paths, or project structure — these can be derived by reading the current project state.".into(),
        "- Git history, recent changes, or who-changed-what — `git log` / `git blame` are authoritative.".into(),
        "- Debugging solutions or fix recipes — the fix is in the code; the commit message has the context.".into(),
        "- Anything already documented in CLAUDE.md files.".into(),
        "- Ephemeral task details: in-progress work, temporary state, current conversation context.".into(),
        String::new(),
        "These exclusions apply even when the user explicitly asks you to save. If they ask you to save a PR list or activity summary, ask what was *surprising* or *non-obvious* about it — that is the part worth keeping.".into(),
    ]
}

/// `## When to access memories` section.
pub fn when_to_access_section() -> Vec<String> {
    vec![
        "## When to access memories".into(),
        "- When memories seem relevant, or the user references prior-conversation work.".into(),
        "- You MUST access memory when the user explicitly asks you to check, recall, or remember.".into(),
        "- If the user says to *ignore* or *not use* memory: proceed as if MEMORY.md were empty. Do not apply remembered facts, cite, compare against, or mention memory content.".into(),
        memory_drift_caveat(),
    ]
}

/// Recall-side drift caveat.
pub fn memory_drift_caveat() -> String {
    "- Memory records can become stale over time. Use memory as context for what was true at a given point in time. Before answering the user or building assumptions based solely on information in memory records, verify that the memory is still correct and up-to-date by reading the current state of the files or resources. If a recalled memory conflicts with current information, trust what you observe now — and update or remove the stale memory rather than acting on it.".into()
}

/// `## Before recommending from memory` section.
pub fn trusting_recall_section() -> Vec<String> {
    vec![
        "## Before recommending from memory".into(),
        String::new(),
        "A memory that names a specific function, file, or flag is a claim that it existed *when the memory was written*. It may have been renamed, removed, or never merged. Before recommending it:".into(),
        String::new(),
        "- If the memory names a file path: check the file exists.".into(),
        "- If the memory names a function or flag: grep for it.".into(),
        "- If the user is about to act on your recommendation (not just asking about history), verify first.".into(),
        String::new(),
        r#""The memory says X exists" is not the same as "X exists now.""#.into(),
        String::new(),
        "A memory that summarizes repo state (activity logs, architecture snapshots) is frozen in time. If the user asks about *recent* or *current* state, prefer `git log` or reading the code over recalling the snapshot.".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory_type_valid() {
        assert_eq!(parse_memory_type(Some("user")), Some(MemoryType::User));
        assert_eq!(parse_memory_type(Some("feedback")), Some(MemoryType::Feedback));
        assert_eq!(parse_memory_type(Some("project")), Some(MemoryType::Project));
        assert_eq!(parse_memory_type(Some("reference")), Some(MemoryType::Reference));
    }

    #[test]
    fn test_parse_memory_type_invalid() {
        assert_eq!(parse_memory_type(Some("unknown")), None);
        assert_eq!(parse_memory_type(None), None);
    }

    #[test]
    fn test_memory_type_display() {
        assert_eq!(MemoryType::User.to_string(), "user");
        assert_eq!(MemoryType::Feedback.to_string(), "feedback");
        assert_eq!(MemoryType::Project.to_string(), "project");
        assert_eq!(MemoryType::Reference.to_string(), "reference");
    }

    #[test]
    fn test_memory_type_roundtrip() {
        for ty in MEMORY_TYPES {
            let s = ty.to_string();
            let parsed: MemoryType = s.parse().unwrap();
            assert_eq!(*ty, parsed);
        }
    }

    #[test]
    fn test_parse_frontmatter_basic() {
        let content = "---\nname: test memory\ndescription: a test\ntype: user\n---\n\nBody content here.";
        let (fm, offset) = parse_frontmatter(content);
        assert_eq!(fm.name.as_deref(), Some("test memory"));
        assert_eq!(fm.description.as_deref(), Some("a test"));
        assert_eq!(fm.memory_type, Some(MemoryType::User));
        assert!(offset > 0);
        assert!(content[offset..].starts_with("Body") || content[offset..].starts_with("\nBody"));
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "Just some regular content.";
        let (fm, offset) = parse_frontmatter(content);
        assert!(fm.name.is_none());
        assert!(fm.description.is_none());
        assert!(fm.memory_type.is_none());
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_parse_frontmatter_unknown_type() {
        let content = "---\nname: test\ntype: banana\n---\n\nBody";
        let (fm, _) = parse_frontmatter(content);
        assert_eq!(fm.name.as_deref(), Some("test"));
        assert_eq!(fm.memory_type, None);
    }

    #[test]
    fn test_parse_frontmatter_partial() {
        let content = "---\nname: only name\n---\nBody";
        let (fm, _) = parse_frontmatter(content);
        assert_eq!(fm.name.as_deref(), Some("only name"));
        assert!(fm.description.is_none());
        assert!(fm.memory_type.is_none());
    }

    #[test]
    fn test_parse_frontmatter_unclosed() {
        let content = "---\nname: test\nno closing delimiter";
        let (fm, offset) = parse_frontmatter(content);
        assert!(fm.name.is_none());
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_frontmatter_example_contains_all_types() {
        let example = memory_frontmatter_example().join("\n");
        for ty in MEMORY_TYPES {
            assert!(example.contains(&ty.to_string()));
        }
    }
}
