use serde::{Deserialize, Serialize};

/// Where a skill was loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    /// Bundled with the application.
    Bundled,
    /// From the user's `~/.claude/skills/` directory.
    User,
    /// From the project's `.claude/skills/` directory.
    Project,
    /// Provided by a plugin.
    Plugin(String),
    /// Managed/remote skill.
    Managed,
}

/// Parsed YAML frontmatter from a skill markdown file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Human-readable name.
    #[serde(default)]
    pub name: Option<String>,
    /// One-line description.
    #[serde(default)]
    pub description: Option<String>,
    /// When the model should use this skill.
    #[serde(default)]
    pub when_to_use: Option<String>,
    /// Tools this skill is allowed to invoke.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Whether end-users can invoke this skill via `/skill-name`.
    #[serde(default)]
    pub user_invocable: bool,
    /// Hint shown in help for user-invocable skills (e.g. `<url>`).
    #[serde(default)]
    pub argument_hint: Option<String>,
    /// Model override for this skill.
    #[serde(default)]
    pub model: Option<String>,
    /// If true, the model cannot invoke this skill via the Skill tool.
    #[serde(default)]
    pub disable_model_invocation: bool,
}

/// A fully resolved skill ready for use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique name (derived from filename or frontmatter).
    pub name: String,
    /// One-line description.
    pub description: String,
    /// Where this skill was loaded from.
    pub source: SkillSource,
    /// When the model should invoke this skill.
    pub when_to_use: Option<String>,
    /// The prompt body (markdown content after frontmatter).
    pub body: String,
    /// Tools this skill is allowed to use.
    pub allowed_tools: Vec<String>,
    /// Model override.
    pub model: Option<String>,
    /// Whether users can invoke this directly.
    pub user_invocable: bool,
    /// Argument hint for user-invocable skills.
    pub argument_hint: Option<String>,
    /// If true, the model cannot invoke this skill via the Skill tool.
    pub disable_model_invocation: bool,
}

impl Skill {
    /// Create a new bundled skill.
    pub fn bundled(name: &str, description: &str, body: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            source: SkillSource::Bundled,
            when_to_use: None,
            body: body.to_string(),
            allowed_tools: Vec::new(),
            model: None,
            user_invocable: false,
            argument_hint: None,
            disable_model_invocation: false,
        }
    }

    /// Set the `when_to_use` field.
    pub fn with_when_to_use(mut self, hint: &str) -> Self {
        self.when_to_use = Some(hint.to_string());
        self
    }

    /// Set allowed tools.
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = tools;
        self
    }

    /// Mark as user-invocable with an optional argument hint.
    pub fn with_user_invocable(mut self, hint: Option<&str>) -> Self {
        self.user_invocable = true;
        self.argument_hint = hint.map(|s| s.to_string());
        self
    }

    /// Rough token count estimate (~4 chars per token).
    pub fn rough_token_estimate(&self) -> usize {
        self.body.len() / 4
    }
}
