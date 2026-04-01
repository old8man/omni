use async_trait::async_trait;

use crate::cost_tracker::format_cost;
use crate::utils::format::format_number;
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
        &["tokens"]
    }

    fn description(&self) -> &str {
        "Show token usage and cost"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let total = ctx.input_tokens + ctx.output_tokens;
        let display_model = model::get_public_model_display_name(&ctx.model)
            .map(|s| s.to_string())
            .unwrap_or_else(|| ctx.model.clone());

        let mut lines = vec![
            format!("Model:              {display_model}"),
            format!("Input tokens:       {}", format_number(ctx.input_tokens)),
            format!("Output tokens:      {}", format_number(ctx.output_tokens)),
            format!("Cache read tokens:  {}", format_number(ctx.cache_read_input_tokens)),
            format!("Cache write tokens: {}", format_number(ctx.cache_creation_input_tokens)),
            format!("Total tokens:       {}", format_number(total)),
            format!("Session cost:       {}", format_cost(ctx.total_cost, 4)),
        ];

        if ctx.model_usage.len() > 1 {
            lines.push(String::new());
            lines.push("By model:".to_string());
            for (model_name, input, output, _cache_read, _cache_write, cost) in &ctx.model_usage {
                let short = model::get_public_model_display_name(model_name)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| model_name.clone());
                lines.push(format!(
                    "  {}: {} in / {} out ({})",
                    short,
                    format_number(*input),
                    format_number(*output),
                    format_cost(*cost, 4),
                ));
            }
        }

        CommandResult::Output(lines.join("\n"))
    }
}
