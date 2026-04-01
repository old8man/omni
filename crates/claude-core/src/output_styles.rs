//! Output style system for controlling the tone and behavior of Claude's responses.
//!
//! This module provides:
//! - Built-in output styles (Normal, Explanatory, Learning, Concise, Formal)
//! - Loading custom styles from user config directories (`~/.claude/output-styles/`)
//! - Plugin output styles with optional force-for-plugin behavior
//! - An `OutputStyleRegistry` for managing and applying styles to system prompts

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Where an output style originates from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StyleSource {
    BuiltIn,
    Plugin,
    UserSettings,
    ProjectSettings,
    PolicySettings,
}

/// A single output style definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputStyle {
    /// Display name of the style.
    pub name: String,
    /// Short description shown in style pickers.
    pub description: String,
    /// The prompt suffix injected into the system prompt when this style is active.
    /// `None` means no extra prompt (the "Normal" / default style).
    pub prompt_suffix: Option<String>,
    /// Whether this is the default style.
    pub is_default: bool,
    /// When `true`, this plugin style is automatically applied when the plugin is
    /// enabled. Only meaningful for styles with `source == Plugin`.
    pub force_for_plugin: bool,
    /// Where this style comes from.
    pub source: StyleSource,
    /// When `true`, the standard coding instructions are preserved even though a
    /// custom prompt is injected.
    pub keep_coding_instructions: bool,
}

// ---------------------------------------------------------------------------
// Built-in style constants
// ---------------------------------------------------------------------------

/// The name used to refer to the default (no-op) style.
pub const DEFAULT_STYLE_NAME: &str = "default";

const EXPLANATORY_FEATURE_PROMPT: &str = r#"
## Insights
In order to encourage learning, before and after writing code, always provide brief educational explanations about implementation choices using (with backticks):
"`* Insight ----------------------------------------`
[2-3 key educational points]
`-------------------------------------------------`"

These insights should be included in the conversation, not in the codebase. You should generally focus on interesting insights that are specific to the codebase or the code you just wrote, rather than general programming concepts."#;

fn build_explanatory_style() -> OutputStyle {
    let prompt = format!(
        r#"You are an interactive CLI tool that helps users with software engineering tasks. In addition to software engineering tasks, you should provide educational insights about the codebase along the way.

You should be clear and educational, providing helpful explanations while remaining focused on the task. Balance educational content with task completion. When providing insights, you may exceed typical length constraints, but remain focused and relevant.

# Explanatory Style Active
{EXPLANATORY_FEATURE_PROMPT}"#
    );
    OutputStyle {
        name: "Explanatory".to_string(),
        description: "Claude explains its implementation choices and codebase patterns".to_string(),
        prompt_suffix: Some(prompt),
        is_default: false,
        force_for_plugin: false,
        source: StyleSource::BuiltIn,
        keep_coding_instructions: true,
    }
}

fn build_learning_style() -> OutputStyle {
    let prompt = format!(
        r#"You are an interactive CLI tool that helps users with software engineering tasks. In addition to software engineering tasks, you should help users learn more about the codebase through hands-on practice and educational insights.

You should be collaborative and encouraging. Balance task completion with learning by requesting user input for meaningful design decisions while handling routine implementation yourself.

# Learning Style Active
## Requesting Human Contributions
In order to encourage learning, ask the human to contribute 2-10 line code pieces when generating 20+ lines involving:
- Design decisions (error handling, data structures)
- Business logic with multiple valid approaches
- Key algorithms or interface definitions

**TodoList Integration**: If using a TodoList for the overall task, include a specific todo item like "Request human input on [specific decision]" when planning to request human input.

### Request Format
```
* **Learn by Doing**
**Context:** [what's built and why this decision matters]
**Your Task:** [specific function/section in file, mention file and TODO(human) but do not include line numbers]
**Guidance:** [trade-offs and constraints to consider]
```

### Key Guidelines
- Frame contributions as valuable design decisions, not busy work
- You must first add a TODO(human) section into the codebase with your editing tools before making the Learn by Doing request
- Make sure there is one and only one TODO(human) section in the code
- Don't take any action or output anything after the Learn by Doing request. Wait for human implementation before proceeding.

### After Contributions
Share one insight connecting their code to broader patterns or system effects. Avoid praise or repetition.

## Insights
{EXPLANATORY_FEATURE_PROMPT}"#
    );
    OutputStyle {
        name: "Learning".to_string(),
        description: "Claude pauses and asks you to write small pieces of code for hands-on practice".to_string(),
        prompt_suffix: Some(prompt),
        is_default: false,
        force_for_plugin: false,
        source: StyleSource::BuiltIn,
        keep_coding_instructions: true,
    }
}

