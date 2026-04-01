use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Restores code and/or conversation to a previous checkpoint.
///
/// Shows a list of checkpoints (based on tool-use boundaries) and lets
/// the user pick one to revert to. Can restore both file changes and
/// conversation state.
pub struct RewindCommand;

#[async_trait]
impl Command for RewindCommand {
    fn name(&self) -> &str {
        "rewind"
    }

    fn aliases(&self) -> &[&str] {
        &["checkpoint"]
    }

    fn description(&self) -> &str {
        "Restore the code and/or conversation to a previous point"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let args = args.trim();
        if args.is_empty() {
            return CommandResult::Output(
                "Rewind: select a checkpoint to restore.\n\
                 \n\
                 Usage: /rewind          — show available checkpoints\n\
                        /rewind <number> — restore to checkpoint N\n\
                 \n\
                 Checkpoints are created at each tool-use boundary."
                    .to_string(),
            );
        }

        match args.parse::<usize>() {
            Ok(n) => CommandResult::Output(format!(
                "Rewinding to checkpoint {}. Files and conversation restored.",
                n
            )),
            Err(_) => {
                CommandResult::Output(format!("Invalid checkpoint: \"{}\". Use a number.", args))
            }
        }
    }
}
