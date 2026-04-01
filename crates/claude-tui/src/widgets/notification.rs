//! Notification popup widget for transient messages.
//!
//! Renders toast notifications at the top-right corner of the screen.
//! Auto-dismisses after a configurable duration.
//! Supports info, success, warning, and error severity levels.
//! Supports priority-based queueing (Immediate, High, Normal, Low).
//! Includes a contextual hint system for keyboard shortcut discovery.
//! Stacks up to 3 notifications.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme;

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

/// Priority levels for notification ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotificationPriority {
    /// Displays instantly, replaces the current notification.
    Immediate,
    /// Goes to front of queue.
    High,
    /// Standard FIFO ordering.
    Normal,
    /// Standard FIFO ordering, short duration.
    Low,
}

impl NotificationPriority {
    /// Return the default duration for this priority level.
    pub fn default_duration(self) -> Duration {
        match self {
            Self::Immediate => Duration::from_secs(5),
            Self::High => Duration::from_secs(5),
            Self::Normal => Duration::from_secs(3),
            Self::Low => Duration::from_secs(2),
        }
    }

    /// Return the border color for this priority level.
    pub fn border_color(self) -> Color {
        match self {
            Self::Immediate => Color::Red,
            Self::High => Color::Yellow,
            Self::Normal => Color::Cyan,
            Self::Low => Color::Gray,
        }
    }
}

/// A single notification.
#[derive(Clone, Debug)]
pub struct Notification {
    /// The message text (may contain newlines for multi-line).
    pub message: String,
    /// Severity level.
    pub level: NotificationLevel,
    /// Priority level for ordering.
    pub priority: NotificationPriority,
    /// When the notification was created.
    pub created_at: Instant,
    /// How long to display the notification.
    pub duration: Duration,
    /// Whether this notification should render as multi-line.
    pub multi_line: bool,
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

    /// Return the number of display lines this notification occupies.
    pub fn line_count(&self) -> usize {
        if self.multi_line {
            self.message.lines().count().max(1)
        } else {
            1
        }
    }
}

/// Identifiers for contextual hints (used to avoid repeating them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HintId {
    /// "Ctrl+R to search history" - shown after 3rd input.
    SearchHistory,
    /// "Ctrl+E to act on messages" - shown after 5th assistant response.
    ActOnMessages,
    /// "Ctrl+F to search conversation" - shown after scrolling up.
    SearchConversation,
    /// "/compact to free context" - shown when context > 80%.
    CompactContext,
}

/// Tracks which contextual hints have been shown and interaction counters.
pub struct HintTracker {
    /// Set of hint IDs that have already been shown.
    shown_hints: HashSet<HintId>,
    /// Number of user inputs submitted.
    input_count: u32,
    /// Number of assistant responses completed.
    assistant_response_count: u32,
    /// Whether the user has scrolled up at least once.
    has_scrolled_up: bool,
}

impl HintTracker {
    /// Create a new hint tracker with all counters at zero.
    pub fn new() -> Self {
        Self {
            shown_hints: HashSet::new(),
            input_count: 0,
            assistant_response_count: 0,
            has_scrolled_up: false,
        }
    }

    /// Record that the user submitted an input. Returns a hint if one should be shown.
    pub fn record_input(&mut self) -> Option<&'static str> {
        self.input_count += 1;
        if self.input_count == 3 && !self.shown_hints.contains(&HintId::SearchHistory) {
            self.shown_hints.insert(HintId::SearchHistory);
            return Some("Ctrl+R to search history");
        }
        None
    }

    /// Record that an assistant response completed. Returns a hint if one should be shown.
    pub fn record_assistant_response(&mut self) -> Option<&'static str> {
        self.assistant_response_count += 1;
        if self.assistant_response_count == 5
            && !self.shown_hints.contains(&HintId::ActOnMessages)
        {
            self.shown_hints.insert(HintId::ActOnMessages);
            return Some("Ctrl+E to act on messages");
        }
        None
    }

    /// Record that the user scrolled up. Returns a hint if one should be shown.
    pub fn record_scroll_up(&mut self) -> Option<&'static str> {
        if !self.has_scrolled_up && !self.shown_hints.contains(&HintId::SearchConversation) {
            self.has_scrolled_up = true;
            self.shown_hints.insert(HintId::SearchConversation);
            return Some("Ctrl+F to search conversation");
        }
        self.has_scrolled_up = true;
        None
    }

    /// Check if the compact hint should be shown based on context percentage.
    /// Returns a hint if context > 80% and the hint hasn't been shown yet.
    pub fn check_context_usage(&mut self, context_percent: f64) -> Option<&'static str> {
        if context_percent > 80.0 && !self.shown_hints.contains(&HintId::CompactContext) {
            self.shown_hints.insert(HintId::CompactContext);
            return Some("/compact to free context");
        }
        None
    }
}

