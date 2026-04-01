//! Bootstrap and initialization sequence.
//!
//! This module mirrors the TypeScript `setup.ts` + `entrypoints/init.ts` +
//! `bootstrap/state.ts` initialization flow, producing a fully configured
//! [`AppState`] ready for the query engine and TUI.
//!
//! The sequence:
//! 1. Detect project root and resolve CWD
//! 2. Load and merge settings (user → project → local → env)
//! 3. Resolve authentication
//! 4. Load MCP config and start MCP servers
//! 5. Discover skills and plugins
//! 6. Load hooks from settings
//! 7. Create session (or resume)
//! 8. Return fully initialized `AppState`

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::auth::resolve::{resolve_auth, AuthResolution};
use crate::config::paths::{detect_project_root, user_settings_path};
use crate::config::settings::Settings;
use crate::hooks::{HookEvent, HookRegistry, HookSource, IndividualHookConfig};
use crate::mcp::{load_mcp_config, McpManager};
use crate::permissions::types::PermissionMode;
use crate::plugins::loader::discover_plugins;
use crate::plugins::PluginRegistry;
use crate::session::SessionManager;
use crate::skills::loader::discover_all_skills;
use crate::skills::SkillRegistry;
use crate::state::{AppState, AppStateStore};

// ── Settings loading ────────────────────────────────────────────────────────

/// Load settings from a JSON file, returning `Settings::default()` if the
/// file does not exist or is malformed.
fn load_settings_file(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(data) => match serde_json::from_str::<Settings>(&data) {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to parse settings at {}: {e}", path.display());
                Settings::default()
            }
        },
        Err(_) => Settings::default(),
    }
}

/// Load and merge settings from all sources in precedence order:
/// 1. User-level: `~/.claude/settings.json`
/// 2. Project-level: `{project_root}/.claude/settings.json`
/// 3. Project-local: `{project_root}/.claude/settings.local.json`
///
/// Each layer overrides the previous via the `Settings::merge()` JSON-level
/// deep merge. Environment variables are applied after merge.
pub fn load_settings(project_root: &Path) -> Settings {
    // 1. User settings
    let user = user_settings_path()
        .map(|p| load_settings_file(&p))
        .unwrap_or_default();

    // 2. Project settings
    let project_settings_path = project_root.join(".claude").join("settings.json");
    let project = load_settings_file(&project_settings_path);

    // 3. Project-local settings (gitignored)
    let local_settings_path = project_root.join(".claude").join("settings.local.json");
    let local = load_settings_file(&local_settings_path);

    // Merge: user < project < local (using the existing JSON-level merge)
    let merged = user.merge(&project);
    let merged = merged.merge(&local);

    // Apply environment variable overrides
    apply_env_overrides(merged)
}

/// Apply environment variable overrides to settings.
fn apply_env_overrides(mut settings: Settings) -> Settings {
    if let Ok(model) = std::env::var("CLAUDE_MODEL") {
        if !model.is_empty() {
            settings.model = Some(model);
        }
    }
    if let Ok(val) = std::env::var("CLAUDE_CODE_MAX_OUTPUT_TOKENS") {
        if let Ok(n) = val.parse::<u32>() {
            settings.max_tokens = Some(n);
        }
    }
    settings
}

// ── Bootstrap configuration ─────────────────────────────────────────────────

/// Configuration options for the bootstrap process, typically derived from
/// CLI arguments.
pub struct BootstrapConfig {
    /// Working directory override (--cd).
    pub working_dir: Option<PathBuf>,
    /// Model override (--model).
    pub model: Option<String>,
    /// Verbose mode (--verbose).
    pub verbose: bool,
    /// Permission mode (from --dangerously-skip-permissions and other flags).
    pub permission_mode: PermissionMode,
    /// Plan mode flag.
    pub plan_mode: bool,
    /// Brief mode (--brief).
    pub brief_mode: bool,
    /// Assistant mode (--assistant).
    pub assistant_mode: bool,
    /// Non-interactive / headless mode (prompt passed on CLI).
    pub non_interactive: bool,
    /// Session ID to resume.
    pub resume_session_id: Option<String>,
    /// Custom session ID.
    pub custom_session_id: Option<String>,
    /// Named agent (--agent).
    pub agent: Option<String>,
    /// Additional system prompt text (--append-system-prompt).
    pub append_system_prompt: Option<String>,
    /// Max conversation turns for non-interactive mode.
    pub max_turns: Option<u32>,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            working_dir: None,
            model: None,
            verbose: false,
            permission_mode: PermissionMode::Default,
            plan_mode: false,
            brief_mode: false,
            assistant_mode: false,
            non_interactive: false,
            resume_session_id: None,
            custom_session_id: None,
            agent: None,
            append_system_prompt: None,
            max_turns: None,
        }
    }
}

/// Result of the bootstrap process.
pub struct BootstrapResult {
    /// Fully initialized application state.
    pub state: AppStateStore,
    /// Resolved authentication method (needed by the API client).
    pub auth: AuthResolution,
    /// Session ID (new or resumed).
    pub session_id: String,
    /// Session memory content loaded from disk (if resuming and available).
    pub session_memory_content: Option<String>,
}

