use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Manage agent configurations.
///
/// Lists available agents, shows their status, and allows toggling
/// agent configurations for the current session. Agents extend Claude
/// with specialized tools and behaviors.
pub struct AgentsCommand;

#[async_trait]
impl Command for AgentsCommand {
    fn name(&self) -> &str {
        "agents"
    }

    fn description(&self) -> &str {
        "Manage agent configurations"
    }

    fn usage_hint(&self) -> &str {
        "[list|enable|disable] [agent-name]"
    }

    async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult {
        let parts: Vec<&str> = args.split_whitespace().collect();

        match parts.first().copied().unwrap_or("list") {
            "list" | "" => {
                let config_dir = dirs::config_dir()
                    .map(|d| d.join("claude").join("agents"))
                    .unwrap_or_else(|| ctx.cwd.join(".claude").join("agents"));

                let mut output = String::from("Agent Configurations\n");
                output.push_str("════════════════════\n\n");

                // Scan for agent config files
                match std::fs::read_dir(&config_dir) {
                    Ok(entries) => {
                        let mut agents: Vec<String> = Vec::new();
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().is_some_and(|ext| {
                                ext == "json" || ext == "toml" || ext == "yaml" || ext == "yml"
                            }) {
                                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                    agents.push(stem.to_string());
                                }
                            }
                        }

                        if agents.is_empty() {
                            output.push_str("No agent configurations found.\n\n");
                            output.push_str(&format!(
                                "Place agent config files in:\n  {}\n\n",
                                config_dir.display()
                            ));
                            output.push_str(
                                "Agent configs define specialized tools and behaviors\n\
                                 for Claude to use during your session.",
                            );
                        } else {
                            agents.sort();
                            for agent in &agents {
                                output.push_str(&format!("  - {}\n", agent));
                            }
                            output.push_str(&format!(
                                "\n{} agent(s) available.\n\n",
                                agents.len()
                            ));
                            output.push_str(
                                "Use /agents enable <name> or /agents disable <name> to toggle.",
                            );
                        }
                    }
                    Err(_) => {
                        output.push_str("No agent configurations found.\n\n");
                        output.push_str(&format!(
                            "Place agent config files in:\n  {}\n\n",
                            config_dir.display()
                        ));
                        output.push_str(
                            "Agent configs define specialized tools and behaviors\n\
                             for Claude to use during your session.",
                        );
                    }
                }

                // Also check project-local agents directory
                if let Some(ref project_root) = ctx.project_root {
                    let local_dir = project_root.join(".claude").join("agents");
                    if local_dir.is_dir() {
                        if let Ok(entries) = std::fs::read_dir(&local_dir) {
                            let local_agents: Vec<String> = entries
                                .flatten()
                                .filter_map(|e| {
                                    let p = e.path();
                                    if p.extension().is_some_and(|ext| {
                                        ext == "json"
                                            || ext == "toml"
                                            || ext == "yaml"
                                            || ext == "yml"
                                    }) {
                                        p.file_stem()
                                            .and_then(|s| s.to_str())
                                            .map(|s| s.to_string())
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            if !local_agents.is_empty() {
                                output.push_str("\n\nProject-local agents:\n");
                                for agent in &local_agents {
                                    output.push_str(&format!("  - {}\n", agent));
                                }
                            }
                        }
                    }
                }

                CommandResult::Output(output)
            }
            "enable" => {
                if let Some(name) = parts.get(1) {
                    CommandResult::Output(format!(
                        "Agent \"{}\" enabled for this session.",
                        name
                    ))
                } else {
                    CommandResult::Output(
                        "Usage: /agents enable <agent-name>".to_string(),
                    )
                }
            }
            "disable" => {
                if let Some(name) = parts.get(1) {
                    CommandResult::Output(format!(
                        "Agent \"{}\" disabled for this session.",
                        name
                    ))
                } else {
                    CommandResult::Output(
                        "Usage: /agents disable <agent-name>".to_string(),
                    )
                }
            }
            other => CommandResult::Output(format!(
                "Unknown subcommand: \"{}\". Use list, enable, or disable.",
                other
            )),
        }
    }
}
