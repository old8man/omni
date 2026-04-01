//! Layout helpers for the TUI.
//!
//! Provides a [`TuiLayout`] that computes the main layout regions (header,
//! messages, spinner, input, status bar) and helpers for overlay dialogs.
//! The input area grows with content up to 40% of terminal height.

use ratatui::layout::{Constraint, Layout, Rect};

/// Regions of the main TUI layout.
pub struct TuiLayout {
    /// Header line (product name, model, session info).
    pub header: Rect,
    /// Separator below header.
    pub header_separator: Rect,
    /// Main message/conversation area (scrollable with virtual scroll).
    pub messages: Rect,
    /// Spinner area (zero height when inactive, grows for sub-spinners).
    pub spinner: Rect,
    /// Prompt input area (grows with content, min 3 lines, max 40% terminal).
    pub input: Rect,
    /// Status bar at the very bottom (full width, colored background).
    pub status_bar: Rect,
}

impl TuiLayout {
    /// Compute the layout regions for the given terminal area.
    ///
    /// - `spinner_height`: number of rows for the spinner (0 when inactive,
    ///   1 for single spinner, more for parallel sub-spinners).
    /// - `input_line_count`: number of content lines in the input area (for
    ///   dynamic growth). The input will be at least 3 rows and at most 40%
    ///   of the terminal height.
    /// - `show_status_bar`: whether to display the bottom status bar.
    pub fn compute(
        area: Rect,
        spinner_height: u16,
        input_line_count: u16,
        show_status_bar: bool,
    ) -> Self {
        let status_height = if show_status_bar { 1 } else { 0 };

        // Input area: minimum 3 rows (border + 1 line + border), grows with
        // content, capped at 40% of terminal height.
        let max_input_height = (area.height as f32 * 0.4) as u16;
        // +2 for top/bottom border of the input block
        let desired_input = (input_line_count + 2).max(3);
        let input_height = desired_input.min(max_input_height).max(3);

        let layout = Layout::vertical([
            Constraint::Length(1),              // Header
            Constraint::Length(1),              // Header separator
            Constraint::Min(1),                 // Messages (flex)
            Constraint::Length(spinner_height),  // Spinner
            Constraint::Length(input_height),   // Input (dynamic)
            Constraint::Length(status_height),   // Status bar
        ]);
        let [header, header_separator, messages, spinner, input, status_bar] =
            area.layout(&layout);

        Self {
            header,
            header_separator,
            messages,
            spinner,
            input,
            status_bar,
        }
    }
}

/// Calculate a centered rect within the given area.
///
/// `percent_x` is the width as a percentage of the container (0-100).
/// `height` is the absolute height in rows.
pub fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    area.centered(
        Constraint::Percentage(percent_x),
        Constraint::Length(height),
    )
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
/// `left_percent` is 0-100. Returns `(left, right)`.
pub fn horizontal_split(area: Rect, left_percent: u16) -> (Rect, Rect) {
    let layout = Layout::horizontal([
        Constraint::Percentage(left_percent),
        Constraint::Percentage(100 - left_percent),
    ]);
    let [left, right] = area.layout(&layout);
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tui_layout_basic() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = TuiLayout::compute(area, 0, 1, true);

        assert_eq!(layout.header.height, 1);
        assert_eq!(layout.header_separator.height, 1);
        assert_eq!(layout.spinner.height, 0);
        assert!(layout.input.height >= 3);
        assert_eq!(layout.status_bar.height, 1);
        // Messages get the rest
        assert!(layout.messages.height > 0);
    }

    #[test]
    fn test_tui_layout_with_spinner() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = TuiLayout::compute(area, 1, 1, true);
        assert_eq!(layout.spinner.height, 1);
    }

    #[test]
    fn test_tui_layout_multi_spinner() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = TuiLayout::compute(area, 3, 1, true);
        assert_eq!(layout.spinner.height, 3);
    }

    #[test]
    fn test_tui_layout_no_status_bar() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = TuiLayout::compute(area, 0, 1, false);
        assert_eq!(layout.status_bar.height, 0);
    }

    #[test]
    fn test_tui_layout_input_grows() {
        let area = Rect::new(0, 0, 80, 40);
        // 10 lines of input content -> 12 rows (+ borders)
        let layout = TuiLayout::compute(area, 0, 10, true);
        assert!(layout.input.height >= 10);
        // But capped at 40% of 40 = 16
        assert!(layout.input.height <= 16);
    }

    #[test]
    fn test_tui_layout_input_max_cap() {
        let area = Rect::new(0, 0, 80, 24);
        // Many lines but capped at 40% of 24 ≈ 9
        let layout = TuiLayout::compute(area, 0, 30, true);
        assert!(layout.input.height <= 10); // 40% of 24 ≈ 9.6
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
