use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Manages MCP servers.
pub struct McpCommand;

#[async_trait]
impl Command for McpCommand {
    fn name(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> &str {
        "Manage MCP servers"
    }

    fn usage_hint(&self) -> &str {
        "[enable|disable <server-name>]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let args = args.trim();
        if args.is_empty() {
            CommandResult::Output(
                "MCP server management:\n\
                 \n\
                 /mcp enable <server>  — Enable an MCP server\n\
                 /mcp disable <server> — Disable an MCP server\n\
                 \n\
                 Servers are configured in .claude/settings.json"
                    .to_string(),
            )
        } else {
            CommandResult::Output(format!("MCP: {}", args))
        }
    }
}
