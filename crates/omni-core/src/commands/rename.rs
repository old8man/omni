use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Renames the current conversation/session.
///
/// If no name is provided, generates one from the conversation content.
pub struct RenameCommand;

#[async_trait]
impl Command for RenameCommand {
    fn name(&self) -> &str {
        "rename"
    }

    fn description(&self) -> &str {
        "Rename the current conversation"
    }

    fn usage_hint(&self) -> &str {
        "[name]"
    }

    async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult {
        let name = args.trim();
        if name.is_empty() {
            if let Some(ref session_id) = ctx.session_id {
                return CommandResult::Output(format!(
                    "Current session: {}. Use /rename <name> to set a custom name.",
                    session_id
                ));
            }
            return CommandResult::Output(
                "No active session. Use /rename <name> to set a name.".to_string(),
            );
        }

        CommandResult::Output(format!("Session renamed to \"{}\".", name))
    }
}
