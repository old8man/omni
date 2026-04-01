use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shows usage statistics and activity summary.
pub struct StatsCommand;

#[async_trait]
impl Command for StatsCommand {
    fn name(&self) -> &str {
        "stats"
    }

    fn description(&self) -> &str {
        "Show your Claude Code usage statistics and activity"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut output = String::from("Claude Code Statistics\n");
        output.push_str("═══════════════════════\n\n");

        output.push_str(&format!("Model: {}\n", ctx.model));

        if let Some(ref sid) = ctx.session_id {
            output.push_str(&format!("Session: {}\n", sid));
        }

        output.push_str(&format!(
            "\nSession usage:\n  Input tokens:  {}\n  Output tokens: {}\n  Total cost:    ${:.4}\n",
            ctx.input_tokens, ctx.output_tokens, ctx.total_cost
        ));

        CommandResult::Output(output)
    }
}
