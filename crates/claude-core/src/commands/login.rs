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

        // Attempt OAuth login
        match crate::auth::pkce::run_oauth_login(true).await {
            Ok(result) => {
                // Store tokens in the legacy location for backward compatibility
                if let Err(e) = crate::auth::storage::store_tokens(&result.tokens).await {
                    tracing::warn!("Failed to store tokens to legacy location: {}", e);
                }

                // Extract email and subscription info from the token response
                let email = extract_email_from_tokens(&result.tokens);
                let sub_type = result
                    .tokens
                    .subscription_type
                    .as_deref()
                    .unwrap_or("pro");

                // Save as a profile and set as active
                match crate::auth::profiles::save_oauth_as_profile(
                    &result.tokens,
                    &email,
                    sub_type,
                ) {
                    Ok(profile) => CommandResult::Output(format!(
                        "Logged in successfully!\n\n\
                         Profile created: {}\n\
                         Set as active profile.",
                        profile.display_name()
                    )),
                    Err(e) => {
                        tracing::warn!("Failed to save profile: {}", e);
                        CommandResult::Output(
                            "Logged in successfully (tokens saved).\n\n\
                             Note: Failed to create profile entry. \
                             Use /profile to manage profiles."
                                .to_string(),
                        )
                    }
                }
            }
            Err(e) => CommandResult::Output(format!(
                "Login failed: {}\n\n\
                 You can also set ANTHROPIC_API_KEY environment variable:\n\
                 export ANTHROPIC_API_KEY=sk-ant-...",
                e
            )),
        }
    }
}

/// Extract email from OAuth token claims or fall back to a default.
fn extract_email_from_tokens(tokens: &crate::auth::storage::OAuthStoredTokens) -> String {
    // Try to decode the access token JWT to get the email claim.
    // JWT format: header.payload.signature (base64url encoded)
    if let Some(email) = decode_jwt_email(&tokens.access_token) {
        return email;
    }
    // Fall back to "unknown" if we can't extract from the token
    "user@anthropic".to_string()
}

/// Attempt to decode email from a JWT access token (without verification).
fn decode_jwt_email(token: &str) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    claims
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
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
