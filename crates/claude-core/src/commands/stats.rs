use async_trait::async_trait;

use crate::cost_tracker::format_cost;
use crate::utils::format::{format_duration, format_number};
use crate::utils::model;

use super::{Command, CommandContext, CommandResult};

/// Shows usage statistics and activity summary for the current session.
pub struct StatsCommand;

#[async_trait]
impl Command for StatsCommand {
    fn name(&self) -> &str {
        "stats"
    }

    fn description(&self) -> &str {
        "Show your Claude Code usage statistics and activity"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let display_model = model::get_public_model_display_name(&ctx.model)
            .map(|s| s.to_string())
            .unwrap_or_else(|| ctx.model.clone());

        let total_tokens = ctx.input_tokens + ctx.output_tokens;

        let mut lines = vec![
            "Session Statistics".to_string(),
            "==================".to_string(),
            String::new(),
        ];

        // Identity
        lines.push(format!("Model:            {}", display_model));
        if let Some(ref sid) = ctx.session_id {
            lines.push(format!("Session:          {}", sid));
        }

        // Mode flags
        let mut modes = Vec::new();
        if ctx.plan_mode { modes.push("plan"); }
        if ctx.vim_mode { modes.push("vim"); }
        if ctx.fast_mode { modes.push("fast"); }
        if ctx.brief_mode { modes.push("brief"); }
        if !modes.is_empty() {
            lines.push(format!("Active modes:     {}", modes.join(", ")));
        }

        // Timing
        lines.push(String::new());
        lines.push("Timing".to_string());
        lines.push("------".to_string());
        lines.push(format!("  Session duration: {}", format_duration(ctx.session_duration_ms)));
        lines.push(format!("  API duration:     {}", format_duration(ctx.api_duration_ms)));
        lines.push(format!("  Tool duration:    {}", format_duration(ctx.tool_duration_ms)));

        // Conversation
        lines.push(String::new());
        lines.push("Conversation".to_string());
        lines.push("------------".to_string());
        lines.push(format!("  Turns:           {}", ctx.turn_count));
        lines.push(format!("  Total tokens:    {}", format_number(total_tokens)));
        lines.push(format!("    Input:         {}", format_number(ctx.input_tokens)));
        lines.push(format!("    Output:        {}", format_number(ctx.output_tokens)));
        lines.push(format!("    Cache read:    {}", format_number(ctx.cache_read_input_tokens)));
        lines.push(format!("    Cache write:   {}", format_number(ctx.cache_creation_input_tokens)));

        // Cost
        lines.push(String::new());
        lines.push(format!("Total cost:        {}", format_cost(ctx.total_cost, 4)));

        // Code changes
        if ctx.lines_added > 0 || ctx.lines_removed > 0 {
            lines.push(String::new());
            lines.push("Code Changes".to_string());
            lines.push("------------".to_string());
            lines.push(format!("  Lines added:     +{}", format_number(ctx.lines_added)));
            lines.push(format!("  Lines removed:   -{}", format_number(ctx.lines_removed)));
        }

        CommandResult::OpenInfoDialog {
            title: "Session Statistics".to_string(),
            content: lines.join("\n"),
        }
    }
}
