use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Toggles plan mode on or off.
pub struct PlanCommand;

#[async_trait]
impl Command for PlanCommand {
    fn name(&self) -> &str {
        "plan"
    }

    fn description(&self) -> &str {
        "Toggle plan mode"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::TogglePlanMode
    }
}
