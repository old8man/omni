use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Configure the terminal status line (tmux, terminal title, etc.).
///
/// Matches the original TypeScript statusline command. Controls what
/// information is displayed in the terminal title bar or tmux status line.
pub struct StatuslineCommand;

const VALID_PRESETS: &[(&str, &str)] = &[
    ("full", "Show model, session, tokens, and cost"),
    ("compact", "Show model and token count only"),
    ("minimal", "Show model name only"),
    ("off", "Disable status line updates"),
];

fn render_statusline(preset: &str, ctx: &CommandContext) -> String {
    match preset {
        "full" => {
            let session = ctx.session_id.as_deref().unwrap_or("none");
            format!(
                "claude | {} | session:{} | {}in/{}out | ${:.4}",
                ctx.model, session, ctx.input_tokens, ctx.output_tokens, ctx.total_cost
            )
        }
        "compact" => {
            format!(
                "claude | {} | {}tok",
                ctx.model,
                ctx.input_tokens + ctx.output_tokens
            )
        }
        "minimal" => {
            format!("claude | {}", ctx.model)
        }
        _ => String::new(),
    }
}

fn set_terminal_title(title: &str) {
    // OSC 2 escape sequence sets the terminal window title
    eprint!("\x1b]2;{}\x07", title);
}

fn set_tmux_status(title: &str) {
    // Set tmux pane title via OSC escape
    if std::env::var("TMUX").is_ok() {
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-p", "pane-title", title])
            .output();
    }
}

#[async_trait]
impl Command for StatuslineCommand {
    fn name(&self) -> &str {
        "statusline"
    }

    fn aliases(&self) -> &[&str] {
        &["status-line"]
    }

    fn description(&self) -> &str {
        "Configure the terminal status line"
    }

    fn usage_hint(&self) -> &str {
        "[full|compact|minimal|off|show]"
    }

    async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult {
        let arg = args.trim().to_lowercase();

        if arg.is_empty() || arg == "show" {
            let mut output = String::from("Status Line Configuration\n");
            output.push_str("=========================\n\n");

            // Show current status line content
            let current = render_statusline("full", ctx);
            output.push_str(&format!("Current: {}\n\n", current));

            output.push_str("Usage: /statusline <preset>\n\n");
            output.push_str("Presets:\n");
            for (name, desc) in VALID_PRESETS {
                output.push_str(&format!("  {:<10} {}\n", name, desc));
            }

            let is_tmux = std::env::var("TMUX").is_ok();
            output.push('\n');
            if is_tmux {
                output.push_str("tmux detected: status updates will be sent to tmux pane title.");
            } else {
                output.push_str("Status updates will be sent to the terminal window title.");
            }

            return CommandResult::Output(output);
        }

        if arg == "off" {
            set_terminal_title("");
            set_tmux_status("");

            let config_dir = dirs::config_dir().map(|d| d.join("claude"));
            if let Some(dir) = config_dir {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join("statusline_preset"), "off");
            }

            return CommandResult::Output("Status line disabled.".to_string());
        }

        if let Some((preset, desc)) = VALID_PRESETS.iter().find(|(n, _)| *n == arg.as_str()) {
            let content = render_statusline(preset, ctx);
            set_terminal_title(&content);
            set_tmux_status(&content);

            let config_dir = dirs::config_dir().map(|d| d.join("claude"));
            if let Some(dir) = config_dir {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join("statusline_preset"), preset);
            }

            CommandResult::Output(format!(
                "Status line set to '{}': {}\n  Current: {}",
                preset, desc, content
            ))
        } else {
            let valid: Vec<&str> = VALID_PRESETS.iter().map(|(n, _)| *n).collect();
            CommandResult::Output(format!(
                "Unknown preset '{}'. Valid presets: {}",
                arg,
                valid.join(", ")
            ))
        }
    }
}
