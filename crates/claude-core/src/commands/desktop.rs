use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Continue the current session in Claude Desktop.
///
/// Matches the TypeScript `desktop` command. Opens the Claude Desktop
/// application so the user can continue the conversation there.
pub struct DesktopCommand;

/// Check if the current platform supports Claude Desktop.
fn is_supported_platform() -> bool {
    cfg!(target_os = "macos") || (cfg!(target_os = "windows") && cfg!(target_arch = "x86_64"))
}

#[async_trait]
impl Command for DesktopCommand {
    fn name(&self) -> &str {
        "desktop"
    }

    fn aliases(&self) -> &[&str] {
        &["app"]
    }

    fn description(&self) -> &str {
        "Continue the current session in Claude Desktop"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        if !is_supported_platform() {
            return CommandResult::Output(
                "Claude Desktop is not available on this platform. \
                 It is supported on macOS and Windows (x64)."
                    .to_string(),
            );
        }

        let session_info = match &ctx.session_id {
            Some(id) => format!("session {}", id),
            None => "your conversation".to_string(),
        };

        // Attempt to open Claude Desktop via the system's open command
        let open_result = if cfg!(target_os = "macos") {
            std::process::Command::new("open")
                .arg("-a")
                .arg("Claude")
                .output()
        } else {
            // Windows
            std::process::Command::new("cmd")
                .args(["/C", "start", "claude://"])
                .output()
        };

        match open_result {
            Ok(output) if output.status.success() => CommandResult::Output(format!(
                "Opening Claude Desktop to continue {}...",
                session_info
            )),
            Ok(_) => CommandResult::Output(
                "Failed to open Claude Desktop. Please make sure it is installed.\n\
                 Download it at: https://claude.ai/download"
                    .to_string(),
            ),
            Err(_) => CommandResult::Output(
                "Could not launch Claude Desktop. Please make sure it is installed.\n\
                 Download it at: https://claude.ai/download"
                    .to_string(),
            ),
        }
    }
}
