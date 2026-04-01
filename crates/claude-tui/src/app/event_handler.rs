use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};

use crate::mouse::{translate_mouse_event, FocusTarget, MouseAction};
use crate::widgets::message_list::{MessageEntry, SystemSeverity};

use super::App;

impl App {
    pub(crate) fn handle_key_standalone(&mut self, key: KeyEvent) {
        // Picker overlay intercepts all keys when open.
        if self.active_picker.is_some() {
            use crate::widgets::picker::PickerAction;
            let action = self.active_picker.as_mut().unwrap().handle_key(key);
            match action {
                PickerAction::Selected(_) | PickerAction::Cancelled => {
                    self.active_picker = None;
                }
                PickerAction::None => {}
            }
            return;
        }

        // Config panel overlay intercepts all keys when open.
        if self.config_panel.is_some() {
            use crate::widgets::config_panel::ConfigPanelAction;
            let action = self.config_panel.as_mut().unwrap().handle_key(key.code, key.modifiers);
            match action {
                ConfigPanelAction::Consumed => {}
                ConfigPanelAction::Close { .. } | ConfigPanelAction::Cancel => {
                    self.config_panel = None;
                }
            }
            return;
        }

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

    pub(crate) fn handle_mouse(&mut self, event: MouseEvent) {
        let action = translate_mouse_event(event);
        match action {
            MouseAction::ScrollUp(n) => {
                if !self.message_list.should_debounce_scroll() {
                    self.message_list.scroll_up(n as usize);
                    if let Some(hint) = self.notifications.hint_tracker.record_scroll_up() {
                        self.notifications.show_hint(hint);
                    }
                }
            }
            MouseAction::ScrollDown(n) => {
                if !self.message_list.should_debounce_scroll() {
                    self.message_list.scroll_down(n as usize);
                }
            }
            MouseAction::Click { col, row } => {
                let now = Instant::now();
                let same_pos = self.last_click_pos == (col, row);
                let quick = self
                    .last_click_time
                    .map_or(false, |t| now.duration_since(t) < Duration::from_millis(400));

                if same_pos && quick {
                    self.click_count = (self.click_count + 1).min(3);
                } else {
                    self.click_count = 1;
                }
                self.last_click_time = Some(now);
                self.last_click_pos = (col, row);

                let area = self.terminal.size().unwrap_or_default();

                match self.click_count {
                    2 => {
                        self.selection.select_word_at(col, row);
                    }
                    3 => {
                        self.selection.select_line_at(row, area.width);
                    }
                    _ => {
                        self.selection.start_at(col, row);
                    }
                }

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
                if self.selection.has_selection() {
                    let text = self.extract_selected_text();
                    if !text.is_empty() {
                        crate::mouse::copy_to_clipboard(&text);
                        self.flash_info("Copied to clipboard");
                    }
                }
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
    pub(crate) fn extract_selected_text(&self) -> String {
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

    /// Open the current prompt text in an external editor (`$EDITOR`, `$VISUAL`, or `vi`).
    ///
    /// Temporarily leaves the alternate screen and disables raw mode so the
    /// editor can run normally. On return the edited text replaces the prompt.
    pub(crate) fn open_external_editor(&mut self) {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "vi".to_string());

        let current_text = self.prompt.text();

        // Write current prompt text to a temp file
        let tmp_path = std::env::temp_dir().join(format!(
            "claude-edit-{}.md",
            std::process::id()
        ));

        if let Err(e) = std::fs::write(&tmp_path, &current_text) {
            self.flash(crate::widgets::status_bar::FlashMessage::warning(
                format!("Failed to write temp file: {}", e),
            ));
            return;
        }

        // Leave TUI so the editor can take over the terminal
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            cursor::Show
        );

        // Run the editor
        let status = std::process::Command::new(&editor)
            .arg(&tmp_path)
            .status();

        // Re-enter TUI
        let _ = terminal::enable_raw_mode();
        let _ = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            cursor::Hide
        );
        // Force full redraw after returning from the editor
        let _ = self.terminal.clear();

        match status {
            Ok(s) if s.success() => {
                match std::fs::read_to_string(&tmp_path) {
                    Ok(new_text) => {
                        let trimmed = new_text.trim_end_matches('\n').to_string();
                        self.prompt.clear_buffer();
                        if !trimmed.is_empty() {
                            self.prompt.insert_paste(&trimmed);
                        }
                        self.flash_success("Editor closed -- prompt updated");
                    }
                    Err(e) => {
                        self.flash(crate::widgets::status_bar::FlashMessage::warning(
                            format!("Failed to read edited file: {}", e),
                        ));
                    }
                }
            }
            Ok(s) => {
                self.flash(crate::widgets::status_bar::FlashMessage::warning(
                    format!("Editor exited with status {}", s),
                ));
            }
            Err(e) => {
                self.flash(crate::widgets::status_bar::FlashMessage::warning(
                    format!("Failed to launch editor '{}': {}", editor, e),
                ));
            }
        }

        // Clean up the temp file
        let _ = std::fs::remove_file(&tmp_path);
    }
}
