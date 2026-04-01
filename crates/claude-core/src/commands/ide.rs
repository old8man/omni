use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Manage IDE integrations and show connection status.
///
/// Matches the TypeScript `ide` command. Detects running IDEs and allows
/// the user to connect Claude Code to their editor for features like
/// inline diffs and go-to-definition.
pub struct IdeCommand;

/// Known IDE types we can detect and connect to.
const KNOWN_IDES: &[&str] = &[
    "VS Code",
    "VS Code Insiders",
    "Cursor",
    "Windsurf",
    "IntelliJ IDEA",
    "PyCharm",
    "WebStorm",
    "GoLand",
    "RustRover",
    "CLion",
];

#[async_trait]
impl Command for IdeCommand {
    fn name(&self) -> &str {
        "ide"
    }

    fn description(&self) -> &str {
        "Manage IDE integrations and show status"
    }

    fn usage_hint(&self) -> &str {
        "[open]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let subcommand = args.trim().to_lowercase();

        if subcommand == "open" {
            return attempt_open_ide();
        }

        // Default: show IDE status and detected IDEs
        let mut lines = vec!["IDE Integration Status".to_string()];
        lines.push("---------------------".to_string());

        // Detect running IDEs by checking for their processes
        let detected = detect_running_ides();

        if detected.is_empty() {
            lines.push(String::new());
            lines.push("No supported IDEs detected.".to_string());
            lines.push(String::new());
            lines.push("Supported IDEs:".to_string());
            for ide in KNOWN_IDES {
                lines.push(format!("  - {}", ide));
            }
            lines.push(String::new());
            lines.push(
                "Start a supported IDE and run /ide again to connect.".to_string(),
            );
        } else {
            lines.push(String::new());
            lines.push("Detected IDEs:".to_string());
            for ide in &detected {
                lines.push(format!("  - {}", ide));
            }
            lines.push(String::new());
            lines.push("Use /ide open to open a file in your connected IDE.".to_string());
        }

        CommandResult::Output(lines.join("\n"))
    }
}

/// Detect running IDE processes on the system.
fn detect_running_ides() -> Vec<String> {
    let mut found = Vec::new();

    // Check for common IDE process names
    let checks: &[(&str, &str)] = &[
        ("code", "VS Code"),
        ("code-insiders", "VS Code Insiders"),
        ("cursor", "Cursor"),
        ("windsurf", "Windsurf"),
        ("idea", "IntelliJ IDEA"),
        ("pycharm", "PyCharm"),
        ("webstorm", "WebStorm"),
        ("goland", "GoLand"),
        ("rustrover", "RustRover"),
        ("clion", "CLion"),
    ];

    // Use pgrep on Unix-like systems
    if cfg!(unix) {
        for (process_name, display_name) in checks {
            let result = std::process::Command::new("pgrep")
                .args(["-x", process_name])
                .output();
            if let Ok(output) = result {
                if output.status.success() {
                    found.push(display_name.to_string());
                }
            }
        }
    }

    found
}

/// Attempt to open the current project in a detected IDE.
fn attempt_open_ide() -> CommandResult {
    // Try VS Code first, then other editors
    let editors: &[(&str, &[&str])] = &[
        ("code", &["."]),
        ("cursor", &["."]),
        ("idea", &["."]),
    ];

    for (cmd, cmd_args) in editors {
        let result = std::process::Command::new(cmd).args(*cmd_args).output();
        if let Ok(output) = result {
            if output.status.success() {
                return CommandResult::Output(format!(
                    "Opened project in {}.",
                    cmd
                ));
            }
        }
    }

    CommandResult::Output(
        "Could not open any IDE. Make sure a supported IDE is installed \
         and its CLI command is available in your PATH."
            .to_string(),
    )
}
