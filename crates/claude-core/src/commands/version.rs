use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shows the application version.
pub struct VersionCommand;

#[async_trait]
impl Command for VersionCommand {
    fn name(&self) -> &str {
        "version"
    }

    fn description(&self) -> &str {
        "Show application version"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output(format!("claude-rs v{}", env!("CARGO_PKG_VERSION")))
    }
}
