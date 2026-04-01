use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shows comprehensive session / system status in a TUI dialog overlay.
pub struct StatusCommand;

#[async_trait]
impl Command for StatusCommand {
    fn name(&self) -> &str {
        "status"
    }

    fn description(&self) -> &str {
        "Show comprehensive session and system status"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::OpenStatusDialog
    }
}
