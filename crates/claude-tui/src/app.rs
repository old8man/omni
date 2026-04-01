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
use ratatui::layout::{Constraint, Layout, Rect};
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

use crate::mouse::{translate_mouse_event, FocusManager, FocusTarget, MouseAction, TextSelection};
use crate::theme::{detect_theme, Theme};
use crate::widgets::message_list::{
    MessageEntry, MessageList, MessageListWidget, SystemSeverity, ToolUseStatus,
};
use crate::widgets::notification::{NotificationManager, NotificationWidget};
use crate::widgets::permission_dialog::PermissionDialog;
use crate::widgets::prompt_input::{InputAction, PromptInput, PromptInputWidget};
use crate::widgets::search_overlay::{SearchAction, SearchOverlay, SearchOverlayWidget};
use crate::widgets::spinner::{SpinnerMode, SpinnerState};

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

pub struct App {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    theme: Theme,
    spinner: SpinnerState,
    should_quit: bool,
    message_list: MessageList,
    prompt: PromptInput,
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
            let mut interval = tokio::time::interval(Duration::from_millis(50));
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
            let mut interval = tokio::time::interval(Duration::from_millis(50));
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
                    // Ctrl+C / Ctrl+D always quits
                    if matches!(
                        (k.modifiers, k.code),
                        (KeyModifiers::CONTROL, KeyCode::Char('c'))
                            | (KeyModifiers::CONTROL, KeyCode::Char('d'))
                    ) {
                        cancel.cancel();
                        self.should_quit = true;
                        continue;
                    }

