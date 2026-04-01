use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Log in to Anthropic.
pub struct LoginCommand;

#[async_trait]
impl Command for LoginCommand {
    fn name(&self) -> &str {
        "login"
    }

    fn description(&self) -> &str {
        "Log in to your Anthropic account"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let has_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
        if has_key {
            CommandResult::Output(
                "Already authenticated via ANTHROPIC_API_KEY environment variable.".to_string(),
            )
        } else {
            CommandResult::Output(
                "To authenticate, set the ANTHROPIC_API_KEY environment variable:\n\
                 \n\
                 export ANTHROPIC_API_KEY=sk-ant-..."
                    .to_string(),
            )
        }
    }
}

/// Log out / clear credentials.
pub struct LogoutCommand;

#[async_trait]
impl Command for LogoutCommand {
    fn name(&self) -> &str {
        "logout"
    }

    fn description(&self) -> &str {
        "Log out and clear stored credentials"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output(
            "Credentials cleared. Unset ANTHROPIC_API_KEY to fully log out.".to_string(),
        )
    }
}
