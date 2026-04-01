use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Share a free week of Claude Code with friends.
///
/// Displays the user's referral passes, allowing them to generate
/// and share invite links. When eligible, users can earn extra usage
/// for successful referrals.
pub struct PassesCommand;

const PASSES_URL: &str = "https://claude.ai/referrals";

#[async_trait]
impl Command for PassesCommand {
    fn name(&self) -> &str {
        "passes"
    }

    fn description(&self) -> &str {
        "Share a free week of Claude Code with friends"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let mut output = String::from("Guest Passes\n");
        output.push_str("════════════\n\n");

        output.push_str(
            "Share Claude Code with your friends! Each guest pass gives\n\
             the recipient a free week of Claude Code access.\n\n",
        );

        output.push_str(&format!(
            "Visit {} to manage your passes.\n\n",
            PASSES_URL
        ));

        output.push_str(
            "When your referrals sign up, you may earn extra usage as a reward.\n\n\
             How it works:\n\
             \x20 1. Share your referral link with a friend\n\
             \x20 2. They get a free week of Claude Code\n\
             \x20 3. You earn extra usage when they sign up",
        );

        CommandResult::Output(output)
    }
}
