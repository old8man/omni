use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shows current session status.
pub struct StatusCommand;

#[async_trait]
impl Command for StatusCommand {
    fn name(&self) -> &str {
        "status"
    }

    fn description(&self) -> &str {
        "Show session status"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let session = ctx.session_id.as_deref().unwrap_or("(no active session)");
        let lines = [
            format!("Session: {}", session),
            format!("Model:   {}", ctx.model),
            format!(
                "Tokens:  {} in / {} out",
                ctx.input_tokens, ctx.output_tokens
            ),
            format!("Cost:    ${:.4}", ctx.total_cost),
        ];
        CommandResult::Output(lines.join("\n"))
    }
}
