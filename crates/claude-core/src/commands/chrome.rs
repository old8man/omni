use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Claude in Chrome (Beta) settings.
///
/// Manages the Claude in Chrome browser extension integration,
/// including installation, connection status, permissions, and
/// default behavior configuration.
pub struct ChromeCommand;

const CHROME_EXTENSION_URL: &str = "https://claude.ai/chrome";
const CHROME_PERMISSIONS_URL: &str = "https://clau.de/chrome/permissions";
const CHROME_RECONNECT_URL: &str = "https://clau.de/chrome/reconnect";

#[async_trait]
impl Command for ChromeCommand {
    fn name(&self) -> &str {
        "chrome"
    }

    fn description(&self) -> &str {
        "Claude in Chrome (Beta) settings"
    }

    fn usage_hint(&self) -> &str {
        "[install|permissions|reconnect|toggle|status]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let arg = args.trim().to_lowercase();

        match arg.as_str() {
            "install" | "install-extension" => {
                CommandResult::Output(format!(
                    "Install the Claude in Chrome extension:\n\n\
                     \x20 {}\n\n\
                     After installing, reload this page and run /chrome status\n\
                     to verify the connection.",
                    CHROME_EXTENSION_URL
                ))
            }
            "permissions" | "manage-permissions" => {
                CommandResult::Output(format!(
                    "Manage Claude in Chrome permissions:\n\n\
                     \x20 {}\n\n\
                     Here you can control which sites the extension can access\n\
                     and what actions it can perform.",
                    CHROME_PERMISSIONS_URL
                ))
            }
            "reconnect" => {
                CommandResult::Output(format!(
                    "Reconnect the Claude in Chrome extension:\n\n\
                     \x20 {}\n\n\
                     Use this if the extension has become disconnected\n\
                     from the Claude Code session.",
                    CHROME_RECONNECT_URL
                ))
            }
            "toggle" => {
                // In a full implementation this would toggle the config setting
                CommandResult::Output(
                    "Claude in Chrome default behavior toggled.\n\n\
                     When enabled by default, new sessions will automatically\n\
                     connect to the Chrome extension if it is installed."
                        .to_string(),
                )
            }
            "" | "status" => {
                let mut output = String::from("Claude in Chrome (Beta)\n");
                output.push_str("══════════════════════\n\n");

                output.push_str("Extension status: not connected\n\n");

                output.push_str("Available actions:\n\n");
                output.push_str(
                    "  /chrome install      — Install the Chrome extension\n\
                     \x20 /chrome reconnect    — Reconnect a disconnected extension\n\
                     \x20 /chrome permissions  — Manage extension permissions\n\
                     \x20 /chrome toggle       — Toggle auto-connect on new sessions\n\
                     \x20 /chrome status       — Show connection status\n\n",
                );

                output.push_str(
                    "Claude in Chrome allows Claude to interact with web pages\n\
                     you are viewing, helping with tasks like filling forms,\n\
                     reading page content, and navigating websites.",
                );

                CommandResult::Output(output)
            }
            other => CommandResult::Output(format!(
                "Unknown subcommand: \"{}\". Use install, permissions, reconnect, toggle, or status.",
                other
            )),
        }
    }
}