                    if self.permission_dialog.is_some() {
                        // Route keys to permission dialog
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
                            _ => {}
                        }
                    } else if self.search_overlay.active {
                        // Route keys to search overlay
                        match self.search_overlay.handle_key(k) {
                            SearchAction::QueryChanged(query) => {
                                self.message_list
                                    .set_search(Some(query));
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
                            SearchAction::None => {}
                        }
                    } else {
                        // Ctrl+F opens search overlay
                        if matches!(
                            (k.modifiers, k.code),
                            (KeyModifiers::CONTROL, KeyCode::Char('f'))
                        ) {
                            self.search_overlay.open(None);
                            self.focus.set(FocusTarget::Search);
                            continue;
                        }

                        // Scroll message list with PageUp/PageDown
                        if matches!(k.code, KeyCode::PageUp) {
                            self.message_list.scroll_up(10);
                            continue;
                        }
                        if matches!(k.code, KeyCode::PageDown) {
                            self.message_list.scroll_down(10);
                            continue;
                        }

                        // Route keys to prompt input
                        match self.prompt.handle_key(k) {
                            InputAction::Submit(text) => {
                                let _ = tx.send(AppEvent::SubmitPrompt(text)).await;
                            }
                            InputAction::None => {}
                        }
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
                                                severity: SystemSeverity::Info,
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
                        .push(MessageEntry::User { text: text.clone() });

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
                                severity: SystemSeverity::Info,
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
                                name: info.name.clone(),
                                output: truncate_result(&result_text),
                                is_error,
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
                                name: info.name.clone(),
                                output: "Permission denied".to_string(),
                                is_error: true,
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
                                severity: SystemSeverity::Info,
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
                        severity: SystemSeverity::Info,
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
                if let Some(MessageEntry::Thinking { text: ref mut t }) =
                    self.message_list.messages_mut().last_mut()
                {
                    t.push_str(&text);
                } else {
                    self.message_list.push(MessageEntry::Thinking { text });
                }
            }
            StreamEvent::ToolStart {
                tool_use_id: _,
                name,
                input,
            } => {
                let summary = serde_json::to_string(&input).unwrap_or_else(|_| input.to_string());
                let summary = if summary.len() > 120 {
                    format!("{}...", &summary[..117])
                } else {
                    summary
                };
                self.message_list.push(MessageEntry::ToolUse {
                    name: name.clone(),
                    input_summary: summary,
                });
                self.spinner.start(SpinnerMode::Tool { name });
            }
            StreamEvent::ToolResult {
                tool_use_id: _,
                result,
            } => {
                self.message_list.push(MessageEntry::ToolResult {
                    name: "tool".to_string(),
                    output: truncate_result(
                        result.data.as_str().unwrap_or(&result.data.to_string()),
                    ),
                    is_error: result.is_error,
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
                    severity: SystemSeverity::Info,
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
                    });
                }
            }
            StreamEvent::Compacted { summary } => {
                self.message_list.push(MessageEntry::System {
                    text: format!("Context compacted: {summary}"),
                    severity: SystemSeverity::Info,
                });
            }
            _ => {}
        }
    }

    fn handle_key_standalone(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => self.should_quit = true,
            (KeyModifiers::CONTROL, KeyCode::Char('d')) => self.should_quit = true,
            _ => {
                self.prompt.handle_key(key);
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
            }
            MouseAction::None => {}
        }
    }

    fn render(&mut self) -> Result<()> {
        // Prune expired notifications before rendering
        self.notifications.prune();

        let theme = &self.theme;
        let spinner = &self.spinner;
        let message_list = &self.message_list;
        let prompt = &self.prompt;
        let permission_dialog = &self.permission_dialog;
        let search_overlay = &self.search_overlay;
        let notifications = &self.notifications;
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

        self.terminal.draw(|frame| {
            let area = frame.area();

            // Layout: header separator, header, separator, messages, spinner, separator, input
            let spinner_height = if spinner.active { 1 } else { 0 };
            let chunks = Layout::default()
                .constraints([
                    Constraint::Length(1),              // Top border
                    Constraint::Length(1),              // Header
                    Constraint::Length(1),              // Header separator
                    Constraint::Min(1),                 // Messages
                    Constraint::Length(spinner_height), // Spinner
                    Constraint::Length(3),              // Input (with top border)
                ])
                .split(area);

            // Top border line
            let border_line = "─".repeat(area.width as usize);
            let top_border = Paragraph::new(border_line.clone())
                .style(ratatui::style::Style::default().fg(theme.border));
            frame.render_widget(top_border, chunks[0]);

            // Header: "Claude Code (Rust) | model: ... | N tokens | $X.XX | PLAN"
            let token_str = if state_tokens > 0 {
                format!("{} tokens", state_tokens)
            } else {
                "0 tokens".to_string()
            };
            let mut header_spans = vec![
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
                    format!("model: {}", state_model),
                    ratatui::style::Style::default().fg(theme.muted),
                ),
                ratatui::text::Span::styled(
                    " | ",
                    ratatui::style::Style::default().fg(theme.border),
                ),
                ratatui::text::Span::styled(
                    token_str,
                    ratatui::style::Style::default().fg(theme.muted),
                ),
            ];
            if state_cost > 0.0 {
                header_spans.push(ratatui::text::Span::styled(
                    " | ",
                    ratatui::style::Style::default().fg(theme.border),
                ));
                let cost_str = if state_cost < 0.01 {
                    format!("${:.4}", state_cost)
                } else if state_cost < 1.0 {
                    format!("${:.3}", state_cost)
                } else {
                    format!("${:.2}", state_cost)
                };
                header_spans.push(ratatui::text::Span::styled(
                    cost_str,
                    ratatui::style::Style::default().fg(theme.muted),
                ));
            }
            if state_plan {
                header_spans.push(ratatui::text::Span::styled(
                    " | ",
                    ratatui::style::Style::default().fg(theme.border),
                ));
                header_spans.push(ratatui::text::Span::styled(
                    "PLAN",
                    ratatui::style::Style::default()
                        .fg(ratatui::style::Color::Yellow)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ));
            }
            if state_vim {
                header_spans.push(ratatui::text::Span::styled(
                    " | ",
                    ratatui::style::Style::default().fg(theme.border),
                ));
                header_spans.push(ratatui::text::Span::styled(
                    "VIM",
                    ratatui::style::Style::default()
                        .fg(ratatui::style::Color::Blue)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ));
            }
            let header = Paragraph::new(ratatui::text::Line::from(header_spans));
            frame.render_widget(header, chunks[1]);

            // Header separator
            let header_sep = Paragraph::new(border_line)
                .style(ratatui::style::Style::default().fg(theme.border));
            frame.render_widget(header_sep, chunks[2]);

            // Messages area
            let msg_widget = MessageListWidget::new(message_list);
            frame.render_widget(msg_widget, chunks[3]);

            // Spinner
            if spinner.active {
                frame.render_widget(spinner, chunks[4]);
            }

            // Input
            let input_widget = PromptInputWidget::new(prompt);
            frame.render_widget(input_widget, chunks[5]);

            // Permission dialog overlay
            if let Some(dialog) = permission_dialog {
                let dialog_area = centered_rect(60, 10, area);
                frame.render_widget(dialog, dialog_area);
            }

            // Search overlay (at bottom of messages area)
            if search_overlay.active {
                let search_area = Rect::new(
                    chunks[3].x,
                    chunks[3].y + chunks[3].height.saturating_sub(1),
                    chunks[3].width,
                    1,
                );
                let search_widget = SearchOverlayWidget::new(search_overlay);
                frame.render_widget(search_widget, search_area);
            }

            // Notification popups (top-right corner)
            if notifications.has_active() {
                let notif_area = Rect::new(
                    area.x,
                    area.y + 1, // below top border
                    area.width,
                    3.min(area.height.saturating_sub(1)),
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

/// Calculate a centered rect within the given area.
fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let width = area.width * percent_x / 100;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height.min(area.height))
}
