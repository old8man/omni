use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Configure the default remote environment for teleport sessions.
///
/// Allows users to select and configure their preferred remote
/// environment (e.g., cloud container, codespace) that will be used
/// when launching teleport sessions from claude.ai.
pub struct RemoteEnvCommand;

#[async_trait]
impl Command for RemoteEnvCommand {
    fn name(&self) -> &str {
        "remote-env"
    }

    fn description(&self) -> &str {
        "Configure the default remote environment for teleport sessions"
    }

    fn usage_hint(&self) -> &str {
        "[show|set|clear]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let arg = args.trim().to_lowercase();

        match arg.as_str() {
            "" | "show" => {
                // Read current configuration
                let config_path = dirs::config_dir()
                    .map(|d| d.join("claude").join("remote-env.json"));

                let current_env = if let Some(ref path) = config_path {
                    match std::fs::read_to_string(path) {
                        Ok(content) => {
                            // Extract environment name from JSON
                            content
                                .lines()
                                .find(|l| l.contains("\"name\""))
                                .and_then(|l| {
                                    l.split('"').nth(3).map(|s| s.to_string())
                                })
                        }
                        Err(_) => None,
                    }
                } else {
                    None
                };

                let mut output = String::from("Remote Environment Configuration\n");
                output.push_str("════════════════════════════════\n\n");

                match current_env {
                    Some(name) => {
                        output.push_str(&format!("Current remote environment: {}\n\n", name));
                        output.push_str(
                            "Use /remote-env set <name> to change the environment.\n\
                             Use /remote-env clear to remove the configuration.",
                        );
                    }
                    None => {
                        output.push_str("No remote environment configured.\n\n");
                        output.push_str(
                            "A remote environment is used for teleport sessions launched\n\
                             from claude.ai. Configure one to enable remote coding sessions.\n\n\
                             Use /remote-env set <name> to configure an environment.",
                        );
                    }
                }

                CommandResult::Output(output)
            }
            "clear" | "reset" => {
                let config_path = dirs::config_dir()
                    .map(|d| d.join("claude").join("remote-env.json"));

                if let Some(path) = config_path {
                    match std::fs::remove_file(&path) {
                        Ok(()) => CommandResult::Output(
                            "Remote environment configuration cleared.".to_string(),
                        ),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            CommandResult::Output(
                                "No remote environment configuration to clear.".to_string(),
                            )
                        }
                        Err(e) => CommandResult::Output(format!(
                            "Failed to clear remote environment configuration: {}",
                            e
                        )),
                    }
                } else {
                    CommandResult::Output(
                        "Could not determine config directory.".to_string(),
                    )
                }
            }
            _ if arg.starts_with("set ") || arg.starts_with("set\t") => {
                let name = args.trim().strip_prefix("set").unwrap_or("").trim();
                if name.is_empty() {
                    return CommandResult::Output(
                        "Usage: /remote-env set <environment-name>".to_string(),
                    );
                }

                let config_path = dirs::config_dir()
                    .map(|d| d.join("claude").join("remote-env.json"));

                if let Some(path) = config_path {
                    let config_dir = path.parent().unwrap();
                    if let Err(e) = std::fs::create_dir_all(config_dir) {
                        return CommandResult::Output(format!(
                            "Failed to create config directory: {}",
                            e
                        ));
                    }

                    let content = format!(
                        "{{\n  \"name\": \"{}\"\n}}\n",
                        name.replace('\\', "\\\\").replace('"', "\\\"")
                    );
                    match std::fs::write(&path, content) {
                        Ok(()) => CommandResult::Output(format!(
                            "Remote environment set to \"{}\".",
                            name
                        )),
                        Err(e) => CommandResult::Output(format!(
                            "Failed to save remote environment configuration: {}",
                            e
                        )),
                    }
                } else {
                    CommandResult::Output(
                        "Could not determine config directory.".to_string(),
                    )
                }
            }
            _ => {
                // Treat bare argument as "set <name>"
                let name = args.trim();
                if name.contains(' ') {
                    CommandResult::Output(format!(
                        "Unknown subcommand. Use show, set <name>, or clear.\n\
                         Did you mean: /remote-env set {}",
                        name
                    ))
                } else {
                    CommandResult::Output(format!(
                        "Unknown subcommand: \"{}\". Use show, set <name>, or clear.",
                        name
                    ))
                }
            }
        }
    }
}
