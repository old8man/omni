use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Extended planning mode with deeper multi-phase analysis.
///
/// Matches the original TypeScript ultraplan command. Unlike the regular
/// /plan command (which toggles plan mode), /ultraplan injects a detailed
/// planning prompt that instructs the model to perform a thorough
/// multi-phase analysis before proposing any code changes.
pub struct UltraplanCommand;

fn get_ultraplan_prompt(task: &str, ctx: &CommandContext) -> String {
    let project_root = ctx
        .project_root
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ctx.cwd.display().to_string());

    format!(
        "You are in ULTRAPLAN mode. This is an extended planning session that requires deep, \
         thorough analysis before any implementation.\n\
         \n\
         PROJECT ROOT: {project_root}\n\
         \n\
         TASK:\n{task}\n\
         \n\
         Follow this multi-phase planning methodology:\n\
         \n\
         ## Phase 1: Discovery and Context Gathering\n\
         \n\
         Thoroughly explore the codebase to understand:\n\
         - Project structure and architecture patterns\n\
         - Relevant existing code, modules, and their relationships\n\
         - Test infrastructure and coverage patterns\n\
         - Configuration files, build system, and dependencies\n\
         - Coding conventions and style patterns used in the project\n\
         \n\
         Use file search and code exploration tools extensively. Read key files in full.\n\
         Do not skip this phase.\n\
         \n\
         ## Phase 2: Requirements Analysis\n\
         \n\
         Break down the task into:\n\
         - Explicit requirements (what the user stated)\n\
         - Implicit requirements (what must also be true for the solution to work)\n\
         - Edge cases and error conditions to handle\n\
         - Backwards compatibility constraints\n\
         - Performance implications\n\
         \n\
         ## Phase 3: Design Alternatives\n\
         \n\
         Propose at least 2-3 different approaches to solving the task. For each:\n\
         - Describe the approach in detail\n\
         - List specific files that would be created or modified\n\
         - Identify risks and tradeoffs\n\
         - Estimate relative complexity\n\
         \n\
         ## Phase 4: Recommended Plan\n\
         \n\
         Select the best approach and create a detailed implementation plan:\n\
         \n\
         1. **File-by-file change list**: For each file, describe exactly what changes \
         are needed and why\n\
         2. **Dependency order**: Specify the order in which changes should be made to \
         keep the codebase building at each step\n\
         3. **Testing strategy**: What tests need to be added or modified\n\
         4. **Risk mitigation**: How to verify the changes work correctly\n\
         5. **Rollback plan**: How to undo the changes if something goes wrong\n\
         \n\
         ## Phase 5: Validation Checklist\n\
         \n\
         Before concluding, verify the plan against:\n\
         - [ ] All explicit requirements are addressed\n\
         - [ ] No existing functionality is broken\n\
         - [ ] Error handling is comprehensive\n\
         - [ ] The plan follows existing project conventions\n\
         - [ ] Tests cover the new behavior\n\
         - [ ] Edge cases are handled\n\
         \n\
         IMPORTANT: Do NOT write any code or make any changes during this planning phase. \
         Output only the analysis and plan. The user will review the plan and then ask you \
         to implement it.\n\
         \n\
         Begin Phase 1 now."
    )
}

#[async_trait]
impl Command for UltraplanCommand {
    fn name(&self) -> &str {
        "ultraplan"
    }

    fn aliases(&self) -> &[&str] {
        &["ultra-plan", "deep-plan"]
    }

    fn description(&self) -> &str {
        "Extended planning mode with deep multi-phase analysis"
    }

    fn usage_hint(&self) -> &str {
        "<task description>"
    }

    async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult {
        let task = args.trim();
        if task.is_empty() {
            return CommandResult::Output(
                "Usage: /ultraplan <task description>\n\n\
                 Describe the task you want to plan. The model will perform a thorough\n\
                 multi-phase analysis including discovery, requirements, design alternatives,\n\
                 and a detailed implementation plan before any code is written."
                    .to_string(),
            );
        }

        CommandResult::Prompt {
            content: get_ultraplan_prompt(task, ctx),
            allowed_tools: None,
            progress_message: Some("deep planning analysis".to_string()),
        }
    }
}
