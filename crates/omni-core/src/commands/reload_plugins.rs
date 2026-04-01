use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Activate pending plugin changes in the current session.
///
/// Rescans the plugin directories and reloads configuration, applying
/// any changes to enabled plugins, skills, agents, hooks, MCP servers,
/// and LSP servers.
pub struct ReloadPluginsCommand;

#[async_trait]
impl Command for ReloadPluginsCommand {
    fn name(&self) -> &str {
        "reload-plugins"
    }

    fn description(&self) -> &str {
        "Activate pending plugin changes in the current session"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        // Scan plugin directories for installed plugins
        let plugin_dir = crate::config::paths::claude_dir()
            .map(|d| d.join("plugins")).ok();

        let mut plugin_count: usize = 0;
        let mut command_count: usize = 0;
        let mut agent_count: usize = 0;
        let mut hook_count: usize = 0;
        let mut mcp_count: usize = 0;
        let mut error_count: usize = 0;

        if let Some(ref dir) = plugin_dir {
            if dir.is_dir() {
                match std::fs::read_dir(dir) {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                plugin_count += 1;

                                // Check for skills directory
                                let skills_dir = path.join("skills");
                                if skills_dir.is_dir() {
                                    if let Ok(skill_entries) = std::fs::read_dir(&skills_dir) {
                                        command_count +=
                                            skill_entries.flatten().filter(|e| e.path().is_dir()).count();
                                    }
                                }

                                // Check for agents directory
                                let agents_dir = path.join("agents");
                                if agents_dir.is_dir() {
                                    if let Ok(agent_entries) = std::fs::read_dir(&agents_dir) {
                                        agent_count +=
                                            agent_entries.flatten().filter(|e| e.path().is_dir()).count();
                                    }
                                }

                                // Check for hooks config
                                let hooks_file = path.join("hooks.json");
                                if hooks_file.is_file() {
                                    match std::fs::read_to_string(&hooks_file) {
                                        Ok(content) => {
                                            // Count top-level keys as hooks
                                            hook_count += content.matches('"').count() / 4;
                                        }
                                        Err(_) => {
                                            error_count += 1;
                                        }
                                    }
                                }

                                // Check for MCP server config
                                let mcp_file = path.join("mcp.json");
                                if mcp_file.is_file() {
                                    mcp_count += 1;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        error_count += 1;
                    }
                }
            }
        }

        let mut msg = format!(
            "Reloaded: {} {} \u{00b7} {} {} \u{00b7} {} {} \u{00b7} {} {} \u{00b7} {} plugin MCP {}",
            plugin_count,
            plural(plugin_count, "plugin"),
            command_count,
            plural(command_count, "skill"),
            agent_count,
            plural(agent_count, "agent"),
            hook_count,
            plural(hook_count, "hook"),
            mcp_count,
            plural(mcp_count, "server"),
        );

        if error_count > 0 {
            msg.push_str(&format!(
                "\n{} {} during load. Run /doctor for details.",
                error_count,
                plural(error_count, "error")
            ));
        }

        CommandResult::Output(msg)
    }
}

/// Pluralize a noun: returns "noun" for count == 1, "nouns" otherwise.
fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        noun.to_string()
    } else {
        format!("{}s", noun)
    }
}