// ── Main initialization function ────────────────────────────────────────────

/// Initialize a Claude session: resolve all configuration, load registries,
/// and return a fully populated `AppState` wrapped in a thread-safe store.
///
/// This is the single entry point for both interactive (TUI) and
/// non-interactive (print/headless) modes.
pub async fn initialize(config: BootstrapConfig) -> Result<BootstrapResult> {
    let start = std::time::Instant::now();

    // ── 1. Resolve working directory ────────────────────────────────────
    let cwd = match &config.working_dir {
        Some(dir) => {
            std::env::set_current_dir(dir)
                .with_context(|| format!("set working dir: {}", dir.display()))?;
            dir.clone()
        }
        None => std::env::current_dir().context("get current dir")?,
    };
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    debug!("bootstrap: cwd = {}", cwd.display());

    // ── 2. Detect project root ──────────────────────────────────────────
    let project_root = detect_project_root(&cwd);
    info!("bootstrap: project_root = {}", project_root.display());

    // ── 3. Load and merge settings ──────────────────────────────────────
    let settings = load_settings(&project_root);
    debug!("bootstrap: settings loaded");

    // ── 4. Resolve model ────────────────────────────────────────────────
    let raw_model = config
        .model
        .clone()
        .or_else(|| settings.model.clone())
        .unwrap_or_else(|| crate::utils::model::OPUS_45.to_string());
    // Resolve aliases (e.g. "opus" → "claude-opus-4-6-20260401") and validate.
    let model = crate::utils::model::resolve_model_string(&raw_model);
    if let Some(warning) = crate::utils::model::get_model_deprecation_warning(&model) {
        tracing::warn!("{warning}");
    }
    info!("bootstrap: model = {model}");

    // ── 5. Resolve authentication ───────────────────────────────────────
    let auth = resolve_auth().await.unwrap_or(AuthResolution::None);
    debug!("bootstrap: auth resolved");

    // ── 6. Create or resume session ─────────────────────────────────────
    let session_dir = SessionManager::project_dir_for_cwd(&cwd.to_string_lossy());
    let session_manager = SessionManager::new(session_dir);

    let (session_id, resumed_session) = if let Some(ref resume_id) = config.resume_session_id {
        match session_manager.load_session(resume_id) {
            Ok(session) => {
                info!("bootstrap: resuming session {}", resume_id);
                (session.id.clone(), Some(session))
            }
            Err(e) => {
                warn!("failed to load session {resume_id}: {e}; starting new session");
                let s = session_manager.create_session()?;
                (s.id.clone(), None)
            }
        }
    } else if let Some(ref custom_id) = config.custom_session_id {
        (custom_id.clone(), None)
    } else {
        let s = session_manager.create_session()?;
        (s.id.clone(), None)
    };
    info!("bootstrap: session_id = {session_id}");

    // ── 7. Load MCP config and start servers ────────────────────────────
    let mcp_config = load_mcp_config(&project_root);
    let mcp_manager = match mcp_config {
        Ok(cfg) => {
            if cfg.servers.is_empty() {
                debug!("bootstrap: no MCP servers configured");
                None
            } else {
                info!("bootstrap: {} MCP servers configured", cfg.servers.len());
                let mgr = McpManager::new();
                if let Err(e) = mgr.start_servers(&cfg).await {
                    warn!("failed to start MCP servers: {e}");
                }
                Some(mgr)
            }
        }
        Err(e) => {
            warn!("failed to load MCP config: {e}");
            None
        }
    };

    // ── 8. Discover skills ──────────────────────────────────────────────
    let skills = discover_all_skills(Some(&project_root));
    let skill_count = skills.len();
    let skill_registry = SkillRegistry::from_skills(skills);
    debug!("bootstrap: {skill_count} skills discovered");

    // ── 9. Discover plugins ─────────────────────────────────────────────
    let plugins = discover_plugins(Some(&project_root));
    let plugin_count = plugins.len();
    let plugin_registry = PluginRegistry::from_plugins(plugins);
    debug!("bootstrap: {plugin_count} plugins discovered");

    // ── 10. Load hooks ──────────────────────────────────────────────────
    let hook_registry = build_hook_registry(&settings, &project_root);
    debug!("bootstrap: hooks loaded");

    // ── 11. Build AppState ──────────────────────────────────────────────
    let result_session_id = session_id.clone();
    let mut state = AppState::new(
        session_id,
        project_root,
        cwd,
        settings,
        model,
        session_manager,
    );

    // Apply CLI flags
    state.verbose = config.verbose;
    state.permission_mode = config.permission_mode;
    state.plan_mode = config.plan_mode;
    state.brief_mode = config.brief_mode;
    state.assistant_mode = config.assistant_mode;
    state.is_non_interactive = config.non_interactive;
    state.agent_name = config.agent;
    state.fast_mode = state.settings.fast_mode.unwrap_or(false);
    state.thinking_enabled = state.settings.always_thinking_enabled.unwrap_or(true);
    state.prompt_suggestion_enabled = state.settings.prompt_suggestion_enabled.unwrap_or(false);

    // Wire registries
    state.hook_registry = hook_registry;
    state.skill_registry = skill_registry;
    state.plugin_registry = plugin_registry;
    state.mcp_manager = mcp_manager;

    // Restore session state if resuming
    if let Some(session) = &resumed_session {
        let snapshot = crate::cost_tracker::StoredCostState {
            total_cost_usd: session.total_cost,
            ..Default::default()
        };
        state.cost_tracker.restore(&snapshot);
    }

    let elapsed = start.elapsed();
    info!("bootstrap: initialized in {}ms", elapsed.as_millis());

    // Set up session memory file (creates dir + permissions) and load content
    let session_memory_content = {
        use crate::services::session_memory;
        match session_memory::setup_session_memory_file(&result_session_id).await {
            Ok((_path, content)) => Some(content),
            Err(e) => {
                debug!("session memory setup: {e}");
                session_memory::load_session_memories(&result_session_id)
                    .await
                    .unwrap_or(None)
            }
        }
    };

    Ok(BootstrapResult {
        state: AppStateStore::new(state),
        auth,
        session_id: result_session_id,
        session_memory_content,
    })
}

