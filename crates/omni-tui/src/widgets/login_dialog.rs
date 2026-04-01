//! Login dialog overlay widget.
//!
//! Provides a centered dialog for authentication: choose between
//! Claude.ai Subscription (OAuth) or Console API Key, then walks
//! through the selected flow with appropriate UI for each phase.

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::theme;

// Braille spinner frames for the OAuth waiting phase.
const SPINNER_FRAMES: &[&str] = &[
    "\u{2839}", "\u{2838}", "\u{2834}", "\u{2826}", "\u{2807}", "\u{280b}", "\u{2819}", "\u{2839}",
];

/// Phase of the login flow.
pub enum LoginPhase {
    /// Select subscription vs API key.
    ChooseMethod,
    /// Browser open, spinner while waiting for OAuth callback.
    OAuthWaiting { url: Option<String> },
    /// Text input for pasting an API key.
    ApiKeyInput,
    /// Login succeeded.
    Success(String),
    /// Login failed.
    Error(String),
}

/// Result of handling a key event in the login dialog.
pub enum LoginDialogAction {
    /// The dialog consumed the key; keep it open.
    Consumed,
    /// Close the dialog without further action.
    Close,
    /// Start the OAuth flow (caller should spawn the async task).
    StartOAuth,
    /// Submit the entered API key for validation and saving.
    SubmitApiKey(String),
}

/// Login dialog state.
pub struct LoginDialog {
    /// Current phase.
    pub phase: LoginPhase,
    /// Selected option in ChooseMethod (0 = subscription, 1 = API key).
    selected: usize,
    /// Text buffer for API key input.
    api_key_input: String,
    /// Byte-offset cursor within api_key_input.
    api_key_cursor: usize,
    /// Spinner animation frame counter (incremented by tick).
    spinner_frame: usize,
}

impl LoginDialog {
    pub fn new() -> Self {
        Self {
            phase: LoginPhase::ChooseMethod,
            selected: 0,
            api_key_input: String::new(),
            api_key_cursor: 0,
            spinner_frame: 0,
        }
    }

    /// Advance the spinner by one frame (call on each spinner tick).
    pub fn tick(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
    }

    /// Set the OAuth waiting phase with an optional URL.
    pub fn set_oauth_waiting(&mut self, url: Option<String>) {
        self.phase = LoginPhase::OAuthWaiting { url };
    }

    /// Set the phase to success with a display message.
    pub fn set_success(&mut self, message: String) {
        self.phase = LoginPhase::Success(message);
    }

    /// Set the phase to error with a display message.
    pub fn set_error(&mut self, message: String) {
        self.phase = LoginPhase::Error(message);
    }

