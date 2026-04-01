mod engine_loop;
mod event_handler;
mod renderer;

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CrosstermEvent, KeyCode, KeyEvent,
    KeyModifiers, MouseEvent,
};
use crossterm::{cursor, execute};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use omni_core::commands::{CommandContext, CommandRegistry, CommandResult};
use omni_core::permissions::types::{PermissionMode, ToolPermissionContext};
use omni_core::query::engine::{QueryEngine, TurnResult};
use omni_core::services::lsp_service::LspManager;
use omni_core::services::notifications::{self, NotificationChannel, NotificationOptions};
use omni_core::services::prompt_suggestion;
use omni_core::services::tips::{self, TipScheduler};
use omni_core::services::tool_use_summary::{self, ToolInfo as SummaryToolInfo};
use omni_core::types::events::StreamEvent;
use omni_tools::{ToolRegistry, ToolUseContext};

use crate::input::{InputHandler, InputResult};
use crate::mouse::{FocusManager, FocusTarget, TextSelection};
use crate::theme::{detect_theme, Theme};
use crate::widgets::message_list::{MessageEntry, MessageList, SystemSeverity};
use crate::widgets::notification::NotificationManager;
use crate::widgets::permission_dialog::PermissionDialog;
use crate::widgets::prompt_input::PromptInput;
use crate::widgets::search_overlay::{SearchAction, SearchOverlay};
use crate::widgets::spinner::{SpinnerMode, SpinnerState, SPINNER_TICK_MS};

use engine_loop::{execute_tool, truncate_result};

pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    Tick,
    SpinnerTick,
    Quit,
    Stream(StreamEvent),
    SubmitPrompt(String),
    PermissionResponse(String),
    /// Fired after all tool results have been added -- tells the main loop to
    /// call `engine.run_turn()` and handle the next result (which may be
    /// another round of tool use or a final answer).
    ContinueTurn,
    /// Result of a background `engine.run_turn()` call.
    TurnComplete(Result<TurnResult, anyhow::Error>),
    /// The manual OAuth URL is available — update the dialog.
    LoginOAuthUrl(String),
    /// Result of a background OAuth login attempt.
    LoginOAuthResult(Result<String, String>),
    /// Result of an API key validation/save attempt.
    LoginApiKeyResult(Result<String, String>),
}

/// Pending tool that needs permission before execution.
pub(crate) struct PendingTool {
    pub(crate) info: omni_core::query::engine::ToolUseInfo,
}

/// State for the Ctrl+E message action popup menu.
pub(crate) struct ActionMenu {
    /// The assistant message text being acted upon.
    pub(crate) text: String,
    /// The message index in the message list.
    pub(crate) msg_index: usize,
}

pub struct App {
    pub(crate) terminal: DefaultTerminal,
    pub(crate) theme: Theme,
    pub(crate) spinner: SpinnerState,
    pub(crate) should_quit: bool,
    pub(crate) message_list: MessageList,
    pub(crate) prompt: PromptInput,
    /// Input handler (vim mode + emacs mode routing)
    pub(crate) input_handler: InputHandler,
    pub(crate) permission_dialog: Option<PermissionDialog>,
    /// True while the engine is processing (prevents double-submit)
    pub(crate) engine_busy: bool,
    /// Model name for display in the header
    pub(crate) model_name: String,
    /// Running total of tokens used in this session
    pub(crate) total_tokens: u64,
    /// Running total cost in USD this session
    pub(crate) total_cost: f64,
    /// Slash-command registry
    pub(crate) command_registry: CommandRegistry,
    /// Whether vim mode is enabled
    pub(crate) vim_mode: bool,
    /// Whether plan mode is enabled
    pub(crate) plan_mode: bool,
    /// Search overlay state
    pub(crate) search_overlay: SearchOverlay,
    /// Text selection state (for mouse selection)
    pub(crate) selection: TextSelection,
    /// Focus manager
    pub(crate) focus: FocusManager,
    /// Notification manager
    pub(crate) notifications: NotificationManager,
    /// Pending input from Prompt-type commands (injected into the next turn)
    pub(crate) pending_input: Option<String>,
    /// Message action menu state (Ctrl+E popup)
    pub(crate) action_menu: Option<ActionMenu>,
    /// Tip scheduler for showing tips in the spinner
    pub(crate) tip_scheduler: TipScheduler,
    /// Terminal notification channel (auto-detect)
    pub(crate) notification_channel: NotificationChannel,
    /// Accumulated tool infos for summary generation in the current turn
    pub(crate) turn_tool_infos: Vec<SummaryToolInfo>,
    /// LSP manager for notifying language servers of file changes
    pub(crate) lsp_manager: LspManager,
    /// Shared application state (for live cost/usage display from engine).
    pub(crate) app_state: Option<omni_core::state::AppStateStore>,
    /// Flash message for status bar (auto-dismissing).
    pub(crate) flash_message: Option<crate::widgets::status_bar::FlashMessage>,
    /// Tracks when the first exit key (Ctrl+C or Ctrl+D) was pressed for double-press confirmation.
    pub(crate) exit_pending: Option<std::time::Instant>,
    /// Timestamp of the last mouse click (for double/triple-click detection).
    pub(crate) last_click_time: Option<Instant>,
    /// Position of the last mouse click (for double/triple-click detection).
    pub(crate) last_click_pos: (u16, u16),
    /// Click count in the current multi-click sequence (1 = single, 2 = double, 3 = triple).
    pub(crate) click_count: u8,
    /// Pending chord keystroke for multi-key shortcuts (e.g. Ctrl+X followed by Ctrl+E).
    pub(crate) pending_chord: Option<(KeyModifiers, KeyCode)>,
    /// Whether the cost threshold warning has been shown this session.
    pub(crate) cost_warning_shown: bool,
    /// Cost threshold in USD that triggers a warning (default $5).
    pub(crate) cost_warning_threshold: f64,
    /// Whether the context window warning has been shown this session.
    pub(crate) context_warning_shown: bool,
    /// Interactive config panel overlay (opened via /config).
    pub(crate) config_panel: Option<crate::widgets::config_panel::ConfigPanel>,
    /// Active picker overlay (model, theme, or session selector).
    pub(crate) active_picker: Option<crate::widgets::picker::ActivePicker>,
    /// Profile manager overlay (opened via /profile).
    pub(crate) profile_manager: Option<crate::widgets::profile_manager::ProfileManager>,
    /// Login dialog overlay (opened via /login).
    pub(crate) login_dialog: Option<crate::widgets::login_dialog::LoginDialog>,
    /// Status dialog overlay (opened via /status).
    pub(crate) status_dialog: Option<crate::widgets::status_dialog::StatusDialog>,
    /// Generic info dialog overlay (opened by /help, /usage, /doctor, etc.).
    pub(crate) info_dialog: Option<crate::widgets::info_dialog::InfoDialog>,
    /// Display name of the active profile (shown in header).
    pub(crate) active_profile_name: Option<String>,
    /// Set to true after a clean shutdown so the Drop impl skips the second restore.
    pub(crate) cleaned_up: bool,
}

impl App {
    /// Restore the terminal to its original state.
    ///
    /// Safe to call more than once — subsequent calls are no-ops.
    pub(crate) fn cleanup(&mut self) {
        if self.cleaned_up {
            return;
        }
        self.cleaned_up = true;

        // Disable raw mode first (tcsetattr syscall — synchronous, takes effect immediately).
        let _ = crossterm::terminal::disable_raw_mode();

        // Send ANSI sequences: leave alternate screen, show cursor, disable mouse capture.
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            cursor::Show,
            DisableMouseCapture,
        );

