use async_trait::async_trait;

use crate::cost_tracker::{format_cost, get_pricing};
use crate::utils::format::{format_duration, format_number};
use crate::utils::model;

use super::{Command, CommandContext, CommandResult};

/// Show the total cost and duration of the current session.
///
/// Matches the TypeScript `cost` command. Displays a breakdown of
/// token usage and the accumulated dollar cost per model.
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

        let mut lines = vec![
            "Session Cost Summary".to_string(),
            "====================".to_string(),
            String::new(),
            format!("Model:               {}", display_model),
            format!("Session duration:     {}", format_duration(ctx.session_duration_ms)),
            format!("API duration:         {}", format_duration(ctx.api_duration_ms)),
            String::new(),
            "Token Usage".to_string(),
            "-----------".to_string(),
            format!("  Input tokens:        {}", format_number(ctx.input_tokens)),
            format!("  Output tokens:       {}", format_number(ctx.output_tokens)),
            format!("  Cache read tokens:   {}", format_number(ctx.cache_read_input_tokens)),
            format!("  Cache write tokens:  {}", format_number(ctx.cache_creation_input_tokens)),
            format!("  Total tokens:        {}", format_number(total_tokens)),
            String::new(),
            format!("Total cost:            {}", format_cost(ctx.total_cost, 4)),
        ];

        if ctx.lines_added > 0 || ctx.lines_removed > 0 {
            lines.push(format!(
                "Code changes:          +{} / -{}",
                format_number(ctx.lines_added),
                format_number(ctx.lines_removed),
            ));
        }

        if !ctx.model_usage.is_empty() {
            lines.push(String::new());
            lines.push("Per-Model Breakdown".to_string());
            lines.push("-------------------".to_string());

            for (model_name, input, output, cache_read, cache_write, cost) in &ctx.model_usage {
                let short_name = model::get_public_model_display_name(model_name)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| model_name.clone());
                let pricing = get_pricing(model_name);
                lines.push(format!("  {short_name} (${:.0}/${:.0} per Mtok):", pricing.input_cost_per_mtok, pricing.output_cost_per_mtok));
                lines.push(format!("    Input:       {}", format_number(*input)));
                lines.push(format!("    Output:      {}", format_number(*output)));
                lines.push(format!("    Cache read:  {}", format_number(*cache_read)));
                lines.push(format!("    Cache write: {}", format_number(*cache_write)));
                lines.push(format!("    Cost:        {}", format_cost(*cost, 4)));
            }
        }

        CommandResult::Output(lines.join("\n"))
    }
}
