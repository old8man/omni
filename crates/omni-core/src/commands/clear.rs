use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Clears the conversation history.
pub struct ClearCommand;

#[async_trait]
impl Command for ClearCommand {
    fn name(&self) -> &str {
        "clear"
    }

    fn description(&self) -> &str {
        "Clear conversation history"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::ClearConversation
    }
}
