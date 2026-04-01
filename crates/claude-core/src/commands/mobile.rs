use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Show download links (and optionally QR codes) for the Claude mobile app.
///
/// Matches the TypeScript `mobile` command which displays QR codes for
/// iOS and Android app stores. In the TUI-only Rust version we emit
/// plain-text URLs; a future TUI layer can render them as QR codes.
pub struct MobileCommand;

const IOS_URL: &str = "https://apps.apple.com/app/claude-by-anthropic/id6473753684";
const ANDROID_URL: &str = "https://play.google.com/store/apps/details?id=com.anthropic.claude";

#[async_trait]
impl Command for MobileCommand {
    fn name(&self) -> &str {
        "mobile"
    }

    fn aliases(&self) -> &[&str] {
        &["ios", "android"]
    }

    fn description(&self) -> &str {
        "Show QR code to download the Claude mobile app"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let lines = [
            "Claude Mobile App".to_string(),
            "-----------------".to_string(),
            String::new(),
            "iOS (iPhone / iPad):".to_string(),
            format!("  {}", IOS_URL),
            String::new(),
            "Android:".to_string(),
            format!("  {}", ANDROID_URL),
            String::new(),
            "Scan the QR codes above or visit the links to download.".to_string(),
        ];

        CommandResult::Output(lines.join("\n"))
    }
}
