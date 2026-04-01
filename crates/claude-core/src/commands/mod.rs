//! Slash-command framework for the Claude TUI.
//!
//! Each command implements the [`Command`] trait. The [`CommandRegistry`]
//! collects them and provides name/alias lookup plus dispatch.

mod advisor;
mod branch;
mod brief;
mod clear;
mod commit;
mod compact;
mod config;
mod context;
mod copy;
mod diff;
mod doctor;
mod effort;
mod env;
mod fast;
mod feedback;
mod help;
mod hooks;
mod init;
mod keybindings;
mod login;
mod mcp;
mod memory;
mod model;
mod permissions;
mod plan;
mod plugin;
mod quit;
mod rename;
mod resume;
mod review;
mod rewind;
mod sandbox;
mod session;
mod share;
mod skills;
mod stats;
mod status;
mod stickers;
mod tag;
mod tasks;
mod theme;
mod thinkback;
mod upgrade;
mod usage;
mod version;
mod vim;
mod voice;

use std::collections::HashMap;

use async_trait::async_trait;

/// Contextual information passed to every command invocation.
#[derive(Debug, Clone)]
pub struct CommandContext {
    /// The current working directory.
    pub cwd: std::path::PathBuf,
    /// Project root (if inside a git repo, for example).
    pub project_root: Option<std::path::PathBuf>,
    /// Currently active model identifier.
    pub model: String,
    /// Session identifier (if any).
    pub session_id: Option<String>,
    /// Running total of input tokens this session.
    pub input_tokens: u64,
    /// Running total of output tokens this session.
    pub output_tokens: u64,
    /// Running total cost in USD this session.
    pub total_cost: f64,
    /// Whether vim-mode is currently enabled.
    pub vim_mode: bool,
    /// Whether plan-mode is currently enabled.
    pub plan_mode: bool,
}

/// Result type returned by a command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Display textual output to the user.
    Output(String),
    /// Inject a prompt into the conversation for the model to execute.
    /// This is used by "prompt"-type commands like /commit, /review, etc.
    /// The String is the prompt content; the optional Vec<String> lists allowed tools.
    Prompt {
        content: String,
        allowed_tools: Option<Vec<String>>,
        progress_message: Option<String>,
    },
    /// Quit the application.
    Quit,
    /// Switch to a different model.
    SwitchModel(String),
    /// Clear the conversation history.
    ClearConversation,
    /// Resume a specific session by ID.
    ResumeSession(String),
    /// Compact the current conversation.
    CompactMessages(Option<String>),
    /// Toggle plan mode.
    TogglePlanMode,
    /// Toggle vim mode.
    ToggleVimMode,
}

/// Trait implemented by every slash command.
#[async_trait]
pub trait Command: Send + Sync {
    /// Primary name (without the leading `/`).
    fn name(&self) -> &str;

    /// Optional aliases.
    fn aliases(&self) -> &[&str] {
        &[]
    }

    /// One-line description shown in `/help`.
    fn description(&self) -> &str;

    /// Usage hint (e.g. `[model-name]`). Empty means no arguments.
    fn usage_hint(&self) -> &str {
        ""
    }

    /// Execute the command and return a result.
    async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult;
}

/// Registry that maps command names and aliases to implementations.
#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<Box<dyn Command>>,
    name_map: HashMap<String, usize>,
}

