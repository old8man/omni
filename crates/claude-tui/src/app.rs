use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CrosstermEvent, KeyCode, KeyEvent,
    KeyModifiers, MouseEvent,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use claude_core::commands::{CommandContext, CommandRegistry, CommandResult};
use claude_core::permissions::evaluator::evaluate_permission_sync;
use claude_core::permissions::types::{PermissionBehavior, PermissionMode, ToolPermissionContext};
use claude_core::query::engine::{QueryEngine, ToolUseInfo, TurnResult};
use claude_core::services::lsp_service::LspManager;
use claude_core::services::notifications::{self, NotificationChannel, NotificationOptions};
use claude_core::services::prompt_suggestion;
use claude_core::services::tips::{self, TipScheduler};
use claude_core::services::tool_use_summary::{self, ToolInfo as SummaryToolInfo};
use claude_core::types::events::{StreamEvent, ToolResultData};
use claude_tools::{ToolRegistry, ToolUseContext};

use crate::input::{InputHandler, InputMode, InputResult};
use crate::mouse::{translate_mouse_event, FocusManager, FocusTarget, MouseAction, TextSelection};
use crate::theme::{detect_theme, Theme};
use crate::widgets::message_list::{
    MessageEntry, MessageList, MessageListWidget, SystemSeverity, ToolUseStatus,
};
use crate::widgets::notification::{NotificationManager, NotificationWidget};
use crate::widgets::permission_dialog::PermissionDialog;
use crate::widgets::prompt_input::{PromptInput, PromptInputWidget};
use crate::widgets::search_overlay::{SearchAction, SearchOverlay, SearchOverlayWidget};
use crate::widgets::spinner::{SpinnerMode, SpinnerState, SPINNER_TICK_MS};
use crate::widgets::status_bar::{StatusBarState, StatusBarWidget};
use crate::widgets::welcome::{WelcomeState, WelcomeWidget};

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
    /// Fired after all tool results have been added — tells the main loop to
    /// call `engine.run_turn()` and handle the next result (which may be
    /// another round of tool use or a final answer).
    ContinueTurn,
}

/// Pending tool that needs permission before execution.
struct PendingTool {
    info: ToolUseInfo,
}

/// State for the Ctrl+E message action popup menu.
struct ActionMenu {
    /// The assistant message text being acted upon.
    text: String,
    /// The message index in the message list.
    msg_index: usize,
}

pub struct App {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    theme: Theme,
    spinner: SpinnerState,
    should_quit: bool,
    message_list: MessageList,
    prompt: PromptInput,
    /// Input handler (vim mode + emacs mode routing)
    input_handler: InputHandler,
    permission_dialog: Option<PermissionDialog>,
    /// True while the engine is processing (prevents double-submit)
    engine_busy: bool,
    /// Model name for display in the header
    model_name: String,
    /// Running total of tokens used in this session
    total_tokens: u64,
    /// Running total cost in USD this session
    total_cost: f64,
    /// Slash-command registry
    command_registry: CommandRegistry,
    /// Whether vim mode is enabled
    vim_mode: bool,
    /// Whether plan mode is enabled
    plan_mode: bool,
    /// Search overlay state
    search_overlay: SearchOverlay,
    /// Text selection state (for mouse selection)
    selection: TextSelection,
    /// Focus manager
    focus: FocusManager,
    /// Notification manager
    notifications: NotificationManager,
    /// Pending input from Prompt-type commands (injected into the next turn)
    pending_input: Option<String>,
    /// Message action menu state (Ctrl+E popup)
    action_menu: Option<ActionMenu>,
    /// Tip scheduler for showing tips in the spinner
    tip_scheduler: TipScheduler,
    /// Terminal notification channel (auto-detect)
    notification_channel: NotificationChannel,
    /// Accumulated tool infos for summary generation in the current turn
    turn_tool_infos: Vec<SummaryToolInfo>,
    /// LSP manager for notifying language servers of file changes
    lsp_manager: LspManager,
    /// Shared application state (for live cost/usage display from engine).
    app_state: Option<claude_core::state::AppStateStore>,
}

