//! Notification popup widget for transient messages.
//!
//! Renders a small popup at the top of the screen that auto-dismisses
//! after a configurable duration.  Supports info, success, warning,
//! and error severity levels with appropriate coloring.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Notification severity level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationLevel {
    /// Informational message.
    Info,
    /// Success confirmation.
    Success,
    /// Warning.
    Warning,
    /// Error.
    Error,
}

impl NotificationLevel {
    /// Return the color associated with this level.
    fn color(self) -> Color {
        match self {
            Self::Info => Color::Cyan,
            Self::Success => Color::Green,
            Self::Warning => Color::Yellow,
            Self::Error => Color::Red,
        }
    }

    /// Return the icon associated with this level.
    fn icon(self) -> &'static str {
        match self {
            Self::Info => "\u{2139}",    // ℹ
            Self::Success => "\u{2714}", // ✔
            Self::Warning => "\u{26A0}", // ⚠
            Self::Error => "\u{2718}",   // ✘
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
}

impl NotificationManager {
    /// Create a new notification manager.
    pub fn new() -> Self {
        Self {
            notifications: Vec::new(),
            max_visible: 3,
        }
    }

    /// Push a new notification.
    pub fn push(&mut self, message: String, level: NotificationLevel) {
        self.push_with_duration(message, level, Duration::from_secs(3));
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
    }

    /// Remove expired notifications.
    pub fn prune(&mut self) {
        self.notifications.retain(|n| !n.is_expired());
    }

    /// Get the currently visible notifications.
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

/// Widget that renders notification popups.
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

            // Calculate notification width
            let msg_width = notif.message.len() + 4; // icon + spaces + message
            let notif_width = msg_width.min(area.width as usize);
            let x = area.x + area.width.saturating_sub(notif_width as u16);

            // Background fill
            let bg_style = Style::default().bg(Color::Rgb(30, 30, 30)).fg(Color::White);
            for col in x..area.x + area.width {
                buf[(col, y)].set_char(' ').set_style(bg_style);
            }

            // Render content
            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", icon),
                    Style::default()
                        .fg(color)
                        .bg(Color::Rgb(30, 30, 30))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    notif.message.clone(),
                    Style::default().fg(Color::White).bg(Color::Rgb(30, 30, 30)),
                ),
                Span::styled(" ", Style::default().bg(Color::Rgb(30, 30, 30))),
            ]);

            buf.set_line(x, y, &line, area.width.saturating_sub(x - area.x));
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
        assert_eq!(NotificationLevel::Info.color(), Color::Cyan);
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
}
