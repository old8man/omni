use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// View and update privacy settings.
///
/// Matches the TypeScript `privacy-settings` command. In the full
/// implementation this opens an interactive dialog to manage the
/// "Help improve Claude" (Grove) privacy setting. In the Rust CLI
/// version we direct the user to the web settings page and provide
/// information about how to manage their privacy preferences.
pub struct PrivacySettingsCommand;

const PRIVACY_SETTINGS_URL: &str = "https://claude.ai/settings/data-privacy-controls";

#[async_trait]
impl Command for PrivacySettingsCommand {
    fn name(&self) -> &str {
        "privacy-settings"
    }

    fn description(&self) -> &str {
        "View and update your privacy settings"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let opened = open_browser(PRIVACY_SETTINGS_URL);

        let mut lines = vec![
            "Privacy Settings".to_string(),
            "----------------".to_string(),
            String::new(),
        ];

        if opened {
            lines.push("Opening privacy settings in your browser...".to_string());
            lines.push(String::new());
            lines.push(format!("If it didn't open, visit: {}", PRIVACY_SETTINGS_URL));
        } else {
            lines.push(format!(
                "Review and manage your privacy settings at:\n  {}",
                PRIVACY_SETTINGS_URL
            ));
        }

        lines.push(String::new());
        lines.push("From the settings page you can:".to_string());
        lines.push("  - Toggle \"Help improve Claude\" (data training opt-in/out)".to_string());
        lines.push("  - Review data retention policies".to_string());
        lines.push("  - Manage conversation history settings".to_string());

        CommandResult::OpenInfoDialog {
            title: "Privacy Settings".to_string(),
            content: lines.join("\n"),
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