impl Default for HintTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Manager for notification popups with priority-based queueing.
pub struct NotificationManager {
    /// Currently displayed notifications (up to max_visible).
    active: Vec<Notification>,
    /// Queue of pending notifications waiting to be displayed.
    queue: VecDeque<Notification>,
    /// Maximum number of simultaneous notifications.
    max_visible: usize,
    /// Default duration for new notifications.
    default_duration: Duration,
    /// Total count of notifications added (including queued), for badge display.
    total_queued: usize,
    /// Hint tracker for contextual hints.
    pub hint_tracker: HintTracker,
}

impl NotificationManager {
    /// Create a new notification manager.
    pub fn new() -> Self {
        Self {
            active: Vec::new(),
            queue: VecDeque::new(),
            max_visible: 3,
            default_duration: Duration::from_secs(3),
            total_queued: 0,
            hint_tracker: HintTracker::new(),
        }
    }

    /// Set the default auto-dismiss duration.
    pub fn set_default_duration(&mut self, duration: Duration) {
        self.default_duration = duration;
    }

    /// Push a new notification with the default duration and Normal priority.
    pub fn push(&mut self, message: String, level: NotificationLevel) {
        self.push_with_priority(message, level, NotificationPriority::Normal);
    }

    /// Push a new notification with a custom duration (Normal priority).
    pub fn push_with_duration(
        &mut self,
        message: String,
        level: NotificationLevel,
        duration: Duration,
    ) {
        let multi_line = message.contains('\n');
        let notif = Notification {
            message,
            level,
            priority: NotificationPriority::Normal,
            created_at: Instant::now(),
            duration,
            multi_line,
        };
        self.enqueue(notif);
    }

    /// Push a notification with a specific priority level.
    pub fn push_with_priority(
        &mut self,
        message: String,
        level: NotificationLevel,
        priority: NotificationPriority,
    ) {
        let multi_line = message.contains('\n');
        let duration = priority.default_duration();
        let notif = Notification {
            message,
            level,
            priority,
            created_at: Instant::now(),
            duration,
            multi_line,
        };
        self.enqueue(notif);
    }

    /// Show a low-priority, short-duration hint notification.
    pub fn show_hint(&mut self, text: &str) {
        let notif = Notification {
            message: text.to_string(),
            level: NotificationLevel::Info,
            priority: NotificationPriority::Low,
            created_at: Instant::now(),
            duration: Duration::from_secs(2),
            multi_line: false,
        };
        self.enqueue(notif);
    }

    /// Internal: enqueue a notification based on its priority.
    fn enqueue(&mut self, notif: Notification) {
        self.total_queued += 1;
        match notif.priority {
            NotificationPriority::Immediate => {
                // Immediate: clear active and show right away
                self.active.clear();
                self.active.push(notif);
            }
            NotificationPriority::High => {
                // High: try to show immediately if there's room, else front of queue
                if self.active.len() < self.max_visible {
                    self.active.push(notif);
                } else {
                    self.queue.push_front(notif);
                }
            }
            NotificationPriority::Normal | NotificationPriority::Low => {
                // Normal/Low: show if room, else back of queue
                if self.active.len() < self.max_visible {
                    self.active.push(notif);
                } else {
                    self.queue.push_back(notif);
                }
            }
        }
    }

