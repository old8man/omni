use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Visualize the current context window as a colored grid.
///
/// Renders a compact visual representation of the conversation's context
/// usage, showing how much of the available context window has been
/// consumed. Each cell in the grid represents a portion of the total
/// context, colored to indicate different content types (system prompt,
/// user messages, assistant responses, tool calls, etc.).
pub struct CtxVizCommand;

/// ANSI color codes for different context segments.
const COLOR_SYSTEM: &str = "\x1b[48;5;33m";   // Blue — system prompt
const COLOR_USER: &str = "\x1b[48;5;35m";     // Green — user messages
const COLOR_ASSISTANT: &str = "\x1b[48;5;141m"; // Purple — assistant responses
const COLOR_TOOL: &str = "\x1b[48;5;214m";    // Orange — tool calls/results
const COLOR_EMPTY: &str = "\x1b[48;5;236m";   // Dark gray — unused context
const RESET: &str = "\x1b[0m";

/// Grid dimensions.
const GRID_WIDTH: usize = 60;
const GRID_HEIGHT: usize = 10;

#[async_trait]
impl Command for CtxVizCommand {
    fn name(&self) -> &str {
        "ctx-viz"
    }

    fn aliases(&self) -> &[&str] {
        &["context-viz"]
    }

    fn description(&self) -> &str {
        "Visualize context window usage as a colored grid"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let total_tokens = ctx.input_tokens + ctx.output_tokens;

        // Assume a 200k context window for visualization purposes
        let context_window: u64 = 200_000;

        // Estimate token distribution (these would come from actual
        // conversation analysis in a full implementation)
        let system_tokens = std::cmp::min(context_window / 50, 4000); // ~2% for system prompt
        let user_tokens = ctx.input_tokens.saturating_sub(system_tokens);
        let assistant_tokens = ctx.output_tokens;
        let used_tokens = total_tokens;
        let _empty_tokens = context_window.saturating_sub(used_tokens);

        let total_cells = GRID_WIDTH * GRID_HEIGHT;

        // Calculate cell counts proportional to token usage
        let cells_system = proportional_cells(system_tokens, context_window, total_cells);
        let cells_user = proportional_cells(user_tokens, context_window, total_cells);
        let cells_assistant = proportional_cells(assistant_tokens, context_window, total_cells);
        let cells_tool = 0usize; // No separate tool tracking yet
        let cells_used = cells_system + cells_user + cells_assistant + cells_tool;
        let cells_empty = total_cells.saturating_sub(cells_used);

        // Build the grid
        let mut grid = Vec::with_capacity(total_cells);
        grid.extend(std::iter::repeat_n(COLOR_SYSTEM, cells_system));
        grid.extend(std::iter::repeat_n(COLOR_USER, cells_user));
        grid.extend(std::iter::repeat_n(COLOR_ASSISTANT, cells_assistant));
        grid.extend(std::iter::repeat_n(COLOR_TOOL, cells_tool));
        grid.extend(std::iter::repeat_n(COLOR_EMPTY, cells_empty));
        grid.truncate(total_cells);

        // Pad if needed
        while grid.len() < total_cells {
            grid.push(COLOR_EMPTY);
        }

        let mut output = String::from("Context Window Visualization\n");
        output.push_str("════════════════════════════\n\n");

        // Render grid
        for row in 0..GRID_HEIGHT {
            let start = row * GRID_WIDTH;
            let end = start + GRID_WIDTH;
            for cell in &grid[start..end] {
                output.push_str(cell);
                output.push_str("  "); // Two spaces make a visible block
                output.push_str(RESET);
            }
            output.push('\n');
        }

        output.push('\n');

        // Legend
        let pct_used = if context_window > 0 {
            (used_tokens as f64 / context_window as f64 * 100.0).min(100.0)
        } else {
            0.0
        };

        output.push_str(&format!(
            "Context usage: {:.1}% ({} / {} tokens)\n\n",
            pct_used, used_tokens, context_window
        ));

        output.push_str(&format!("{}  {}  System prompt\n", COLOR_SYSTEM, RESET));
        output.push_str(&format!("{}  {}  User messages\n", COLOR_USER, RESET));
        output.push_str(&format!("{}  {}  Assistant responses\n", COLOR_ASSISTANT, RESET));
        output.push_str(&format!("{}  {}  Tool calls/results\n", COLOR_TOOL, RESET));
        output.push_str(&format!("{}  {}  Available context\n", COLOR_EMPTY, RESET));

        CommandResult::OpenInfoDialog {
            title: "Context Visualization".to_string(),
            content: output,
        }
    }
}

/// Calculate the number of cells proportional to the token count.
fn proportional_cells(tokens: u64, total_tokens: u64, total_cells: usize) -> usize {
    if total_tokens == 0 {
        return 0;
    }
    ((tokens as f64 / total_tokens as f64) * total_cells as f64).round() as usize
}
