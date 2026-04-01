pub mod agent_tool;
pub mod ask_user;
pub mod bash;
pub mod bash_permissions;
pub mod bash_security;
pub mod brief_tool;
pub mod config_tool;
pub mod edit;
pub mod glob_tool;
pub mod grep;
pub mod lsp_tool;
pub mod mcp_auth;
pub mod mcp_resources;
pub mod mcp_tool;
pub mod notebook_edit;
pub mod plan_mode;
pub mod powershell_tool;
pub mod read;
pub mod registry;
pub mod remote_trigger;
pub mod repl_tool;
pub mod schedule_cron;
pub mod send_message;
pub mod skill_tool;
pub mod sleep_tool;
pub mod synthetic_output;
pub mod task_tools;
pub mod team_tools;
pub mod todo_write;
pub mod tool_search;
pub mod web_fetch;
pub mod web_search;
pub mod worktree_tools;
pub mod write;

pub use registry::{MessageSender, ProgressSender, ToolExecutor, ToolRegistry, ToolUseContext};

use std::sync::Arc;

use omni_core::skills::SkillRegistry;

/// Build the default tool registry with all built-in tools.
///
/// Tools that require external configuration (e.g., `AskUserQuestionTool`)
/// must be registered separately by the application layer.
pub fn build_default_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();

    // Core file and shell tools
    reg.register(Arc::new(bash::BashTool));
    reg.register(Arc::new(read::FileReadTool));
    reg.register(Arc::new(write::FileWriteTool));
    reg.register(Arc::new(edit::FileEditTool));
    reg.register(Arc::new(grep::GrepTool));
    reg.register(Arc::new(glob_tool::GlobTool));

    // Web tools
    reg.register(Arc::new(web_search::WebSearchTool::new()));
    reg.register(Arc::new(web_fetch::WebFetchTool::new()));

    // Utility tools
    reg.register(Arc::new(sleep_tool::SleepTool));
    reg.register(Arc::new(notebook_edit::NotebookEditTool));
    reg.register(Arc::new(lsp_tool::LspTool::new()));

    // Task management tools
    reg.register(Arc::new(task_tools::TaskCreateTool));
    reg.register(Arc::new(task_tools::TaskListTool));
    reg.register(Arc::new(task_tools::TaskGetTool));
    reg.register(Arc::new(task_tools::TaskUpdateTool));
    reg.register(Arc::new(task_tools::TaskStopTool));
    reg.register(Arc::new(task_tools::TaskOutputTool));

    // Agent and team tools
    reg.register(Arc::new(agent_tool::AgentTool));
    reg.register(Arc::new(team_tools::TeamCreateTool));
    reg.register(Arc::new(team_tools::TeamDeleteTool));
    reg.register(Arc::new(send_message::SendMessageTool));

    // Plan mode tools
    reg.register(Arc::new(plan_mode::EnterPlanModeTool));
    reg.register(Arc::new(plan_mode::ExitPlanModeTool));

    // Worktree tools
    reg.register(Arc::new(worktree_tools::EnterWorktreeTool));
    reg.register(Arc::new(worktree_tools::ExitWorktreeTool));

    // Remote triggers
    reg.register(Arc::new(remote_trigger::RemoteTriggerTool::new()));

    // Config tool
    reg.register(Arc::new(config_tool::ConfigTool));

    // Brief / KAIROS output tool
    reg.register(Arc::new(brief_tool::BriefTool));

    // Structured output tool
    reg.register(Arc::new(synthetic_output::SyntheticOutputTool));

    // Todo list tool
    reg.register(Arc::new(todo_write::TodoWriteTool));

    // Scheduled agent management
    reg.register(Arc::new(schedule_cron::ScheduleCronTool));

    // PowerShell tool (Windows)
    reg.register(Arc::new(powershell_tool::PowerShellTool));

    // REPL tool (Node.js, Python)
    reg.register(Arc::new(repl_tool::ReplTool));

    // ToolSearchTool must be registered last since it snapshots the registry
    let tool_search = tool_search::ToolSearchTool::from_registry(&reg);
    reg.register(Arc::new(tool_search));

    reg
}

/// Build a tool registry that includes the Skill tool backed by the given registry.
pub fn build_registry_with_skills(skill_registry: Arc<SkillRegistry>) -> ToolRegistry {
    let mut reg = build_default_registry();

    // Insert skill tool before the ToolSearch snapshot (ToolSearch already in registry,
    // so it won't include Skill — callers can rebuild if needed).
    reg.register(Arc::new(skill_tool::SkillTool::new(skill_registry)));

    reg
}
