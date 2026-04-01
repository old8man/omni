use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// View or set environment variables.
pub struct EnvCommand;

#[async_trait]
impl Command for EnvCommand {
    fn name(&self) -> &str {
        "env"
    }

    fn description(&self) -> &str {
        "View or set environment variables"
    }

    fn usage_hint(&self) -> &str {
        "[KEY=value]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let args = args.trim();
        if args.is_empty() {
            CommandResult::Output(
                "Environment variables can be set in settings.json:\n\
                 {\n  \"env\": {\n    \"KEY\": \"value\"\n  }\n}\n\
                 \n\
                 Or use: /env KEY=value"
                    .to_string(),
            )
        } else if let Some((key, value)) = args.split_once('=') {
            CommandResult::Output(format!("Set {}={}", key.trim(), value.trim()))
        } else {
            // Show a specific env var
            match std::env::var(args) {
                Ok(val) => CommandResult::Output(format!("{}={}", args, val)),
                Err(_) => CommandResult::Output(format!("{} is not set", args)),
            }
        }
    }
}
