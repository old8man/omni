use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shows your Claude Code Year in Review.
///
/// Always enabled in claude-rs (no feature gate).
pub struct ThinkbackCommand;

#[async_trait]
impl Command for ThinkbackCommand {
    fn name(&self) -> &str {
        "think-back"
    }

    fn aliases(&self) -> &[&str] {
        &["thinkback"]
    }

    fn description(&self) -> &str {
        "Your 2025 Claude Code Year in Review"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut output = String::from("Claude Code — Year in Review\n");
        output.push_str("═══════════════════════════════\n\n");

        output.push_str(&format!("Current model: {}\n", ctx.model));
        output.push_str(&format!(
            "Session tokens: {} in / {} out\n",
            ctx.input_tokens, ctx.output_tokens
        ));
        output.push_str(&format!("Session cost: ${:.4}\n\n", ctx.total_cost));

        output.push_str(
            "For a full year-in-review with session history analysis,\n\
             run this command after accumulating session history.",
        );

        CommandResult::Output(output)
    }
}
