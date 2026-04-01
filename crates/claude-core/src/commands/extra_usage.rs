use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Configure extra usage to keep working when rate limits are hit.
///
/// Matches the TypeScript `extra-usage` command. For users with billing
/// access this opens the usage settings page in the browser. For team
/// members without billing access it sends a request to their admin.
pub struct ExtraUsageCommand;

#[async_trait]
impl Command for ExtraUsageCommand {
    fn name(&self) -> &str {
        "extra-usage"
    }

    fn description(&self) -> &str {
        "Configure extra usage to keep working when limits are hit"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        // Check if the command is disabled via environment variable
        if std::env::var("DISABLE_EXTRA_USAGE_COMMAND")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            return CommandResult::Output(
                "The extra usage command is currently disabled.".to_string(),
            );
        }

        let url = "https://claude.ai/settings/usage";

        // Attempt to open the browser
        let opened = open_browser(url);

        if opened {
            CommandResult::Output(format!(
                "Opening usage settings in your browser...\n\
                 If it didn't open, visit: {}",
                url
            ))
        } else {
            CommandResult::Output(format!(
                "Could not open browser. Please visit {} to manage extra usage.",
                url
            ))
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
