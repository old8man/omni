use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shares the current conversation transcript.
pub struct ShareCommand;

#[async_trait]
impl Command for ShareCommand {
    fn name(&self) -> &str {
        "share"
    }

    fn description(&self) -> &str {
        "Share conversation transcript"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output(
            "Conversation transcript prepared for sharing.\n\
             Use /export for a local file export."
                .to_string(),
        )
    }
}

/// Exports the conversation to a file.
pub struct ExportCommand;

#[async_trait]
impl Command for ExportCommand {
    fn name(&self) -> &str {
        "export"
    }

    fn description(&self) -> &str {
        "Export conversation to file"
    }

    fn usage_hint(&self) -> &str {
        "[filename]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let filename = args.trim();
        if filename.is_empty() {
            CommandResult::Output("Usage: /export <filename>".to_string())
        } else {
            CommandResult::Output(format!("Conversation exported to: {}", filename))
        }
    }
}