fn build_concise_style() -> OutputStyle {
    OutputStyle {
        name: "Concise".to_string(),
        description: "Claude keeps responses brief and to the point".to_string(),
        prompt_suffix: Some(
            r#"You are an interactive CLI tool that helps users with software engineering tasks.

# Concise Style Active
Be brief and direct. Minimize explanations. Focus on code and commands. Use short sentences. Skip pleasantries and filler. Only explain when the user explicitly asks for an explanation. Prefer showing code over describing it."#
                .to_string(),
        ),
        is_default: false,
        force_for_plugin: false,
        source: StyleSource::BuiltIn,
        keep_coding_instructions: true,
    }
}

fn build_formal_style() -> OutputStyle {
    OutputStyle {
        name: "Formal".to_string(),
        description: "Claude uses a formal, professional tone".to_string(),
        prompt_suffix: Some(
            r#"You are an interactive CLI tool that helps users with software engineering tasks.

# Formal Style Active
Use a formal, professional tone throughout all responses. Write in complete sentences with proper grammar. Avoid colloquialisms, contractions, and casual language. Structure responses with clear headings and numbered lists where appropriate. Address the user respectfully and maintain a professional demeanor at all times."#
                .to_string(),
        ),
        is_default: false,
        force_for_plugin: false,
        source: StyleSource::BuiltIn,
        keep_coding_instructions: true,
    }
}

fn build_default_style() -> OutputStyle {
    OutputStyle {
        name: DEFAULT_STYLE_NAME.to_string(),
        description: "Standard Claude behavior with no extra style instructions".to_string(),
        prompt_suffix: None,
        is_default: true,
        force_for_plugin: false,
        source: StyleSource::BuiltIn,
        keep_coding_instructions: true,
    }
}

/// Return all built-in styles keyed by name.
fn built_in_styles() -> HashMap<String, OutputStyle> {
    let styles = vec![
        build_default_style(),
        build_explanatory_style(),
        build_learning_style(),
        build_concise_style(),
        build_formal_style(),
    ];
    styles.into_iter().map(|s| (s.name.clone(), s)).collect()
}

// ---------------------------------------------------------------------------
// Loading custom styles from disk
// ---------------------------------------------------------------------------

/// Load output styles from markdown files in a directory.
///
/// Each `.md` file in the directory becomes a style. The file name (minus
/// extension) is used as the style name, and the full file content is the
/// prompt. An optional YAML front-matter block can supply `name`,
/// `description`, and `keep-coding-instructions`.
///
/// Front-matter format:
/// ```text
/// ---
/// name: My Style
/// description: A cool style
/// keep-coding-instructions: true
/// ---
/// Actual prompt content here...
/// ```
pub fn load_styles_from_directory(dir: &Path) -> Result<Vec<OutputStyle>> {
    let mut styles = Vec::new();

    if !dir.is_dir() {
        debug!("Output styles directory does not exist: {}", dir.display());
        return Ok(styles);
    }

    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        match load_style_from_file(&path) {
            Ok(style) => styles.push(style),
            Err(e) => {
                warn!("Failed to load output style from {}: {e}", path.display());
            }
        }
    }

    Ok(styles)
}

/// Parse a single markdown file into an `OutputStyle`.
fn load_style_from_file(path: &Path) -> Result<OutputStyle> {
    let content = std::fs::read_to_string(path)?;
    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let (frontmatter, body) = parse_frontmatter(&content);

    let name = frontmatter
        .get("name")
        .cloned()
        .unwrap_or_else(|| file_stem.clone());

    let description = frontmatter
        .get("description")
        .cloned()
        .unwrap_or_else(|| format!("Custom {file_stem} output style"));

    let keep_coding_instructions = frontmatter
        .get("keep-coding-instructions")
        .map(|v| v == "true")
        .unwrap_or(false);

    // Determine source based on path: if inside ~/.claude use UserSettings,
    // if inside .claude in a project use ProjectSettings.
    let source = if path
        .to_string_lossy()
        .contains(".claude/output-styles")
    {
        // Check if it's in the home directory
        if let Some(home) = dirs::home_dir() {
            if path.starts_with(home.join(".claude")) {
                StyleSource::UserSettings
            } else {
                StyleSource::ProjectSettings
            }
        } else {
            StyleSource::ProjectSettings
        }
    } else {
        StyleSource::UserSettings
    };

    Ok(OutputStyle {
        name,
        description,
        prompt_suffix: Some(body.trim().to_string()),
        is_default: false,
        force_for_plugin: false,
        source,
        keep_coding_instructions,
    })
}

