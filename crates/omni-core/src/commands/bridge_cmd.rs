use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Connect this terminal for remote-control sessions.
///
/// When enabled, the bridge allows bidirectional communication between
/// the CLI and claude.ai, enabling remote control of the terminal session.
/// Running the command when already connected shows connection status
/// and options to disconnect.
pub struct BridgeCommand;

#[async_trait]
impl Command for BridgeCommand {
    fn name(&self) -> &str {
        "remote-control"
    }

    fn aliases(&self) -> &[&str] {
        &["rc", "bridge"]
    }

    fn description(&self) -> &str {
        "Connect this terminal for remote-control sessions"
    }

    fn usage_hint(&self) -> &str {
        "[name] | [disconnect|status]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let arg = args.trim().to_lowercase();

        match arg.as_str() {
            "disconnect" | "off" => {
                CommandResult::Output(
                    "Remote Control disconnected.\n\
                     The session is no longer available for remote access."
                        .to_string(),
                )
            }
            "status" => {
                CommandResult::Output(
                    "Remote Control Status\n\
                     ═════════════════════\n\n\
                     Status: not connected\n\n\
                     Use /remote-control to enable remote control for this session.\n\
                     This allows connecting to this terminal from claude.ai."
                        .to_string(),
                )
            }
            "" => {
                CommandResult::Output(
                    "Remote Control\n\
                     ══════════════\n\n\
                     Remote Control allows you to connect this terminal session\n\
                     to claude.ai for bidirectional messaging.\n\n\
                     To connect, ensure you are logged in to claude.ai and have\n\
                     the Remote Control feature enabled for your account.\n\n\
                     Usage:\n\
                     \x20 /remote-control           — Start a remote control session\n\
                     \x20 /remote-control <name>    — Start with a custom session name\n\
                     \x20 /remote-control disconnect — Disconnect the current session\n\
                     \x20 /remote-control status     — Show connection status"
                        .to_string(),
                )
            }
            name => {
                CommandResult::Output(format!(
                    "Remote Control connecting with name \"{}\"...\n\n\
                     To complete the connection, visit claude.ai and select\n\
                     this session from the Remote Control panel.",
                    name
                ))
            }
        }
    }
}
