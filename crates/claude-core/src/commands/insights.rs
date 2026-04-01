use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Show session insights and analytics.
///
/// Matches the original TypeScript insights command. Displays a summary
/// of the current session including token usage patterns, cost breakdown,
/// efficiency metrics, and usage recommendations.
pub struct InsightsCommand;

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn cost_per_1k_tokens(cost: f64, tokens: u64) -> f64 {
    if tokens == 0 {
        0.0
    } else {
        (cost / tokens as f64) * 1000.0
    }
}

fn token_ratio_assessment(input: u64, output: u64) -> &'static str {
    if input == 0 && output == 0 {
        return "No tokens used yet.";
    }
    let total = input + output;
    let input_pct = (input as f64 / total as f64) * 100.0;

    if input_pct > 90.0 {
        "Very high input ratio. Consider using /compact to reduce context size."
    } else if input_pct > 75.0 {
        "High input ratio. Large context windows are being sent to the model."
    } else if input_pct > 50.0 {
        "Balanced input/output ratio. Normal usage pattern."
    } else {
        "Output-heavy session. The model is generating substantial content."
    }
}

fn cost_assessment(cost: f64) -> &'static str {
    if cost == 0.0 {
        "No cost incurred yet."
    } else if cost < 0.10 {
        "Very low cost session."
    } else if cost < 1.00 {
        "Moderate cost session."
    } else if cost < 5.00 {
        "Significant cost session. Consider compacting conversation if context is large."
    } else {
        "High cost session. Review token usage and consider starting a new session."
    }
}

#[async_trait]
impl Command for InsightsCommand {
    fn name(&self) -> &str {
        "insights"
    }

    fn aliases(&self) -> &[&str] {
        &["analytics"]
    }

    fn description(&self) -> &str {
        "Show session insights and analytics"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut output = String::from("Session Insights\n");
        output.push_str("================\n\n");

        // Session info
        output.push_str("Session\n");
        output.push_str("-------\n");
        output.push_str(&format!("  Model:   {}\n", ctx.model));
        if let Some(ref sid) = ctx.session_id {
            output.push_str(&format!("  ID:      {}\n", sid));
        }
        if let Some(ref root) = ctx.project_root {
            output.push_str(&format!("  Project: {}\n", root.display()));
        }
        output.push('\n');

        // Token usage
        let total_tokens = ctx.input_tokens + ctx.output_tokens;
        output.push_str("Token Usage\n");
        output.push_str("-----------\n");
        output.push_str(&format!(
            "  Input tokens:  {:>10} ({})\n",
            ctx.input_tokens,
            format_tokens(ctx.input_tokens)
        ));
        output.push_str(&format!(
            "  Output tokens: {:>10} ({})\n",
            ctx.output_tokens,
            format_tokens(ctx.output_tokens)
        ));
        output.push_str(&format!(
            "  Total tokens:  {:>10} ({})\n",
            total_tokens,
            format_tokens(total_tokens)
        ));

        if total_tokens > 0 {
            let input_pct = (ctx.input_tokens as f64 / total_tokens as f64) * 100.0;
            let output_pct = (ctx.output_tokens as f64 / total_tokens as f64) * 100.0;
            output.push_str(&format!(
                "  Ratio:         {:>5.1}% input / {:.1}% output\n",
                input_pct, output_pct
            ));
        }
        output.push('\n');

        // Cost breakdown
        output.push_str("Cost Analysis\n");
        output.push_str("-------------\n");
        output.push_str(&format!("  Total cost:    ${:.4}\n", ctx.total_cost));

        if total_tokens > 0 {
            let cost_per_1k = cost_per_1k_tokens(ctx.total_cost, total_tokens);
            output.push_str(&format!("  Cost/1K tok:   ${:.4}\n", cost_per_1k));
        }
        output.push('\n');

        // Efficiency metrics
        output.push_str("Efficiency\n");
        output.push_str("----------\n");
        output.push_str(&format!(
            "  {}\n",
            token_ratio_assessment(ctx.input_tokens, ctx.output_tokens)
        ));
        output.push_str(&format!("  {}\n", cost_assessment(ctx.total_cost)));
        output.push('\n');

        // Context window utilization
        output.push_str("Context Window\n");
        output.push_str("--------------\n");

        let context_limit: u64 = 200_000;
        let utilization = if ctx.input_tokens > 0 {
            (ctx.input_tokens as f64 / context_limit as f64) * 100.0
        } else {
            0.0
        };
        output.push_str(&format!(
            "  Utilization:   {:.1}% of {}K limit\n",
            utilization,
            context_limit / 1000
        ));

        if utilization > 75.0 {
            output.push_str("  Warning: Context is filling up. Use /compact to reduce size.\n");
        } else if utilization > 50.0 {
            output.push_str("  Context usage is moderate. Consider /compact if responses slow down.\n");
        } else {
            output.push_str("  Context usage is healthy.\n");
        }
        output.push('\n');

        // Recommendations
        output.push_str("Recommendations\n");
        output.push_str("---------------\n");

        let mut recs = Vec::new();

        if ctx.input_tokens > 100_000 {
            recs.push("  - Run /compact to reduce context window size and lower costs.");
        }
        if ctx.total_cost > 2.0 {
            recs.push(
                "  - Consider starting a new session with /clear for a fresh context.",
            );
        }
        if ctx.output_tokens > ctx.input_tokens && ctx.output_tokens > 10_000 {
            recs.push(
                "  - High output volume. Use /brief mode for more concise responses.",
            );
        }

        if recs.is_empty() {
            output.push_str("  No concerns. Session is running efficiently.\n");
        } else {
            for rec in recs {
                output.push_str(rec);
                output.push('\n');
            }
        }

        CommandResult::OpenInfoDialog {
            title: "Session Insights".to_string(),
            content: output,
        }
    }
}
