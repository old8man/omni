//! Notification popup widget for transient messages.
//!
//! Renders toast notifications at the top-right corner of the screen.
//! Auto-dismisses after a configurable duration (default 3s).
//! Supports info, success, warning, and error severity levels.
//! Stacks up to 3 notifications.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Notification severity level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationLevel {
    /// Informational message (blue).
    Info,
    /// Success confirmation (green).
    Success,
    /// Warning (yellow).
    Warning,
    /// Error (red).
    Error,
}

impl NotificationLevel {
    /// Return the color associated with this level.
    fn color(self) -> Color {
        match self {
            Self::Info => Color::Blue,
            Self::Success => Color::Green,
            Self::Warning => Color::Yellow,
            Self::Error => Color::Red,
        }
    }

    /// Return the icon associated with this level.
    fn icon(self) -> &'static str {
        match self {
            Self::Info => "\u{2139}\u{FE0F}",    // ℹ️
            Self::Success => "\u{2714}",          // ✔
            Self::Warning => "\u{26A0}\u{FE0F}",  // ⚠️
            Self::Error => "\u{2718}",            // ✘
        }
    }

    /// Return the label for this level.
    fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Success => "OK",
            Self::Warning => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// A single notification.
#[derive(Clone, Debug)]
pub struct Notification {
    /// The message text.
    pub message: String,
    /// Severity level.
    pub level: NotificationLevel,
    /// When the notification was created.
    pub created_at: Instant,
    /// How long to display the notification.
    pub duration: Duration,
}

impl Notification {
    /// Check if this notification has expired.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.duration
    }

    /// Return the remaining time before expiry as a fraction (1.0 = full, 0.0 = expired).
    pub fn remaining_fraction(&self) -> f64 {
        let elapsed = self.created_at.elapsed().as_secs_f64();
        let total = self.duration.as_secs_f64();
        (1.0 - elapsed / total).max(0.0)
    }
}

/// Manager for notification popups.
pub struct NotificationManager {
    /// Active notifications (most recent last).
    notifications: Vec<Notification>,
    /// Maximum number of simultaneous notifications.
    max_visible: usize,
    /// Default duration for new notifications.
    default_duration: Duration,
}

impl NotificationManager {
    /// Create a new notification manager.
    pub fn new() -> Self {
        Self {
            notifications: Vec::new(),
            max_visible: 3,
            default_duration: Duration::from_secs(3),
        }
    }

    /// Set the default auto-dismiss duration.
    pub fn set_default_duration(&mut self, duration: Duration) {
        self.default_duration = duration;
    }

    /// Push a new notification with the default duration.
    pub fn push(&mut self, message: String, level: NotificationLevel) {
        self.push_with_duration(message, level, self.default_duration);
    }

    /// Push a new notification with a custom duration.
    pub fn push_with_duration(
        &mut self,
        message: String,
        level: NotificationLevel,
        duration: Duration,
    ) {
        self.notifications.push(Notification {
            message,
            level,
            created_at: Instant::now(),
            duration,
        });
        // Trim old notifications if we have too many total
        while self.notifications.len() > self.max_visible * 2 {
            self.notifications.remove(0);
        }
    }

    /// Remove expired notifications.
    pub fn prune(&mut self) {
        self.notifications.retain(|n| !n.is_expired());
    }

    /// Get the currently visible notifications (most recent, up to max_visible).
    pub fn visible(&self) -> &[Notification] {
        let start = self.notifications.len().saturating_sub(self.max_visible);
        &self.notifications[start..]
    }

    /// Whether there are any active notifications.
    pub fn has_active(&self) -> bool {
        self.notifications.iter().any(|n| !n.is_expired())
    }

    /// Clear all notifications immediately.
    pub fn clear(&mut self) {
        self.notifications.clear();
    }
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Widget that renders notification popups at the top-right corner.
pub struct NotificationWidget<'a> {
    manager: &'a NotificationManager,
}

impl<'a> NotificationWidget<'a> {
    /// Create a new notification widget.
    pub fn new(manager: &'a NotificationManager) -> Self {
        Self { manager }
    }
}

impl<'a> Widget for NotificationWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let visible = self.manager.visible();
        if visible.is_empty() || area.height == 0 || area.width < 10 {
            return;
        }

        for (i, notif) in visible.iter().enumerate() {
            if notif.is_expired() {
                continue;
            }

            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }

            let color = notif.level.color();
            let icon = notif.level.icon();
            let label = notif.level.label();

            // Calculate notification width: " ICON LABEL: message "
            let content = format!(" {} {}: {} ", icon, label, notif.message);
            let notif_width = content.len().min(area.width as usize);
            let x = area.x + area.width.saturating_sub(notif_width as u16);

            // Background fill for the notification area
            let bg_color = Color::Rgb(30, 30, 30);
            let bg_style = Style::default().bg(bg_color).fg(Color::White);
            for col in x..area.x + area.width {
                buf[(col, y)].set_char(' ').set_style(bg_style);
            }

            // Left accent bar
            if x < area.x + area.width {
                buf[(x, y)]
                    .set_char('\u{2588}') // █
                    .set_style(Style::default().fg(color).bg(bg_color));
            }

            // Render content
            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", icon),
                    Style::default()
                        .fg(color)
                        .bg(bg_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{}: ", label),
                    Style::default()
                        .fg(color)
                        .bg(bg_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    notif.message.clone(),
                    Style::default().fg(Color::White).bg(bg_color),
                ),
                Span::styled(" ", Style::default().bg(bg_color)),
            ]);

            let render_x = x + 1; // after accent bar
            buf.set_line(
                render_x,
                y,
                &line,
                area.width.saturating_sub(render_x - area.x),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_visible() {
        let mut mgr = NotificationManager::new();
        mgr.push("hello".to_string(), NotificationLevel::Info);
        assert_eq!(mgr.visible().len(), 1);
    }

    #[test]
    fn test_max_visible() {
        let mut mgr = NotificationManager::new();
        for i in 0..5 {
            mgr.push(format!("msg {}", i), NotificationLevel::Info);
        }
        assert_eq!(mgr.visible().len(), 3);
    }

    #[test]
    fn test_prune_expired() {
        let mut mgr = NotificationManager::new();
        mgr.push_with_duration(
            "quick".to_string(),
            NotificationLevel::Info,
            Duration::from_millis(0),
        );
        std::thread::sleep(Duration::from_millis(1));
        mgr.prune();
        assert!(mgr.visible().is_empty());
    }

    #[test]
    fn test_notification_level_colors() {
        assert_eq!(NotificationLevel::Info.color(), Color::Blue);
        assert_eq!(NotificationLevel::Error.color(), Color::Red);
        assert_eq!(NotificationLevel::Success.color(), Color::Green);
        assert_eq!(NotificationLevel::Warning.color(), Color::Yellow);
    }

    #[test]
    fn test_clear() {
        let mut mgr = NotificationManager::new();
        mgr.push("test".to_string(), NotificationLevel::Info);
        mgr.clear();
        assert!(mgr.visible().is_empty());
    }

    #[test]
    fn test_custom_default_duration() {
        let mut mgr = NotificationManager::new();
        mgr.set_default_duration(Duration::from_secs(10));
        mgr.push("long".to_string(), NotificationLevel::Warning);
        assert_eq!(mgr.visible()[0].duration, Duration::from_secs(10));
    }
}
