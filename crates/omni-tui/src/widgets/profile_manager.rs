//! Full-screen overlay panel for managing authentication profiles.
//!
//! Displays a rich interactive list of all configured profiles with
//! subscription badges, token expiry information, and keyboard navigation.
//! Supports switching, adding, and deleting profiles.

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::theme;

// ── Profile entry ─────────────────────────────────────────────────────────

/// A single profile entry prepared for display.
struct ProfileEntry {
    /// Profile name (e.g. "user@gmail.com-pro").
    name: String,
    /// Email address.
    email: String,
    /// Subscription type display label (e.g. "Pro", "Max").
    subscription_type: String,
    /// Whether this is the currently active profile.
    is_active: bool,
    /// Whether the token has expired.
    is_expired: bool,
    /// Whether the profile has a refresh token (used for expiry decisions).
    _has_refresh: bool,
    /// Human-readable expiry string (e.g. "23h 15m", "6d"), or None for API key / expired.
    expires_in: Option<String>,
    /// Authentication type: "OAuth" or "API Key".
    auth_type: String,
}

// ── Action result ─────────────────────────────────────────────────────────

/// What the caller should do after a key event in the profile manager.
pub enum ProfileManagerAction {
    /// The panel consumed the key; keep it open.
    Consumed,
    /// Close the panel without action.
    Close,
    /// Switch to the named profile.
    SwitchTo(String),
    /// Show a message that the user should use /login.
    AddNew,
    /// A profile was deleted.
    Deleted(String),
}

// ── Main widget state ─────────────────────────────────────────────────────

pub struct ProfileManager {
    /// Loaded profile entries.
    profiles: Vec<ProfileEntry>,
    /// Index of the currently highlighted profile.
    selected: usize,
    /// Name of the currently active profile (if any).
    _active_name: Option<String>,
    /// Whether we are waiting for delete confirmation (y/N).
    confirm_delete: bool,
}

impl ProfileManager {
    /// Create a new profile manager, loading all profiles from disk.
    pub fn new() -> Self {
        use omni_core::auth::profiles;

        let all = profiles::list_profiles();
        let active_name = profiles::get_active_profile_name();

        let entries: Vec<ProfileEntry> = all
            .iter()
            .map(|p| {
                let is_active = active_name.as_deref() == Some(p.name.as_str());
                let is_expired = p.is_expired();
                let _has_refresh = p.credentials.refresh_token.is_some();
                let auth_type = if p.credentials.api_key.is_some() {
                    "API Key".to_string()
                } else {
                    "OAuth".to_string()
                };
                let expires_in = if p.credentials.api_key.is_some() {
                    None // API keys don't expire
                } else {
                    p.credentials.expires_at.map(|ea| format_expires_in(ea))
                };
                let subscription_type = capitalize_sub(&p.subscription_type);

                ProfileEntry {
                    name: p.name.clone(),
                    email: p.email.clone(),
                    subscription_type,
                    is_active,
                    is_expired,
                    _has_refresh,
                    expires_in,
                    auth_type,
                }
            })
            .collect();

        // Pre-select the active profile if there is one.
        let selected = entries
            .iter()
            .position(|e| e.is_active)
            .unwrap_or(0);

        Self {
            profiles: entries,
            selected,
            _active_name: active_name,
            confirm_delete: false,
        }
    }

    /// Returns true if the profile list is empty.
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    // ── Navigation ────────────────────────────────────────────────────────

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn move_down(&mut self) {
        if !self.profiles.is_empty() && self.selected + 1 < self.profiles.len() {
            self.selected += 1;
        }
    }

    // ── Key handling ──────────────────────────────────────────────────────

