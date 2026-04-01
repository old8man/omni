use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Checks for and applies Claude Code updates.
pub struct UpgradeCommand;

#[async_trait]
impl Command for UpgradeCommand {
    fn name(&self) -> &str {
        "upgrade"
    }

    fn aliases(&self) -> &[&str] {
        &["update"]
    }

    fn description(&self) -> &str {
        "Check for and install Claude Code updates"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output(format!(
            "Claude Code (Rust) v{}\n\
             \n\
             To upgrade, use your package manager:\n\
             \n\
             cargo install claude-rs\n\
             \n\
             Or build from source:\n\
             cargo build --release",
            env!("CARGO_PKG_VERSION")
        ))
    }
}
