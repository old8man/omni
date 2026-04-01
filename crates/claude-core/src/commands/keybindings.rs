use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Manages keybindings.
pub struct KeybindingsCommand;

#[async_trait]
impl Command for KeybindingsCommand {
    fn name(&self) -> &str {
        "keybindings"
    }

    fn description(&self) -> &str {
        "View and customize keyboard shortcuts"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output(
            "Keybindings are configured in ~/.claude/keybindings.json\n\
             \n\
             Use /keybindings-help skill for detailed customization help."
                .to_string(),
        )
    }
}
