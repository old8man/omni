use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Switch the output format between different rendering styles.
///
/// Matches the original TypeScript output-style command. Allows users
/// to control how Claude's responses are formatted in the terminal.
pub struct OutputStyleCommand;

const VALID_STYLES: &[(&str, &str)] = &[
    ("markdown", "Rich markdown with headers, lists, and code blocks (default)"),
    ("plain", "Plain text with no formatting"),
    ("minimal", "Concise output with reduced whitespace"),
    ("json", "Structured JSON output for programmatic consumption"),
    ("streaming", "Character-by-character streaming display"),
];

#[async_trait]
impl Command for OutputStyleCommand {
    fn name(&self) -> &str {
        "output-style"
    }

    fn aliases(&self) -> &[&str] {
        &["style"]
    }

    fn description(&self) -> &str {
        "Change the output rendering style"
    }

    fn usage_hint(&self) -> &str {
        "[markdown|plain|minimal|json|streaming]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let style = args.trim().to_lowercase();

        if style.is_empty() {
            let mut output = String::from("Output Styles\n");
            output.push_str("=============\n\n");
            output.push_str("Usage: /output-style <style>\n\n");
            output.push_str("Available styles:\n");
            for (name, desc) in VALID_STYLES {
                output.push_str(&format!("  {:<12} {}\n", name, desc));
            }
            output.push_str("\nCurrent style can be persisted with /config set output_style <style>");
            return CommandResult::Output(output);
        }

        if let Some((name, desc)) = VALID_STYLES.iter().find(|(n, _)| *n == style.as_str()) {
            let config_dir = dirs::config_dir().map(|d| d.join("claude"));

            if let Some(dir) = config_dir {
                let _ = std::fs::create_dir_all(&dir);
                let style_path = dir.join("output_style");
                let _ = std::fs::write(&style_path, name);
            }

            CommandResult::Output(format!(
                "Output style set to: {}\n  {}",
                name, desc
            ))
        } else {
            let valid: Vec<&str> = VALID_STYLES.iter().map(|(n, _)| *n).collect();
            CommandResult::Output(format!(
                "Unknown style '{}'. Valid styles: {}",
                style,
                valid.join(", ")
            ))
        }
    }
}
