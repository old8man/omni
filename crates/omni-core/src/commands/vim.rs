use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Toggles vim mode on or off.
pub struct VimCommand;

#[async_trait]
impl Command for VimCommand {
    fn name(&self) -> &str {
        "vim"
    }

    fn description(&self) -> &str {
        "Toggle vim mode"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::ToggleVimMode
    }
}