impl App {
    pub fn new() -> Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
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
        })
    }

    /// Set the model name displayed in the header.
    pub fn set_model_name(&mut self, name: &str) {
        self.model_name = name.to_string();
    }

    /// Attach shared application state for live cost display.
    pub fn set_app_state(&mut self, state: claude_core::state::AppStateStore) {
        self.app_state = Some(state);
    }

    /// Original standalone run loop (no engine). Kept for backwards compatibility.
    pub async fn run(&mut self) -> Result<()> {
        terminal::enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            cursor::Hide
        )?;

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

        terminal::disable_raw_mode()?;
        execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            cursor::Show
        )?;
        Ok(())
    }

    /// Run the TUI wired to the QueryEngine.
    pub async fn run_with_engine(
        &mut self,
        mut engine: QueryEngine,
        tools: ToolRegistry,
        cancel: CancellationToken,
        permission_mode: PermissionMode,
    ) -> Result<()> {
        terminal::enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            cursor::Hide
        )?;

        let (tx, mut rx) = mpsc::channel::<AppEvent>(256);

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

        let cwd = if let Some(ref state) = self.app_state {
            state.read().cwd.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };

        // Wire streaming tool execution into the engine
        {
            let tool_map: std::sync::Arc<
                std::collections::HashMap<String, std::sync::Arc<dyn claude_tools::ToolExecutor>>,
            > = std::sync::Arc::new(
                tools
                    .all()
                    .into_iter()
                    .map(|t| (t.name().to_string(), t))
                    .collect(),
            );
            let cwd_clone = cwd.clone();
            let tool_call_fn: claude_core::query::tool_executor::ToolCallFn =
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
                                None => Ok(claude_core::types::events::ToolResultData {
                                    data: serde_json::json!(format!("Unknown tool: {}", name)),
                                    is_error: true,
                                }),
                            }
                        })
                    },
                );
            engine.set_tool_call_fn(tool_call_fn);
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
                            // Empty input, no request — hint user
                            self.message_list.push(MessageEntry::System {
                                text: "Press Ctrl+D to quit, or type a message.".to_string(),
                                severity: SystemSeverity::Info,
                            });
                        }
                        continue;
                    }

                    // Ctrl+D: quit only on empty input
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('d'))
                    ) {
                        if self.prompt.is_empty() {
                            cancel.cancel();
                            self.should_quit = true;
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

                    // --- Escape: close dialogs, exit vim mode, cancel search ---
                    if k.code == KeyCode::Esc {
                        if self.prompt.completion.is_some() {
                            self.prompt.completion = None;
                        } else if self.prompt.history_search.active {
                            self.prompt.history_search.active = false;
                        } else {
                            // Pass Escape through to input handler (for vim normal mode)
                            self.input_handler.handle_key(k, &mut self.prompt);
                        }
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

                    // --- Scroll message list with PageUp/PageDown/Home/End ---
                    if matches!(k.code, KeyCode::PageUp) {
                        self.message_list.scroll_up(10);
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
                    if self.input_handler.mode() == InputMode::Normal {
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
                                    engine.clear_messages();
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
                                    match engine.compact(&stream_tx).await {
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
                                        claude_core::session::SessionManager::project_dir_for_cwd(
                                            &cwd_str,
                                        );
                                    let mgr =
                                        claude_core::session::SessionManager::new(project_dir);
                                    match mgr.load_session(&id) {
                                        Ok(session) => {
                                            // Restore messages into the engine
                                            engine.clear_messages();
                                            for msg in &session.messages {
                                                engine.add_raw_message(msg.clone());
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
                            }
                            continue;
                        }
                        // Unknown slash command — fall through to send as regular message
                    }

                    // Add user message to display
                    self.message_list
                        .push(MessageEntry::User { text: text.clone(), images: vec![] });

                    // Add to engine
                    engine.add_user_message(&text);
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

                    match engine.run_turn(&stream_tx).await {
                        Ok(TurnResult::Done(_stop_reason)) => {
                            self.spinner.stop();
                            self.engine_busy = false;
                            // Send terminal notification that the task completed
                            let _ = notifications::send_notification(
                                &NotificationOptions {
                                    message: "Response complete".to_string(),
                                    title: Some("Claude Code".to_string()),
                                    notification_type: "turn_complete".to_string(),
                                },
                                &self.notification_channel,
                            );
                        }
                        Ok(TurnResult::ToolUse(tool_uses)) => {
                            // Start processing tool uses
                            pending_tools = tool_uses
                                .into_iter()
                                .map(|info| PendingTool { info })
                                .collect();
                            pending_tool_index = 0;
                            // Kick off permission check for first tool
                            self.check_next_tool_permission(
                                &pending_tools,
                                &mut pending_tool_index,
                                &perm_ctx,
                                &tools,
                                &tx,
                            );
                        }
                        Ok(TurnResult::ContinueRecovery)
                        | Ok(TurnResult::StopHookBlocking)
                        | Ok(TurnResult::TokenBudgetContinuation) => {
                            // Re-run the turn (recovery, stop-hook block, or budget continuation)
                            self.message_list.push(MessageEntry::System {
                                text: "Continuing...".to_string(),
                                severity: SystemSeverity::Info,
                            });
                            let tx2 = tx.clone();
                            tokio::spawn(async move {
                                let _ = tx2.send(AppEvent::ContinueTurn).await;
                            });
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
                            engine.add_tool_result(&info.id, &result_text, is_error);
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
                                // All tools done — fire ContinueTurn to re-enter the engine
                                let tx2 = tx.clone();
                                tokio::spawn(async move {
                                    let _ = tx2.send(AppEvent::ContinueTurn).await;
                                });
                            }
                        }
                        "deny" => {
                            let info = &pending_tools[tool_idx].info;
                            engine.add_tool_result(&info.id, "Permission denied by user", true);
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
                                // All tools done — fire ContinueTurn to re-enter the engine
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
                    // All pending tool results have been fed back — run the next turn.
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

                    match engine.run_turn(&stream_tx).await {
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
                            // Another round of tool use — re-enter the permission/execute cycle
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
            }
        }

        // Cleanup
        terminal::disable_raw_mode()?;
        execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            cursor::Show
        )?;
        Ok(())
    }

    /// Check permissions for the next tool in the pending list.
    /// If the tool is auto-allowed, execute it immediately and advance.
    /// If it needs user permission, show the dialog.
    fn check_next_tool_permission(
        &mut self,
        pending_tools: &[PendingTool],
        pending_tool_index: &mut usize,
        perm_ctx: &ToolPermissionContext,
        tools: &ToolRegistry,
        tx: &mpsc::Sender<AppEvent>,
    ) {
        if *pending_tool_index < pending_tools.len() {
            let tool = &pending_tools[*pending_tool_index];
            let info = &tool.info;
            *pending_tool_index += 1;

            // Determine if tool is read-only
            let is_read_only = tools
                .get(&info.name)
                .map(|t| t.is_read_only(&info.input))
                .unwrap_or(false);

            let decision =
                evaluate_permission_sync(&info.name, &info.input, perm_ctx, is_read_only);

            match decision.behavior {
                PermissionBehavior::Allow => {
                    // Auto-execute: send an "allow" permission response immediately
                    let tx2 = tx.clone();
                    tokio::spawn(async move {
                        let _ = tx2
                            .send(AppEvent::PermissionResponse("allow".to_string()))
                            .await;
                    });
                }
                PermissionBehavior::Ask => {
                    let message = decision.message.unwrap_or_else(|| "Permission required".to_string());
                    let input_preview = serde_json::to_string_pretty(&info.input)
                        .unwrap_or_else(|_| info.input.to_string());
                    self.permission_dialog = Some(PermissionDialog::new(
                        info.name.clone(),
                        message,
                        input_preview,
                    ));
                }
                PermissionBehavior::Deny => {
                    let message = decision.message.unwrap_or_else(|| "Denied".to_string());
                    // Auto-deny, send deny response
                    let tx2 = tx.clone();
                    tokio::spawn(async move {
                        let _ = tx2
                            .send(AppEvent::PermissionResponse("deny".to_string()))
                            .await;
                    });
                    self.message_list.push(MessageEntry::System {
                        text: format!("Denied: {}", message),
                        severity: SystemSeverity::Warning,
                    });
                }
            }
        }
    }

    fn handle_stream_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::TextDelta { text } => {
                // Append to current assistant message, or create one
                if let Some(MessageEntry::Assistant { text: ref mut t }) =
                    self.message_list.messages_mut().last_mut()
                {
                    t.push_str(&text);
                } else {
                    self.message_list.push(MessageEntry::Assistant { text });
                }
            }
            StreamEvent::ThinkingDelta { text } => {
                if let Some(MessageEntry::Thinking { text: ref mut t, .. }) =
                    self.message_list.messages_mut().last_mut()
                {
                    t.push_str(&text);
                } else {
                    self.message_list.push(MessageEntry::Thinking {
                        text,
                        is_collapsed: true,
                    });
                }
            }
            StreamEvent::ToolStart {
                tool_use_id,
                name,
                input,
            } => {
                self.message_list.push(MessageEntry::ToolUse {
                    id: tool_use_id,
                    name: name.clone(),
                    input,
                    status: ToolUseStatus::Running,
                });
                self.spinner.start(SpinnerMode::Tool { name });
            }
            StreamEvent::ToolResult {
                tool_use_id,
                result,
            } => {
                // Update the corresponding ToolUse status
                let status = if result.is_error {
                    ToolUseStatus::Error
                } else {
                    ToolUseStatus::Complete
                };
                self.message_list.update_tool_status(&tool_use_id, status);
                self.message_list.push(MessageEntry::ToolResult {
                    id: tool_use_id,
                    name: "tool".to_string(),
                    output: truncate_result(
                        result.data.as_str().unwrap_or(&result.data.to_string()),
                    ),
                    is_error: result.is_error,
                    duration_ms: None,
                });
            }
            StreamEvent::Done { stop_reason: _ } => {
                self.spinner.stop();
            }
            StreamEvent::UsageUpdate(usage) => {
                self.spinner.tokens = usage.output_tokens;
                self.total_tokens = self.total_tokens.saturating_add(usage.output_tokens);
                // Sync cost from shared state (updated by engine via CostTracker)
                if let Some(ref state) = self.app_state {
                    if let Some(s) = state.try_read() {
                        self.total_cost = s.total_cost_usd();
                    }
                }
            }
            StreamEvent::RequestStart { request_id: _ } => {
                self.spinner.start(SpinnerMode::Thinking);
            }
            StreamEvent::Error(err) => {
                self.message_list.push(MessageEntry::System {
                    text: format!("Error: {}", err),
                    severity: SystemSeverity::Error,
                });
            }
            StreamEvent::RetryWait {
                attempt,
                delay_ms,
                status,
            } => {
                if status == 529 && delay_ms == 0 {
                    // 529 fallback — switching to non-streaming
                    self.spinner.start(SpinnerMode::Thinking);
                    self.message_list.push(MessageEntry::System {
                        text: "API overloaded, falling back to non-streaming request...".into(),
                        severity: SystemSeverity::Info,
                    });
                } else {
                    let status_label = if status == 0 {
                        "network error".to_string()
                    } else {
                        format!("HTTP {status}")
                    };
                    self.message_list.push(MessageEntry::System {
                        text: format!(
                            "Retrying ({status_label}, attempt {attempt}, waiting {delay_ms}ms)..."
                        ),
                        severity: SystemSeverity::Warning,
                    });
                }
            }
            StreamEvent::Compacted { summary } => {
                self.message_list.push(MessageEntry::CompactBoundary { summary });
            }
            _ => {}
        }
    }

    fn handle_key_standalone(&mut self, key: KeyEvent) {
        // Cmd key shortcuts (macOS)
        if key.modifiers.contains(KeyModifiers::SUPER) {
            match key.code {
                KeyCode::Char('c') => {
                    let text = self.prompt.text();
                    if !text.is_empty() {
                        crate::mouse::copy_to_clipboard(&text);
                    }
                    return;
                }
                KeyCode::Char('v') => {
                    self.prompt.paste_clipboard();
                    return;
                }
                KeyCode::Char('a') => {
                    self.prompt.select_all();
                    return;
                }
                KeyCode::Char('k') => {
                    self.message_list.clear();
                    let _ = self.terminal.clear();
                    return;
                }
                _ => {}
            }
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                // In standalone mode (no engine), Ctrl+C clears input or hints
                if !self.prompt.is_empty() {
                    self.prompt.clear();
                } else {
                    self.message_list.push(MessageEntry::System {
                        text: "Press Ctrl+D to quit.".to_string(),
                        severity: SystemSeverity::Info,
                    });
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                if self.prompt.is_empty() {
                    self.should_quit = true;
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('l')) => {
                let _ = self.terminal.clear();
            }
            _ => {
                // Sync vim mode state
                if self.vim_mode != self.input_handler.vim_enabled() {
                    self.input_handler.set_vim_enabled(self.vim_mode);
                }
                self.input_handler.handle_key(key, &mut self.prompt);
            }
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent) {
        let action = translate_mouse_event(event);
        match action {
            MouseAction::ScrollUp(n) => {
                self.message_list.scroll_up(n as usize);
            }
            MouseAction::ScrollDown(n) => {
                self.message_list.scroll_down(n as usize);
            }
            MouseAction::Click { col, row } => {
                self.selection.start_at(col, row);
                let area = self.terminal.size().unwrap_or_default();
                if row >= area.height.saturating_sub(3) {
                    self.focus.set(FocusTarget::Prompt);
                } else {
                    self.focus.set(FocusTarget::Messages);
                }
            }
            MouseAction::Drag { col, row } => {
                self.selection.extend_to(col, row);
            }
            MouseAction::Release { col, row } => {
                self.selection.extend_to(col, row);
                self.selection.finalize();
                // Selection is now visible (highlighted in next render).
                // User can copy with Ctrl+C / Cmd+C.
            }
            MouseAction::RightClick { col, row } => {
                // Right-click: position cursor at click
                self.selection.start_at(col, row);
                self.selection.finalize();
            }
            MouseAction::CtrlClick { .. } => {
                // Ctrl+click: reserved for future use (e.g. open file)
            }
            MouseAction::None => {}
        }
    }

    /// Extract visible text from message entries within the selection area.
    /// Since we can't access the terminal's internal buffer after draw(),
    /// we reconstruct text from the message_list entries.
    fn extract_selected_text(&self) -> String {
        if !self.selection.has_selection() {
            return String::new();
        }
        // Collect all message text — each entry is a "line" in the viewport
        let mut all_text = String::new();
        for entry in self.message_list.entries() {
            let text = match entry {
                MessageEntry::User { text, .. } => text.clone(),
                MessageEntry::Assistant { text } => text.clone(),
                MessageEntry::ToolResult { output, .. } => output.clone(),
                MessageEntry::Thinking { text, .. } => text.clone(),
                MessageEntry::System { text, .. } => text.clone(),
                MessageEntry::CommandOutput { output, .. } => output.clone(),
                MessageEntry::CompactBoundary { summary } => summary.clone(),
                MessageEntry::DiffPreview { diff_text, .. } => diff_text.clone(),
                _ => String::new(),
            };
            if !all_text.is_empty() {
                all_text.push('\n');
            }
            all_text.push_str(&text);
        }
        // For now return all visible text — full buffer-based selection
        // requires storing the rendered buffer which we'll add later
        all_text
    }

    fn render(&mut self) -> Result<()> {
        // Capture vim mode indicator for rendering
        let vim_mode_str: Option<String> = if self.vim_mode {
            Some(self.input_handler.mode().to_string())
        } else {
            None
        };
        // Compute dynamic input area height based on line count
        let _input_area_height = (self.prompt.line_count() as u16).clamp(1, 10) + 1;
        // Prune expired notifications before rendering
        self.notifications.prune();

        let theme = &self.theme;
        let spinner = &self.spinner;
        let message_list = &self.message_list;
        let prompt = &self.prompt;
        let permission_dialog = &self.permission_dialog;
        let action_menu = &self.action_menu;
        let search_overlay = &self.search_overlay;
        let notifications = &self.notifications;
        let input_handler = &self.input_handler;
        let selection = &self.selection;
        let show_welcome = message_list.is_empty() && !spinner.active;
        // Read live state from AppStateStore (non-blocking try_read to avoid
        // stalling the 60fps render loop if the engine holds a write lock).
        let (state_model, state_tokens, state_cost, state_plan, state_vim) =
            if let Some(ref state) = self.app_state {
                if let Some(s) = state.try_read() {
                    (
                        s.active_model().to_string(),
                        s.total_input_tokens() + s.total_output_tokens(),
                        s.total_cost_usd(),
                        s.is_plan_mode(),
                        s.vim_mode,
                    )
                } else {
                    (
                        self.model_name.clone(),
                        self.total_tokens,
                        self.total_cost,
                        self.plan_mode,
                        self.vim_mode,
                    )
                }
            } else {
                (
                    self.model_name.clone(),
                    self.total_tokens,
                    self.total_cost,
                    self.plan_mode,
                    self.vim_mode,
                )
            };

        // Compute input line count for dynamic input area sizing
        let input_line_count = prompt.line_count().max(1) as u16;
        let spinner_height = spinner.render_height();

        self.terminal.draw(|frame| {
            let area = frame.area();

            // Use the new layout system: Header | Separator | Messages | Spinner | Input | StatusBar
            let layout = crate::layout::TuiLayout::compute(
                area,
                spinner_height,
                input_line_count,
                true, // always show status bar
            );

            // Header: "Claude Code (Rust) | model"
            let header_spans = vec![
                ratatui::text::Span::styled(
                    " Claude Code (Rust)",
                    ratatui::style::Style::default()
                        .fg(theme.accent)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ),
                ratatui::text::Span::styled(
                    " | ",
                    ratatui::style::Style::default().fg(theme.border),
                ),
                ratatui::text::Span::styled(
                    &state_model,
                    ratatui::style::Style::default().fg(theme.muted),
                ),
            ];
            let header = Paragraph::new(ratatui::text::Line::from(header_spans));
            frame.render_widget(header, layout.header);

            // Header separator
            let border_line = "\u{2500}".repeat(area.width as usize);
            let header_sep = Paragraph::new(border_line)
                .style(ratatui::style::Style::default().fg(theme.border));
            frame.render_widget(header_sep, layout.header_separator);

            // Messages area (or welcome screen for new sessions)
            if show_welcome {
                let welcome_state = WelcomeState::new(state_model.clone());
                let welcome_widget = WelcomeWidget::new(&welcome_state);
                frame.render_widget(welcome_widget, layout.messages);
            } else {
                let msg_widget = MessageListWidget::new(message_list);
                frame.render_widget(msg_widget, layout.messages);

                // Apply selection highlighting over the message area
                if selection.has_selection() {
                    let ((sc, sr), (ec, er)) = selection.normalized();
                    let msg_area = layout.messages;
                    for row in sr..=er {
                        if row < msg_area.y || row >= msg_area.y + msg_area.height {
                            continue;
                        }
                        let col_start = if row == sr { sc } else { msg_area.x };
                        let col_end = if row == er { ec } else { msg_area.x + msg_area.width - 1 };
                        for col in col_start..=col_end {
                            if col >= msg_area.x + msg_area.width {
                                break;
                            }
                            if let Some(cell) = frame.buffer_mut().cell_mut(ratatui::layout::Position::new(col, row)) {
                                // Invert foreground/background for selection highlight
                                let fg = cell.fg;
                                let bg = cell.bg;
                                cell.fg = if bg == ratatui::style::Color::Reset {
                                    ratatui::style::Color::Black
                                } else {
                                    bg
                                };
                                cell.bg = if fg == ratatui::style::Color::Reset {
                                    ratatui::style::Color::White
                                } else {
                                    fg
                                };
                            }
                        }
                    }
                }
            }

            // Spinner
            if spinner.active {
                frame.render_widget(spinner, layout.spinner);
            }

            // Input
            let mut input_widget = PromptInputWidget::new(prompt);
            if let Some(ref mode_str) = vim_mode_str {
                input_widget = input_widget.vim_mode(mode_str);
            }
            frame.render_widget(input_widget, layout.input);

            // Set cursor position in the input area so it's visible
            if permission_dialog.is_none() && !search_overlay.active {
                let cursor_pos = prompt.cursor_pos();
                let prompt_prefix_len: u16 = 2; // "> "
                // Convert byte offset to display column width (handles UTF-8 properly)
                let text_before_cursor = &prompt.lines()[cursor_pos.row][..cursor_pos.col];
                let display_col = crate::unicode_width::display_width(text_before_cursor) as u16;
                let cursor_x = layout.input.x + prompt_prefix_len + display_col;
                // +1 for the top border of the input block
                let cursor_y = layout.input.y + 1 + cursor_pos.row as u16;
                if cursor_x < layout.input.x + layout.input.width
                    && cursor_y < layout.input.y + layout.input.height
                {
                    frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
                }
            }

            // Completion popup (rendered above input area so it's not clipped)
            if let Some(ref comp) = prompt.completion {
                let max_visible = comp.items.len().min(10);
                let popup_height = max_visible as u16;
                // Position popup above the input area
                let popup_y = layout.input.y.saturating_sub(popup_height);
                let popup_x = layout.input.x + 2; // align with "> " prefix

                // Scroll window so selected item is always visible
                let scroll_start = if comp.selected >= max_visible {
                    comp.selected - max_visible + 1
                } else {
                    0
                };

                // Compute widths from ALL items (not just visible window)
                let max_label_w = comp.items.iter()
                    .map(|item| item.label.len())
                    .max()
                    .unwrap_or(10)
                    .min(30);
                let max_desc_w = comp.items.iter()
                    .filter_map(|item| item.description.as_ref())
                    .map(|d| d.len())
                    .max()
                    .unwrap_or(0)
                    .min(40);
                let entry_width = (max_label_w + max_desc_w + 4).min(area.width as usize - 4) as u16;

                for (display_idx, item_idx) in (scroll_start..comp.items.len())
                    .enumerate()
                    .take(max_visible)
                {
                    let item = &comp.items[item_idx];
                    let y = popup_y + display_idx as u16;
                    if y >= layout.input.y {
                        break;
                    }
                    let is_selected = item_idx == comp.selected;
                    let label_style = if is_selected {
                        ratatui::style::Style::default()
                            .fg(ratatui::style::Color::Black)
                            .bg(ratatui::style::Color::Cyan)
                            .add_modifier(ratatui::style::Modifier::BOLD)
                    } else {
                        ratatui::style::Style::default()
                            .fg(ratatui::style::Color::White)
                            .bg(ratatui::style::Color::Rgb(40, 40, 50))
                    };
                    let desc_style = if is_selected {
                        ratatui::style::Style::default()
                            .fg(ratatui::style::Color::DarkGray)
                            .bg(ratatui::style::Color::Cyan)
                    } else {
                        ratatui::style::Style::default()
                            .fg(ratatui::style::Color::Gray)
                            .bg(ratatui::style::Color::Rgb(40, 40, 50))
                    };

                    let label = format!(" {:<width$}", item.label, width = max_label_w);
                    let desc = item.description.as_deref()
                        .map(|d| format!("  {}", d))
                        .unwrap_or_default();
                    let pad_len = entry_width as usize - label.len().min(entry_width as usize) - desc.len().min(entry_width as usize);
                    let padding = " ".repeat(pad_len.min(20));

                    let line = ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(label, label_style),
                        ratatui::text::Span::styled(desc, desc_style),
                        ratatui::text::Span::styled(padding, label_style),
                    ]);
                    frame.render_widget(
                        ratatui::widgets::Paragraph::new(line),
                        ratatui::layout::Rect::new(popup_x, y, entry_width, 1),
                    );
                }

                // Scroll indicators
                let indicator_x = popup_x + entry_width;
                if indicator_x < area.width {
                    if scroll_start > 0 {
                        // Show "▲" at top
                        let up_span = ratatui::text::Span::styled(
                            "▲",
                            ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
                        );
                        frame.render_widget(
                            ratatui::widgets::Paragraph::new(ratatui::text::Line::from(up_span)),
                            ratatui::layout::Rect::new(indicator_x, popup_y, 2, 1),
                        );
                    }
                    if scroll_start + max_visible < comp.items.len() {
                        // Show "▼" at bottom
                        let down_y = popup_y + popup_height.saturating_sub(1);
                        let down_span = ratatui::text::Span::styled(
                            "▼",
                            ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
                        );
                        frame.render_widget(
                            ratatui::widgets::Paragraph::new(ratatui::text::Line::from(down_span)),
                            ratatui::layout::Rect::new(indicator_x, down_y, 2, 1),
                        );
                    }
                }
            }

            // Status bar (full-featured, bottom of screen)
            let status_state = StatusBarState {
                model_name: state_model.clone(),
                total_tokens: state_tokens,
                total_cost: state_cost,
                input_mode: input_handler.mode(),
                vim_enabled: state_vim,
                plan_mode: state_plan,
                context_percent: 0.0, // TODO: wire context window tracking
                session_name: None,   // TODO: wire session name from state
                rate_limited: false,  // TODO: wire rate limit detection
            };
            let status_widget = StatusBarWidget::new(&status_state);
            frame.render_widget(status_widget, layout.status_bar);

            // Permission dialog overlay (sized for JSON preview)
            if let Some(dialog) = permission_dialog {
                let dialog_height = (area.height * 60 / 100).max(12).min(area.height);
                let dialog_area = crate::layout::centered_rect(70, dialog_height, area);
                frame.render_widget(dialog, dialog_area);
            }

            // Action menu overlay (Ctrl+E popup)
            if action_menu.is_some() {
                let menu_width = 40u16.min(area.width.saturating_sub(4));
                let menu_height = 7u16.min(area.height.saturating_sub(4));
                let menu_x = area.width.saturating_sub(menu_width) / 2;
                let menu_y = area.height.saturating_sub(menu_height) / 2;
                let menu_area = Rect::new(menu_x, menu_y, menu_width, menu_height);

                // Render a Clear widget first to blank the area
                frame.render_widget(ratatui::widgets::Clear, menu_area);

                let block = ratatui::widgets::Block::default()
                    .title(" Message Actions ")
                    .borders(ratatui::widgets::Borders::ALL)
                    .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Cyan))
                    .style(ratatui::style::Style::default().bg(ratatui::style::Color::Rgb(30, 30, 40)));

                let menu_lines = vec![
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [C] ", ratatui::style::Style::default().fg(ratatui::style::Color::Cyan).add_modifier(ratatui::style::Modifier::BOLD)),
                        ratatui::text::Span::raw("Copy to clipboard"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [E] ", ratatui::style::Style::default().fg(ratatui::style::Color::Cyan).add_modifier(ratatui::style::Modifier::BOLD)),
                        ratatui::text::Span::raw("Edit (put in input)"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [R] ", ratatui::style::Style::default().fg(ratatui::style::Color::Cyan).add_modifier(ratatui::style::Modifier::BOLD)),
                        ratatui::text::Span::raw("Rewind to here"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [S] ", ratatui::style::Style::default().fg(ratatui::style::Color::Cyan).add_modifier(ratatui::style::Modifier::BOLD)),
                        ratatui::text::Span::raw("Summarize"),
                    ]),
                    ratatui::text::Line::from(ratatui::text::Span::styled(
                        " Esc to close",
                        ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
                    )),
                ];
                let menu_paragraph = ratatui::widgets::Paragraph::new(menu_lines).block(block);
                frame.render_widget(menu_paragraph, menu_area);
            }

            // Search overlay (at bottom of messages area)
            if search_overlay.active {
                let search_area = Rect::new(
                    layout.messages.x,
                    layout.messages.y + layout.messages.height.saturating_sub(1),
                    layout.messages.width,
                    1,
                );
                let search_widget = SearchOverlayWidget::new(search_overlay);
                frame.render_widget(search_widget, search_area);
            }

            // Notification popups (top-right corner, below header)
            if notifications.has_active() {
                let notif_area = Rect::new(
                    area.x,
                    area.y + 2, // below header + separator
                    area.width,
                    3.min(area.height.saturating_sub(2)),
                );
                let notif_widget = NotificationWidget::new(notifications);
                frame.render_widget(notif_widget, notif_area);
            }
        })?;
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            cursor::Show
        );
    }
}

/// Execute a tool call.
async fn execute_tool(
    tools: &ToolRegistry,
    name: &str,
    input: &serde_json::Value,
    cwd: &std::path::Path,
    cancel: CancellationToken,
) -> Result<ToolResultData> {
    let executor = tools
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?;
    let ctx = ToolUseContext::with_working_directory(cwd.to_path_buf());
    executor.call(input, &ctx, cancel, None).await
}

/// Truncate long tool results for display.
fn truncate_result(s: &str) -> String {
    const MAX_DISPLAY: usize = 2000;
    if s.len() <= MAX_DISPLAY {
        s.to_string()
    } else {
        format!("{}... ({} chars total)", &s[..MAX_DISPLAY], s.len())
    }
}


