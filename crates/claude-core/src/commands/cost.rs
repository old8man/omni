use async_trait::async_trait;

use crate::utils::format::format_number;
use crate::utils::model;

use super::{Command, CommandContext, CommandResult};

/// Show the total cost and duration of the current session.
///
/// Matches the TypeScript `cost` command. Displays a breakdown of
/// token usage and the accumulated dollar cost.
pub struct CostCommand;

#[async_trait]
impl Command for CostCommand {
    fn name(&self) -> &str {
        "cost"
    }

    fn description(&self) -> &str {
        "Show the total cost and duration of the current session"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let display_model = model::get_public_model_display_name(&ctx.model)
            .map(|s| s.to_string())
            .unwrap_or_else(|| ctx.model.clone());

        let total_tokens = ctx.input_tokens + ctx.output_tokens;

        let lines = [
            "Session Cost Summary".to_string(),
            "--------------------".to_string(),
            format!("Model:          {}", display_model),
            format!("Input tokens:   {}", format_number(ctx.input_tokens)),
            format!("Output tokens:  {}", format_number(ctx.output_tokens)),
            format!("Total tokens:   {}", format_number(total_tokens)),
            format!("Total cost:     ${:.4}", ctx.total_cost),
        ];

        CommandResult::Output(lines.join("\n"))
    }
}
