use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Compacts the conversation context, optionally with custom instructions.
pub struct CompactCommand;

#[async_trait]
impl Command for CompactCommand {
    fn name(&self) -> &str {
        "compact"
    }

    fn description(&self) -> &str {
        "Compact conversation context"
    }

    fn usage_hint(&self) -> &str {
        "[instructions]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let instructions = if args.trim().is_empty() {
            None
        } else {
            Some(args.trim().to_string())
        };
        CommandResult::CompactMessages(instructions)
    }
}
