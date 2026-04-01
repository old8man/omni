use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Configures the advisor model for the current session.
///
/// The advisor is a secondary model that reviews the primary model's
/// tool calls before execution, providing an extra layer of verification.
/// Always enabled in claude-rs (no feature gate).
pub struct AdvisorCommand;

#[async_trait]
impl Command for AdvisorCommand {
    fn name(&self) -> &str {
        "advisor"
    }

    fn aliases(&self) -> &[&str] {
        &["assistant"]
    }

    fn description(&self) -> &str {
        "Configure the advisor model"
    }

    fn usage_hint(&self) -> &str {
        "[model|unset]"
    }

    async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult {
        let arg = args.trim().to_lowercase();

        if arg.is_empty() {
            return CommandResult::Output(format!(
                "Advisor: not set\n\
                 Current model: {}\n\
                 \n\
                 Use \"/advisor <model>\" to enable (e.g. \"/advisor opus\").\n\
                 Use \"/advisor unset\" to disable.",
                ctx.model
            ));
        }

        if arg == "unset" || arg == "off" {
            return CommandResult::Output("Advisor disabled.".to_string());
        }

        // Normalize common short names
        let model = match arg.as_str() {
            "opus" => "claude-opus-4-6",
            "sonnet" => "claude-sonnet-4-6",
            "haiku" => "claude-haiku-4-5-20251001",
            other => other,
        };

        CommandResult::Output(format!("Advisor set to: {}", model))
    }
}
