use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Install the Claude Slack app.
///
/// Matches the TypeScript `install-slack-app` command. Opens the Slack
/// marketplace page for the Claude app so the user can install it into
/// their workspace.
pub struct InstallSlackAppCommand;

const SLACK_APP_URL: &str = "https://slack.com/marketplace/A08SF47R6P4-claude";

#[async_trait]
impl Command for InstallSlackAppCommand {
    fn name(&self) -> &str {
        "install-slack-app"
    }

    fn description(&self) -> &str {
        "Install the Claude Slack app"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let opened = open_browser(SLACK_APP_URL);

        let content = if opened {
            format!("Opening Slack app installation page in browser...\n\n  {}", SLACK_APP_URL)
        } else {
            format!("Install the Claude Slack app:\n\n  {}", SLACK_APP_URL)
        };
        CommandResult::OpenInfoDialog {
            title: "Slack App Setup".to_string(),
            content,
        }
    }
}

/// Try to open a URL in the default browser.
fn open_browser(url: &str) -> bool {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).output()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .output()
    } else {
        std::process::Command::new("xdg-open").arg(url).output()
    };

    result.map(|o| o.status.success()).unwrap_or(false)
}
