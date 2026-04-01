use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Toggles brief-only (KAIROS) mode.
///
/// When brief mode is enabled, the model uses the BriefTool for all
/// user-facing output. Plain text outside the tool is hidden.
pub struct BriefCommand;

#[async_trait]
impl Command for BriefCommand {
    fn name(&self) -> &str {
        "brief"
    }

    fn description(&self) -> &str {
        "Toggle brief-only mode"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::ToggleBriefMode
    }
}
