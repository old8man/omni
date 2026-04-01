use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Lists available skills.
pub struct SkillsCommand;

#[async_trait]
impl Command for SkillsCommand {
    fn name(&self) -> &str {
        "skills"
    }

    fn description(&self) -> &str {
        "List available skills"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Output(
            "Use the Skill tool to invoke skills. Available bundled skills:\n\
             \n\
             - simplify — Review changed code for reuse, quality, and efficiency\n\
             - update-config — Configure Claude Code via settings.json\n\
             - keybindings-help — Customize keyboard shortcuts\n\
             - claude-api — Build apps with the Claude API\n\
             - loop — Run commands on a recurring interval\n\
             - schedule — Manage scheduled agents\n\
             - verify — Run tests and checks\n\
             - debug — Diagnose and fix bugs\n\
             - remember — Save context to CLAUDE.md\n\
             - stuck — Break out of error loops\n\
             - batch — Run commands on multiple inputs in bulk\n\
             - skillify — Convert a prompt into a reusable skill\n\
             - loremIpsum — Generate placeholder text\n\
             \n\
             Custom skills: ~/.claude/skills/ and .claude/skills/"
                .to_string(),
        )
    }
}