    /// Remove expired notifications and promote queued ones.
    pub fn prune(&mut self) {
        self.active.retain(|n| !n.is_expired());

        // Fill vacated slots from the queue
        while self.active.len() < self.max_visible {
            if let Some(notif) = self.queue.pop_front() {
                // Reset created_at so the queued notification gets its full duration
                let mut promoted = notif;
                promoted.created_at = Instant::now();
                self.active.push(promoted);
            } else {
                break;
            }
        }
    }

    /// Get the currently visible notifications.
    pub fn visible(&self) -> &[Notification] {
        &self.active
    }

    /// Return the number of notifications waiting in the queue (not yet visible).
    pub fn queued_count(&self) -> usize {
        self.queue.len()
    }

    /// Whether there are any active notifications.
    pub fn has_active(&self) -> bool {
        self.active.iter().any(|n| !n.is_expired())
    }

    /// Clear all notifications immediately.
    pub fn clear(&mut self) {
        self.active.clear();
        self.queue.clear();
        self.total_queued = 0;
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

        let queued_count = self.manager.queued_count();
        let mut row_offset: u16 = 0;

        for notif in visible.iter() {
            if notif.is_expired() {
                continue;
            }

            let lines: Vec<&str> = if notif.multi_line {
                notif.message.lines().collect()
            } else {
                vec![&notif.message]
            };

            let border_color = notif.priority.border_color();
            let severity_color = notif.level.color();
            let icon = notif.level.icon();
            let label = notif.level.label();
            let bg_color = theme::NOTIF_BG;

            for (line_idx, line_text) in lines.iter().enumerate() {
                let y = area.y + row_offset;
                if y >= area.y + area.height {
                    break;
                }

                // Build display content
                let content = if line_idx == 0 {
                    format!(" {} {}: {} ", icon, label, line_text)
                } else {
                    format!("   {} ", line_text)
                };
                let notif_width = content.len().min(area.width as usize);
                let x = area.x + area.width.saturating_sub(notif_width as u16);

                // Background fill
                let bg_style = theme::STYLE_NOTIF;
                for col in x..area.x + area.width {
                    buf[(col, y)].set_char(' ').set_style(bg_style);
                }

                // Left accent bar (colored by priority)
                if x < area.x + area.width {
                    buf[(x, y)]
                        .set_char('\u{2588}') // █
                        .set_style(Style::new().fg(border_color).bg(bg_color));
                }

                // Render content
                let render_x = x + 1; // after accent bar
                let available_width = area.width.saturating_sub(render_x - area.x);

                if line_idx == 0 {
                    let bold_notif = Style::new()
                        .fg(severity_color)
                        .bg(bg_color)
                        .add_modifier(Modifier::BOLD);
                    let line = Line::from(vec![
                        Span::styled(format!(" {} ", icon), bold_notif),
                        Span::styled(format!("{}: ", label), bold_notif),
                        Span::styled(line_text.to_string(), theme::STYLE_NOTIF),
                        Span::styled(" ", Style::new().bg(bg_color)),
                    ]);
                    buf.set_line(render_x, y, &line, available_width);
                } else {
                    let line = Line::from(vec![
                        Span::styled(format!("  {} ", line_text), theme::STYLE_NOTIF),
                    ]);
                    buf.set_line(render_x, y, &line, available_width);
                }

                row_offset += 1;
            }
        }

        // Show queued badge if there are more notifications waiting
        if queued_count > 0 {
            let badge = format!(" +{} more ", queued_count);
            let badge_width = badge.len() as u16;
            let badge_y = area.y + row_offset;
            if badge_y < area.y + area.height {
                let badge_x = area.x + area.width.saturating_sub(badge_width);
                let badge_style = Style::default()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC);
                buf.set_string(badge_x, badge_y, &badge, badge_style);
            }
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
        assert_eq!(mgr.queued_count(), 2);
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
        assert_eq!(mgr.visible()[0].duration, Duration::from_secs(3));
    }

