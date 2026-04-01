use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// First-time setup wizard for Claude Code.
///
/// Matches the original TypeScript install command. Checks environment
/// prerequisites, creates necessary directories and configuration files,
/// and guides the user through initial authentication setup.
pub struct InstallCommand;

struct SetupCheck {
    label: String,
    ok: bool,
    detail: String,
}

fn check_prerequisite(name: &str) -> SetupCheck {
    match which::which(name) {
        Ok(path) => SetupCheck {
            label: name.to_string(),
            ok: true,
            detail: path.display().to_string(),
        },
        Err(_) => SetupCheck {
            label: name.to_string(),
            ok: false,
            detail: "not found in PATH".to_string(),
        },
    }
}

fn ensure_config_dir() -> SetupCheck {
    let dir = match dirs::config_dir() {
        Some(d) => d.join("claude"),
        None => {
            return SetupCheck {
                label: "Config directory".to_string(),
                ok: false,
                detail: "Could not determine config directory".to_string(),
            };
        }
    };

    if dir.exists() {
        return SetupCheck {
            label: "Config directory".to_string(),
            ok: true,
            detail: format!("exists at {}", dir.display()),
        };
    }

    match std::fs::create_dir_all(&dir) {
        Ok(()) => SetupCheck {
            label: "Config directory".to_string(),
            ok: true,
            detail: format!("created at {}", dir.display()),
        },
        Err(e) => SetupCheck {
            label: "Config directory".to_string(),
            ok: false,
            detail: format!("failed to create {}: {}", dir.display(), e),
        },
    }
}

fn ensure_project_config(cwd: &std::path::Path) -> Vec<SetupCheck> {
    let mut checks = Vec::new();

    let claude_dir = cwd.join(".claude");
    if claude_dir.exists() {
        checks.push(SetupCheck {
            label: ".claude/ directory".to_string(),
            ok: true,
            detail: "exists".to_string(),
        });
    } else {
        match std::fs::create_dir_all(&claude_dir) {
            Ok(()) => {
                checks.push(SetupCheck {
                    label: ".claude/ directory".to_string(),
                    ok: true,
                    detail: "created".to_string(),
                });
            }
            Err(e) => {
                checks.push(SetupCheck {
                    label: ".claude/ directory".to_string(),
                    ok: false,
                    detail: format!("failed to create: {}", e),
                });
            }
        }
    }

    let claude_md = cwd.join("CLAUDE.md");
    if claude_md.exists() {
        checks.push(SetupCheck {
            label: "CLAUDE.md".to_string(),
            ok: true,
            detail: "exists".to_string(),
        });
    } else {
        let template = "# Project Guidelines\n\n\
                        Add project-specific instructions, conventions, and context here.\n\
                        Claude will read this file automatically when working in this directory.\n";
        match std::fs::write(&claude_md, template) {
            Ok(()) => {
                checks.push(SetupCheck {
                    label: "CLAUDE.md".to_string(),
                    ok: true,
                    detail: "created with template".to_string(),
                });
            }
            Err(e) => {
                checks.push(SetupCheck {
                    label: "CLAUDE.md".to_string(),
                    ok: false,
                    detail: format!("failed to create: {}", e),
                });
            }
        }
    }

    checks
}

fn check_auth() -> SetupCheck {
    let has_env_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let has_stored_key = dirs::config_dir()
        .map(|d| d.join("claude").join("credentials.json").exists())
        .unwrap_or(false);

    if has_env_key {
        SetupCheck {
            label: "Authentication".to_string(),
            ok: true,
            detail: "ANTHROPIC_API_KEY environment variable is set".to_string(),
        }
    } else if has_stored_key {
        SetupCheck {
            label: "Authentication".to_string(),
            ok: true,
            detail: "stored credentials found".to_string(),
        }
    } else {
        SetupCheck {
            label: "Authentication".to_string(),
            ok: false,
            detail: "no API key found; set ANTHROPIC_API_KEY or run /login".to_string(),
        }
    }
}

fn check_git_repo(cwd: &std::path::Path) -> SetupCheck {
    let in_repo = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    SetupCheck {
        label: "Git repository".to_string(),
        ok: in_repo,
        detail: if in_repo {
            "inside a git repository".to_string()
        } else {
            "not inside a git repository (some features will be limited)".to_string()
        },
    }
}

#[async_trait]
impl Command for InstallCommand {
    fn name(&self) -> &str {
        "install"
    }

    fn aliases(&self) -> &[&str] {
        &["setup"]
    }

    fn description(&self) -> &str {
        "Run the first-time setup wizard"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut output = String::from("Claude Code Setup Wizard\n");
        output.push_str("========================\n\n");

        let mut all_ok = true;
        let mut checks: Vec<SetupCheck> = Vec::new();

        // 1. Prerequisites
        output.push_str("Prerequisites\n");
        output.push_str("-------------\n");
        checks.push(check_prerequisite("git"));
        checks.push(check_prerequisite("rg"));
        checks.push(check_prerequisite("gh"));

        for check in &checks {
            let icon = if check.ok { "[ok]" } else { "[!!]" };
            output.push_str(&format!("  {} {} - {}\n", icon, check.label, check.detail));
            if !check.ok {
                all_ok = false;
            }
        }
        checks.clear();

        // 2. Configuration
        output.push_str("\nConfiguration\n");
        output.push_str("-------------\n");

        let config_check = ensure_config_dir();
        if !config_check.ok {
            all_ok = false;
        }
        output.push_str(&format!(
            "  {} {} - {}\n",
            if config_check.ok { "[ok]" } else { "[!!]" },
            config_check.label,
            config_check.detail
        ));

        // 3. Project setup
        output.push_str("\nProject Setup\n");
        output.push_str("-------------\n");

        let project_checks = ensure_project_config(&ctx.cwd);
        for check in &project_checks {
            let icon = if check.ok { "[ok]" } else { "[!!]" };
            output.push_str(&format!("  {} {} - {}\n", icon, check.label, check.detail));
            if !check.ok {
                all_ok = false;
            }
        }

        let git_check = check_git_repo(&ctx.cwd);
        output.push_str(&format!(
            "  {} {} - {}\n",
            if git_check.ok { "[ok]" } else { "[!!]" },
            git_check.label,
            git_check.detail
        ));
        if !git_check.ok {
            all_ok = false;
        }

        // 4. Authentication
        output.push_str("\nAuthentication\n");
        output.push_str("--------------\n");

        let auth_check = check_auth();
        output.push_str(&format!(
            "  {} {} - {}\n",
            if auth_check.ok { "[ok]" } else { "[!!]" },
            auth_check.label,
            auth_check.detail
        ));
        if !auth_check.ok {
            all_ok = false;
        }

        // Summary
        output.push('\n');
        if all_ok {
            output.push_str(
                "Setup complete. All checks passed.\n\
                 You're ready to use Claude Code. Type a message to get started.",
            );
        } else {
            output.push_str(
                "Setup completed with warnings. Review the items marked [!!] above.\n\
                 Claude Code will work but some features may be limited.",
            );
        }

        CommandResult::Output(output)
    }
}
