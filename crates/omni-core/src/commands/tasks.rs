use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// View current task list.
pub struct TasksCommand;

#[async_trait]
impl Command for TasksCommand {
    fn name(&self) -> &str {
        "tasks"
    }

    fn aliases(&self) -> &[&str] {
        &["todos"]
    }

    fn description(&self) -> &str {
        "View current task list"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output("Use the TodoWrite or TaskList tools to manage tasks.".to_string())
    }
}
