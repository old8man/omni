use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Displays and manages hooks configuration.
pub struct HooksCommand;

#[async_trait]
impl Command for HooksCommand {
    fn name(&self) -> &str {
        "hooks"
    }

    fn description(&self) -> &str {
        "View and manage hooks"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut lines = vec!["Hooks configuration:".to_string()];

        // Check project settings
        let project_settings = ctx.cwd.join(crate::config::paths::PROJECT_DIR_NAME).join("settings.json");
        if project_settings.exists() {
            lines.push(format!("  Project: {}", project_settings.display()));
        }

        // Check user settings
        if let Some(home) = dirs::home_dir() {
            let user_settings = home.join(crate::config::paths::OMNI_DIR_NAME).join("settings.json");
            if user_settings.exists() {
                lines.push(format!("  User:    {}", user_settings.display()));
            }
        }

        if lines.len() == 1 {
            lines.push("  No hooks configured.".to_string());
        }

        lines.push(String::new());
        lines.push(
            "Hook events: PreToolUse, PostToolUse, Stop, Notification, SessionStart".to_string(),
        );
        lines.push("Use /update-config to add or modify hooks.".to_string());

        CommandResult::Output(lines.join("\n"))
    }
}