/// Simple YAML front-matter parser. Returns (key-value pairs, remaining body).
fn parse_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let mut map = HashMap::new();

    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (map, content.to_string());
    }

    // Find the closing "---"
    let after_open = &trimmed[3..];
    if let Some(close_pos) = after_open.find("\n---") {
        let frontmatter_block = &after_open[..close_pos];
        let body_start = 3 + close_pos + 4; // skip opening "---", frontmatter, "\n---"
        let body = &trimmed[body_start..];

        for line in frontmatter_block.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
        }

        (map, body.to_string())
    } else {
        // No closing delimiter -- treat entire content as body
        (map, content.to_string())
    }
}

// ---------------------------------------------------------------------------
// OutputStyleRegistry
// ---------------------------------------------------------------------------

/// Registry that manages all available output styles and determines the active style.
#[derive(Debug, Clone)]
pub struct OutputStyleRegistry {
    /// All known styles, keyed by name. Later insertions override earlier ones.
    styles: HashMap<String, OutputStyle>,
    /// The name of the currently active style. Defaults to `DEFAULT_STYLE_NAME`.
    active_style: String,
}

impl OutputStyleRegistry {
    /// Create a new registry populated with the built-in styles.
    pub fn new() -> Self {
        Self {
            styles: built_in_styles(),
            active_style: DEFAULT_STYLE_NAME.to_string(),
        }
    }

    /// Load user styles from `~/.claude/output-styles/`.
    pub fn load_user_styles(&mut self) -> Result<usize> {
        let dir = match dirs::home_dir() {
            Some(home) => home.join(".claude").join("output-styles"),
            None => return Ok(0),
        };
        let styles = load_styles_from_directory(&dir)?;
        let count = styles.len();
        for style in styles {
            self.styles.insert(style.name.clone(), style);
        }
        Ok(count)
    }

    /// Load project-local styles from `<cwd>/.claude/output-styles/`.
    pub fn load_project_styles(&mut self, cwd: &Path) -> Result<usize> {
        let dir = cwd.join(".claude").join("output-styles");
        let styles = load_styles_from_directory(&dir)?;
        let count = styles.len();
        for mut style in styles {
            style.source = StyleSource::ProjectSettings;
            self.styles.insert(style.name.clone(), style);
        }
        Ok(count)
    }

    /// Register plugin output styles.
    pub fn add_plugin_styles(&mut self, styles: Vec<OutputStyle>) {
        for mut style in styles {
            style.source = StyleSource::Plugin;
            debug!("Registered plugin output style: {}", style.name);
            self.styles.insert(style.name.clone(), style);
        }
    }

    /// Set the active style by name. Returns `false` if the name is unknown.
    pub fn set_active(&mut self, name: &str) -> bool {
        if self.styles.contains_key(name) {
            self.active_style = name.to_string();
            true
        } else {
            warn!("Unknown output style: {name}");
            false
        }
    }

    /// Get the currently active output style.
    ///
    /// If a plugin has `force_for_plugin` set, that style takes precedence
    /// over the user's selection.
    pub fn active_style(&self) -> &OutputStyle {
        // Check for forced plugin styles first.
        let forced: Vec<&OutputStyle> = self
            .styles
            .values()
            .filter(|s| s.source == StyleSource::Plugin && s.force_for_plugin)
            .collect();

        if let Some(first) = forced.first() {
            if forced.len() > 1 {
                warn!(
                    "Multiple plugins have forced output styles: {}. Using: {}",
                    forced.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "),
                    first.name
                );
            }
            debug!("Using forced plugin output style: {}", first.name);
            return first;
        }

        self.styles
            .get(&self.active_style)
            .unwrap_or_else(|| self.styles.get(DEFAULT_STYLE_NAME).unwrap())
    }

    /// Get the name of the active style.
    pub fn active_style_name(&self) -> &str {
        &self.active_style
    }

    /// Whether a non-default output style is active.
    pub fn has_custom_style(&self) -> bool {
        self.active_style != DEFAULT_STYLE_NAME
    }