impl CommandRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            name_map: HashMap::new(),
        }
    }

    /// Register a command. Overwrites any existing entry with the same name.
    pub fn register(&mut self, cmd: Box<dyn Command>) {
        let idx = self.commands.len();
        self.name_map.insert(cmd.name().to_string(), idx);
        for alias in cmd.aliases() {
            self.name_map.insert(alias.to_string(), idx);
        }
        self.commands.push(cmd);
    }

    /// Look up a command by name or alias (case-insensitive).
    pub fn find(&self, name: &str) -> Option<&dyn Command> {
        let lower = name.to_lowercase();
        self.name_map.get(&lower).map(|&i| &*self.commands[i])
    }

    /// Return all registered commands (de-duplicated, sorted by name).
    pub fn all_commands(&self) -> Vec<&dyn Command> {
        let mut cmds: Vec<&dyn Command> = self.commands.iter().map(|b| &**b).collect();
        cmds.sort_by_key(|c| c.name());
        cmds
    }

    /// Parse a `/command args` input string. Returns `(command, args)` if matched.
    pub fn parse_and_find<'a>(&'a self, input: &str) -> Option<(&'a dyn Command, String)> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }
        let without_slash = &trimmed[1..];
        let (name, args) = match without_slash.find(char::is_whitespace) {
            Some(pos) => (&without_slash[..pos], without_slash[pos..].trim()),
            None => (without_slash, ""),
        };
        self.find(name).map(|cmd| (cmd, args.to_string()))
    }

    /// Build a registry pre-loaded with all built-in commands.
    pub fn default_registry() -> Self {
        let mut reg = Self::new();

        // Core commands
        reg.register(Box::new(help::HelpCommand));
        reg.register(Box::new(clear::ClearCommand));
        reg.register(Box::new(compact::CompactCommand));
        reg.register(Box::new(quit::QuitCommand));
        reg.register(Box::new(version::VersionCommand));

        // Git workflow commands
        reg.register(Box::new(commit::CommitCommand));
        reg.register(Box::new(commit::CommitPushPrCommand));
        reg.register(Box::new(review::ReviewCommand));
        reg.register(Box::new(review::SecurityReviewCommand));
        reg.register(Box::new(diff::DiffCommand));

        // Session and conversation commands
        reg.register(Box::new(session::SessionCommand));
        reg.register(Box::new(branch::BranchCommand));
        reg.register(Box::new(resume::ResumeCommand));
        reg.register(Box::new(share::ShareCommand));
        reg.register(Box::new(share::ExportCommand));

        // Configuration commands
        reg.register(Box::new(config::ConfigCommand));
        reg.register(Box::new(model::ModelCommand));
        reg.register(Box::new(effort::EffortCommand));
        reg.register(Box::new(theme::ThemeCommand));
        reg.register(Box::new(fast::FastCommand));
        reg.register(Box::new(env::EnvCommand));
        reg.register(Box::new(plan::PlanCommand));
        reg.register(Box::new(vim::VimCommand));
        reg.register(Box::new(voice::VoiceCommand));

        // Management commands
        reg.register(Box::new(hooks::HooksCommand));
        reg.register(Box::new(permissions::PermissionsCommand));
        reg.register(Box::new(mcp::McpCommand));
        reg.register(Box::new(plugin::PluginCommand));
        reg.register(Box::new(keybindings::KeybindingsCommand));
        reg.register(Box::new(skills::SkillsCommand));
        reg.register(Box::new(tasks::TasksCommand));

        // Info commands
        reg.register(Box::new(status::StatusCommand));
        reg.register(Box::new(usage::UsageCommand));
        reg.register(Box::new(context::ContextCommand));
        reg.register(Box::new(doctor::DoctorCommand));
        reg.register(Box::new(memory::MemoryCommand));

        // Auth commands
        reg.register(Box::new(login::LoginCommand));
        reg.register(Box::new(login::LogoutCommand));

        // Project commands
        reg.register(Box::new(init::InitCommand));
        reg.register(Box::new(feedback::FeedbackCommand));

        // Conversation management commands
        reg.register(Box::new(copy::CopyCommand));
        reg.register(Box::new(rename::RenameCommand));
        reg.register(Box::new(rewind::RewindCommand));
        reg.register(Box::new(tag::TagCommand));

        // Mode and feature commands
        reg.register(Box::new(brief::BriefCommand));
        reg.register(Box::new(sandbox::SandboxCommand));
        reg.register(Box::new(advisor::AdvisorCommand));

        // Info and utility commands
        reg.register(Box::new(stats::StatsCommand));
        reg.register(Box::new(upgrade::UpgradeCommand));
        reg.register(Box::new(stickers::StickersCommand));
        reg.register(Box::new(thinkback::ThinkbackCommand));

        reg
    }
}
