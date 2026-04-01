use ratatui::style::{Color, Modifier, Style};

// ── Modifier-only styles ────────────────────────────────────────────────
pub const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
pub const DIM: Style = Style::new().add_modifier(Modifier::DIM);
pub const ITALIC: Style = Style::new().add_modifier(Modifier::ITALIC);

// ── Single-color styles ─────────────────────────────────────────────────
pub const STYLE_DARK_GRAY: Style = Style::new().fg(Color::DarkGray);
pub const STYLE_CYAN: Style = Style::new().fg(Color::Cyan);
pub const STYLE_GREEN: Style = Style::new().fg(Color::Green);
pub const STYLE_YELLOW: Style = Style::new().fg(Color::Yellow);
pub const STYLE_RED: Style = Style::new().fg(Color::Red);
pub const STYLE_MAGENTA: Style = Style::new().fg(Color::Magenta);
pub const STYLE_WHITE: Style = Style::new().fg(Color::White);
pub const STYLE_BLUE: Style = Style::new().fg(Color::Blue);
pub const STYLE_GRAY: Style = Style::new().fg(Color::Gray);

// ── Combined color + modifier styles ────────────────────────────────────
pub const STYLE_BOLD_WHITE: Style = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);
pub const STYLE_BOLD_CYAN: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
pub const STYLE_BOLD_GREEN: Style = Style::new().fg(Color::Green).add_modifier(Modifier::BOLD);
pub const STYLE_BOLD_YELLOW: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
pub const STYLE_BOLD_RED: Style = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);
pub const STYLE_BOLD_MAGENTA: Style = Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD);
pub const STYLE_BOLD_BLUE: Style = Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD);

// ── Status bar styles ───────────────────────────────────────────────────
pub const STATUS_BG: Color = Color::Rgb(30, 30, 40);
pub const STYLE_STATUS: Style = Style::new().bg(Color::Rgb(30, 30, 40)).fg(Color::White);
pub const STYLE_STATUS_DARK_GRAY: Style = Style::new().fg(Color::DarkGray).bg(Color::Rgb(30, 30, 40));

// ── Notification background ─────────────────────────────────────────────
pub const NOTIF_BG: Color = Color::Rgb(30, 30, 30);
pub const STYLE_NOTIF: Style = Style::new().bg(Color::Rgb(30, 30, 30)).fg(Color::White);

pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub muted: Color,
    pub border: Color,
    pub user_message: Color,
    pub assistant_message: Color,
    pub tool_name: Color,
    pub thinking: Color,
}

pub fn dark_theme() -> Theme {
    Theme {
        bg: Color::Reset,
        fg: Color::White,
        accent: Color::Cyan,
        error: Color::Red,
        warning: Color::Yellow,
        success: Color::Green,
        muted: Color::DarkGray,
        border: Color::DarkGray,
        user_message: Color::Blue,
        assistant_message: Color::White,
        tool_name: Color::Magenta,
        thinking: Color::DarkGray,
    }
}

pub fn light_theme() -> Theme {
    Theme {
        bg: Color::Reset,
        fg: Color::Black,
        accent: Color::Blue,
        error: Color::Red,
        warning: Color::Yellow,
        success: Color::Green,
        muted: Color::Gray,
        border: Color::Gray,
        user_message: Color::Blue,
        assistant_message: Color::Black,
        tool_name: Color::Magenta,
        thinking: Color::Gray,
    }
}

pub fn detect_theme() -> Theme {
    // Check COLORFGBG env var, default to dark
    if let Ok(val) = std::env::var("COLORFGBG") {
        if val.ends_with(";15") || val.ends_with(";7") {
            return light_theme();
        }
    }
    dark_theme()
}
