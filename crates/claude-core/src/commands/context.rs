use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Shows context window usage information.
pub struct ContextCommand;

/// Context window size for a given model.
///
/// Matches TS `getContextWindowForModel()`: default 200k, with 1M for
/// models that support it (Sonnet 4+, Opus 4.6) when the `[1m]` suffix
/// is present in the model name.
fn model_context_limit(model: &str) -> u64 {
    // Env override (ant-only in TS, always available here for testing)
    if let Ok(val) = std::env::var("CLAUDE_CODE_MAX_CONTEXT_TOKENS") {
        if let Ok(n) = val.parse::<u64>() {
            if n > 0 {
                return n;
            }
        }
    }
    // Explicit [1m] suffix
    if model.to_lowercase().contains("[1m]") {
        return 1_000_000;
    }
    // Default
    200_000
}

#[async_trait]
impl Command for ContextCommand {
    fn name(&self) -> &str {
        "context"
    }

    fn description(&self) -> &str {
        "Show context window usage"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let total = ctx.input_tokens + ctx.output_tokens;
        let limit = model_context_limit(&ctx.model);
        let pct = if limit > 0 {
            (total as f64 / limit as f64) * 100.0
        } else {
            0.0
        };
        let lines = [
            format!("Context window: {:.1}% used", pct),
            format!("  Input tokens:  {}", ctx.input_tokens),
            format!("  Output tokens: {}", ctx.output_tokens),
            format!("  Total:         {} / {}", total, limit),
        ];
        CommandResult::OpenInfoDialog {
            title: "Context Window".to_string(),
            content: lines.join("\n"),
        }
    }
}
