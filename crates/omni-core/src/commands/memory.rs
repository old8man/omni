use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Lists CLAUDE.md memory files found in standard locations.
pub struct MemoryCommand;

#[async_trait]
impl Command for MemoryCommand {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "List CLAUDE.md memory files"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut found: Vec<String> = Vec::new();

        // Check project root
        if let Some(ref root) = ctx.project_root {
            let p = root.join("CLAUDE.md");
            if p.exists() {
                found.push(format!("  {}", p.display()));
            }
            let p2 = root.join(crate::config::paths::PROJECT_DIR_NAME).join("CLAUDE.md");
            if p2.exists() {
                found.push(format!("  {}", p2.display()));
            }
        }

        // Check home directory
        if let Some(home) = dirs::home_dir() {
            let p = home.join(crate::config::paths::OMNI_DIR_NAME).join("CLAUDE.md");
            if p.exists() {
                found.push(format!("  {}", p.display()));
            }
        }

        let content = if found.is_empty() {
            "No CLAUDE.md files found.".to_string()
        } else {
            let mut lines = vec![format!("Found {} CLAUDE.md file(s):", found.len())];
            lines.extend(found);
            lines.join("\n")
        };
        CommandResult::OpenInfoDialog {
            title: "Memory Files".to_string(),
            content,
        }
    }
}
