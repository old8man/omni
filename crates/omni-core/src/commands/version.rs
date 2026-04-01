use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shows the application version and build info.
pub struct VersionCommand;

#[async_trait]
impl Command for VersionCommand {
    fn name(&self) -> &str {
        "version"
    }

    fn aliases(&self) -> &[&str] {
        &["ver"]
    }

    fn description(&self) -> &str {
        "Show application version"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut lines = vec![
            format!("claude-rs v{}", env!("CARGO_PKG_VERSION")),
        ];

        lines.push(format!("Model: {}", ctx.model));

        // Show Rust version used at compile time
        if let Some(rustc) = option_env!("RUSTC_VERSION") {
            lines.push(format!("Compiled with: rustc {rustc}"));
        }

        lines.push(format!("Platform: {} {}", std::env::consts::OS, std::env::consts::ARCH));

        CommandResult::OpenInfoDialog {
            title: "About OMNI".to_string(),
            content: lines.join("\n"),
        }
    }
}