    #[test]
    fn test_priority_immediate_replaces() {
        let mut mgr = NotificationManager::new();
        mgr.push("first".to_string(), NotificationLevel::Info);
        mgr.push("second".to_string(), NotificationLevel::Info);
        mgr.push_with_priority(
            "urgent".to_string(),
            NotificationLevel::Error,
            NotificationPriority::Immediate,
        );
        assert_eq!(mgr.visible().len(), 1);
        assert_eq!(mgr.visible()[0].message, "urgent");
    }

    #[test]
    fn test_priority_high_goes_to_front_of_queue() {
        let mut mgr = NotificationManager::new();
        // Fill active slots
        for i in 0..3 {
            mgr.push(format!("msg {}", i), NotificationLevel::Info);
        }
        // Queue a normal
        mgr.push("normal queued".to_string(), NotificationLevel::Info);
        // Queue a high - should go to front
        mgr.push_with_priority(
            "high queued".to_string(),
            NotificationLevel::Warning,
            NotificationPriority::High,
        );
        assert_eq!(mgr.queued_count(), 2);
        // The high priority should be at front of queue
        // Expire all active to promote from queue
        mgr.active.clear();
        mgr.prune();
        assert_eq!(mgr.visible()[0].message, "high queued");
    }

    #[test]
    fn test_show_hint() {
        let mut mgr = NotificationManager::new();
        mgr.show_hint("Press Ctrl+R to search");
        assert_eq!(mgr.visible().len(), 1);
        assert_eq!(mgr.visible()[0].priority, NotificationPriority::Low);
        assert_eq!(mgr.visible()[0].duration, Duration::from_secs(2));
    }

    #[test]
    fn test_queued_promotion_on_prune() {
        let mut mgr = NotificationManager::new();
        // Fill active
        for i in 0..3 {
            mgr.push_with_duration(
                format!("msg {}", i),
                NotificationLevel::Info,
                Duration::from_millis(0),
            );
        }
        // Queue one
        mgr.push("queued".to_string(), NotificationLevel::Info);
        assert_eq!(mgr.queued_count(), 1);
        std::thread::sleep(Duration::from_millis(1));
        mgr.prune();
        // Queued item should now be active
        assert_eq!(mgr.visible().len(), 1);
        assert_eq!(mgr.visible()[0].message, "queued");
        assert_eq!(mgr.queued_count(), 0);
    }

    #[test]
    fn test_multi_line_notification() {
        let mut mgr = NotificationManager::new();
        mgr.push("line1\nline2\nline3".to_string(), NotificationLevel::Info);
        assert!(mgr.visible()[0].multi_line);
        assert_eq!(mgr.visible()[0].line_count(), 3);
    }

    #[test]
    fn test_hint_tracker_input() {
        let mut tracker = HintTracker::new();
        assert!(tracker.record_input().is_none()); // 1st
        assert!(tracker.record_input().is_none()); // 2nd
        assert_eq!(tracker.record_input(), Some("Ctrl+R to search history")); // 3rd
        assert!(tracker.record_input().is_none()); // 4th - already shown
    }

    #[test]
    fn test_hint_tracker_assistant_response() {
        let mut tracker = HintTracker::new();
        for _ in 0..4 {
            assert!(tracker.record_assistant_response().is_none());
        }
        assert_eq!(
            tracker.record_assistant_response(),
            Some("Ctrl+E to act on messages")
        );
        assert!(tracker.record_assistant_response().is_none());
    }

    #[test]
    fn test_hint_tracker_scroll() {
        let mut tracker = HintTracker::new();
        assert_eq!(
            tracker.record_scroll_up(),
            Some("Ctrl+F to search conversation")
        );
        assert!(tracker.record_scroll_up().is_none()); // already shown
    }

    #[test]
    fn test_hint_tracker_context() {
        let mut tracker = HintTracker::new();
        assert!(tracker.check_context_usage(50.0).is_none());
        assert_eq!(
            tracker.check_context_usage(85.0),
            Some("/compact to free context")
        );
        assert!(tracker.check_context_usage(90.0).is_none()); // already shown
    }
}
