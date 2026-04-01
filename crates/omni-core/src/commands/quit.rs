use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Exits the application.
pub struct QuitCommand;

#[async_trait]
impl Command for QuitCommand {
    fn name(&self) -> &str {
        "quit"
    }

    fn aliases(&self) -> &[&str] {
        &["exit"]
    }

    fn description(&self) -> &str {
        "Quit the application"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Quit
    }
}
