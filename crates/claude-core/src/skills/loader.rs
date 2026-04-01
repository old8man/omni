//! Filesystem skill loader.
//!
//! Skills are `.md` files with optional YAML frontmatter between `---` delimiters.
//! A directory containing an `index.md` is also treated as a single skill.

use std::path::Path;

use super::bundled::get_bundled_skills;
use super::types::{Skill, SkillFrontmatter, SkillSource};

/// Parse YAML frontmatter and body from a markdown string.
///
/// Returns `(frontmatter, body)`. If there is no frontmatter the returned
/// [`SkillFrontmatter`] will have all fields at their defaults.
pub fn parse_skill_frontmatter(content: &str) -> (SkillFrontmatter, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (SkillFrontmatter::default(), content.to_string());
    }

    // Find the closing `---`
    let after_first = &trimmed[3..];
    let rest = after_first.trim_start_matches(['\r', '\n']);
    if let Some(end) = rest.find("\n---") {
        let yaml_block = &rest[..end];
        let body = &rest[end + 4..]; // skip "\n---"
        let body = body.trim_start_matches(['\r', '\n']);
        let fm = serde_yaml_frontmatter_parse(yaml_block);
        (fm, body.to_string())
    } else {
        (SkillFrontmatter::default(), content.to_string())
    }
}

/// Minimal key-value YAML parser sufficient for skill frontmatter.
///
/// We avoid pulling in a full YAML crate by parsing the simple key: value
/// and key: [list] structures that frontmatter uses.
fn serde_yaml_frontmatter_parse(yaml: &str) -> SkillFrontmatter {
    let mut fm = SkillFrontmatter::default();

    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            match key {
                "name" => fm.name = Some(value.to_string()),
                "description" => fm.description = Some(value.to_string()),
                // Support both underscore and hyphenated forms
                "when_to_use" | "when-to-use" => fm.when_to_use = Some(value.to_string()),
                "model" => fm.model = Some(value.to_string()),
                "argument_hint" | "argument-hint" => fm.argument_hint = Some(value.to_string()),
                "user_invocable" | "user-invocable" => fm.user_invocable = value == "true",
                "disable_model_invocation" | "disable-model-invocation" => {
                    fm.disable_model_invocation = value == "true";
                }
                "allowed_tools" | "allowed-tools" => {
                    // Parse inline list: [tool1, tool2]
                    let inner = value.trim_start_matches('[').trim_end_matches(']');
                    if !inner.is_empty() {
                        fm.allowed_tools = inner
                            .split(',')
                            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                }
                _ => {}
            }
        }
    }

    fm
}

/// Load all `.md` skills from a directory (non-recursive for individual files,
/// but recognises subdirectories with `index.md`).
pub fn load_skills_dir(dir: &Path, source: SkillSource) -> Vec<Skill> {
    let mut skills = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return skills,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "md") {
            if let Some(skill) = load_skill_file(&path, source.clone()) {
                skills.push(skill);
            }
        } else if path.is_dir() {
            // Prefer SKILL.md (matches upstream convention), fall back to index.md
            let skill_md = path.join("SKILL.md");
            let index = path.join("index.md");
            let candidate = if skill_md.exists() {
                Some(skill_md)
            } else if index.exists() {
                Some(index)
            } else {
                None
            };
            if let Some(file) = candidate {
                if let Some(skill) = load_skill_file(&file, source.clone()) {
                    skills.push(skill);
                }
            }
        }
    }

    skills
}

/// Load a single skill from a markdown file.
fn load_skill_file(path: &Path, source: SkillSource) -> Option<Skill> {
    let content = std::fs::read_to_string(path).ok()?;
    let (fm, body) = parse_skill_frontmatter(&content);

    let file_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    // For SKILL.md or index.md, use the parent directory name
    let default_name = if file_name == "index" || file_name == "SKILL" {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
    } else {
        file_name
    };

    let name = fm.name.unwrap_or_else(|| default_name.to_string());
    let description = fm
        .description
        .unwrap_or_else(|| extract_description_from_markdown(&body));

    Some(Skill {
        name,
        description,
        source,
        when_to_use: fm.when_to_use,
        body,
        allowed_tools: fm.allowed_tools,
        model: fm.model,
        user_invocable: fm.user_invocable,
        argument_hint: fm.argument_hint,
        disable_model_invocation: fm.disable_model_invocation,
    })
}

/// Extract the first non-heading, non-empty paragraph from markdown as a description.
fn extract_description_from_markdown(md: &str) -> String {
    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let desc = if trimmed.len() > 120 {
            format!("{}...", &trimmed[..117])
        } else {
            trimmed.to_string()
        };
        return desc;
    }
    String::new()
}

/// Discover all skills from standard locations plus bundled skills.
pub fn discover_all_skills(project_root: Option<&Path>) -> Vec<Skill> {
    let mut skills = get_bundled_skills();

    // User skills: ~/.claude-omni/skills/
    if let Some(home) = dirs::home_dir() {
        let user_dir = home.join(crate::config::paths::OMNI_DIR_NAME).join("skills");
        skills.extend(load_skills_dir(&user_dir, SkillSource::User));
    }

    // Project skills: <project_root>/.claude-omni/skills/
    if let Some(root) = project_root {
        let project_dir = root.join(crate::config::paths::PROJECT_DIR_NAME).join("skills");
        skills.extend(load_skills_dir(&project_dir, SkillSource::Project));
    }

    skills
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_with_yaml() {
        let content = r#"---
name: my-skill
description: A test skill
user_invocable: true
allowed_tools: [Read, Write]
---

This is the body."#;

        let (fm, body) = parse_skill_frontmatter(content);
        assert_eq!(fm.name.as_deref(), Some("my-skill"));
        assert_eq!(fm.description.as_deref(), Some("A test skill"));
        assert!(fm.user_invocable);
        assert_eq!(fm.allowed_tools, vec!["Read", "Write"]);
        assert!(body.contains("This is the body."));
    }

    #[test]
    fn test_parse_frontmatter_without_yaml() {
        let content = "Just plain markdown body";
        let (fm, body) = parse_skill_frontmatter(content);
        assert!(fm.name.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn test_extract_description() {
        let md = "# Title\n\nThis is the description.\n\nMore text.";
        assert_eq!(
            extract_description_from_markdown(md),
            "This is the description."
        );
    }
}