    /// Handle a key event. Returns an action for the caller.
    pub fn handle_key(&mut self, code: KeyCode) -> LoginDialogAction {
        match &self.phase {
            LoginPhase::ChooseMethod => self.handle_choose_method(code),
            LoginPhase::OAuthWaiting { url } => match code {
                KeyCode::Esc => LoginDialogAction::Close,
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    // Copy URL to clipboard
                    if let Some(u) = url {
                        crate::mouse::copy_to_clipboard(u);
                    }
                    LoginDialogAction::Consumed
                }
                _ => LoginDialogAction::Consumed,
            },
            LoginPhase::ApiKeyInput => self.handle_api_key_input(code),
            LoginPhase::Success(_) | LoginPhase::Error(_) => match code {
                KeyCode::Enter | KeyCode::Esc => LoginDialogAction::Close,
                _ => LoginDialogAction::Consumed,
            },
        }
    }

    fn handle_choose_method(&mut self, code: KeyCode) -> LoginDialogAction {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = 0;
                LoginDialogAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = 1;
                LoginDialogAction::Consumed
            }
            KeyCode::Enter => {
                if self.selected == 0 {
                    self.phase = LoginPhase::OAuthWaiting { url: None };
                    LoginDialogAction::StartOAuth
                } else {
                    self.phase = LoginPhase::ApiKeyInput;
                    LoginDialogAction::Consumed
                }
            }
            KeyCode::Esc => LoginDialogAction::Close,
            _ => LoginDialogAction::Consumed,
        }
    }

    fn handle_api_key_input(&mut self, code: KeyCode) -> LoginDialogAction {
        match code {
            KeyCode::Char(c) => {
                self.api_key_input.insert(self.api_key_cursor, c);
                self.api_key_cursor += c.len_utf8();
                LoginDialogAction::Consumed
            }
            KeyCode::Backspace => {
                if self.api_key_cursor > 0 {
                    // Find the previous character boundary
                    let prev = self.api_key_input[..self.api_key_cursor]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.api_key_input.remove(prev);
                    self.api_key_cursor = prev;
                }
                LoginDialogAction::Consumed
            }
            KeyCode::Delete => {
                if self.api_key_cursor < self.api_key_input.len() {
                    self.api_key_input.remove(self.api_key_cursor);
                }
                LoginDialogAction::Consumed
            }
            KeyCode::Left => {
                if self.api_key_cursor > 0 {
                    let prev = self.api_key_input[..self.api_key_cursor]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.api_key_cursor = prev;
                }
                LoginDialogAction::Consumed
            }
            KeyCode::Right => {
                if self.api_key_cursor < self.api_key_input.len() {
                    let next_char = self.api_key_input[self.api_key_cursor..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    self.api_key_cursor += next_char;
                }
                LoginDialogAction::Consumed
            }
            KeyCode::Home => {
                self.api_key_cursor = 0;
                LoginDialogAction::Consumed
            }
            KeyCode::End => {
                self.api_key_cursor = self.api_key_input.len();
                LoginDialogAction::Consumed
            }
            KeyCode::Enter => {
                let key = self.api_key_input.trim().to_string();
                if key.is_empty() {
                    LoginDialogAction::Consumed
                } else {
                    LoginDialogAction::SubmitApiKey(key)
                }
            }
            KeyCode::Esc => LoginDialogAction::Close,
            _ => LoginDialogAction::Consumed,
        }
    }

    /// Mask an API key for display: show first 10 + "..." + last 4 chars when > 20 chars.
    fn masked_key(key: &str) -> String {
        if key.len() > 20 {
            let start = &key[..10];
            let end = &key[key.len() - 4..];
            format!("{start}...{end}")
        } else {
            key.to_string()
        }
    }

    /// Compute the dialog height for the current phase.
    fn dialog_height(&self, dialog_width: u16) -> u16 {
        match &self.phase {
            LoginPhase::ChooseMethod => 14,
            LoginPhase::OAuthWaiting { url } => {
                if let Some(u) = url {
                    // URL wraps across multiple lines — calculate how many
                    let usable = dialog_width.saturating_sub(6) as usize; // 2 border + 4 padding
                    let url_lines = if usable > 0 { (u.len() + usable - 1) / usable } else { 1 };
                    // 1 blank + 1 "Opening..." + 1 blank + 1 "If browser..." + url_lines + 1 blank + 1 "Waiting..." + 1 blank + 1 footer
                    (7 + url_lines as u16).max(10)
                } else {
                    8
                }
            }
            LoginPhase::ApiKeyInput => 12,
            LoginPhase::Success(_) => 10,
            LoginPhase::Error(_) => 10,
        }
    }
}

impl Widget for &LoginDialog {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Use most of the terminal width for the OAuth URL phase
        let dialog_width = match &self.phase {
            LoginPhase::OAuthWaiting { url: Some(_) } => {
                area.width.saturating_sub(4).min(120)
            }
            _ => 54u16.min(area.width.saturating_sub(4)),
        };
        let dialog_height = self.dialog_height(dialog_width).min(area.height.saturating_sub(2));
        let dialog_x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = area.y + (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);

        // Clear the background
        Clear.render(dialog_area, buf);

        let block = Block::default()
            .title(" Log In ")
            .title_alignment(ratatui::layout::Alignment::Center)
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(theme::STYLE_CYAN)
            .style(Style::new().bg(Color::Rgb(20, 20, 30)));

        let inner = block.inner(dialog_area);
        block.render(dialog_area, buf);

        match &self.phase {
            LoginPhase::ChooseMethod => render_choose_method(inner, buf, self.selected),
            LoginPhase::OAuthWaiting { url } => {
                render_oauth_waiting(inner, buf, self.spinner_frame, url.as_deref())
            }
            LoginPhase::ApiKeyInput => {
                render_api_key_input(inner, buf, &self.api_key_input, self.api_key_cursor)
            }
            LoginPhase::Success(msg) => render_success(inner, buf, msg),
            LoginPhase::Error(msg) => render_error(inner, buf, msg),
        }
    }
}

