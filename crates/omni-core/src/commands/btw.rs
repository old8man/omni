use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Ask a quick side question without interrupting the main conversation.
///
/// This sends the question to the model as a self-contained prompt,
/// separate from the ongoing conversation context.
pub struct BtwCommand;

#[async_trait]
impl Command for BtwCommand {
    fn name(&self) -> &str {
        "btw"
    }

    fn description(&self) -> &str {
        "Ask a quick side question without interrupting the main conversation"
    }

    fn usage_hint(&self) -> &str {
        "<question>"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let question = args.trim();

        if question.is_empty() {
            return CommandResult::Output(
                "Please provide a question. Usage: /btw <question>".to_string(),
            );
        }

        CommandResult::Prompt {
            content: format!(
                "The user has a quick side question that is separate from the main conversation. \
                 Answer it concisely and directly.\n\n\
                 Side question: {}",
                question
            ),
            allowed_tools: None,
            progress_message: Some("thinking about your side question".to_string()),
        }
    }
}
