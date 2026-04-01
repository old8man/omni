use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Play the thinkback animation.
///
/// This is a hidden command invoked by the thinkback skill after
/// generation is complete. It plays a visual animation in the terminal
/// showing the "Year in Review" experience.
pub struct ThinkbackPlayCommand;

/// Animation frames for the thinkback visual effect.
const FRAMES: &[&str] = &[
    "  *  .  *  .  *  .  *  .  *  ",
    " .  *  .  *  .  *  .  *  .  *",
    "*  .  *  .  *  .  *  .  *  . ",
    " *  .  *  .  *  .  *  .  *  .",
];

/// ANSI escape codes for animation colors.
const PURPLE: &str = "\x1b[35m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

#[async_trait]
impl Command for ThinkbackPlayCommand {
    fn name(&self) -> &str {
        "thinkback-play"
    }

    fn description(&self) -> &str {
        "Play the thinkback animation"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        // Check for thinkback plugin installation
        let plugin_dir = dirs::config_dir()
            .map(|d| d.join("claude").join("plugins"));

        let thinkback_installed = plugin_dir
            .as_ref()
            .map(|dir| {
                // Look for any directory containing "thinkback" in the plugins dir
                if let Ok(entries) = std::fs::read_dir(dir) {
                    entries.flatten().any(|e| {
                        e.file_name()
                            .to_str()
                            .is_some_and(|n| n.contains("thinkback"))
                    })
                } else {
                    false
                }
            })
            .unwrap_or(false);

        if !thinkback_installed {
            return CommandResult::Output(
                "Thinkback plugin not installed. Run /think-back first to install it."
                    .to_string(),
            );
        }

        // Build the animation output (rendered as a single frame since we
        // cannot do real-time terminal animation from a command result).
        let mut output = String::new();

        // Header with sparkle effect
        output.push_str(PURPLE);
        output.push_str(FRAMES[0]);
        output.push_str(RESET);
        output.push('\n');

        output.push_str(&format!(
            "\n{}  Claude Code — Year in Review  {}\n\n",
            CYAN, RESET
        ));

        // Stats section
        output.push_str(&format!("{}Session Highlights:{}\n", YELLOW, RESET));
        output.push_str(&format!("  Model: {}\n", ctx.model));
        output.push_str(&format!("  Input tokens:  {}\n", ctx.input_tokens));
        output.push_str(&format!("  Output tokens: {}\n", ctx.output_tokens));
        output.push_str(&format!("  Session cost:  ${:.4}\n\n", ctx.total_cost));

        // Closing sparkle
        output.push_str(PURPLE);
        output.push_str(FRAMES[2]);
        output.push_str(RESET);
        output.push('\n');

        output.push_str(&format!(
            "\n{}Thank you for coding with Claude!{}\n",
            CYAN, RESET
        ));

        CommandResult::Output(output)
    }
}
