use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shows options when a rate limit is reached.
///
/// This command is typically invoked internally when the user hits a rate
/// limit. It presents available options such as upgrading the subscription,
/// enabling extra usage, or waiting for the limit to reset.
pub struct RateLimitOptionsCommand;

#[async_trait]
impl Command for RateLimitOptionsCommand {
    fn name(&self) -> &str {
        "rate-limit-options"
    }

    fn description(&self) -> &str {
        "Show options when rate limit is reached"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut output = String::from("Rate Limit Reached\n");
        output.push_str("══════════════════\n\n");
        output.push_str(&format!("Current model: {}\n\n", ctx.model));

        output.push_str("You have reached your usage limit. Here are your options:\n\n");

        output.push_str("  1. Wait for your rate limit to reset\n");
        output.push_str("     Limits typically reset within the hour.\n\n");

        output.push_str("  2. Switch to a different model\n");
        output.push_str(
            "     Use /model to switch to a less busy model.\n\n",
        );

        output.push_str("  3. Upgrade your plan\n");
        output.push_str(
            "     Visit https://claude.ai/settings/billing to upgrade\n\
             \x20    for higher rate limits.\n\n",
        );

        output.push_str("  4. Enable extra usage\n");
        output.push_str(
            "     If you are on a Pro or Max plan, you can enable\n\
             \x20    extra usage at https://claude.ai/settings/billing\n\
             \x20    for pay-as-you-go access beyond your plan limits.\n\n",
        );

        output.push_str("  5. Use an API key\n");
        output.push_str(
            "     Set ANTHROPIC_API_KEY for pay-per-token usage\n\
             \x20    with no rate limits.",
        );

        CommandResult::Output(output)
    }
}
