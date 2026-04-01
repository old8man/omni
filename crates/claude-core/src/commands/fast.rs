use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Toggles fast mode (faster output, same model).
pub struct FastCommand;

#[async_trait]
impl Command for FastCommand {
    fn name(&self) -> &str {
        "fast"
    }

    fn description(&self) -> &str {
        "Toggle fast mode"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::ToggleFastMode
    }
}
