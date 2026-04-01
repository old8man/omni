use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Toggles voice input mode.
pub struct VoiceCommand;

#[async_trait]
impl Command for VoiceCommand {
    fn name(&self) -> &str {
        "voice"
    }

    fn description(&self) -> &str {
        "Toggle voice input mode"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output("Voice mode toggled.".to_string())
    }
}
