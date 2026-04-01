use async_trait::async_trait;

use crate::utils::model;

use super::{Command, CommandContext, CommandResult};

/// Shows the current model or switches to a different one.
pub struct ModelCommand;

#[async_trait]
impl Command for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }

    fn description(&self) -> &str {
        "Show or switch model"
    }

    fn usage_hint(&self) -> &str {
        "[model-name]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let name = args.trim();
        if name.is_empty() {
            // Open the interactive model picker
            CommandResult::OpenPicker("model".to_string())
        } else {
            // Validate input: accept aliases and known models
            let resolved = model::resolve_model_string(name);
            if !model::is_valid_model(&resolved) {
                return CommandResult::Output(format!(
                    "Unknown model: {name}\n\nValid aliases: {}",
                    model::MODEL_ALIASES.join(", ")
                ));
            }

            // Warn about deprecated models
            if let Some(warning) = model::get_model_deprecation_warning(&resolved) {
                // Still allow switching, but show the warning
                return CommandResult::Output(format!(
                    "Switching to {resolved}\n\nWarning: {warning}"
                ));
            }

            CommandResult::SwitchModel(resolved)
        }
    }
}
