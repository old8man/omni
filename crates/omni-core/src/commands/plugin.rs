use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Manages plugins.
pub struct PluginCommand;

#[async_trait]
impl Command for PluginCommand {
    fn name(&self) -> &str {
        "plugin"
    }

    fn description(&self) -> &str {
        "Manage plugins"
    }

    fn usage_hint(&self) -> &str {
        "[enable|disable|list]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let args = args.trim();
        if args.is_empty() || args == "list" {
            CommandResult::Output(
                "Plugins are managed in settings.json:\n\
                 {\n  \"enabledPlugins\": {\n    \"plugin-name@source\": true\n  }\n}\n\
                 \n\
                 Check ~/.claude/plugins/ and .claude/plugins/ for installed plugins."
                    .to_string(),
            )
        } else {
            CommandResult::Output(format!("Plugin command: {}", args))
        }
    }
}