    pub fn handle_key(&mut self, code: KeyCode) -> ProfileManagerAction {
        // Delete confirmation mode.
        if self.confirm_delete {
            self.confirm_delete = false;
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(entry) = self.profiles.get(self.selected) {
                        let name = entry.name.clone();
                        if omni_core::auth::profiles::remove_profile(&name).is_ok() {
                            self.profiles.remove(self.selected);
                            if self.selected >= self.profiles.len() && self.selected > 0 {
                                self.selected -= 1;
                            }
                            return ProfileManagerAction::Deleted(name);
                        }
                    }
                    return ProfileManagerAction::Consumed;
                }
                _ => {
                    return ProfileManagerAction::Consumed;
                }
            }
        }

        match code {
            KeyCode::Esc | KeyCode::Char('q') => ProfileManagerAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                ProfileManagerAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                ProfileManagerAction::Consumed
            }
            KeyCode::Enter => {
                if let Some(entry) = self.profiles.get(self.selected) {
                    if entry.is_active {
                        // Already active, just close.
                        ProfileManagerAction::Close
                    } else {
                        ProfileManagerAction::SwitchTo(entry.name.clone())
                    }
                } else {
                    ProfileManagerAction::Close
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => ProfileManagerAction::AddNew,
            KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Delete => {
                if !self.profiles.is_empty() {
                    self.confirm_delete = true;
                }
                ProfileManagerAction::Consumed
            }
            _ => ProfileManagerAction::Consumed,
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

impl Widget for &ProfileManager {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled("Authentication Profiles", theme::STYLE_BOLD_CYAN),
                Span::styled(" ", Style::default()),
            ]))
            .borders(Borders::ALL)
            .border_style(theme::STYLE_CYAN)
            .style(Style::new().bg(Color::Rgb(20, 20, 30)));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 6 || inner.width < 20 {
            return;
        }

        let content_width = inner.width.saturating_sub(2) as usize;

        // Empty state.
        if self.profiles.is_empty() {
            let empty_y = inner.y + inner.height / 2;
            let msg = "No profiles found. Use /login to add one.";
            buf.set_line(
                inner.x + 1,
                empty_y,
                &Line::from(Span::styled(msg, theme::STYLE_DARK_GRAY)),
                inner.width.saturating_sub(2),
            );
            render_footer(self, inner, buf, content_width);
            return;
        }

        // Reserve footer (2 lines: separator + keybindings).
        let footer_height: u16 = 2;
        let list_bottom = inner.y + inner.height.saturating_sub(footer_height);
        let mut y = inner.y + 1; // one line of padding at top

        // Render profile entries.
        for (i, entry) in self.profiles.iter().enumerate() {
            if y + 2 >= list_bottom {
                break;
            }

            let is_selected = i == self.selected;
            let row_bg = if is_selected {
                Color::Rgb(40, 40, 60)
            } else {
                Color::Rgb(20, 20, 30)
            };

            // Line 1: dot + email
            let dot = if entry.is_active {
                Span::styled("\u{25cf} ", Style::new().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else if entry.is_expired {
                Span::styled("\u{25cb} ", Style::new().fg(Color::Red))
            } else {
                Span::styled("\u{25cb} ", theme::STYLE_WHITE)
            };

            let email_style = if entry.is_expired {
                Style::new().fg(Color::DarkGray).bg(row_bg)
            } else if is_selected {
                Style::new()
                    .fg(Color::White)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new()
                    .fg(Color::White)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            };

            let email_line = Line::from(vec![
                Span::styled("  ", Style::new().bg(row_bg)),
                dot,
                Span::styled(&entry.email, email_style),
            ]);
            buf.set_line(inner.x + 1, y, &email_line, inner.width.saturating_sub(2));
            y += 1;

            if y >= list_bottom {
                break;
            }

            // Line 2: subscription badge + status + expiry
            let badge_style = subscription_badge_style(&entry.subscription_type);
            let badge = Span::styled(&entry.subscription_type, badge_style);

            let mut detail_spans: Vec<Span> = vec![
                Span::styled("    ", Style::new().bg(row_bg)),
                badge,
            ];

            if entry.is_expired {
                detail_spans.push(Span::styled(
                    " \u{00b7} ",
                    Style::new().fg(Color::DarkGray).bg(row_bg),
                ));
                detail_spans.push(Span::styled(
                    "\u{2717} Expired",
                    Style::new().fg(Color::Red).bg(row_bg),
                ));
            } else if entry.is_active {
                detail_spans.push(Span::styled(
                    " \u{00b7} ",
                    Style::new().fg(Color::DarkGray).bg(row_bg),
                ));
                detail_spans.push(Span::styled(
                    "Active",
                    Style::new().fg(Color::Green).bg(row_bg),
                ));
                // Token expiry.
                if entry.auth_type == "API Key" {
                    detail_spans.push(Span::styled(
                        " \u{00b7} API Key \u{00b7} No expiry",
                        Style::new().fg(Color::DarkGray).bg(row_bg),
                    ));
                } else if let Some(ref exp) = entry.expires_in {
                    detail_spans.push(Span::styled(
                        format!(" \u{00b7} Token valid (expires in {})", exp),
                        Style::new().fg(Color::DarkGray).bg(row_bg),
                    ));
                }
            } else {
                detail_spans.push(Span::styled(
                    " \u{00b7} ",
                    Style::new().fg(Color::DarkGray).bg(row_bg),
                ));
                detail_spans.push(Span::styled(
                    "Valid",
                    Style::new().fg(Color::Green).bg(row_bg),
                ));
                if entry.auth_type == "API Key" {
                    detail_spans.push(Span::styled(
                        " \u{00b7} API Key \u{00b7} No expiry",
                        Style::new().fg(Color::DarkGray).bg(row_bg),
                    ));
                } else if let Some(ref exp) = entry.expires_in {
                    detail_spans.push(Span::styled(
                        format!(" \u{00b7} Token valid (expires in {})", exp),
                        Style::new().fg(Color::DarkGray).bg(row_bg),
                    ));
                }
            }

            let detail_line = Line::from(detail_spans);
            buf.set_line(inner.x + 1, y, &detail_line, inner.width.saturating_sub(2));
            y += 1;

            if y >= list_bottom {
                break;
            }

            // Line 3: separator (thin dotted line)
            let sep: String = "\u{2504}".repeat(content_width.saturating_sub(4));
            let sep_line = Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled(sep, Style::new().fg(Color::Rgb(50, 50, 60))),
            ]);
            buf.set_line(inner.x + 1, y, &sep_line, inner.width.saturating_sub(2));
            y += 1;
        }

        // Footer.
        render_footer(self, inner, buf, content_width);
    }
}