        // Flush all pending output.
        let _ = std::io::Write::flush(&mut io::stdout());
    }

    /// Show a flash message in the status bar (auto-dismisses).
    pub(crate) fn flash(&mut self, msg: crate::widgets::status_bar::FlashMessage) {
        self.flash_message = Some(msg);
    }

    /// Show an info flash in status bar.
    pub(crate) fn flash_info(&mut self, text: impl Into<String>) {
        self.flash(crate::widgets::status_bar::FlashMessage::info(text));
    }

    /// Show a success flash in status bar.
    pub(crate) fn flash_success(&mut self, text: impl Into<String>) {
        self.flash(crate::widgets::status_bar::FlashMessage::success(text));
    }

    pub fn new() -> Result<Self> {
        // ratatui::try_init() handles raw mode, alternate screen, and panic hook
        let terminal = ratatui::try_init()?;
        // Mouse capture and cursor hiding are not covered by ratatui::init()
        execute!(io::stdout(), EnableMouseCapture, cursor::Hide)?;
        Ok(Self {
            terminal,
            theme: detect_theme(),
            spinner: SpinnerState::new(),
            should_quit: false,
            message_list: MessageList::new(),
            prompt: PromptInput::new(),
            input_handler: InputHandler::new(),
            permission_dialog: None,
            engine_busy: false,
            model_name: "claude-sonnet-4-6".to_string(),
            total_tokens: 0,
            total_cost: 0.0,
            command_registry: CommandRegistry::default_registry(),
            vim_mode: false,
            plan_mode: false,
            search_overlay: SearchOverlay::new(),
            selection: TextSelection::new(),
            focus: FocusManager::new(),
            notifications: NotificationManager::new(),
            pending_input: None,
            action_menu: None,
            tip_scheduler: {
                let mut registry = tips::TipRegistry::new();
                registry.register_all(tips::default_tips());
                let history = tips::TipHistory::new(0);
                TipScheduler::new(registry, history)
            },
            notification_channel: NotificationChannel::Auto,
            turn_tool_infos: Vec::new(),
            lsp_manager: LspManager::new(),
            app_state: None,
            flash_message: None,
            exit_pending: None,
            last_click_time: None,
            last_click_pos: (0, 0),
            click_count: 0,
            pending_chord: None,
            cost_warning_shown: false,
            cost_warning_threshold: 5.0,
            context_warning_shown: false,
            config_panel: None,
            active_picker: None,
            profile_manager: None,
            login_dialog: None,
            status_dialog: None,
            info_dialog: None,
            active_profile_name: omni_core::auth::profiles::get_active_profile()
                .map(|p| p.display_name()),
            cleaned_up: false,
        })
    }

    /// Set the model name displayed in the header.
    pub fn set_model_name(&mut self, name: &str) {
        self.model_name = name.to_string();
    }

    /// Attach shared application state for live cost display.
    pub fn set_app_state(&mut self, state: omni_core::state::AppStateStore) {
        self.app_state = Some(state);
    }

    /// Original standalone run loop (no engine). Kept for backwards compatibility.
    pub async fn run(&mut self) -> Result<()> {
        let (tx, mut rx) = mpsc::channel::<AppEvent>(100);

        // Spawn input reader
        let tx_input = tx.clone();
        tokio::spawn(async move {
            loop {
                if event::poll(Duration::from_millis(16)).unwrap_or(false) {
                    if let Ok(evt) = event::read() {
                        let app_evt = match evt {
                            CrosstermEvent::Key(k) => Some(AppEvent::Key(k)),
                            CrosstermEvent::Mouse(m) => Some(AppEvent::Mouse(m)),
                            CrosstermEvent::Resize(w, h) => Some(AppEvent::Resize(w, h)),
                            _ => None,
                        };
                        if let Some(e) = app_evt {
                            if tx_input.send(e).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Spawn render tick (60fps)
        let tx_tick = tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(16));
            loop {
                interval.tick().await;
                if tx_tick.send(AppEvent::Tick).await.is_err() {
                    break;
                }
            }
        });

        // Spawn spinner tick (50ms)
        let tx_spinner = tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(SPINNER_TICK_MS));
            loop {
                interval.tick().await;
                if tx_spinner.send(AppEvent::SpinnerTick).await.is_err() {
                    break;
                }
            }
        });

        while !self.should_quit {
            if let Some(event) = rx.recv().await {
                match event {
                    AppEvent::Tick => self.render()?,
                    AppEvent::SpinnerTick => self.spinner.advance(),
                    AppEvent::Key(k) => self.handle_key_standalone(k),
                    AppEvent::Mouse(m) => self.handle_mouse(m),
                    AppEvent::Resize(_, _) => self.render()?,
                    AppEvent::Quit => self.should_quit = true,
                    _ => {}
                }
            }
        }

        self.cleanup();
        Ok(())
    }

    /// Run the TUI wired to the QueryEngine.
    pub async fn run_with_engine(
        &mut self,
        engine: QueryEngine,
        tools: ToolRegistry,
        cancel: CancellationToken,
        permission_mode: PermissionMode,
    ) -> Result<()> {
        let engine = std::sync::Arc::new(tokio::sync::Mutex::new(engine));

        let (tx, mut rx) = mpsc::channel::<AppEvent>(256);

        // Spawn input reader — keep handle so we can abort it on exit.
        // event::poll() is a blocking syscall; without abort() the tokio runtime
        // would wait up to the poll timeout before the process could exit, which
        // causes the shell prompt to not appear until the user presses Enter.
        let tx_input = tx.clone();
        let input_task = tokio::spawn(async move {
            loop {
                if event::poll(Duration::from_millis(16)).unwrap_or(false) {
                    if let Ok(evt) = event::read() {
                        let app_evt = match evt {
                            CrosstermEvent::Key(k) => Some(AppEvent::Key(k)),
                            CrosstermEvent::Mouse(m) => Some(AppEvent::Mouse(m)),
                            CrosstermEvent::Resize(w, h) => Some(AppEvent::Resize(w, h)),
                            _ => None,
                        };
                        if let Some(e) = app_evt {
                            if tx_input.send(e).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Spawn render tick (60fps)
        let tx_tick = tx.clone();
        let tick_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(16));
            loop {
                interval.tick().await;
                if tx_tick.send(AppEvent::Tick).await.is_err() {
                    break;
                }
            }
        });

        // Spawn spinner tick (50ms)
        let tx_spinner = tx.clone();
        let spinner_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(SPINNER_TICK_MS));
            loop {
                interval.tick().await;
                if tx_spinner.send(AppEvent::SpinnerTick).await.is_err() {
                    break;
                }
            }
        });

        let cwd = if let Some(ref state) = self.app_state {
            state.read().cwd.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };

        // Wire streaming tool execution into the engine
        {
            let tool_map: std::sync::Arc<
                std::collections::HashMap<String, std::sync::Arc<dyn omni_tools::ToolExecutor>>,
            > = std::sync::Arc::new(
                tools
                    .all()
                    .into_iter()
                    .map(|t| (t.name().to_string(), t))
                    .collect(),
            );
            let cwd_clone = cwd.clone();
            let tool_call_fn: omni_core::query::tool_executor::ToolCallFn =
                std::sync::Arc::new(
                    move |name: String,
                          _id: String,
                          input: serde_json::Value,
                          cancel_tok: CancellationToken| {
                        let tool_map = std::sync::Arc::clone(&tool_map);
                        let cwd = cwd_clone.clone();
                        tokio::spawn(async move {
                            let ctx = ToolUseContext::with_working_directory(cwd);
                            match tool_map.get(&name) {
                                Some(exec) => exec.call(&input, &ctx, cancel_tok, None).await,
                                None => Ok(omni_core::types::events::ToolResultData {
                                    data: serde_json::json!(format!("Unknown tool: {}", name)),
                                    is_error: true,
                                }),
                            }
                        })
                    },
                );
            engine.lock().await.set_tool_call_fn(tool_call_fn);
        }

        let perm_ctx = ToolPermissionContext {
            mode: permission_mode,
            ..Default::default()
        };

        // Tools waiting for permission resolution
        let mut pending_tools: Vec<PendingTool> = Vec::new();
        // Current index into pending_tools when walking through permission dialogs
        let mut pending_tool_index: usize = 0;

        // Main event loop
        while !self.should_quit {
            let Some(event) = rx.recv().await else {
                break;
            };

            match event {
                AppEvent::Tick => {
                    self.render()?;
                }
                AppEvent::SpinnerTick => {
                    self.spinner.advance();
                    // Also advance login dialog spinner
                    if let Some(ref mut dialog) = self.login_dialog {
                        dialog.tick();
                    }
                }
                AppEvent::Mouse(mouse) => {
                    self.handle_mouse(mouse);
                }
                AppEvent::Resize(_, _) => {
                    self.render()?;
                }
                AppEvent::Quit => {
                    self.should_quit = true;
                }
                AppEvent::Key(k) => {
                    // --- Config panel overlay intercepts all keys ---
                    if self.config_panel.is_some() {
                        use crate::widgets::config_panel::ConfigPanelAction;
                        let action = self.config_panel.as_mut().unwrap().handle_key(k.code, k.modifiers);
                        match action {
                            ConfigPanelAction::Consumed => {}
                            ConfigPanelAction::Close { changes } => {
                                // Apply changes from config panel.
                                for (key, val) in &changes {
                                    match key.as_str() {
                                        "editor_mode" => {
                                            let new_vim = val == "vim";
                                            self.vim_mode = new_vim;
                                            self.input_handler.set_vim_enabled(new_vim);
                                            if let Some(ref state) = self.app_state {
                                                state.write().vim_mode = new_vim;
                                            }
                                        }
                                        "model" => {
                                            self.model_name = val.clone();
                                            if let Some(ref state) = self.app_state {
                                                state.write().set_model_override(Some(val.clone()));
                                            }
                                        }
                                        "default_permission_mode" => {
                                            let is_plan = val == "plan";
                                            self.plan_mode = is_plan;
                                            if let Some(ref state) = self.app_state {
                                                state.write().set_plan_mode(is_plan);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                if !changes.is_empty() {
                                    let summary: Vec<String> = changes
                                        .iter()
                                        .map(|(k, v)| format!("{} = {}", k, v))
                                        .collect();
                                    self.flash_success(format!("Config updated: {}", summary.join(", ")));
                                }
                                self.config_panel = None;
                            }
                            ConfigPanelAction::Cancel => {
                                self.config_panel = None;
                            }
                        }
                        continue;
                    }

                    // --- Profile manager overlay intercepts all keys ---
                    if self.profile_manager.is_some() {
                        use crate::widgets::profile_manager::ProfileManagerAction;
                        let action = self.profile_manager.as_mut().unwrap().handle_key(k.code);
                        match action {
                            ProfileManagerAction::Consumed => {}
                            ProfileManagerAction::Close => {
                                self.profile_manager = None;
                            }
                            ProfileManagerAction::SwitchTo(name) => {
                                match omni_core::auth::profiles::set_active_profile(&name) {
                                    Ok(()) => {
                                        let display = omni_core::auth::profiles::get_active_profile()
                                            .map(|p| p.display_name())
                                            .unwrap_or_else(|| name.clone());
                                        // Refresh status bar profile name
                                        self.active_profile_name = Some(display.clone());
                                        self.flash_success(format!("Switched to {}", display));
                                        self.message_list.push(MessageEntry::System {
                                            text: format!("Switched to profile: {}", display),
                                            severity: SystemSeverity::Info,
                                        });
                                    }
                                    Err(e) => {
                                        self.message_list.push(MessageEntry::System {
                                            text: format!("Failed to switch profile: {}", e),
                                            severity: SystemSeverity::Error,
                                        });
                                    }
                                }
                                self.profile_manager = None;
                            }
                            ProfileManagerAction::AddNew => {
                                self.message_list.push(MessageEntry::System {
                                    text: "Use /login to add a new profile via OAuth.".to_string(),
                                    severity: SystemSeverity::Info,
                                });
                                self.profile_manager = None;
                            }
                            ProfileManagerAction::Deleted(name) => {
                                self.flash_success(format!("Removed profile: {}", name));
                                self.message_list.push(MessageEntry::System {
                                    text: format!("Removed profile: {}", name),
                                    severity: SystemSeverity::Info,
                                });
                                // If list is now empty, close the panel.
                                if self.profile_manager.as_ref().map_or(true, |pm| pm.is_empty()) {
                                    self.profile_manager = None;
                                }
                            }
                        }
                        continue;
                    }

                    // --- Login dialog overlay intercepts all keys ---
                    if self.login_dialog.is_some() {
                        use crate::widgets::login_dialog::LoginDialogAction;
                        let action = self.login_dialog.as_mut().unwrap().handle_key(k.code);
                        match action {
                            LoginDialogAction::Consumed => {}
                            LoginDialogAction::Close => {
                                self.login_dialog = None;
                            }
                            LoginDialogAction::StartOAuth => {
                                // Phase 1: prepare OAuth (get URL + start listener) synchronously-ish
                                let tx_login = tx.clone();
                                tokio::spawn(async move {
                                    // prepare_oauth_login returns immediately with the URL
                                    // and a future to await for the callback.
                                    let flow = match omni_core::auth::pkce::prepare_oauth_login(true).await {
                                        Ok(f) => f,
                                        Err(e) => {
                                            let _ = tx_login
                                                .send(AppEvent::LoginOAuthResult(Err(e.to_string())))
                                                .await;
                                            return;
                                        }
                                    };

                                    // Send the manual URL back to the dialog immediately
                                    let _ = tx_login
                                        .send(AppEvent::LoginOAuthUrl(flow.manual_url.clone()))
                                        .await;

                                    // Phase 2: wait for browser callback + token exchange
                                    match flow.wait().await {
                                        Ok(result) => {
                                            // Store tokens in the legacy location
                                            if let Err(e) = omni_core::auth::storage::store_tokens(&result.tokens).await {
                                                tracing::warn!("Failed to store tokens to legacy location: {}", e);
                                            }

                                            // Use email + subscription_type fetched from /api/oauth/profile
                                            let email = result.email
                                                .unwrap_or_else(|| "unknown".to_string());
                                            let sub_type = result
                                                .subscription_type
                                                .as_deref()
                                                .or(result.tokens.subscription_type.as_deref())
                                                .unwrap_or("pro");

                                            // Save as a profile and set as active
                                            match omni_core::auth::profiles::save_oauth_as_profile(
                                                &result.tokens,
                                                &email,
                                                sub_type,
                                            ) {
                                                Ok(profile) => {
                                                    let msg = format!(
                                                        "Profile: {}\nSet as active profile.",
                                                        profile.display_name()
                                                    );
                                                    let _ = tx_login
                                                        .send(AppEvent::LoginOAuthResult(Ok(msg)))
                                                        .await;
                                                }
                                                Err(e) => {
                                                    let _ = tx_login
                                                        .send(AppEvent::LoginOAuthResult(Err(
                                                            format!("Logged in but failed to save profile: {}", e),
                                                        )))
                                                        .await;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            let _ = tx_login
                                                .send(AppEvent::LoginOAuthResult(Err(e.to_string())))
                                                .await;
                                        }
                                    }
                                });
                            }
                            LoginDialogAction::SubmitApiKey(key) => {
                                // Validate key format
                                if !key.starts_with("sk-ant-") && !key.starts_with("sk-") {
                                    if let Some(ref mut dialog) = self.login_dialog {
                                        dialog.set_error(
                                            "Invalid key format. Key must start with sk-ant- or sk-"
                                                .to_string(),
                                        );
                                    }
                                } else {
                                    // Build a profile name from the key
                                    let key_suffix = if key.len() >= 8 {
                                        &key[key.len() - 8..]
                                    } else {
                                        &key
                                    };
                                    let profile_name = format!("{}-api", key_suffix);
                                    let now = chrono::Utc::now().to_rfc3339();

                                    let profile = omni_core::auth::profiles::Profile {
                                        name: profile_name.clone(),
                                        email: format!("api-key-...{}", key_suffix),
                                        subscription_type: "api".to_string(),
                                        credentials: omni_core::auth::profiles::ProfileCredentials {
                                            access_token: None,
                                            refresh_token: None,
                                            expires_at: None,
                                            api_key: Some(key),
                                            scopes: vec![],
                                            account_uuid: None,
                                            organization_name: None,
                                        },
                                        created_at: now,
                                    };

                                    match omni_core::auth::profiles::save_profile(&profile) {
                                        Ok(()) => {
                                            match omni_core::auth::profiles::set_active_profile(&profile_name) {
                                                Ok(()) => {
                                                    let msg = format!(
                                                        "Profile: {} (API)\nSet as active profile.",
                                                        profile_name
                                                    );
                                                    if let Some(ref mut dialog) = self.login_dialog {
                                                        dialog.set_success(msg);
                                                    }
                                                }
                                                Err(e) => {
                                                    if let Some(ref mut dialog) = self.login_dialog {
                                                        dialog.set_error(format!(
                                                            "Saved profile but failed to set as active: {}",
                                                            e
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            if let Some(ref mut dialog) = self.login_dialog {
                                                dialog.set_error(format!(
                                                    "Failed to save profile: {}",
                                                    e
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    // --- Picker overlay intercepts all keys ---
                    if self.active_picker.is_some() {
                        use crate::widgets::picker::PickerAction;
                        let action = self.active_picker.as_mut().unwrap().handle_key(k);
                        match action {
                            PickerAction::Selected(value) => {
                                // Determine picker type and apply the selection
                                let picker_kind = match self.active_picker.as_ref().unwrap() {
                                    crate::widgets::picker::ActivePicker::Model(_) => "model",
                                    crate::widgets::picker::ActivePicker::Theme(_) => "theme",
                                    crate::widgets::picker::ActivePicker::Session(_) => "session",
                                    crate::widgets::picker::ActivePicker::Profile(_) => "profile",
                                };
                                match picker_kind {
                                    "model" => {
                                        self.model_name = value.clone();
                                        if let Some(ref state) = self.app_state {
                                            state.write().set_model_override(Some(value.clone()));
                                        }
                                        let display = omni_core::utils::model::get_public_model_display_name(&value)
                                            .map(|s| s.to_string())
                                            .unwrap_or_else(|| value.clone());
                                        self.message_list.push(MessageEntry::System {
                                            text: format!("Switched to model: {}", display),
                                            severity: SystemSeverity::Info,
                                        });
                                    }
                                    "theme" => {
                                        self.message_list.push(MessageEntry::System {
                                            text: format!("Theme set to: {}", value),
                                            severity: SystemSeverity::Info,
                                        });
                                    }
                                    "session" => {
                                        // Trigger session resume via the same path as /resume <id>
                                        let cwd = std::env::current_dir().unwrap_or_default();
                                        let cwd_str = cwd.to_string_lossy().to_string();
                                        let project_dir =
                                            omni_core::session::SessionManager::project_dir_for_cwd(&cwd_str);
                                        let mgr = omni_core::session::SessionManager::new(project_dir);
                                        match mgr.load_session(&value) {
                                            Ok(session) => {
                                                engine.lock().await.clear_messages();
                                                for msg in &session.messages {
                                                    engine.lock().await.add_raw_message(msg.clone());
                                                }
                                                self.message_list.clear();
                                                self.message_list.push(MessageEntry::System {
                                                    text: format!(
                                                        "Resumed session {} ({} messages)",
                                                        value, session.messages.len()
                                                    ),
                                                    severity: SystemSeverity::Info,
                                                });
                                                for msg in &session.messages {
                                                    if let Some(role) = msg.get("role").and_then(|v| v.as_str()) {
                                                        let text = msg
                                                            .get("content")
                                                            .and_then(|c| {
                                                                if let Some(s) = c.as_str() {
                                                                    Some(s.to_string())
                                                                } else if let Some(arr) = c.as_array() {
                                                                    Some(arr.iter()
                                                                        .filter_map(|b| b.get("text").and_then(|v| v.as_str()).map(String::from))
                                                                        .collect::<Vec<_>>()
                                                                        .join("\n"))
                                                                } else {
                                                                    None
                                                                }
                                                            })
                                                            .unwrap_or_default();
                                                        match role {
                                                            "user" => {
                                                                self.message_list.push(MessageEntry::User {
                                                                    text: text.clone(),
                                                                    images: vec![],
                                                                });
                                                            }
                                                            "assistant" => {
                                                                self.message_list.push(MessageEntry::Assistant {
                                                                    text: text.clone(),
                                                                });
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                self.message_list.push(MessageEntry::System {
                                                    text: format!("Failed to resume session: {}", e),
                                                    severity: SystemSeverity::Error,
                                                });
                                            }
                                        }
                                    }
                                    "profile" => {
                                        match omni_core::auth::profiles::set_active_profile(&value) {
                                            Ok(()) => {
                                                let display = omni_core::auth::profiles::get_active_profile()
                                                    .map(|p| p.display_name())
                                                    .unwrap_or_else(|| value.clone());
                                                self.flash_success(format!("Switched to {}", display));
                                                self.message_list.push(MessageEntry::System {
                                                    text: format!("Switched to profile: {}", display),
                                                    severity: SystemSeverity::Info,
                                                });
                                            }
                                            Err(e) => {
                                                self.message_list.push(MessageEntry::System {
                                                    text: format!("Failed to switch profile: {}", e),
                                                    severity: SystemSeverity::Error,
                                                });
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                self.active_picker = None;
                            }
                            PickerAction::Cancelled => {
                                self.active_picker = None;
                            }
                            PickerAction::None => {}
                        }
                        continue;
                    }

                    // --- Chord detection (multi-key shortcuts) ---
                    if let Some((mod_first, code_first)) = self.pending_chord.take() {
                        // We had a pending chord prefix; check if this key completes it.
                        if mod_first == KeyModifiers::CONTROL
                            && code_first == KeyCode::Char('x')
                            && k.modifiers == KeyModifiers::CONTROL
                            && k.code == KeyCode::Char('e')
                        {
                            // Ctrl+X Ctrl+E: open external editor
                            self.open_external_editor();
                            continue;
                        }
                        // Chord didn't match -- fall through and handle `k` normally.
                    }

                    // Ctrl+X starts a chord prefix (wait for next key)
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('x'))
                    ) {
                        self.pending_chord = Some((k.modifiers, k.code));
                        continue;
                    }

                    // --- Global shortcuts that always apply ---

                    // Ctrl+C: copy selection, cancel request, or hint
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('c'))
                    ) {
                        // If text is selected, copy it to clipboard first
                        if self.selection.has_selection() {
                            let text = self.extract_selected_text();
                            if !text.is_empty() {
                                crate::mouse::copy_to_clipboard(&text);
                                self.selection.clear();
                                self.message_list.push(MessageEntry::System {
                                    text: "Copied to clipboard.".to_string(),
                                    severity: SystemSeverity::Info,
                                });
                            }
                            continue;
                        }
                        if self.engine_busy {
                            cancel.cancel();
                            self.spinner.stop();
                            self.engine_busy = false;
                            self.message_list.push(MessageEntry::System {
                                text: "Interrupted.".to_string(),
                                severity: SystemSeverity::Warning,
                            });
                        } else if !self.prompt.is_empty() {
                            // Clear current input (like bash Ctrl+C)
                            self.prompt.clear();
                        } else {
                            // Empty input, no request -- double-press to exit
                            if let Some(first_press) = self.exit_pending {
                                if first_press.elapsed() < std::time::Duration::from_millis(1500) {
                                    cancel.cancel();
                                    self.should_quit = true;
                                } else {
                                    self.exit_pending = Some(std::time::Instant::now());
                                    self.flash(crate::widgets::status_bar::FlashMessage::warning("Press Ctrl+C again to exit, Ctrl+D to quit"));
                                }
                            } else {
                                self.exit_pending = Some(std::time::Instant::now());
                                self.flash(crate::widgets::status_bar::FlashMessage::warning("Press Ctrl+C again to exit, Ctrl+D to quit"));
                            }
                        }
                        continue;
                    }

                    // Ctrl+D: quit on empty input with double-press confirmation
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('d'))
                    ) {
                        if self.prompt.is_empty() {
                            if let Some(first_press) = self.exit_pending {
                                if first_press.elapsed() < std::time::Duration::from_millis(1500) {
                                    cancel.cancel();
                                    self.should_quit = true;
                                } else {
                                    self.exit_pending = Some(std::time::Instant::now());
                                    self.flash(crate::widgets::status_bar::FlashMessage::warning("Press Ctrl+D again to exit"));
                                }
                            } else {
                                self.exit_pending = Some(std::time::Instant::now());
                                self.flash(crate::widgets::status_bar::FlashMessage::warning("Press Ctrl+D again to exit"));
                            }
                        }
                        continue;
                    }

                    // Ctrl+L: clear screen (redraw)
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('l'))
                    ) {
                        self.terminal.clear()?;
                        continue;
                    }

                    // --- Action menu routing (Ctrl+E popup) ---
                    if self.action_menu.is_some() {
                        match k.code {
                            KeyCode::Char('c') | KeyCode::Char('C') => {
                                if let Some(ref menu) = self.action_menu {
                                    crate::mouse::copy_to_clipboard(&menu.text);
                                    self.message_list.push(MessageEntry::System {
                                        text: "Copied to clipboard.".to_string(),
                                        severity: SystemSeverity::Info,
                                    });
                                }
                                self.action_menu = None;
                            }
                            KeyCode::Char('e') | KeyCode::Char('E') => {
                                if let Some(ref menu) = self.action_menu {
                                    let text = menu.text.clone();
                                    self.prompt.clear_buffer();
                                    self.prompt.insert_paste(&text);
                                }
                                self.action_menu = None;
                            }
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                if let Some(ref menu) = self.action_menu {
                                    self.message_list.truncate(menu.msg_index);
                                    self.message_list.push(MessageEntry::System {
                                        text: "Conversation rewound.".to_string(),
                                        severity: SystemSeverity::Info,
                                    });
                                }
                                self.action_menu = None;
                            }
                            KeyCode::Char('s') | KeyCode::Char('S') => {
                                if let Some(ref menu) = self.action_menu {
                                    // Put a summarize request into the input
                                    let preview = if menu.text.len() > 100 {
                                        format!("{}...", &menu.text[..97])
                                    } else {
                                        menu.text.clone()
                                    };
                                    self.prompt.clear_buffer();
                                    self.prompt.insert_paste(&format!(
                                        "Please summarize the following:\n\n{}",
                                        preview
                                    ));
                                }
                                self.action_menu = None;
                            }
                            KeyCode::Esc => {
                                self.action_menu = None;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- Cmd key shortcuts (macOS) ---
                    if k.modifiers.contains(KeyModifiers::SUPER) {
                        match k.code {
                            KeyCode::Char('c') => {
                                // Cmd+C: copy selected text (if any)
                                if self.selection.has_selection() {
                                    // Selection text extraction is best-effort;
                                    // for now copy what the clipboard already holds
                                    // from the selection logic.
                                    // We don't have buffer text extraction from
                                    // coordinates, so use prompt text as fallback.
                                    let text = self.prompt.text();
                                    if !text.is_empty() {
                                        crate::mouse::copy_to_clipboard(&text);
                                    }
                                }
                                continue;
                            }
                            KeyCode::Char('v') => {
                                // Cmd+V: paste from clipboard
                                self.prompt.paste_clipboard();
                                continue;
                            }
                            KeyCode::Char('a') => {
                                // Cmd+A: select all in input
                                self.prompt.select_all();
                                continue;
                            }
                            KeyCode::Char('k') => {
                                // Cmd+K: clear screen
                                self.message_list.clear();
                                self.terminal.clear()?;
                                continue;
                            }
                            _ => {}
                        }
                    }

                    // --- Permission dialog routing ---
                    if self.permission_dialog.is_some() {
                        match k.code {
                            KeyCode::Tab | KeyCode::Right => {
                                if let Some(ref mut dialog) = self.permission_dialog {
                                    dialog.next_button();
                                }
                            }
                            KeyCode::BackTab | KeyCode::Left => {
                                if let Some(ref mut dialog) = self.permission_dialog {
                                    dialog.prev_button();
                                }
                            }
                            KeyCode::Enter => {
                                let response = self
                                    .permission_dialog
                                    .as_ref()
                                    .map(|d| d.selected().to_string())
                                    .unwrap_or_else(|| "deny".to_string());
                                let _ = tx.send(AppEvent::PermissionResponse(response)).await;
                            }
                            KeyCode::Esc => {
                                // Escape closes permission dialog (deny)
                                let _ = tx
                                    .send(AppEvent::PermissionResponse("deny".to_string()))
                                    .await;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- Search overlay routing ---
                    if self.search_overlay.active {
                        match self.search_overlay.handle_key(k) {
                            SearchAction::QueryChanged(query) => {
                                self.message_list.set_search(Some(query));
                                self.search_overlay.update_match_info(
                                    self.message_list.search_match_count(),
                                    0,
                                );
                            }
                            SearchAction::NextMatch => {
                                self.message_list.search_next();
                            }
                            SearchAction::PreviousMatch => {
                                self.message_list.search_prev();
                            }
                            SearchAction::Close => {
                                self.message_list.set_search(None);
                                self.focus.set(FocusTarget::Prompt);
                            }
                            SearchAction::Accept => {
                                // Keep scroll position, close search
                                self.search_overlay.active = false;
                                self.focus.set(FocusTarget::Prompt);
                            }
                            SearchAction::None => {}
                        }
                        continue;
                    }

                    // --- When history search is active, route all keys to prompt directly ---
                    if self.prompt.history_search.active {
                        use crate::widgets::prompt_input::InputAction;
                        match self.prompt.handle_key(k) {
                            InputAction::Submit(text) => {
                                let _ = tx.send(AppEvent::SubmitPrompt(text)).await;
                            }
                            InputAction::None => {}
                        }
                        continue;
                    }

                    // --- Escape: close dialogs, exit vim mode, cancel search ---
                    if k.code == KeyCode::Esc {
                        if self.prompt.completion.is_some() {
                            self.prompt.completion = None;
                        } else {
                            // Pass Escape through to input handler (for vim normal mode)
                            self.input_handler.handle_key(k, &mut self.prompt);
                        }
                        continue;
                    }

                    // --- Ctrl+R: reverse history search ---
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('r'))
                    ) {
                        self.prompt.start_history_search();
                        continue;
                    }

                    // --- Ctrl+F opens search overlay ---
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('f'))
                    ) {
                        self.search_overlay.open(None);
                        self.focus.set(FocusTarget::Search);
                        continue;
                    }

                    // --- Ctrl+O: toggle transcript view mode ---
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('o'))
                    ) {
                        // Toggle expanded/collapsed state of the last focused message
                        let last_idx = self.message_list.len().saturating_sub(1);
                        self.message_list.toggle_expanded(last_idx);
                        continue;
                    }

                    // --- Ctrl+E: open message action menu ---
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('e'))
                    ) {
                        if let Some(idx) = self.message_list.last_assistant_index() {
                            if let Some(text) = self.message_list.last_assistant_text() {
                                self.action_menu = Some(ActionMenu {
                                    text,
                                    msg_index: idx,
                                });
                            }
                        }
                        continue;
                    }

                    // --- Ctrl+G: open external editor ---
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('g'))
                    ) {
                        self.open_external_editor();
                        continue;
                    }

                    // --- Scroll message list with PageUp/PageDown/Home/End ---
                    if matches!(k.code, KeyCode::PageUp) {
                        self.message_list.scroll_up(10);
                        if let Some(hint) = self.notifications.hint_tracker.record_scroll_up() {
                            self.notifications.show_hint(hint);
                        }
                        continue;
                    }
                    if matches!(k.code, KeyCode::PageDown) {
                        self.message_list.scroll_down(10);
                        continue;
                    }
                    if matches!(k.code, KeyCode::Home) {
                        self.message_list.scroll_to_top();
                        continue;
                    }
                    if matches!(k.code, KeyCode::End) {
                        self.message_list.scroll_to_bottom();
                        continue;
                    }

                    // --- n/N: next/prev search match (when in vim normal mode) ---
                    if self.input_handler.mode() == crate::input::InputMode::Normal {
                        if let KeyCode::Char('n') = k.code {
                            if k.modifiers.is_empty() {
                                self.message_list.search_next();
                                continue;
                            }
                        }
                        if let KeyCode::Char('N') = k.code {
                            if k.modifiers.is_empty() {
                                self.message_list.search_prev();
                                continue;
                            }
                        }
                        // "/" in normal mode opens search in transcript
                        if let KeyCode::Char('/') = k.code {
                            if k.modifiers.is_empty() && self.prompt.is_empty() {
                                self.search_overlay.open(None);
                                self.focus.set(FocusTarget::Search);
                                continue;
                            }
                        }
                    }

                    // --- Route keys through the InputHandler ---
                    // Sync vim mode state from app
                    if self.vim_mode != self.input_handler.vim_enabled() {
                        self.input_handler.set_vim_enabled(self.vim_mode);
                    }

                    match self.input_handler.handle_key(k, &mut self.prompt) {
                        InputResult::Submit(text) => {
                            let _ = tx.send(AppEvent::SubmitPrompt(text)).await;
                        }
                        InputResult::ExCommand(cmd) => {
                            // Handle vim ex-commands
                            match cmd.as_str() {
                                "q" | "quit" => {
                                    cancel.cancel();
                                    self.should_quit = true;
                                }
                                _ => {
                                    self.message_list.push(MessageEntry::System {
                                        text: format!("Unknown command: :{}", cmd),
                                        severity: SystemSeverity::Warning,
                                    });
                                }
                            }
                        }
                        InputResult::Consumed => {}
                        InputResult::NotConsumed => {}
                    }
                }
                AppEvent::SubmitPrompt(text) => {
                    if self.engine_busy || text.trim().is_empty() {
                        continue;
                    }

                    // Slash command dispatch
                    if text.trim().starts_with('/') {
                        let cmd_ctx = if let Some(ref state) = self.app_state {
                            let s = state.read();
                            let mu: Vec<(String, u64, u64, u64, u64, f64)> = s.cost_tracker.model_usage()
                                .into_iter()
                                .map(|(name, u)| (name, u.input_tokens, u.output_tokens, u.cache_read_input_tokens, u.cache_creation_input_tokens, u.cost_usd))
                                .collect();
                            CommandContext {
                                cwd: s.cwd.clone(),
                                project_root: Some(s.project_root.clone()),
                                model: s.active_model().to_string(),
                                session_id: Some(s.session_id.clone()),
                                input_tokens: s.total_input_tokens(),
                                output_tokens: s.total_output_tokens(),
                                total_cost: s.total_cost_usd(),
                                vim_mode: s.vim_mode,
                                plan_mode: s.plan_mode,
                                fast_mode: s.fast_mode,
                                brief_mode: s.brief_mode,
                                cache_read_input_tokens: s.cost_tracker.total_cache_read_input_tokens(),
                                cache_creation_input_tokens: s.cost_tracker.total_cache_creation_input_tokens(),
                                turn_count: s.turn_count,
                                session_duration_ms: s.session_duration().as_millis() as u64,
                                api_duration_ms: s.cost_tracker.total_api_duration().as_millis() as u64,
                                tool_duration_ms: s.total_tool_duration_ms as u64,
                                lines_added: s.cost_tracker.total_lines_added(),
                                lines_removed: s.cost_tracker.total_lines_removed(),
                                model_usage: mu,
                            }
                        } else {
                            CommandContext {
                                cwd: cwd.clone(),
                                project_root: None,
                                model: self.model_name.clone(),
                                session_id: None,
                                input_tokens: self.total_tokens,
                                output_tokens: 0,
                                total_cost: self.total_cost,
                                vim_mode: self.vim_mode,
                                plan_mode: self.plan_mode,
                                fast_mode: false,
                                brief_mode: false,
                                cache_read_input_tokens: 0,
                                cache_creation_input_tokens: 0,
                                turn_count: 0,
                                session_duration_ms: 0,
                                api_duration_ms: 0,
                                tool_duration_ms: 0,
                                lines_added: 0,
                                lines_removed: 0,
                                model_usage: vec![],
                            }
                        };
                        if let Some((cmd, args)) = self.command_registry.parse_and_find(text.trim())
                        {
                            let result = cmd.execute(&args, &cmd_ctx).await;
                            match result {
                                CommandResult::Output(msg) => {
                                    self.message_list.push(MessageEntry::System { text: msg, severity: SystemSeverity::Info });
                                }
                                CommandResult::Quit => {
                                    self.should_quit = true;
                                }
                                CommandResult::SwitchModel(name) => {
                                    self.model_name = name.clone();
                                    if let Some(ref state) = self.app_state {
                                        state.write().set_model_override(Some(name.clone()));
                                    }
                                    self.message_list.push(MessageEntry::System {
                                        text: format!("Switched to model: {}", name),
                                        severity: SystemSeverity::Info,
                                    });
                                }
                                CommandResult::ClearConversation => {
                                    engine.lock().await.clear_messages();
                                    self.message_list.clear();
                                    self.message_list.push(MessageEntry::System {
                                        text: "Conversation cleared.".to_string(),
                                        severity: SystemSeverity::Info,
                                    });
                                }
                                CommandResult::CompactMessages(_instructions) => {
                                    self.engine_busy = true;
                                    self.spinner.start(SpinnerMode::Thinking);
                                    self.message_list.push(MessageEntry::System {
                                        text: "Compacting conversation...".to_string(),
                                        severity: SystemSeverity::Info,
                                    });
                                    let (stream_tx, mut stream_rx) =
                                        mpsc::channel::<StreamEvent>(128);
                                    let tx_forward = tx.clone();
                                    tokio::spawn(async move {
                                        while let Some(ev) = stream_rx.recv().await {
                                            if tx_forward.send(AppEvent::Stream(ev)).await.is_err()
                                            {
                                                break;
                                            }
                                        }
                                    });
                                    match engine.lock().await.compact(&stream_tx).await {
                                        Ok(summary) => {
                                            self.message_list.push(MessageEntry::System {
                                                text: format!("Compacted: {summary}"),
                                                severity: SystemSeverity::Info,
                                            });
                                        }
                                        Err(e) => {
                                            self.message_list.push(MessageEntry::System {
                                                text: format!("Compact error: {e}"),
                                                severity: SystemSeverity::Error,
                                            });
                                        }
                                    }
                                    self.spinner.stop();
                                    self.engine_busy = false;
                                }
                                CommandResult::ResumeSession(id) => {
                                    let cwd_str = cwd.to_string_lossy().to_string();
                                    let project_dir =
                                        omni_core::session::SessionManager::project_dir_for_cwd(
                                            &cwd_str,
                                        );
                                    let mgr =
                                        omni_core::session::SessionManager::new(project_dir);
                                    match mgr.load_session(&id) {
                                        Ok(session) => {
                                            // Restore messages into the engine
                                            engine.lock().await.clear_messages();
                                            for msg in &session.messages {
                                                engine.lock().await.add_raw_message(msg.clone());
                                            }
                                            self.message_list.clear();
                                            self.message_list.push(MessageEntry::System {
                                                text: format!(
                                                    "Resumed session {} ({} messages)",
                                                    id,
                                                    session.messages.len()
                                                ),
                                                severity: SystemSeverity::Info,
                                            });
                                            // Re-display prior conversation messages
                                            for msg in &session.messages {
                                                if let Some(role) =
                                                    msg.get("role").and_then(|v| v.as_str())
                                                {
                                                    let text = msg
                                                        .get("content")
                                                        .and_then(|c| {
                                                            if let Some(s) = c.as_str() {
                                                                Some(s.to_string())
                                                            } else if let Some(arr) = c.as_array() {
                                                                Some(
                                                                    arr.iter()
                                                                        .filter_map(|b| {
                                                                            b.get("text")
                                                                                .and_then(|v| {
                                                                                    v.as_str()
                                                                                })
                                                                                .map(String::from)
                                                                        })
                                                                        .collect::<Vec<_>>()
                                                                        .join("\n"),
                                                                )
                                                            } else {
                                                                None
                                                            }
                                                        })
                                                        .unwrap_or_default();
                                                    match role {
                                                        "user" => {
                                                            self.message_list.push(
                                                                MessageEntry::User {
                                                                    text: text.clone(),
                                                                    images: vec![],
                                                                },
                                                            );
                                                        }
                                                        "assistant" => {
                                                            self.message_list.push(
                                                                MessageEntry::Assistant {
                                                                    text: text.clone(),
                                                                },
                                                            );
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            self.message_list.push(MessageEntry::System {
                                                text: format!(
                                                    "Failed to resume session {}: {}",
                                                    id, e
                                                ),
                                                severity: SystemSeverity::Error,
                                            });
                                        }
                                    }
                                }
                                CommandResult::TogglePlanMode => {
                                    self.plan_mode = !self.plan_mode;
                                    if let Some(ref state) = self.app_state {
                                        state.write().set_plan_mode(self.plan_mode);
                                    }
                                    self.message_list.push(MessageEntry::System {
                                        text: format!(
                                            "Plan mode: {}",
                                            if self.plan_mode { "on" } else { "off" }
                                        ),
                                        severity: SystemSeverity::Info,
                                    });
                                }
                                CommandResult::ToggleVimMode => {
                                    self.vim_mode = !self.vim_mode;
                                    if let Some(ref state) = self.app_state {
                                        state.write().vim_mode = self.vim_mode;
                                    }
                                    self.message_list.push(MessageEntry::System {
                                        text: format!(
                                            "Vim mode: {}",
                                            if self.vim_mode { "on" } else { "off" }
                                        ),
                                        severity: SystemSeverity::Info,
                                    });
                                }
                                CommandResult::ToggleFastMode => {
                                    if let Some(ref state) = self.app_state {
                                        let mut s = state.write();
                                        s.fast_mode = !s.fast_mode;
                                        let enabled = s.fast_mode;
                                        drop(s);
                                        self.message_list.push(MessageEntry::System {
                                            text: format!(
                                                "Fast mode {}",
                                                if enabled { "enabled" } else { "disabled" }
                                            ),
                                            severity: SystemSeverity::Info,
                                        });
                                    }
                                }
                                CommandResult::ToggleBriefMode => {
                                    if let Some(ref state) = self.app_state {
                                        let mut s = state.write();
                                        s.brief_mode = !s.brief_mode;
                                        let enabled = s.brief_mode;
                                        drop(s);
                                        self.message_list.push(MessageEntry::System {
                                            text: format!(
                                                "Brief mode {}",
                                                if enabled { "enabled" } else { "disabled" }
                                            ),
                                            severity: SystemSeverity::Info,
                                        });
                                    }
                                }
                                CommandResult::Prompt {
                                    content,
                                    progress_message,
                                    ..
                                } => {
                                    let msg = progress_message
                                        .unwrap_or_else(|| "Running prompt...".to_string());
                                    self.message_list.push(MessageEntry::System { text: msg, severity: SystemSeverity::Info });
                                    // Send the prompt content as a user message to the engine.
                                    self.pending_input = Some(content);
                                }
                                CommandResult::OpenConfigPanel => {
                                    self.config_panel = Some(
                                        crate::widgets::config_panel::ConfigPanel::new(
                                            &self.model_name,
                                            self.vim_mode,
                                            self.plan_mode,
                                        ),
                                    );
                                }
                                CommandResult::OpenProfileManager => {
                                    self.profile_manager = Some(
                                        crate::widgets::profile_manager::ProfileManager::new(),
                                    );
                                }
                                CommandResult::OpenLoginDialog => {
                                    self.login_dialog = Some(
                                        crate::widgets::login_dialog::LoginDialog::new(),
                                    );
                                }
                                CommandResult::OpenStatusDialog => {
                                    use crate::widgets::status_dialog::{StatusDialog, StatusDialogContext};
                                    let status_ctx = StatusDialogContext {
                                        model: cmd_ctx.model.clone(),
                                        session_id: cmd_ctx.session_id.clone(),
                                        input_tokens: cmd_ctx.input_tokens,
                                        output_tokens: cmd_ctx.output_tokens,
                                        cache_read_tokens: cmd_ctx.cache_read_input_tokens,
                                        cache_write_tokens: cmd_ctx.cache_creation_input_tokens,
                                        total_cost: cmd_ctx.total_cost,
                                        turn_count: cmd_ctx.turn_count,
                                        session_duration_ms: cmd_ctx.session_duration_ms,
                                        api_duration_ms: cmd_ctx.api_duration_ms,
                                        tool_duration_ms: cmd_ctx.tool_duration_ms,
                                        lines_added: cmd_ctx.lines_added,
                                        lines_removed: cmd_ctx.lines_removed,
                                        vim_mode: cmd_ctx.vim_mode,
                                        plan_mode: cmd_ctx.plan_mode,
                                        fast_mode: cmd_ctx.fast_mode,
                                        brief_mode: cmd_ctx.brief_mode,
                                        model_usage: cmd_ctx.model_usage.clone(),
                                        cwd: cmd_ctx.cwd.clone(),
                                    };
                                    self.status_dialog = Some(StatusDialog::new(&status_ctx));
                                }
                                CommandResult::OpenInfoDialog { title, content } => {
                                    use crate::widgets::info_dialog::InfoDialog;
                                    self.info_dialog = Some(InfoDialog::new(title, content));
                                }
                                CommandResult::OpenPicker(picker_name) => {
                                    use crate::widgets::picker::{
                                        ActivePicker, build_model_picker,
                                        build_profile_picker, build_session_picker,
                                        build_theme_picker,
                                    };
                                    match picker_name.as_str() {
                                        "model" => {
                                            self.active_picker = Some(ActivePicker::Model(
                                                build_model_picker(&self.model_name),
                                            ));
                                        }
                                        "theme" => {
                                            self.active_picker = Some(ActivePicker::Theme(
                                                build_theme_picker("auto"),
                                            ));
                                        }
                                        "session" => {
                                            let cwd_str = cwd.to_string_lossy().to_string();
                                            let project_dir =
                                                omni_core::session::SessionManager::project_dir_for_cwd(
                                                    &cwd_str,
                                                );
                                            let mgr =
                                                omni_core::session::SessionManager::new(project_dir);
                                            match mgr.list_sessions() {
                                                Ok(sessions) => {
                                                    self.active_picker = Some(ActivePicker::Session(
                                                        build_session_picker(&sessions),
                                                    ));
                                                }
                                                Err(e) => {
                                                    self.message_list.push(MessageEntry::System {
                                                        text: format!("Failed to list sessions: {}", e),
                                                        severity: SystemSeverity::Error,
                                                    });
                                                }
                                            }
                                        }
                                        "profile" => {
                                            self.active_picker = Some(ActivePicker::Profile(
                                                build_profile_picker(),
                                            ));
                                        }
                                        _ => {
                                            self.message_list.push(MessageEntry::System {
                                                text: format!("Unknown picker: {}", picker_name),
                                                severity: SystemSeverity::Error,
                                            });
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                        // Unknown slash command -- fall through to send as regular message
                    }

                    // Add user message to display
                    self.message_list
                        .push(MessageEntry::User { text: text.clone(), images: vec![] });

                    // Check if a contextual hint should be shown after user input
                    if let Some(hint) = self.notifications.hint_tracker.record_input() {
                        self.notifications.show_hint(hint);
                    }

                    // Add to engine
                    engine.lock().await.add_user_message(&text);
                    self.engine_busy = true;
                    self.spinner.start(SpinnerMode::Thinking);
                    // Show a tip in the spinner while thinking (filter out bad suggestions)
                    if let Some(tip) = self.tip_scheduler.show_tip() {
                        if prompt_suggestion::should_filter_suggestion(&tip.content).is_none() {
                            self.spinner.tip = Some(tip.content.clone());
                        }
                    }
                    self.turn_tool_infos.clear();

                    // Run turn
                    let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(128);
                    let tx_forward = tx.clone();

                    // Spawn a task to forward stream events to the main event loop
                    tokio::spawn(async move {
                        while let Some(ev) = stream_rx.recv().await {
                            if tx_forward.send(AppEvent::Stream(ev)).await.is_err() {
                                break;
                            }
                        }
                    });

                    // Spawn run_turn in background so UI event loop stays responsive
                    {
                        let engine_clone = engine.clone();
                        let tx_result = tx.clone();
                        tokio::spawn(async move {
                            let result = {
                                let mut eng = engine_clone.lock().await;
                                eng.run_turn(&stream_tx).await
                            };
                            let _ = tx_result.send(AppEvent::TurnComplete(result)).await;
                        });
                    }
                }
                AppEvent::Stream(stream_event) => {
                    self.handle_stream_event(stream_event);
                }
                AppEvent::PermissionResponse(response) => {
                    self.permission_dialog = None;

                    if pending_tool_index == 0 || pending_tool_index > pending_tools.len() {
                        continue;
                    }
                    let tool_idx = pending_tool_index - 1; // We already advanced past it

                    match response.as_str() {
                        "allow" | "always" => {
                            // Execute this tool
                            let info = &pending_tools[tool_idx].info;
                            self.spinner.start(SpinnerMode::Tool {
                                name: info.name.clone(),
                            });
                            let tool_result =
                                execute_tool(&tools, &info.name, &info.input, &cwd, cancel.clone())
                                    .await;
                            let (result_text, is_error) = match &tool_result {
                                Ok(data) => (
                                    data.data
                                        .as_str()
                                        .unwrap_or(&data.data.to_string())
                                        .to_string(),
                                    data.is_error,
                                ),
                                Err(e) => (format!("Error: {}", e), true),
                            };
                            engine.lock().await.add_tool_result(&info.id, &result_text, is_error);
                            // Accumulate tool info for summary generation
                            self.turn_tool_infos.push(SummaryToolInfo {
                                name: info.name.clone(),
                                input: info.input.clone(),
                                output: serde_json::Value::String(result_text.clone()),
                            });
                            self.message_list.push(MessageEntry::ToolResult {
                                id: info.id.clone(),
                                name: info.name.clone(),
                                output: truncate_result(&result_text),
                                is_error,
                                duration_ms: None,
                            });
                            // Notify LSP of file changes after Edit/Write
                            if matches!(info.name.as_str(), "Edit" | "Write") {
                                if let Some(path) = info.input.get("file_path").and_then(|v| v.as_str()) {
                                    let _ = self.lsp_manager.save_file(path);
                                }
                            }
                            self.spinner.stop();

                            // Check next tool or continue turn
                            if pending_tool_index < pending_tools.len() {
                                self.check_next_tool_permission(
                                    &pending_tools,
                                    &mut pending_tool_index,
                                    &perm_ctx,
                                    &tools,
                                    &tx,
                                );
                            } else {
                                // All tools done -- fire ContinueTurn to re-enter the engine
                                let tx2 = tx.clone();
                                tokio::spawn(async move {
                                    let _ = tx2.send(AppEvent::ContinueTurn).await;
                                });
                            }
                        }
                        "deny" => {
                            let info = &pending_tools[tool_idx].info;
                            engine.lock().await.add_tool_result(&info.id, "Permission denied by user", true);
                            self.message_list.push(MessageEntry::ToolResult {
                                id: info.id.clone(),
                                name: info.name.clone(),
                                output: "Permission denied".to_string(),
                                is_error: true,
                                duration_ms: None,
                            });

                            // Check next tool or continue turn
                            if pending_tool_index < pending_tools.len() {
                                self.check_next_tool_permission(
                                    &pending_tools,
                                    &mut pending_tool_index,
                                    &perm_ctx,
                                    &tools,
                                    &tx,
                                );
                            } else {
                                // All tools done -- fire ContinueTurn to re-enter the engine
                                let tx2 = tx.clone();
                                tokio::spawn(async move {
                                    let _ = tx2.send(AppEvent::ContinueTurn).await;
                                });
                            }
                        }
                        _ => {}
                    }
                }
                AppEvent::ContinueTurn => {
                    // All pending tool results have been fed back -- run the next turn.
                    self.spinner.start(SpinnerMode::Thinking);

                    let (stream_tx, stream_rx) = mpsc::channel::<StreamEvent>(128);
                    let tx_forward = tx.clone();
                    tokio::spawn(async move {
                        let mut stream_rx = stream_rx;
                        while let Some(ev) = stream_rx.recv().await {
                            if tx_forward.send(AppEvent::Stream(ev)).await.is_err() {
                                break;
                            }
                        }
                    });

                    // Spawn in background so UI stays responsive
                    {
                        let engine_clone = engine.clone();
                        let tx_result = tx.clone();
                        tokio::spawn(async move {
                            let result = {
                                let mut eng = engine_clone.lock().await;
                                eng.run_turn(&stream_tx).await
                            };
                            let _ = tx_result.send(AppEvent::TurnComplete(result)).await;
                        });
                    }
                }
                AppEvent::TurnComplete(result) => {
                    match result {
                        Ok(TurnResult::Done(_)) => {
                            self.spinner.stop();
                            self.engine_busy = false;
                            // Generate tool use summary for accumulated tools
                            if let Some(summary) =
                                tool_use_summary::generate_simple_summary(&self.turn_tool_infos)
                            {
                                self.message_list.push(MessageEntry::System {
                                    text: format!("Summary: {}", summary),
                                    severity: SystemSeverity::Info,
                                });
                            }
                            self.turn_tool_infos.clear();
                            // Check if a contextual hint should be shown after assistant response
                            if let Some(hint) = self.notifications.hint_tracker.record_assistant_response() {
                                self.notifications.show_hint(hint);
                            }
                            // Check cost threshold warning
                            self.check_cost_threshold();
                            // Send terminal notification
                            let _ = notifications::send_notification(
                                &NotificationOptions {
                                    message: "Response complete".to_string(),
                                    title: Some("Claude Code".to_string()),
                                    notification_type: "turn_complete".to_string(),
                                },
                                &self.notification_channel,
                            );
                        }
                        Ok(TurnResult::ContinueRecovery)
                        | Ok(TurnResult::StopHookBlocking)
                        | Ok(TurnResult::TokenBudgetContinuation) => {
                            self.message_list.push(MessageEntry::System {
                                text: "Continuing...".to_string(),
                                severity: SystemSeverity::Info,
                            });
                            // Fire another ContinueTurn to keep going
                            let tx2 = tx.clone();
                            tokio::spawn(async move {
                                let _ = tx2.send(AppEvent::ContinueTurn).await;
                            });
                        }
                        Ok(TurnResult::ToolUse(tool_uses)) => {
                            // Another round of tool use -- re-enter the permission/execute cycle
                            pending_tools = tool_uses
                                .into_iter()
                                .map(|info| PendingTool { info })
                                .collect();
                            pending_tool_index = 0;
                            self.spinner.stop();
                            self.check_next_tool_permission(
                                &pending_tools,
                                &mut pending_tool_index,
                                &perm_ctx,
                                &tools,
                                &tx,
                            );
                        }
                        Err(e) => {
                            self.spinner.stop();
                            self.engine_busy = false;
                            self.message_list.push(MessageEntry::System {
                                text: format!("Error: {}", e),
                                severity: SystemSeverity::Error,
                            });
                        }
                    }
                }
                AppEvent::LoginOAuthUrl(url) => {
                    // Update the dialog to show the URL (arrives before the callback)
                    if let Some(ref mut dialog) = self.login_dialog {
                        dialog.set_oauth_waiting(Some(url));
                    }
                }
                AppEvent::LoginOAuthResult(result) => {
                    if let Some(ref mut dialog) = self.login_dialog {
                        match result {
                            Ok(msg) => {
                                dialog.set_success(msg);
                                // Refresh active profile in status bar
                                self.active_profile_name = omni_core::auth::profiles::get_active_profile()
                                    .map(|p| p.display_name());
                            }
                            Err(msg) => dialog.set_error(msg),
                        }
                    }
                }
                AppEvent::LoginApiKeyResult(result) => {
                    if let Some(ref mut dialog) = self.login_dialog {
                        match result {
                            Ok(msg) => {
                                dialog.set_success(msg);
                                // Refresh active profile in status bar
                                self.active_profile_name = omni_core::auth::profiles::get_active_profile()
                                    .map(|p| p.display_name());
                            }
                            Err(msg) => dialog.set_error(msg),
                        }
                    }
                }
            }
        }

        // Abort the three long-running infrastructure tasks.
        input_task.abort();
        tick_task.abort();
        spinner_task.abort();

        // Restore terminal state synchronously.
        self.cleanup();

        // Exit immediately instead of returning through the tokio runtime shutdown.
        //
        // If we just return Ok(()), tokio's runtime teardown runs: it drops tasks,
        // joins threads, and flushes async I/O — all of which can take tens to
        // hundreds of milliseconds.  During that window the process is still alive,
        // and some shells (zsh in particular) won't print their prompt until the
        // child process has fully exited.  That is why users had to press Enter.
        //
        // std::process::exit() terminates the process immediately after the OS has
        // seen the terminal-restore writes above, so the shell prompt appears right
        // away.  The Drop impl's cleanup() call becomes a no-op because cleaned_up
        // is already true.
        std::process::exit(0);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // cleanup() is idempotent — safe to call even if run/run_with_engine already called it.
        // This ensures the terminal is always restored on panic or early return.
        self.cleanup();
    }
}

