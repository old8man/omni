use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Sends feedback or reports an issue.
pub struct FeedbackCommand;

#[async_trait]
impl Command for FeedbackCommand {
    fn name(&self) -> &str {
        "feedback"
    }

    fn description(&self) -> &str {
        "Send feedback or report an issue"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output(
            "To report issues or give feedback:\n\
             https://github.com/anthropics/claude-code/issues"
                .to_string(),
        )
    }
}
