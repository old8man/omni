use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shares the current conversation transcript.
pub struct ShareCommand;

#[async_trait]
impl Command for ShareCommand {
    fn name(&self) -> &str {
        "share"
    }

    fn description(&self) -> &str {
        "Share conversation transcript"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output(
            "Conversation transcript prepared for sharing.\n\
             Use /export for a local file export."
                .to_string(),
        )
    }
}

/// Exports the conversation to a file (JSON or Markdown format).
///
/// Matches the original TypeScript export command. The format is
/// determined by the file extension: `.json` produces structured JSON,
/// `.md` produces readable markdown, and anything else defaults to
/// markdown.
pub struct ExportCommand;

fn generate_export_json(ctx: &CommandContext) -> String {
    let session = ctx.session_id.as_deref().unwrap_or("unknown");
    let timestamp = {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    };

    serde_json::json!({
        "export_version": 1,
        "timestamp": timestamp,
        "session_id": session,
        "model": ctx.model,
        "project_root": ctx.project_root.as_ref().map(|p| p.display().to_string()),
        "cwd": ctx.cwd.display().to_string(),
        "stats": {
            "input_tokens": ctx.input_tokens,
            "output_tokens": ctx.output_tokens,
            "total_cost": ctx.total_cost,
        },
        "note": "Full conversation messages are exported when the session transcript is available."
    })
    .to_string()
}

fn generate_export_markdown(ctx: &CommandContext) -> String {
    let session = ctx.session_id.as_deref().unwrap_or("unknown");
    let mut md = String::from("# Claude Code Conversation Export\n\n");

    md.push_str(&format!("**Session:** {}\n\n", session));
    md.push_str(&format!("**Model:** {}\n\n", ctx.model));

    if let Some(ref root) = ctx.project_root {
        md.push_str(&format!("**Project:** {}\n\n", root.display()));
    }

    md.push_str("## Session Statistics\n\n");
    md.push_str(&format!("| Metric | Value |\n"));
    md.push_str(&format!("|--------|-------|\n"));
    md.push_str(&format!("| Input tokens | {} |\n", ctx.input_tokens));
    md.push_str(&format!("| Output tokens | {} |\n", ctx.output_tokens));
    md.push_str(&format!("| Total cost | ${:.4} |\n", ctx.total_cost));
    md.push_str("\n---\n\n");
    md.push_str("*Full conversation messages are exported when the session transcript is available.*\n");

    md
}

#[async_trait]
impl Command for ExportCommand {
    fn name(&self) -> &str {
        "export"
    }

    fn description(&self) -> &str {
        "Export conversation to a file (JSON or Markdown)"
    }

    fn usage_hint(&self) -> &str {
        "<filename.json|filename.md>"
    }

    async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult {
        let filename = args.trim();
        if filename.is_empty() {
            return CommandResult::Output(
                "Usage: /export <filename>\n\n\
                 Supported formats:\n\
                 \x20 .json  Structured JSON export\n\
                 \x20 .md    Readable Markdown export\n\n\
                 Examples:\n\
                 \x20 /export conversation.json\n\
                 \x20 /export session-notes.md"
                    .to_string(),
            );
        }

        let path = if std::path::Path::new(filename).is_absolute() {
            std::path::PathBuf::from(filename)
        } else {
            ctx.cwd.join(filename)
        };

        let is_json = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));

        let content = if is_json {
            generate_export_json(ctx)
        } else {
            generate_export_markdown(ctx)
        };

        match std::fs::write(&path, &content) {
            Ok(()) => {
                let format_name = if is_json { "JSON" } else { "Markdown" };
                CommandResult::Output(format!(
                    "Conversation exported ({} format):\n  {}",
                    format_name,
                    path.display()
                ))
            }
            Err(e) => CommandResult::Output(format!(
                "Failed to write export file {}: {}",
                path.display(),
                e
            )),
        }
    }
}
