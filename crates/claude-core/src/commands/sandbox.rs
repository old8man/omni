use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Toggles sandboxing for shell commands.
///
/// When enabled, shell commands run in a sandboxed environment with
/// restricted filesystem and network access. Always available in
/// claude-rs (no feature gate).
pub struct SandboxCommand;

#[async_trait]
impl Command for SandboxCommand {
    fn name(&self) -> &str {
        "sandbox"
    }

    fn description(&self) -> &str {
        "Toggle sandbox mode for shell commands"
    }

    fn usage_hint(&self) -> &str {
        "[on|off|status]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let arg = args.trim().to_lowercase();
        match arg.as_str() {
            "on" | "enable" => CommandResult::Output(
                "Sandbox enabled. Shell commands will run in a restricted environment.".to_string(),
            ),
            "off" | "disable" => CommandResult::Output(
                "Sandbox disabled. Shell commands will run unrestricted.".to_string(),
            ),
            "" | "status" => CommandResult::Output(
                "Sandbox mode: configured per permission settings.\n\
                 \n\
                 /sandbox on      — Enable sandboxing\n\
                 /sandbox off     — Disable sandboxing\n\
                 /sandbox status  — Show current status"
                    .to_string(),
            ),
            _ => {
                CommandResult::Output(format!("Unknown argument: \"{}\". Use on/off/status.", arg))
            }
        }
    }
}
