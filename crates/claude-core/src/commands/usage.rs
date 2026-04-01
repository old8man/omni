use async_trait::async_trait;

use crate::utils::format::{format_tokens, format_number};
use crate::utils::model;

use super::{Command, CommandContext, CommandResult};

/// Shows token usage and cost information.
pub struct UsageCommand;

#[async_trait]
impl Command for UsageCommand {
    fn name(&self) -> &str {
        "usage"
    }

    fn aliases(&self) -> &[&str] {
        &["cost"]
    }

    fn description(&self) -> &str {
        "Show token usage and cost"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let total = ctx.input_tokens + ctx.output_tokens;
        let display_model = model::get_public_model_display_name(&ctx.model)
            .map(|s| s.to_string())
            .unwrap_or_else(|| ctx.model.clone());
        let lines = [
            format!("Model:         {display_model}"),
            format!("Input tokens:  {}", format_number(ctx.input_tokens)),
            format!("Output tokens: {}", format_number(ctx.output_tokens)),
            format!("Total tokens:  {}", format_tokens(total)),
            format!("Session cost:  ${:.4}", ctx.total_cost),
        ];
        CommandResult::Output(lines.join("\n"))
    }
}
