use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Log in to Anthropic.
pub struct LoginCommand;

#[async_trait]
impl Command for LoginCommand {
    fn name(&self) -> &str {
        "login"
    }

    fn description(&self) -> &str {
        "Log in to your Anthropic account"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let has_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
        if has_key {
            return CommandResult::Output(
                "Already authenticated via ANTHROPIC_API_KEY environment variable.\n\n\
                 To add this as a profile, use /profile add."
                    .to_string(),
            );
        }

        // Open the TUI login dialog instead of running OAuth directly
        CommandResult::OpenLoginDialog
    }
}

/// Log out / clear credentials.
///
/// Removes stored OAuth tokens, API keys, and session credentials from
/// the configuration directory. Environment variables are not affected
/// (the user must unset those separately).
pub struct LogoutCommand;

/// Credential files that may be stored in the config directory.
const CREDENTIAL_FILES: &[&str] = &[
    "credentials.json",
    "oauth_token",
    "api_key",
    "auth.json",
    "session_token",
];

#[async_trait]
impl Command for LogoutCommand {
    fn name(&self) -> &str {
        "logout"
    }

    fn description(&self) -> &str {
        "Log out and clear stored credentials"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let config_dir = match crate::config::paths::claude_dir() {
            Ok(d) => d,
            Err(_) => {
                return CommandResult::Output(
                    "Could not determine config directory. \
                     If you set ANTHROPIC_API_KEY as an environment variable, \
                     unset it manually:\n\n  unset ANTHROPIC_API_KEY"
                        .to_string(),
                );
            }
        };

        let mut removed = Vec::new();
        let mut not_found = Vec::new();

        for filename in CREDENTIAL_FILES {
            let path = config_dir.join(filename);
            if path.exists() {
                match std::fs::remove_file(&path) {
                    Ok(()) => removed.push(*filename),
                    Err(e) => {
                        return CommandResult::Output(format!(
                            "Failed to remove {}: {}",
                            path.display(),
                            e
                        ));
                    }
                }
            } else {
                not_found.push(*filename);
            }
        }

        let mut output = String::from("Logout\n");
        output.push_str("======\n\n");

        if removed.is_empty() {
            output.push_str("No stored credential files found.\n");
        } else {
            output.push_str("Removed credential files:\n");
            for f in &removed {
                output.push_str(&format!("  - {}\n", f));
            }
        }

        let has_env_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
        if has_env_key {
            output.push_str(
                "\nNote: ANTHROPIC_API_KEY is set as an environment variable.\n\
                 To fully log out, also run:\n\n  unset ANTHROPIC_API_KEY\n",
            );
        } else if removed.is_empty() {
            output.push_str("\nNo active authentication found. You are already logged out.\n");
        } else {
            output.push_str("\nSuccessfully logged out.\n");
        }

        CommandResult::Output(output)
    }
}
