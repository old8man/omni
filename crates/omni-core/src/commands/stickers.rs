use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Opens the Claude Code sticker order page.
pub struct StickersCommand;

#[async_trait]
impl Command for StickersCommand {
    fn name(&self) -> &str {
        "stickers"
    }

    fn description(&self) -> &str {
        "Order Claude Code stickers"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output(
            "Visit https://www.anthropic.com/store to order Claude Code stickers!".to_string(),
        )
    }
}
