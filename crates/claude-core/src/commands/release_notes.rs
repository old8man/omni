use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// View release notes for Claude Code.
///
/// Attempts to read cached release notes from the local changelog file.
/// Falls back to displaying a link to the online changelog if no local
/// data is available.
pub struct ReleaseNotesCommand;

const CHANGELOG_URL: &str = "https://github.com/anthropics/claude-code/releases";

#[async_trait]
impl Command for ReleaseNotesCommand {
    fn name(&self) -> &str {
        "release-notes"
    }

    fn description(&self) -> &str {
        "View release notes"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        // Try to read a locally cached changelog
        let changelog_path = crate::config::paths::claude_dir()
            .map(|d| d.join("changelog.md")).ok();

        if let Some(path) = changelog_path {
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                let notes = parse_changelog(&content);
                if !notes.is_empty() {
                    return CommandResult::Output(format_release_notes(&notes));
                }
            }
        }

        // Fallback: show link
        CommandResult::Output(format!(
            "See the full changelog at: {}",
            CHANGELOG_URL
        ))
    }
}

/// A parsed release note entry: (version, list of changes).
type ReleaseEntry = (String, Vec<String>);

/// Parse a markdown changelog into structured release entries.
///
/// Expects headings like `## Version X.Y.Z` or `## X.Y.Z` followed by
/// bullet-point lines starting with `- ` or `* `.
fn parse_changelog(content: &str) -> Vec<ReleaseEntry> {
    let mut entries: Vec<ReleaseEntry> = Vec::new();
    let mut current_version: Option<String> = None;
    let mut current_notes: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Detect version headings: ## Version X.Y.Z or ## X.Y.Z
        if trimmed.starts_with("## ") {
            // Flush previous entry
            if let Some(ver) = current_version.take() {
                if !current_notes.is_empty() {
                    entries.push((ver, std::mem::take(&mut current_notes)));
                }
            }

            let heading = trimmed.trim_start_matches("## ").trim();
            let version = heading
                .strip_prefix("Version ")
                .or_else(|| heading.strip_prefix("v"))
                .unwrap_or(heading)
                .to_string();
            current_version = Some(version);
        } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let note = trimmed[2..].trim().to_string();
            if !note.is_empty() {
                current_notes.push(note);
            }
        }
    }

    // Flush last entry
    if let Some(ver) = current_version {
        if !current_notes.is_empty() {
            entries.push((ver, current_notes));
        }
    }

    entries
}

/// Format parsed release entries into a display string.
fn format_release_notes(entries: &[ReleaseEntry]) -> String {
    entries
        .iter()
        .map(|(version, notes)| {
            let header = format!("Version {}:", version);
            let bullets: String = notes
                .iter()
                .map(|note| format!("\u{00b7} {}", note))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{}\n{}", header, bullets)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_changelog() {
        let input = "\
## Version 1.2.0
- Added new feature
- Fixed a bug

## 1.1.0
* Improved performance
* Updated dependencies
";
        let entries = parse_changelog(input);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "1.2.0");
        assert_eq!(entries[0].1.len(), 2);
        assert_eq!(entries[1].0, "1.1.0");
        assert_eq!(entries[1].1.len(), 2);
    }

    #[test]
    fn test_format_release_notes() {
        let entries = vec![
            ("1.0.0".to_string(), vec!["Initial release".to_string()]),
        ];
        let output = format_release_notes(&entries);
        assert!(output.contains("Version 1.0.0:"));
        assert!(output.contains("\u{00b7} Initial release"));
    }
}
