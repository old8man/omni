use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Initializes project setup (CLAUDE.md, .claude/ directory).
pub struct InitCommand;

#[async_trait]
impl Command for InitCommand {
    fn name(&self) -> &str {
        "init"
    }

    fn description(&self) -> &str {
        "Initialize project configuration"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let claude_dir = ctx.cwd.join(".claude");
        let claude_md = ctx.cwd.join("CLAUDE.md");

        let mut lines = vec!["Project initialization:".to_string()];

        if claude_dir.exists() {
            lines.push(format!(
                "  [ok] .claude/ directory exists: {}",
                claude_dir.display()
            ));
        } else {
            lines.push(
                "  [!!] .claude/ directory not found — create it to store project settings"
                    .to_string(),
            );
        }

        if claude_md.exists() {
            lines.push(format!("  [ok] CLAUDE.md exists: {}", claude_md.display()));
        } else {
            lines.push(
                "  [!!] CLAUDE.md not found — create it to define project conventions".to_string(),
            );
        }

        lines.push(String::new());
        lines.push("To complete setup, create any missing files above.".to_string());

        CommandResult::Output(lines.join("\n"))
    }
}
