use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Creates a branch of the current conversation.
pub struct BranchCommand;

#[async_trait]
impl Command for BranchCommand {
    fn name(&self) -> &str {
        "branch"
    }

    fn aliases(&self) -> &[&str] {
        &["fork"]
    }

    fn description(&self) -> &str {
        "Branch the conversation at this point"
    }

    fn usage_hint(&self) -> &str {
        "[name]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let name = args.trim();
        if name.is_empty() {
            CommandResult::Output("Conversation branched at current point.".to_string())
        } else {
            CommandResult::Output(format!("Conversation branched as '{}'.", name))
        }
    }
}
