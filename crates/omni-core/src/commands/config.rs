use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Opens the interactive configuration panel.
pub struct ConfigCommand;

#[async_trait]
impl Command for ConfigCommand {
    fn name(&self) -> &str {
        "config"
    }

    fn description(&self) -> &str {
        "Open interactive configuration panel"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::OpenConfigPanel
    }
}
