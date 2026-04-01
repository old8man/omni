use async_trait::async_trait;

use super::{Command, CommandContext, CommandRegistry, CommandResult};

/// Lists all available commands with descriptions and aliases.
pub struct HelpCommand;

#[async_trait]
impl Command for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }

    fn description(&self) -> &str {
        "Show available commands"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let reg = CommandRegistry::default_registry();
        let cmds = reg.all_commands();
        let mut lines = vec!["Available commands:".to_string(), String::new()];
        for cmd in &cmds {
            let aliases = cmd.aliases();
            let alias_str = if aliases.is_empty() {
                String::new()
            } else {
                format!(" ({})", aliases.join(", "))
            };
            let usage = if cmd.usage_hint().is_empty() {
                String::new()
            } else {
                format!(" {}", cmd.usage_hint())
            };
            lines.push(format!(
                "  /{}{}{} — {}",
                cmd.name(),
                usage,
                alias_str,
                cmd.description()
            ));
        }
        CommandResult::Output(lines.join("\n"))
    }
}
