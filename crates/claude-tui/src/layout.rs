//! Layout helpers for the TUI.
//!
//! Provides a [`TuiLayout`] that computes the main layout regions (header,
//! messages, spinner, input, status bar) and a helper for centered overlay
//! dialogs.

use ratatui::layout::{Constraint, Layout, Rect};

/// Regions of the main TUI layout.
pub struct TuiLayout {
    /// Top decorative border.
    pub top_border: Rect,
    /// Header line (app name, model, tokens).
    pub header: Rect,
    /// Separator below header.
    pub header_separator: Rect,
    /// Main message/conversation area.
    pub messages: Rect,
    /// Spinner area (zero height when inactive).
    pub spinner: Rect,
    /// Prompt input area.
    pub input: Rect,
    /// Status bar at the bottom.
    pub status_bar: Rect,
}

impl TuiLayout {
    /// Compute the layout regions for the given terminal area.
    ///
    /// `spinner_active` controls whether the spinner row gets a non-zero height.
    /// `show_status_bar` controls whether the bottom status bar is shown.
    pub fn compute(area: Rect, spinner_active: bool, show_status_bar: bool) -> Self {
        let spinner_height = if spinner_active { 1 } else { 0 };
        let status_height = if show_status_bar { 1 } else { 0 };

        let chunks = Layout::default()
            .constraints([
                Constraint::Length(1),              // Top border
                Constraint::Length(1),              // Header
                Constraint::Length(1),              // Header separator
                Constraint::Min(1),                 // Messages
                Constraint::Length(spinner_height),  // Spinner
                Constraint::Length(3),              // Input (with top border)
                Constraint::Length(status_height),   // Status bar
            ])
            .split(area);

        Self {
            top_border: chunks[0],
            header: chunks[1],
            header_separator: chunks[2],
            messages: chunks[3],
            spinner: chunks[4],
            input: chunks[5],
            status_bar: chunks[6],
        }
    }
}

/// Calculate a centered rect within the given area.
///
/// `percent_x` is the width as a percentage of the container (0–100).
/// `height` is the absolute height in rows.
pub fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let width = area.width * percent_x / 100;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height.min(area.height))
}

/// Calculate a rect anchored to the bottom of the area.
///
/// Useful for dropdowns or completion menus that grow upward.
pub fn bottom_anchored_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height.saturating_sub(height);
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

/// Split a rect horizontally into left and right panes at the given ratio.
///
/// `left_percent` is 0–100. Returns `(left, right)`.
pub fn horizontal_split(area: Rect, left_percent: u16) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left_percent),
            Constraint::Percentage(100 - left_percent),
        ])
        .split(area);
    (chunks[0], chunks[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tui_layout_basic() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = TuiLayout::compute(area, false, true);

        assert_eq!(layout.top_border.height, 1);
        assert_eq!(layout.header.height, 1);
        assert_eq!(layout.header_separator.height, 1);
        assert_eq!(layout.spinner.height, 0);
        assert_eq!(layout.input.height, 3);
        assert_eq!(layout.status_bar.height, 1);
        // Messages get the rest
        assert!(layout.messages.height > 0);
    }

    #[test]
    fn test_tui_layout_with_spinner() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = TuiLayout::compute(area, true, true);
        assert_eq!(layout.spinner.height, 1);
    }

    #[test]
    fn test_tui_layout_no_status_bar() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = TuiLayout::compute(area, false, false);
        assert_eq!(layout.status_bar.height, 0);
    }

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 40);
        let rect = centered_rect(60, 10, area);
        assert_eq!(rect.width, 60);
        assert_eq!(rect.height, 10);
        assert_eq!(rect.x, 20); // (100 - 60) / 2
        assert_eq!(rect.y, 15); // (40 - 10) / 2
    }

    #[test]
    fn test_bottom_anchored_rect() {
        let area = Rect::new(0, 0, 80, 24);
        let rect = bottom_anchored_rect(40, 5, area);
        assert_eq!(rect.width, 40);
        assert_eq!(rect.height, 5);
        assert_eq!(rect.y, 19); // 24 - 5
    }

    #[test]
    fn test_horizontal_split() {
        let area = Rect::new(0, 0, 100, 40);
        let (left, right) = horizontal_split(area, 30);
        assert_eq!(left.width, 30);
        assert_eq!(right.width, 70);
    }
}