fn render_choose_method(area: Rect, buf: &mut Buffer, selected: usize) {
    let lines: Vec<(u16, Line<'_>)> = vec![
        (0, Line::from("")),
        (
            1,
            Line::from(Span::styled(
                "  How would you like to authenticate?",
                theme::STYLE_WHITE,
            )),
        ),
        (2, Line::from("")),
        (
            3,
            {
                let marker = if selected == 0 { "\u{25b8}" } else { "\u{25cb}" };
                let style = if selected == 0 {
                    theme::STYLE_BOLD_CYAN
                } else {
                    theme::STYLE_WHITE
                };
                Line::from(vec![
                    Span::styled(format!("  {marker} "), style),
                    Span::styled("Claude.ai Subscription", style),
                ])
            },
        ),
        (
            4,
            Line::from(Span::styled(
                "    Pro, Max, Team, or Enterprise plan",
                theme::STYLE_DARK_GRAY,
            )),
        ),
        (
            5,
            Line::from(Span::styled(
                "    Authenticates via browser OAuth",
                theme::STYLE_DARK_GRAY,
            )),
        ),
        (6, Line::from("")),
        (
            7,
            {
                let marker = if selected == 1 { "\u{25b8}" } else { "\u{25cb}" };
                let style = if selected == 1 {
                    theme::STYLE_BOLD_CYAN
                } else {
                    theme::STYLE_WHITE
                };
                Line::from(vec![
                    Span::styled(format!("  {marker} "), style),
                    Span::styled("Console API Key", style),
                ])
            },
        ),
        (
            8,
            Line::from(Span::styled(
                "    Paste your sk-ant-... API key",
                theme::STYLE_DARK_GRAY,
            )),
        ),
        (
            9,
            Line::from(Span::styled(
                "    For API/developer access",
                theme::STYLE_DARK_GRAY,
            )),
        ),
        (10, Line::from("")),
        (
            11,
            Line::from(vec![
                Span::styled("  [Enter]", theme::STYLE_BOLD_CYAN),
                Span::styled(" Select  ", theme::STYLE_DARK_GRAY),
                Span::styled("[Esc]", theme::STYLE_BOLD_CYAN),
                Span::styled(" Cancel", theme::STYLE_DARK_GRAY),
            ]),
        ),
    ];

    for (row_offset, line) in lines {
        if row_offset < area.height {
            let row_area = Rect::new(area.x, area.y + row_offset, area.width, 1);
            buf.set_line(row_area.x, row_area.y, &line, row_area.width);
        }
    }
}

fn render_oauth_waiting(area: Rect, buf: &mut Buffer, spinner_frame: usize, url: Option<&str>) {
    let frame = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
    let url_style = Style::new()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);

    let mut row: u16 = 0;
    let mut put = |line: Line<'_>| {
        if row < area.height {
            buf.set_line(area.x, area.y + row, &line, area.width);
        }
        row += 1;
    };

    put(Line::from(""));
    put(Line::from(Span::styled(
        "  Opening browser for authentication...",
        theme::STYLE_WHITE,
    )));
    put(Line::from(""));

    if let Some(u) = url {
        put(Line::from(Span::styled(
            "  If the browser doesn't open, copy this URL:",
            theme::STYLE_DARK_GRAY,
        )));

        // Wrap URL across multiple lines so the user can select & copy it
        let usable_width = area.width.saturating_sub(6) as usize; // 2 padding each side + 2 border
        if usable_width > 0 {
            let mut remaining: &str = u;
            while !remaining.is_empty() {
                let chunk_len = remaining.len().min(usable_width);
                let chunk = &remaining[..chunk_len];
                put(Line::from(Span::styled(format!("  {chunk}"), url_style)));
                remaining = &remaining[chunk_len..];
            }
        } else {
            put(Line::from(Span::styled(format!("  {u}"), url_style)));
        }
        put(Line::from(""));
    }

    put(Line::from(vec![
        Span::styled("  Waiting for callback...  ", theme::STYLE_YELLOW),
        Span::styled(frame, theme::STYLE_BOLD_CYAN),
    ]));
    put(Line::from(""));
    put(Line::from(vec![
        Span::styled("  [Esc]", theme::STYLE_BOLD_CYAN),
        Span::styled(" Cancel  ", theme::STYLE_DARK_GRAY),
        Span::styled("[C]", theme::STYLE_BOLD_CYAN),
        Span::styled(" Copy URL", theme::STYLE_DARK_GRAY),
    ]));
}

