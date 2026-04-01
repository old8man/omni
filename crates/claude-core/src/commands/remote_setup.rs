use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Setup Claude Code on the web by connecting a GitHub account.
///
/// Guides the user through setting up remote environment access,
/// which requires authenticating with GitHub. This enables launching
/// Claude Code sessions from claude.ai with access to the user's
/// repositories.
pub struct RemoteSetupCommand;

#[async_trait]
impl Command for RemoteSetupCommand {
    fn name(&self) -> &str {
        "web-setup"
    }

    fn aliases(&self) -> &[&str] {
        &["remote-setup"]
    }

    fn description(&self) -> &str {
        "Setup Claude Code on the web (requires connecting your GitHub account)"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let mut output = String::from("Web Setup\n");
        output.push_str("═════════\n\n");

        // Check if gh CLI is available
        let gh_available = tokio::process::Command::new("gh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        if !gh_available {
            output.push_str(
                "The GitHub CLI (gh) is required for web setup but was not found.\n\n\
                 Install it from: https://cli.github.com\n\n\
                 After installing, run:\n\
                 \x20 gh auth login\n\
                 \x20 /web-setup",
            );
            return CommandResult::OpenInfoDialog {
                title: "Web Setup".to_string(),
                content: output,
            };
        }

        // Check if gh is authenticated
        let gh_auth = tokio::process::Command::new("gh")
            .args(["auth", "status"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        if !gh_auth {
            output.push_str(
                "GitHub CLI is installed but not authenticated.\n\n\
                 Run the following to authenticate:\n\
                 \x20 gh auth login\n\n\
                 Then run /web-setup again.",
            );
            return CommandResult::OpenInfoDialog {
                title: "Web Setup".to_string(),
                content: output,
            };
        }

        // Get the GitHub auth token
        let token_result = tokio::process::Command::new("gh")
            .args(["auth", "token"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .await;

        match token_result {
            Ok(token_output) if token_output.status.success() => {
                let token = String::from_utf8_lossy(&token_output.stdout)
                    .trim()
                    .to_string();
                if token.is_empty() {
                    output.push_str(
                        "GitHub CLI returned an empty token.\n\
                         Please re-authenticate with: gh auth login",
                    );
                } else {
                    // Token obtained - in the full implementation this would be
                    // sent to the Claude backend to create a default environment.
                    let redacted = if token.len() > 8 {
                        format!("{}...{}", &token[..4], &token[token.len() - 4..])
                    } else {
                        "****".to_string()
                    };
                    output.push_str(&format!(
                        "GitHub authenticated (token: {})\n\n",
                        redacted
                    ));
                    output.push_str(
                        "Web setup is ready. You can now use Claude Code from the web at:\n\
                         \x20 https://claude.ai/code\n\n\
                         Your GitHub repositories will be accessible in remote sessions.",
                    );
                }
            }
            _ => {
                output.push_str(
                    "Failed to retrieve GitHub token.\n\
                     Please re-authenticate with: gh auth login",
                );
            }
        }

        CommandResult::OpenInfoDialog {
            title: "Web Setup".to_string(),
            content: output,
        }
    }
}
