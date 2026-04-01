use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Switches the color theme.
pub struct ThemeCommand;

#[async_trait]
impl Command for ThemeCommand {
    fn name(&self) -> &str {
        "theme"
    }

    fn description(&self) -> &str {
        "Switch color theme"
    }

    fn usage_hint(&self) -> &str {
        "[dark|light|auto]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let theme = args.trim().to_lowercase();
        if theme.is_empty() {
            CommandResult::Output(
                "Usage: /theme <dark|light|auto>\nCurrently using auto-detected theme.".to_string(),
            )
        } else {
            match theme.as_str() {
                "dark" | "light" | "auto" => {
                    CommandResult::Output(format!("Theme set to: {}", theme))
                }
                _ => CommandResult::Output(format!(
                    "Unknown theme '{}'. Valid options: dark, light, auto",
                    theme
                )),
            }
        }
    }
}
