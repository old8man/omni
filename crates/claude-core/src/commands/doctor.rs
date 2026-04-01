use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Runs diagnostic checks on the environment.
pub struct DoctorCommand;

fn check_binary(name: &str) -> String {
    match which::which(name) {
        Ok(path) => format!("  [ok] {} — {}", name, path.display()),
        Err(_) => format!("  [!!] {} — not found", name),
    }
}

#[async_trait]
impl Command for DoctorCommand {
    fn name(&self) -> &str {
        "doctor"
    }

    fn description(&self) -> &str {
        "Run diagnostic checks"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut lines = vec!["Environment checks:".to_string(), String::new()];

        // Required binaries
        lines.push(check_binary("git"));
        lines.push(check_binary("rg"));

        // Git repo check
        let in_repo = std::process::Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(&ctx.cwd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        lines.push(if in_repo {
            "  [ok] Inside a git repository".to_string()
        } else {
            "  [!!] Not inside a git repository".to_string()
        });

        // Config directory
        let config_ok = dirs::config_dir()
            .map(|d| d.join("claude").exists())
            .unwrap_or(false);
        lines.push(if config_ok {
            "  [ok] Config directory exists".to_string()
        } else {
            "  [!!] Config directory not found".to_string()
        });

        // Auth check (API key environment variable)
        let has_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
        lines.push(if has_key {
            "  [ok] ANTHROPIC_API_KEY is set".to_string()
        } else {
            "  [!!] ANTHROPIC_API_KEY not set".to_string()
        });

        CommandResult::Output(lines.join("\n"))
    }
}
