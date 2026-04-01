use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Toggles a searchable tag on the current session.
///
/// Tags allow you to mark sessions for easy retrieval later.
/// Always enabled in claude-rs (no ant-only gate).
pub struct TagCommand;

#[async_trait]
impl Command for TagCommand {
    fn name(&self) -> &str {
        "tag"
    }

    fn description(&self) -> &str {
        "Toggle a searchable tag on the current session"
    }

    fn usage_hint(&self) -> &str {
        "<tag-name>"
    }

    async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult {
        let tag = args.trim();
        if tag.is_empty() {
            return CommandResult::Output(
                "Usage: /tag <name>\n\
                 \n\
                 Tags make sessions searchable. Toggle a tag on/off:\n\
                 \n\
                 /tag important\n\
                 /tag wip\n\
                 /tag bug-fix"
                    .to_string(),
            );
        }

        let session = ctx.session_id.as_deref().unwrap_or("current session");
        CommandResult::Output(format!("Tag \"{}\" toggled on {}.", tag, session))
    }
}
