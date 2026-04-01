use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Manages keybindings.
pub struct KeybindingsCommand;

#[async_trait]
impl Command for KeybindingsCommand {
    fn name(&self) -> &str {
        "keybindings"
    }

    fn description(&self) -> &str {
        "View and customize keyboard shortcuts"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::OpenInfoDialog {
            title: "Keybindings".to_string(),
            content: "Keyboard Shortcuts:\n\
                      \n\
                      Navigation:\n\
                        Ctrl+C / Ctrl+D  Exit (double-tap)\n\
                        Ctrl+L           Clear screen\n\
                        Up/Down          Scroll messages\n\
                        Ctrl+F / /       Search messages\n\
                      \n\
                      Input:\n\
                        Enter            Submit message\n\
                        Shift+Enter      Insert newline\n\
                        Up/Down          History navigation\n\
                        Ctrl+A           Move to line start\n\
                        Ctrl+E           Move to line end\n\
                        Ctrl+U           Clear line\n\
                        Ctrl+W           Delete word\n\
                      \n\
                      Dialogs:\n\
                        Esc / q          Close dialog\n\
                        j / k            Scroll down/up\n\
                        PgDn / PgUp      Page scroll\n\
                        g / G            Jump to top/bottom\n\
                      \n\
                      Configuration file: ~/.claude/keybindings.json\n\
                      Use /keybindings-help skill for customization help."
                .to_string(),
        }
    }
}
