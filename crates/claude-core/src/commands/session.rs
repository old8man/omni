use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shows remote session URL and information.
pub struct SessionCommand;

#[async_trait]
impl Command for SessionCommand {
    fn name(&self) -> &str {
        "session"
    }

    fn aliases(&self) -> &[&str] {
        &["remote"]
    }

    fn description(&self) -> &str {
        "Show session info and remote URL"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let session_id = ctx.session_id.as_deref().unwrap_or("(no active session)");
        CommandResult::Output(format!(
            "Session ID: {}\nModel: {}\nWorking dir: {}",
            session_id,
            ctx.model,
            ctx.cwd.display()
        ))
    }
}