    /// Return a sorted list of all available style names.
    pub fn available_styles(&self) -> Vec<String> {
        let mut names: Vec<String> = self.styles.keys().cloned().collect();
        names.sort();
        names
    }

    /// Return all styles.
    pub fn all_styles(&self) -> &HashMap<String, OutputStyle> {
        &self.styles
    }

    /// Apply the active output style to a system prompt.
    ///
    /// If the active style has a `prompt_suffix`, it is appended to the base
    /// system prompt (separated by two newlines). If `keep_coding_instructions`
    /// is `false` on the style, the base prompt is *replaced* entirely.
    ///
    /// Returns the final system prompt text.
    pub fn apply_to_system_prompt(&self, base_system_prompt: &str) -> String {
        let style = self.active_style();

        match &style.prompt_suffix {
            None => base_system_prompt.to_string(),
            Some(suffix) => {
                if style.keep_coding_instructions {
                    format!("{base_system_prompt}\n\n{suffix}")
                } else {
                    suffix.clone()
                }
            }
        }
    }

    /// Get a specific style by name.
    pub fn get_style(&self, name: &str) -> Option<&OutputStyle> {
        self.styles.get(name)
    }

    /// Register a single custom style.
    pub fn register_style(&mut self, style: OutputStyle) {
        self.styles.insert(style.name.clone(), style);
    }

    /// Convenience: load all styles from standard locations.
    ///
    /// Loads user styles from `~/.claude/output-styles/` and project styles
    /// from `<cwd>/.claude/output-styles/`. Project styles override user styles
    /// which override built-in styles.
    pub fn load_all(&mut self, cwd: &Path) -> Result<()> {
        let user_count = self.load_user_styles()?;
        let project_count = self.load_project_styles(cwd)?;
        debug!(
            "Loaded output styles: {} user, {} project, {} built-in",
            user_count,
            project_count,
            built_in_styles().len()
        );
        Ok(())
    }
}