/// Render the footer separator and keybinding hints.
fn render_footer(pm: &ProfileManager, inner: Rect, buf: &mut Buffer, content_width: usize) {
    let footer_height: u16 = 2;
    let footer_sep_y = inner.y + inner.height.saturating_sub(footer_height);
    let hint_y = inner.y + inner.height - 1;

    // Separator line.
    let sep: String = "\u{2500}".repeat(content_width);
    buf.set_line(
        inner.x + 1,
        footer_sep_y,
        &Line::from(Span::styled(sep, theme::STYLE_DARK_GRAY)),
        inner.width.saturating_sub(2),
    );

    // Keybinding hints (context-dependent).
    if pm.confirm_delete {
        let delete_name = pm
            .profiles
            .get(pm.selected)
            .map(|e| e.name.as_str())
            .unwrap_or("?");
        let hint = format!(" Delete {}? [y/N] ", delete_name);
        let hints_line = Line::from(vec![
            Span::styled(hint, Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)),
        ]);
        buf.set_line(
            inner.x + 1,
            hint_y,
            &hints_line,
            inner.width.saturating_sub(2),
        );
    } else if pm.profiles.is_empty() {
        let hints_line = Line::from(vec![
            Span::styled(" [A]", theme::STYLE_BOLD_CYAN),
            Span::styled(" Add new  ", theme::STYLE_DARK_GRAY),
            Span::styled("[Esc]", theme::STYLE_BOLD_CYAN),
            Span::styled(" Close", theme::STYLE_DARK_GRAY),
        ]);
        buf.set_line(
            inner.x + 1,
            hint_y,
            &hints_line,
            inner.width.saturating_sub(2),
        );
    } else {
        let hints_line = Line::from(vec![
            Span::styled(" [Enter]", theme::STYLE_BOLD_CYAN),
            Span::styled(" Switch  ", theme::STYLE_DARK_GRAY),
            Span::styled("[A]", theme::STYLE_BOLD_CYAN),
            Span::styled(" Add new  ", theme::STYLE_DARK_GRAY),
            Span::styled("[D]", theme::STYLE_BOLD_CYAN),
            Span::styled(" Delete  ", theme::STYLE_DARK_GRAY),
            Span::styled("[Esc]", theme::STYLE_BOLD_CYAN),
            Span::styled(" Close", theme::STYLE_DARK_GRAY),
        ]);
        buf.set_line(
            inner.x + 1,
            hint_y,
            &hints_line,
            inner.width.saturating_sub(2),
        );
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Format a token expiry timestamp into a human-readable relative string.
fn format_expires_in(expires_at: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    if expires_at <= now_ms {
        return "Expired".to_string();
    }

    let remaining = expires_at - now_ms;
    let minutes = remaining / 60_000;
    let hours = minutes / 60;
    let days = hours / 24;

    if days > 0 {
        format!("{}d", days)
    } else if hours > 0 {
        let rem_min = minutes % 60;
        if rem_min > 0 {
            format!("{}h {}m", hours, rem_min)
        } else {
            format!("{}h", hours)
        }
    } else {
        format!("{}m", minutes.max(1))
    }
}

/// Return the style for a subscription badge based on type.
fn subscription_badge_style(sub: &str) -> Style {
    match sub.to_lowercase().as_str() {
        "pro" => Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        "max" => Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        "team" => Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        "enterprise" => Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        "api" | "api key" => Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
        _ => Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
    }
}

/// Capitalize subscription type for display.
fn capitalize_sub(s: &str) -> String {
    match s.to_lowercase().as_str() {
        "pro" => "Pro".to_string(),
        "max" => "Max".to_string(),
        "team" => "Team".to_string(),
        "enterprise" => "Enterprise".to_string(),
        "api" => "API Key".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}
