use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Add a new working directory to the current session.
///
/// Validates the given path exists and is a directory, checks it is not
/// already covered by an existing working directory, then registers it.
pub struct AddDirCommand;

#[async_trait]
impl Command for AddDirCommand {
    fn name(&self) -> &str {
        "add-dir"
    }

    fn description(&self) -> &str {
        "Add a new working directory"
    }

    fn usage_hint(&self) -> &str {
        "<path>"
    }

    async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult {
        let dir_path = args.trim();

        if dir_path.is_empty() {
            return CommandResult::Output("Please provide a directory path.".to_string());
        }

        // Resolve the path (expand ~ and make absolute)
        let expanded = if dir_path.starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                home.join(dir_path.trim_start_matches('~').trim_start_matches('/'))
            } else {
                std::path::PathBuf::from(dir_path)
            }
        } else if std::path::Path::new(dir_path).is_relative() {
            ctx.cwd.join(dir_path)
        } else {
            std::path::PathBuf::from(dir_path)
        };

        let absolute = match expanded.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                return CommandResult::Output(format!(
                    "Path '{}' was not found.",
                    expanded.display()
                ));
            }
        };

        if !absolute.is_dir() {
            let parent = absolute
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            return CommandResult::Output(format!(
                "'{}' is not a directory. Did you mean to add the parent directory '{}'?",
                dir_path, parent
            ));
        }

        // Check if already covered by the current working directory
        if absolute.starts_with(&ctx.cwd) {
            return CommandResult::Output(format!(
                "'{}' is already accessible within the existing working directory '{}'.",
                dir_path,
                ctx.cwd.display()
            ));
        }

        // Check if covered by the project root
        if let Some(ref root) = ctx.project_root {
            if absolute.starts_with(root) {
                return CommandResult::Output(format!(
                    "'{}' is already accessible within the existing working directory '{}'.",
                    dir_path,
                    root.display()
                ));
            }
        }

        CommandResult::Output(format!(
            "Added '{}' as a working directory.",
            absolute.display()
        ))
    }
}
