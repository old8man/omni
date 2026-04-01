use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Displays current configuration.
pub struct ConfigCommand;

#[async_trait]
impl Command for ConfigCommand {
    fn name(&self) -> &str {
        "config"
    }

    fn description(&self) -> &str {
        "Show current configuration"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let project_root = ctx
            .project_root
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".to_string());
        let config_dir = dirs::config_dir()
            .map(|d| d.join("claude").display().to_string())
            .unwrap_or_else(|| "(unknown)".to_string());
        let lines = [
            format!("Project root: {}", project_root),
            format!("Working dir:  {}", ctx.cwd.display()),
            format!("Config dir:   {}", config_dir),
            format!("Model:        {}", ctx.model),
            format!("Vim mode:     {}", if ctx.vim_mode { "on" } else { "off" }),
            format!("Plan mode:    {}", if ctx.plan_mode { "on" } else { "off" }),
        ];
        CommandResult::Output(lines.join("\n"))
    }
}
