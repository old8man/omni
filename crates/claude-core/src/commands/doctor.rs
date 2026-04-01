use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Runs diagnostic checks on the environment.
pub struct DoctorCommand;

fn check_binary(name: &str) -> (bool, String) {
    match which::which(name) {
        Ok(path) => {
            // Try to get version
            let version = std::process::Command::new(name)
                .arg("--version")
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        String::from_utf8(o.stdout)
                            .ok()
                            .map(|s| s.lines().next().unwrap_or("").trim().to_string())
                    } else {
                        None
                    }
                });
            let ver_str = version
                .map(|v| format!(" ({v})"))
                .unwrap_or_default();
            (true, format!("  [ok] {name}{ver_str} -- {}", path.display()))
        }
        Err(_) => (false, format!("  [!!] {name} -- not found")),
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
        let mut lines = vec![
            "Environment Diagnostics".to_string(),
            "=======================".to_string(),
            String::new(),
            "Required tools:".to_string(),
        ];

        let (_git_ok, git_line) = check_binary("git");
        lines.push(git_line);
        let (_rg_ok, rg_line) = check_binary("rg");
        lines.push(rg_line);

        lines.push(String::new());
        lines.push("Optional tools:".to_string());
        let (_gh_ok, gh_line) = check_binary("gh");
        lines.push(gh_line);

        // Git repo check
        lines.push(String::new());
        lines.push("Repository:".to_string());
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

        if in_repo {
            // Check for uncommitted changes
            let dirty = std::process::Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(&ctx.cwd)
                .output()
                .ok()
                .map(|o| !o.stdout.is_empty())
                .unwrap_or(false);
            if dirty {
                lines.push("  [..] Working tree has uncommitted changes".to_string());
            } else {
                lines.push("  [ok] Working tree clean".to_string());
            }
        }

        // Config directory
        lines.push(String::new());
        lines.push("Configuration:".to_string());
        let config_dir = dirs::config_dir().map(|d| d.join("claude"));
        let config_ok = config_dir.as_ref().map(|d| d.exists()).unwrap_or(false);
        lines.push(if config_ok {
            format!("  [ok] Config directory: {}", config_dir.unwrap().display())
        } else {
            "  [!!] Config directory not found (~/.config/claude)".to_string()
        });

        // Auth check
        lines.push(String::new());
        lines.push("Authentication:".to_string());
        let has_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
        lines.push(if has_key {
            "  [ok] ANTHROPIC_API_KEY is set".to_string()
        } else {
            "  [!!] ANTHROPIC_API_KEY not set".to_string()
        });

        // API connectivity check
        let api_reachable = std::process::Command::new("curl")
            .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "3", "https://api.anthropic.com"])
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            });
        match api_reachable {
            Some(code) if code.starts_with('2') || code.starts_with('4') => {
                lines.push("  [ok] api.anthropic.com reachable".to_string());
            }
            Some(code) => {
                lines.push(format!("  [!!] api.anthropic.com returned HTTP {code}"));
            }
            None => {
                lines.push("  [!!] api.anthropic.com not reachable".to_string());
            }
        }

        // Session info
        lines.push(String::new());
        lines.push("Session:".to_string());
        lines.push(format!("  Model: {}", ctx.model));
        if let Some(ref sid) = ctx.session_id {
            lines.push(format!("  Session ID: {sid}"));
        }
        if let Some(ref root) = ctx.project_root {
            lines.push(format!("  Project root: {}", root.display()));
        }
        lines.push(format!("  CWD: {}", ctx.cwd.display()));

        CommandResult::Output(lines.join("\n"))
    }
}
