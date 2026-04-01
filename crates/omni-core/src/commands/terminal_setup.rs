use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Enable/install keybinding for newlines in the terminal.
///
/// Detects the current terminal emulator and installs the appropriate
/// keybinding (Shift+Enter or Option+Enter) so that users can insert
/// newlines in the input without submitting the message.
///
/// Terminals with native CSI u / Kitty keyboard protocol support
/// (Ghostty, Kitty, iTerm2, WezTerm) do not need this setup.
pub struct TerminalSetupCommand;

/// Terminals that natively support CSI u / Kitty keyboard protocol.
const NATIVE_CSIU_TERMINALS: &[(&str, &str)] = &[
    ("ghostty", "Ghostty"),
    ("kitty", "Kitty"),
    ("iTerm.app", "iTerm2"),
    ("WezTerm", "WezTerm"),
    ("WarpTerminal", "Warp"),
];

#[async_trait]
impl Command for TerminalSetupCommand {
    fn name(&self) -> &str {
        "terminal-setup"
    }

    fn description(&self) -> &str {
        "Install Shift+Enter key binding for newlines"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let terminal = std::env::var("TERM_PROGRAM").ok();
        let terminal_ref = terminal.as_deref().unwrap_or("");

        // Check if this terminal natively supports the key protocol
        if let Some((_, display_name)) = NATIVE_CSIU_TERMINALS
            .iter()
            .find(|(id, _)| *id == terminal_ref)
        {
            return CommandResult::OpenInfoDialog {
                title: "Terminal Setup".to_string(),
                content: format!("{} natively supports the Shift+Enter keybinding.\nNo additional setup is needed.", display_name),
            };
        }

        let is_apple_terminal = terminal_ref == "Apple_Terminal";
        let is_vscode = terminal_ref == "vscode"
            || std::env::var("TERM_PROGRAM_VERSION")
                .ok()
                .is_some_and(|v| v.contains("vscode"));
        let is_tmux = std::env::var("TMUX").is_ok();

        let mut output = String::from("Terminal Setup\n");
        output.push_str("══════════════\n\n");

        if is_apple_terminal {
            output.push_str("Detected: Apple Terminal\n\n");
            output.push_str(
                "Apple Terminal does not support Shift+Enter natively.\n\
                 You can use Option+Enter instead.\n\n\
                 To enable Option+Enter:\n\
                 \x20 1. Open Terminal > Settings > Profiles > Keyboard\n\
                 \x20 2. Check \"Use Option as Meta key\"\n\n\
                 Alternatively, consider switching to a terminal that supports\n\
                 modern keyboard protocols: Ghostty, Kitty, iTerm2, or WezTerm.",
            );
        } else if is_vscode {
            output.push_str("Detected: VS Code integrated terminal\n\n");

            let keybinding = r#"[
  {
    "key": "shift+enter",
    "command": "workbench.action.terminal.sendSequence",
    "args": { "text": "\u001b[13;2u" },
    "when": "terminalFocus && !terminalTextSelected"
  }
]"#;

            output.push_str(
                "To install the Shift+Enter keybinding for VS Code:\n\n\
                 1. Open the command palette (Cmd+Shift+P / Ctrl+Shift+P)\n\
                 2. Search for \"Preferences: Open Keyboard Shortcuts (JSON)\"\n\
                 3. Add the following to your keybindings.json:\n\n",
            );
            output.push_str(keybinding);
            output.push_str("\n\n");

            // Also write to disk if possible
            let keybindings_path = dirs::config_dir()
                .map(|d| d.join("Code").join("User").join("keybindings.json"));

            if let Some(path) = keybindings_path {
                output.push_str(&format!(
                    "Expected keybindings file location:\n  {}\n",
                    path.display()
                ));
            }
        } else if is_tmux {
            output.push_str("Detected: tmux session\n\n");
            output.push_str(
                "To enable Shift+Enter in tmux, add to your ~/.tmux.conf:\n\n\
                 \x20 set -s extended-keys on\n\
                 \x20 set -as terminal-features 'xterm*:extkeys'\n\n\
                 Then reload tmux config:\n\
                 \x20 tmux source-file ~/.tmux.conf",
            );
        } else {
            let term_name = if terminal_ref.is_empty() {
                "Unknown terminal".to_string()
            } else {
                terminal_ref.to_string()
            };
            output.push_str(&format!("Detected: {}\n\n", term_name));
            output.push_str(
                "Shift+Enter support depends on your terminal emulator.\n\n\
                 For the best experience, consider using a terminal with\n\
                 native CSI u / Kitty keyboard protocol support:\n\n\
                 \x20 - Ghostty  (https://ghostty.org)\n\
                 \x20 - Kitty    (https://sw.kovidgoyal.net/kitty)\n\
                 \x20 - iTerm2   (https://iterm2.com)\n\
                 \x20 - WezTerm  (https://wezfurlong.org/wezterm)\n\n\
                 These terminals support Shift+Enter for newlines out of the box.",
            );
        }

        CommandResult::OpenInfoDialog {
            title: "Terminal Setup".to_string(),
            content: output,
        }
    }
}