impl Default for OutputStyleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_built_in_styles_exist() {
        let registry = OutputStyleRegistry::new();
        let names = registry.available_styles();
        assert!(names.contains(&"default".to_string()));
        assert!(names.contains(&"Explanatory".to_string()));
        assert!(names.contains(&"Learning".to_string()));
        assert!(names.contains(&"Concise".to_string()));
        assert!(names.contains(&"Formal".to_string()));
    }

    #[test]
    fn test_default_style_is_active() {
        let registry = OutputStyleRegistry::new();
        assert_eq!(registry.active_style_name(), DEFAULT_STYLE_NAME);
        assert!(registry.active_style().is_default);
        assert!(!registry.has_custom_style());
    }

    #[test]
    fn test_set_active_style() {
        let mut registry = OutputStyleRegistry::new();
        assert!(registry.set_active("Concise"));
        assert_eq!(registry.active_style_name(), "Concise");
        assert_eq!(registry.active_style().name, "Concise");
        assert!(registry.has_custom_style());
    }

    #[test]
    fn test_set_unknown_style_returns_false() {
        let mut registry = OutputStyleRegistry::new();
        assert!(!registry.set_active("Nonexistent"));
        assert_eq!(registry.active_style_name(), DEFAULT_STYLE_NAME);
    }

    #[test]
    fn test_apply_default_style_passes_through() {
        let registry = OutputStyleRegistry::new();
        let base = "You are a helpful assistant.";
        assert_eq!(registry.apply_to_system_prompt(base), base);
    }

    #[test]
    fn test_apply_concise_style_appends() {
        let mut registry = OutputStyleRegistry::new();
        registry.set_active("Concise");
        let base = "You are a helpful assistant.";
        let result = registry.apply_to_system_prompt(base);
        assert!(result.starts_with(base));
        assert!(result.contains("Concise Style Active"));
    }

    #[test]
    fn test_apply_style_replaces_when_keep_false() {
        let mut registry = OutputStyleRegistry::new();
        let style = OutputStyle {
            name: "Replace".to_string(),
            description: "Replaces entire prompt".to_string(),
            prompt_suffix: Some("Totally new prompt.".to_string()),
            is_default: false,
            force_for_plugin: false,
            source: StyleSource::BuiltIn,
            keep_coding_instructions: false,
        };
        registry.register_style(style);
        registry.set_active("Replace");

        let result = registry.apply_to_system_prompt("Original prompt.");
        assert_eq!(result, "Totally new prompt.");
    }

    #[test]
    fn test_forced_plugin_style_overrides_active() {
        let mut registry = OutputStyleRegistry::new();
        registry.set_active("Concise");

        let plugin_style = OutputStyle {
            name: "PluginForced".to_string(),
            description: "Forced plugin style".to_string(),
            prompt_suffix: Some("Plugin prompt.".to_string()),
            is_default: false,
            force_for_plugin: true,
            source: StyleSource::Plugin,
            keep_coding_instructions: true,
        };
        registry.register_style(plugin_style);

        let active = registry.active_style();
        assert_eq!(active.name, "PluginForced");
    }

    #[test]
    fn test_parse_frontmatter_valid() {
        let content = "---\nname: TestStyle\ndescription: A test\nkeep-coding-instructions: true\n---\nPrompt body here.";
        let (fm, body) = parse_frontmatter(content);
        assert_eq!(fm.get("name").unwrap(), "TestStyle");
        assert_eq!(fm.get("description").unwrap(), "A test");
        assert_eq!(fm.get("keep-coding-instructions").unwrap(), "true");
        assert!(body.contains("Prompt body here."));
    }

    #[test]
    fn test_parse_frontmatter_missing() {
        let content = "Just a prompt with no frontmatter.";
        let (fm, body) = parse_frontmatter(content);
        assert!(fm.is_empty());
        assert_eq!(body, content);
    }

    #[test]
    fn test_load_styles_from_directory() {
        let dir = tempfile::tempdir().unwrap();

        // Write a style file
        let style_path = dir.path().join("MyStyle.md");
        fs::write(
            &style_path,
            "---\nname: My Custom Style\ndescription: Custom desc\n---\nBe very custom.",
        )
        .unwrap();

        // Write a non-md file (should be ignored)
        fs::write(dir.path().join("notes.txt"), "not a style").unwrap();

        let styles = load_styles_from_directory(dir.path()).unwrap();
        assert_eq!(styles.len(), 1);
        assert_eq!(styles[0].name, "My Custom Style");
        assert_eq!(styles[0].description, "Custom desc");
        assert_eq!(
            styles[0].prompt_suffix.as_deref(),
            Some("Be very custom.")
        );
    }

    #[test]
    fn test_load_styles_from_nonexistent_directory() {
        let styles =
            load_styles_from_directory(Path::new("/tmp/nonexistent_output_styles_dir_12345"))
                .unwrap();
        assert!(styles.is_empty());
    }

    #[test]
    fn test_register_custom_style() {
        let mut registry = OutputStyleRegistry::new();
        let initial_count = registry.available_styles().len();

        registry.register_style(OutputStyle {
            name: "Custom".to_string(),
            description: "A custom style".to_string(),
            prompt_suffix: Some("Be custom.".to_string()),
            is_default: false,
            force_for_plugin: false,
            source: StyleSource::UserSettings,
            keep_coding_instructions: true,
        });

        assert_eq!(registry.available_styles().len(), initial_count + 1);
        assert!(registry.set_active("Custom"));
        assert_eq!(registry.active_style().description, "A custom style");
    }

    #[test]
    fn test_add_plugin_styles() {
        let mut registry = OutputStyleRegistry::new();
        let plugin_style = OutputStyle {
            name: "PluginStyle".to_string(),
            description: "From a plugin".to_string(),
            prompt_suffix: Some("Plugin prompt".to_string()),
            is_default: false,
            force_for_plugin: false,
            source: StyleSource::BuiltIn, // will be overridden
            keep_coding_instructions: true,
        };
        registry.add_plugin_styles(vec![plugin_style]);

        let style = registry.get_style("PluginStyle").unwrap();
        assert_eq!(style.source, StyleSource::Plugin);
    }

    #[test]
    fn test_explanatory_style_content() {
        let registry = OutputStyleRegistry::new();
        let style = registry.get_style("Explanatory").unwrap();
        assert!(style.prompt_suffix.as_ref().unwrap().contains("Insight"));
        assert!(style.keep_coding_instructions);
    }

    #[test]
    fn test_learning_style_content() {
        let registry = OutputStyleRegistry::new();
        let style = registry.get_style("Learning").unwrap();
        let prompt = style.prompt_suffix.as_ref().unwrap();
        assert!(prompt.contains("Learn by Doing"));
        assert!(prompt.contains("TODO(human)"));
        assert!(style.keep_coding_instructions);
    }
}
