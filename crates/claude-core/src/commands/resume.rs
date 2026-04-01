use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Resumes a previous session by ID.
pub struct ResumeCommand;

#[async_trait]
impl Command for ResumeCommand {
    fn name(&self) -> &str {
        "resume"
    }

    fn description(&self) -> &str {
        "Resume a previous session"
    }

    fn usage_hint(&self) -> &str {
        "<session-id>"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let id = args.trim();
        if id.is_empty() {
            // Open the interactive session picker
            CommandResult::OpenPicker("session".to_string())
        } else {
            CommandResult::ResumeSession(id.to_string())
        }
    }
}