/// Build the hook registry from merged settings.
///
/// Loads hooks from user-level and project-level settings. The settings
/// `hooks` field is `Option<HashMap<String, Vec<HookEntry>>>` but we need to
/// convert to `HookMatcher` format for the hook registry loader.
fn build_hook_registry(settings: &Settings, _project_root: &Path) -> HookRegistry {
    let mut registry = HookRegistry::new();

    // User-level hooks (from ~/.claude/settings.json)
    if let Ok(user_path) = user_settings_path() {
        let user_settings = load_settings_file(&user_path);
        load_hooks_into_registry(&mut registry, &user_settings, HookSource::UserSettings);
    }

    // Project-level hooks (from the merged settings which include project + local)
    load_hooks_into_registry(&mut registry, settings, HookSource::ProjectSettings);

    registry
}

/// Parse hooks from settings and register them into the hook registry.
///
/// The settings `hooks` field maps event names → `Vec<HookEntry>`. Each
/// `HookEntry` has a `hook_type` ("command", "url") and the relevant fields.
/// We convert these to `IndividualHookConfig` for the registry.
fn load_hooks_into_registry(registry: &mut HookRegistry, settings: &Settings, source: HookSource) {
    let hooks = match &settings.hooks {
        Some(h) => h,
        None => return,
    };

    for (event_name, entries) in hooks {
        let event = match event_name.parse::<HookEvent>() {
            Ok(e) => e,
            Err(err) => {
                warn!("ignoring unknown hook event {event_name:?}: {err}");
                continue;
            }
        };

        for entry in entries {
            let hook_cmd = match entry.hook_type.as_str() {
                "command" => {
                    if let Some(cmd) = &entry.command {
                        crate::hooks::HookCommand::Command {
                            command: cmd.clone(),
                            shell: "bash".to_string(),
                            condition: None,
                            timeout: entry.timeout,
                        }
                    } else {
                        continue;
                    }
                }
                "url" | "http" => {
                    if let Some(url) = &entry.url {
                        crate::hooks::HookCommand::Http {
                            url: url.clone(),
                            condition: None,
                            timeout: entry.timeout,
                        }
                    } else {
                        continue;
                    }
                }
                _ => {
                    warn!("unknown hook type {:?} for event {event_name}", entry.hook_type);
                    continue;
                }
            };

            registry.register(IndividualHookConfig {
                event,
                config: hook_cmd,
                matcher: None,
                source: source.clone(),
            });
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_settings_file_missing() {
        let settings = load_settings_file(Path::new("/nonexistent/settings.json"));
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn test_load_settings_file_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"model": "claude-opus-4-6"}"#).unwrap();
        let settings = load_settings_file(&path);
        assert_eq!(settings.model, Some("claude-opus-4-6".to_string()));
    }

    #[test]
    fn test_load_settings_file_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "not json").unwrap();
        let settings = load_settings_file(&path);
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn test_load_settings_project_layering() {
        let dir = tempfile::tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        // Project settings
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{"model": "project-model"}"#,
        )
        .unwrap();

        // Local override
        std::fs::write(
            claude_dir.join("settings.local.json"),
            r#"{"model": "local-model"}"#,
        )
        .unwrap();

        let settings = load_settings(dir.path());
        assert_eq!(settings.model, Some("local-model".to_string()));
    }

    #[test]
    fn test_apply_env_overrides_model() {
        std::env::set_var("CLAUDE_MODEL", "env-model");
        let settings = apply_env_overrides(Settings::default());
        assert_eq!(settings.model, Some("env-model".to_string()));
        std::env::remove_var("CLAUDE_MODEL");
    }
}
