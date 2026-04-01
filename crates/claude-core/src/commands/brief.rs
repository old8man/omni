use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Toggles brief-only (KAIROS) mode.
///
/// When brief mode is enabled, the model uses the BriefTool for all
/// user-facing output. Plain text outside the tool is hidden.
/// Always enabled in claude-rs (no feature gate).
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
        // In the full implementation this toggles isBriefOnly state and
        // makes the BriefTool available/unavailable. For now we output
        // a toggle message; the TUI layer handles actual state.
        CommandResult::Output("Brief mode toggled.".to_string())
    }
}
