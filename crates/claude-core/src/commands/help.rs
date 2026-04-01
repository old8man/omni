use async_trait::async_trait;

use super::{Command, CommandContext, CommandRegistry, CommandResult};

/// Lists all available commands with descriptions and aliases, grouped by category.
pub struct HelpCommand;

struct CommandCategory {
    name: &'static str,
    commands: &'static [&'static str],
}

const CATEGORIES: &[CommandCategory] = &[
    CommandCategory {
        name: "Core",
        commands: &["help", "clear", "compact", "quit", "version"],
    },
    CommandCategory {
        name: "Git Workflow",
        commands: &["commit", "commit-push-pr", "review", "security-review", "diff", "pr-comments"],
    },
    CommandCategory {
        name: "Session",
        commands: &["session", "branch", "resume", "share", "export", "rename", "copy", "rewind", "tag"],
    },
    CommandCategory {
        name: "Configuration",
        commands: &["config", "model", "effort", "theme", "color", "fast", "env", "output-style", "plan", "ultraplan", "vim", "voice", "privacy-settings"],
    },
    CommandCategory {
        name: "Management",
        commands: &["hooks", "permissions", "mcp", "plugin", "keybindings", "skills", "tasks"],
    },
    CommandCategory {
        name: "Info & Diagnostics",
        commands: &["status", "statusline", "usage", "cost", "context", "doctor", "memory", "files", "stats", "insights"],
    },
    CommandCategory {
        name: "Mode Toggles",
        commands: &["brief", "sandbox", "advisor"],
    },
    CommandCategory {
        name: "Auth",
        commands: &["login", "logout"],
    },
    CommandCategory {
        name: "Project",
        commands: &["init", "install", "feedback", "add-dir"],
    },
];

#[async_trait]
impl Command for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }

    fn aliases(&self) -> &[&str] {
        &["?"]
    }

    fn description(&self) -> &str {
        "Show available commands"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let reg = CommandRegistry::default_registry();

        // If a specific command name is given, show detailed help for that command.
        let trimmed = args.trim();
        if !trimmed.is_empty() {
            if let Some(cmd) = reg.find(trimmed) {
                let aliases = cmd.aliases();
                let alias_str = if aliases.is_empty() {
                    String::new()
                } else {
                    format!("\nAliases: {}", aliases.iter().map(|a| format!("/{a}")).collect::<Vec<_>>().join(", "))
                };
                let usage = if cmd.usage_hint().is_empty() {
                    String::new()
                } else {
                    format!(" {}", cmd.usage_hint())
                };
                return CommandResult::OpenInfoDialog {
                    title: format!("/{}", cmd.name()),
                    content: format!(
                        "/{}{}\n{}\n{}",
                        cmd.name(),
                        usage,
                        cmd.description(),
                        alias_str,
                    ),
                };
            } else {
                return CommandResult::Output(format!("Unknown command: /{trimmed}. Type /help for a list."));
            }
        }

        // Build a lookup from command name -> &dyn Command for quick access.
        let all = reg.all_commands();
        let mut lookup = std::collections::HashMap::new();
        for cmd in &all {
            lookup.insert(cmd.name().to_string(), *cmd);
            for alias in cmd.aliases() {
                lookup.insert(alias.to_string(), *cmd);
            }
        }

        let mut lines = vec![
            "Available Commands".to_string(),
            "==================".to_string(),
        ];

        let mut categorized = std::collections::HashSet::new();

        for cat in CATEGORIES {
            let mut cat_lines: Vec<String> = Vec::new();
            for &cmd_name in cat.commands {
                if let Some(cmd) = lookup.get(cmd_name) {
                    categorized.insert(cmd.name().to_string());
                    let aliases = cmd.aliases();
                    let alias_str = if aliases.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", aliases.iter().map(|a| format!("/{a}")).collect::<Vec<_>>().join(", "))
                    };
                    let usage = if cmd.usage_hint().is_empty() {
                        String::new()
                    } else {
                        format!(" {}", cmd.usage_hint())
                    };
                    cat_lines.push(format!(
                        "  /{}{}{:<20} {}",
                        cmd.name(),
                        usage,
                        alias_str,
                        cmd.description(),
                    ));
                }
            }
            if !cat_lines.is_empty() {
                lines.push(String::new());
                lines.push(format!("{}:", cat.name));
                lines.extend(cat_lines);
            }
        }

        // Show any uncategorized commands
        let mut other_lines: Vec<String> = Vec::new();
        for cmd in &all {
            if !categorized.contains(cmd.name()) {
                let aliases = cmd.aliases();
                let alias_str = if aliases.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", aliases.iter().map(|a| format!("/{a}")).collect::<Vec<_>>().join(", "))
                };
                other_lines.push(format!(
                    "  /{}{} -- {}",
                    cmd.name(),
                    alias_str,
                    cmd.description(),
                ));
            }
        }
        if !other_lines.is_empty() {
            lines.push(String::new());
            lines.push("Other:".to_string());
            lines.extend(other_lines);
        }

        lines.push(String::new());
        lines.push("Type /help <command> for details on a specific command.".to_string());

        CommandResult::OpenInfoDialog {
            title: "Help".to_string(),
            content: lines.join("\n"),
        }
    }
}
