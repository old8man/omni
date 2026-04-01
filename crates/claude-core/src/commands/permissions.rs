use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Displays current permission rules.
pub struct PermissionsCommand;

#[async_trait]
impl Command for PermissionsCommand {
    fn name(&self) -> &str {
        "permissions"
    }

    fn description(&self) -> &str {
        "View and manage permission rules"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::OpenInfoDialog {
            title: "Permissions".to_string(),
            content: "Permission rules are configured in settings.json files.\n\
                     \n\
                     Locations checked (in order):\n\
                     1. ~/.claude/settings.json (user)\n\
                     2. .claude/settings.json (project)\n\
                     3. .claude/settings.local.json (local overrides)\n\
                     \n\
                     Use /update-config to modify permission rules."
                .to_string(),
        }
    }
}
