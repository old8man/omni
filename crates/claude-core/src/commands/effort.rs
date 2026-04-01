use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Sets the effort level for model responses.
pub struct EffortCommand;

const VALID_LEVELS: &[&str] = &["low", "medium", "high", "max", "auto"];

#[async_trait]
impl Command for EffortCommand {
    fn name(&self) -> &str {
        "effort"
    }

    fn description(&self) -> &str {
        "Set effort level for model usage"
    }

    fn usage_hint(&self) -> &str {
        "[low|medium|high|max|auto]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let level = args.trim().to_lowercase();
        if level.is_empty() {
            CommandResult::Output(format!(
                "Usage: /effort <level>\nValid levels: {}",
                VALID_LEVELS.join(", ")
            ))
        } else if VALID_LEVELS.contains(&level.as_str()) {
            CommandResult::Output(format!("Effort level set to: {}", level))
        } else {
            CommandResult::Output(format!(
                "Invalid effort level '{}'. Valid levels: {}",
                level,
                VALID_LEVELS.join(", ")
            ))
        }
    }
}
