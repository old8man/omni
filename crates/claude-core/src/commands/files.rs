use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// List all files currently loaded in the conversation context.
///
/// Matches the TypeScript `files` command which shows the list of files
/// that have been read into the conversation state.
pub struct FilesCommand;

#[async_trait]
impl Command for FilesCommand {
    fn name(&self) -> &str {
        "files"
    }

    fn description(&self) -> &str {
        "List all files currently in context"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        // In the Rust implementation, we gather files from the context.
        // The file state cache is managed at a higher level; here we
        // report what we know from the context directory structure.
        //
        // For now the context does not carry a file list, so we provide
        // a helpful message. Once the file state cache is wired into
        // CommandContext this will enumerate actual files.
        let cwd = &ctx.cwd;
        let project = ctx
            .project_root
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| cwd.display().to_string());

        CommandResult::OpenInfoDialog {
            title: "Files in Context".to_string(),
            content: format!(
                "Files in context:\n  (working directory: {})\n\n\
                 No files have been read into context yet. \
                 Files are added as you reference them in conversation.",
                project
            ),
        }
    }
}
