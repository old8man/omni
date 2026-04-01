use async_trait::async_trait;
use std::path::Path;

use crate::utils::git;

use super::{Command, CommandContext, CommandResult};

/// Shows git diff output for the current project.
pub struct DiffCommand;

const MAX_DIFF_LINES: usize = 200;

#[async_trait]
impl Command for DiffCommand {
    fn name(&self) -> &str {
        "diff"
    }

    fn description(&self) -> &str {
        "Show git diff for current project"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let repo = Path::new(&ctx.cwd);

        // Use utils::git for numstat + diff stat summary
        let numstat_output = git::generate_numstat(repo);
        let stat_section = match &numstat_output {
            Some(raw) => {
                let (stats, _per_file) = git::parse_git_numstat(raw);
                format!(
                    "{} file(s) changed, {} insertions(+), {} deletions(-)",
                    stats.files_count, stats.lines_added, stats.lines_removed
                )
            }
            None => {
                // Fall back to `git diff --stat` if numstat failed
                let stat = std::process::Command::new("git")
                    .args(["diff", "--stat"])
                    .current_dir(&ctx.cwd)
                    .output();
                match stat {
                    Ok(output) if output.status.success() => {
                        String::from_utf8_lossy(&output.stdout).trim().to_string()
                    }
                    Ok(output) => {
                        return CommandResult::Output(format!(
                            "git diff --stat failed: {}",
                            String::from_utf8_lossy(&output.stderr)
                        ));
                    }
                    Err(e) => {
                        return CommandResult::Output(format!("Failed to run git: {}", e));
                    }
                }
            }
        };

        if stat_section.trim().is_empty() {
            return CommandResult::Output("No changes (working tree clean).".to_string());
        }

        // Use utils::git for the full diff with hunk parsing
        let diff_section = match git::generate_diff(repo) {
            Some(full) => {
                let lines: Vec<&str> = full.lines().collect();
                if lines.len() > MAX_DIFF_LINES {
                    let truncated: String = lines[..MAX_DIFF_LINES].join("\n");
                    format!(
                        "{}\n\n... truncated ({} lines total, showing first {})",
                        truncated,
                        lines.len(),
                        MAX_DIFF_LINES
                    )
                } else {
                    full
                }
            }
            None => "(could not retrieve full diff)".to_string(),
        };

        CommandResult::Output(format!(
            "{}\n\n{}",
            stat_section.trim(),
            diff_section.trim()
        ))
    }
}
