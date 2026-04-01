use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Copies the last assistant response to the system clipboard.
///
/// Optionally accepts a number N to copy the Nth-latest response.
pub struct CopyCommand;

#[async_trait]
impl Command for CopyCommand {
    fn name(&self) -> &str {
        "copy"
    }

    fn description(&self) -> &str {
        "Copy Claude's last response to clipboard (or /copy N for the Nth-latest)"
    }

    fn usage_hint(&self) -> &str {
        "[N]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let n: usize = args.trim().parse().unwrap_or(1);
        // The TUI layer will handle extracting the Nth-latest response
        // and copying to clipboard via arboard or similar.
        CommandResult::Output(format!("Copied response #{} to clipboard.", n))
    }
}
