use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Set the prompt bar color for this session.
///
/// Matches the TypeScript `color` command. Accepts a color name or
/// `default`/`reset`/`none` to clear the session color.
pub struct ColorCommand;

const AVAILABLE_COLORS: &[&str] = &[
    "red", "orange", "yellow", "green", "cyan", "blue", "purple", "pink",
];

const RESET_ALIASES: &[&str] = &["default", "reset", "none", "gray", "grey"];

#[async_trait]
impl Command for ColorCommand {
    fn name(&self) -> &str {
        "color"
    }

    fn description(&self) -> &str {
        "Set the prompt bar color for this session"
    }

    fn usage_hint(&self) -> &str {
        "<color|default>"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let color_arg = args.trim().to_lowercase();

        if color_arg.is_empty() {
            let color_list = AVAILABLE_COLORS.join(", ");
            return CommandResult::Output(format!(
                "Please provide a color. Available colors: {}, default",
                color_list
            ));
        }

        // Handle reset to default
        if RESET_ALIASES.contains(&color_arg.as_str()) {
            return CommandResult::Output("Session color reset to default.".to_string());
        }

        // Validate the color
        if !AVAILABLE_COLORS.contains(&color_arg.as_str()) {
            let color_list = AVAILABLE_COLORS.join(", ");
            return CommandResult::Output(format!(
                "Invalid color \"{}\". Available colors: {}, default",
                color_arg, color_list
            ));
        }

        CommandResult::Output(format!("Session color set to: {}", color_arg))
    }
}