fn render_api_key_input(area: Rect, buf: &mut Buffer, input: &str, _cursor: usize) {
    let mut row: u16 = 0;
    let mut put = |line: Line<'_>| {
        if row < area.height {
            buf.set_line(area.x, area.y + row, &line, area.width);
        }
        row += 1;
    };

    put(Line::from(""));
    put(Line::from(Span::styled(
        "  Enter your Anthropic API key:",
        theme::STYLE_WHITE,
    )));
    put(Line::from(""));

    // Render the input field with masking
    let display_text = if input.is_empty() {
        "sk-ant-api03-________________".to_string()
    } else {
        LoginDialog::masked_key(input)
    };
    let input_style = if input.is_empty() {
        theme::STYLE_DARK_GRAY
    } else {
        theme::STYLE_WHITE
    };

    put(Line::from(vec![
        Span::styled("  \u{2503} ", theme::STYLE_CYAN),
        Span::styled(display_text, input_style),
    ]));
    put(Line::from(""));

    put(Line::from(Span::styled(
        "  Get your key at:",
        theme::STYLE_DARK_GRAY,
    )));
    put(Line::from(Span::styled(
        "  console.anthropic.com/settings/keys",
        Style::new()
            .fg(Color::Blue)
            .add_modifier(Modifier::UNDERLINED),
    )));
    put(Line::from(""));

    // Validation hint
    if !input.is_empty() && !input.starts_with("sk-ant-") && !input.starts_with("sk-") {
        put(Line::from(Span::styled(
            "  Key should start with sk-ant- or sk-",
            theme::STYLE_RED,
        )));
    } else {
        put(Line::from(vec![
            Span::styled("  [Enter]", theme::STYLE_BOLD_CYAN),
            Span::styled(" Submit  ", theme::STYLE_DARK_GRAY),
            Span::styled("[Esc]", theme::STYLE_BOLD_CYAN),
            Span::styled(" Cancel", theme::STYLE_DARK_GRAY),
        ]));
    }
}

fn render_success(area: Rect, buf: &mut Buffer, message: &str) {
    let mut row: u16 = 0;
    let mut put = |line: Line<'_>| {
        if row < area.height {
            buf.set_line(area.x, area.y + row, &line, area.width);
        }
        row += 1;
    };

    put(Line::from(""));
    put(Line::from(Span::styled(
        "  \u{2713} Logged in successfully!",
        theme::STYLE_BOLD_GREEN,
    )));
    put(Line::from(""));

    // Split message by newlines to display profile info
    for line_text in message.lines() {
        put(Line::from(Span::styled(
            format!("  {line_text}"),
            theme::STYLE_WHITE,
        )));
    }

    put(Line::from(""));
    put(Line::from(vec![
        Span::styled("  [Enter]", theme::STYLE_BOLD_CYAN),
        Span::styled(" Close", theme::STYLE_DARK_GRAY),
    ]));
}

fn render_error(area: Rect, buf: &mut Buffer, message: &str) {
    let mut row: u16 = 0;
    let mut put = |line: Line<'_>| {
        if row < area.height {
            buf.set_line(area.x, area.y + row, &line, area.width);
        }
        row += 1;
    };

    put(Line::from(""));
    put(Line::from(Span::styled(
        "  \u{2717} Login failed",
        theme::STYLE_BOLD_RED,
    )));
    put(Line::from(""));

    // Wrap long error messages
    let max_width = area.width.saturating_sub(4) as usize;
    for chunk in message.as_bytes().chunks(max_width) {
        let text = String::from_utf8_lossy(chunk);
        put(Line::from(Span::styled(
            format!("  {text}"),
            theme::STYLE_RED,
        )));
    }

    put(Line::from(""));
    put(Line::from(vec![
        Span::styled("  [Enter]", theme::STYLE_BOLD_CYAN),
        Span::styled(" Close  ", theme::STYLE_DARK_GRAY),
        Span::styled("[Esc]", theme::STYLE_BOLD_CYAN),
        Span::styled(" Cancel", theme::STYLE_DARK_GRAY),
    ]));
}
