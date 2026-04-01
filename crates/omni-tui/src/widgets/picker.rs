//! Generic picker/selector overlay widget.
//!
//! Provides a reusable selection dialog that renders as a centered popup
//! with search, scrolling, keyboard navigation, and badge display.
//! Used for model selection, theme switching, session resume, etc.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::theme;

// ── Picker item ─────────────────────────────────────────────────────────────

/// A single selectable entry in a picker list.
#[derive(Clone, Debug)]
pub struct PickerItem<T: Clone> {
    /// Primary display label.
    pub label: String,
    /// Optional secondary description (rendered dimmed below the label).
    pub description: Option<String>,
    /// The value returned when this item is selected.
    pub value: T,
    /// Whether this item represents the currently active selection.
    pub is_current: bool,
    /// Optional right-aligned badge text (e.g. "[Default]", "[Current]").
    pub badge: Option<String>,
}

// ── Picker action ───────────────────────────────────────────────────────────

/// Result of handling a key event in the picker.
#[derive(Clone, Debug)]
pub enum PickerAction<T: Clone> {
    /// An item was selected (Enter).
    Selected(T),
    /// The picker was cancelled (Esc).
    Cancelled,
    /// No action; the picker consumed the key for internal state changes.
    None,
}

// ── Picker state ────────────────────────────────────────────────────────────

/// State for the generic picker dialog.
pub struct PickerState<T: Clone> {
    /// Title shown in the popup border.
    pub title: String,
    /// Optional description line shown below the title.
    pub description: Option<String>,
    /// All available items (unfiltered).
    pub items: Vec<PickerItem<T>>,
    /// Index into the *filtered* list.
    pub selected: usize,
    /// Scroll offset for the visible window.
    pub scroll_offset: usize,
    /// Current search/filter query.
    pub search_query: String,
    /// Whether the search input is active (typing filters items).
    pub search_active: bool,
    /// Cached filtered indices (into `items`).
    filtered_indices: Vec<usize>,
}

impl<T: Clone> PickerState<T> {
    /// Create a new picker with the given title and items.
    pub fn new(title: impl Into<String>, items: Vec<PickerItem<T>>) -> Self {
        let count = items.len();
        let indices: Vec<usize> = (0..count).collect();
        // Pre-select the current item if one is marked
        let initial_selected = items
            .iter()
            .position(|item| item.is_current)
            .unwrap_or(0);
        Self {
            title: title.into(),
            description: None,
            items,
            selected: initial_selected,
            scroll_offset: 0,
            search_query: String::new(),
            search_active: true,
            filtered_indices: indices,
        }
    }

    /// Set an optional description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Return the filtered items (references).
    pub fn filtered_items(&self) -> Vec<&PickerItem<T>> {
        self.filtered_indices
            .iter()
            .map(|&i| &self.items[i])
            .collect()
    }

    /// Number of items after filtering.
    pub fn filtered_count(&self) -> usize {
        self.filtered_indices.len()
    }

    /// Recompute the filtered indices based on the current search query.
    fn refilter(&mut self) {
        let query = self.search_query.to_lowercase();
        if query.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    let label_lower = item.label.to_lowercase();
                    let desc_lower = item
                        .description
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase();
                    let badge_lower = item.badge.as_deref().unwrap_or("").to_lowercase();
                    // Fuzzy: all query chars appear in order in the label
                    fuzzy_match(&query, &label_lower)
                        || desc_lower.contains(&query)
                        || badge_lower.contains(&query)
                })
                .map(|(i, _)| i)
                .collect();
        }
        // Clamp selection
        if self.filtered_indices.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len() - 1;
        }
        // Clamp scroll
        self.clamp_scroll();
    }

    /// Ensure the selected item is visible given the scroll offset.
    fn clamp_scroll(&mut self) {
        // We use a default visible height of 10; actual render adapts.
        let visible = 10usize;
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible {
            self.scroll_offset = self.selected.saturating_sub(visible - 1);
        }
    }

    /// Handle a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> PickerAction<T> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => PickerAction::Cancelled,
            (_, KeyCode::Enter) => {
                if let Some(&idx) = self.filtered_indices.get(self.selected) {
                    PickerAction::Selected(self.items[idx].value.clone())
                } else {
                    PickerAction::Cancelled
                }
            }
            (_, KeyCode::Up) | (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
                if self.selected > 0 {
                    self.selected -= 1;
                    self.clamp_scroll();
                }
                PickerAction::None
            }
            (_, KeyCode::Down) | (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
                if self.selected + 1 < self.filtered_indices.len() {
                    self.selected += 1;
                    self.clamp_scroll();
                }
                PickerAction::None
            }
            (_, KeyCode::Home) | (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                self.selected = 0;
                self.clamp_scroll();
                PickerAction::None
            }
            (_, KeyCode::End) | (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                if !self.filtered_indices.is_empty() {
                    self.selected = self.filtered_indices.len() - 1;
                }
                self.clamp_scroll();
                PickerAction::None
            }
            (_, KeyCode::Backspace) => {
                if !self.search_query.is_empty() {
                    self.search_query.pop();
                    self.refilter();
                }
                PickerAction::None
            }
            (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                self.search_query.clear();
                self.refilter();
                PickerAction::None
            }
            (_, KeyCode::Char(c)) if self.search_active => {
                self.search_query.push(c);
                self.refilter();
                PickerAction::None
            }
            _ => PickerAction::None,
        }
    }
}

// ── Fuzzy match helper ──────────────────────────────────────────────────────

/// Simple fuzzy match: all characters in `needle` appear in order in `haystack`.
fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let mut hay_chars = haystack.chars();
    for nc in needle.chars() {
        loop {
            match hay_chars.next() {
                Some(hc) if hc == nc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

// ── Picker widget ───────────────────────────────────────────────────────────

/// Renders a `PickerState` as a centered overlay.
pub struct PickerWidget<'a, T: Clone> {
    state: &'a PickerState<T>,
}

impl<'a, T: Clone> PickerWidget<'a, T> {
    pub fn new(state: &'a PickerState<T>) -> Self {
        Self { state }
    }
}

impl<T: Clone> Widget for PickerWidget<'_, T> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Compute popup dimensions
        let popup_width = (area.width * 70 / 100).max(30).min(area.width);
        let max_items = self.state.filtered_count().min(12);
        // title(1) + description?(1) + search(1) + separator(1) + items + separator(1) + footer(1)
        let desc_lines: u16 = if self.state.description.is_some() {
            1
        } else {
            0
        };
        let popup_height = (2 + desc_lines + 1 + 1 + max_items as u16 + 1 + 1)
            .max(8)
            .min(area.height);

        let popup = area.centered(
            ratatui::layout::Constraint::Length(popup_width),
            ratatui::layout::Constraint::Length(popup_height),
        );

        // Clear background
        Clear.render(popup, buf);

        // Draw block with border
        let block = Block::default()
            .title(format!(" {} ", self.state.title))
            .borders(Borders::ALL)
            .border_style(theme::STYLE_CYAN)
            .style(Style::new().bg(theme::STATUS_BG));
        let inner = block.inner(popup);
        block.render(popup, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let mut y = inner.y;

        // Description line (optional)
        if let Some(ref desc) = self.state.description {
            if y < inner.y + inner.height {
                let line = Line::from(Span::styled(desc.as_str(), theme::DIM));
                buf.set_line(inner.x + 1, y, &line, inner.width.saturating_sub(2));
                y += 1;
            }
        }

        // Search input line
        if y < inner.y + inner.height {
            let search_label = Span::styled(" Search: ", theme::STYLE_BOLD_CYAN);
            let search_text = Span::styled(
                self.state.search_query.as_str(),
                Style::new().fg(Color::White),
            );
            let match_count = Span::styled(
                format!("  ({} matches)", self.state.filtered_count()),
                theme::DIM,
            );
            let line = Line::from(vec![search_label, search_text, match_count]);
            buf.set_line(inner.x, y, &line, inner.width);
            y += 1;
        }

        // Separator
        if y < inner.y + inner.height {
            let sep = "\u{2500}".repeat(inner.width as usize);
            let line = Line::from(Span::styled(sep, theme::STYLE_DARK_GRAY));
            buf.set_line(inner.x, y, &line, inner.width);
            y += 1;
        }

        // Compute visible window
        let items_area_height = inner
            .y
            .saturating_add(inner.height)
            .saturating_sub(y)
            .saturating_sub(2) as usize; // reserve 2 lines for separator + footer
        let visible_count = items_area_height.min(self.state.filtered_count());

        // Adjust scroll offset so selected is visible
        let scroll = if self.state.selected < self.state.scroll_offset {
            self.state.selected
        } else if self.state.selected >= self.state.scroll_offset + visible_count {
            self.state
                .selected
                .saturating_sub(visible_count.saturating_sub(1))
        } else {
            self.state.scroll_offset
        };

        let filtered = self.state.filtered_items();
        let above_count = scroll;
        let below_count = self
            .state
            .filtered_count()
            .saturating_sub(scroll + visible_count);

        // Scroll indicator (above)
        if above_count > 0 && y < inner.y + inner.height {
            let indicator = format!("  \u{2191} {} more above", above_count);
            let line = Line::from(Span::styled(indicator, theme::STYLE_DARK_GRAY));
            buf.set_line(inner.x, y, &line, inner.width);
            y += 1;
        }

        // Render items
        for (display_idx, item_idx) in (scroll..self.state.filtered_count())
            .enumerate()
            .take(visible_count)
        {
            if y >= inner.y + inner.height.saturating_sub(2) {
                break;
            }

            let item = &filtered[item_idx];
            let is_selected = item_idx == self.state.selected;

            let pointer = if is_selected { "\u{25b8} " } else { "  " };
            let pointer_style = if is_selected {
                theme::STYLE_BOLD_CYAN
            } else {
                Style::default()
            };

            let label_style = if is_selected {
                Style::new()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if item.is_current {
                theme::STYLE_GREEN
            } else {
                Style::new().fg(Color::White)
            };

            let mut spans = vec![
                Span::styled(pointer, pointer_style),
                Span::styled(item.label.as_str(), label_style),
            ];

            // Badge (right-aligned, dimmed)
            if let Some(ref badge) = item.badge {
                let used_width = pointer.len() + item.label.len();
                let available = inner.width as usize;
                let badge_str = format!(" {}", badge);
                let pad_len = available
                    .saturating_sub(used_width)
                    .saturating_sub(badge_str.len())
                    .saturating_sub(1);
                let padding = " ".repeat(pad_len);
                spans.push(Span::raw(padding));
                spans.push(Span::styled(
                    badge_str,
                    if item.is_current {
                        theme::STYLE_GREEN
                    } else {
                        theme::DIM
                    },
                ));
            }

            let line = Line::from(spans);
            buf.set_line(inner.x, y, &line, inner.width);
            y += 1;

            // Description below label (dimmed, indented)
            if let Some(ref desc) = item.description {
                if y < inner.y + inner.height.saturating_sub(2) {
                    let indent = "    ";
                    let desc_line = Line::from(Span::styled(
                        format!("{}{}", indent, desc),
                        theme::DIM,
                    ));
                    buf.set_line(inner.x, y, &desc_line, inner.width);
                    y += 1;
                }
            }

            let _ = display_idx; // used in the for pattern
        }

        // Scroll indicator (below)
        if below_count > 0 {
            if y < inner.y + inner.height.saturating_sub(1) {
                let indicator = format!("  \u{2193} {} more below", below_count);
                let line = Line::from(Span::styled(indicator, theme::STYLE_DARK_GRAY));
                buf.set_line(inner.x, y, &line, inner.width);
                y += 1;
            }
        }

        // Footer separator
        let footer_y = inner.y + inner.height.saturating_sub(2);
        if footer_y > y || footer_y == y {
            let sep = "\u{2500}".repeat(inner.width as usize);
            let line = Line::from(Span::styled(sep, theme::STYLE_DARK_GRAY));
            buf.set_line(inner.x, footer_y, &line, inner.width);
        }

        // Footer with keybinding hints
        let hint_y = inner.y + inner.height.saturating_sub(1);
        let hints = Line::from(vec![
            Span::styled(" \u{2191}\u{2193}", theme::STYLE_BOLD_CYAN),
            Span::styled(" navigate  ", theme::DIM),
            Span::styled("Enter", theme::STYLE_BOLD_CYAN),
            Span::styled(" select  ", theme::DIM),
            Span::styled("Esc", theme::STYLE_BOLD_CYAN),
            Span::styled(" cancel  ", theme::DIM),
            Span::styled("Type", theme::STYLE_BOLD_CYAN),
            Span::styled(" to filter", theme::DIM),
        ]);
        buf.set_line(inner.x, hint_y, &hints, inner.width);
    }
}

// ── Specific picker constructors ────────────────────────────────────────────

/// The kind of picker currently active.
pub enum ActivePicker {
    Model(PickerState<String>),
    Theme(PickerState<String>),
    Session(PickerState<String>),
    Profile(PickerState<String>),
}

impl ActivePicker {
    /// Handle a key event, returning the action for the specific picker type.
    pub fn handle_key(&mut self, key: KeyEvent) -> PickerAction<String> {
        match self {
            ActivePicker::Model(ref mut state) => state.handle_key(key),
            ActivePicker::Theme(ref mut state) => state.handle_key(key),
            ActivePicker::Session(ref mut state) => state.handle_key(key),
            ActivePicker::Profile(ref mut state) => state.handle_key(key),
        }
    }

    /// Render the picker overlay.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        match self {
            ActivePicker::Model(ref state) => PickerWidget::new(state).render(area, buf),
            ActivePicker::Theme(ref state) => PickerWidget::new(state).render(area, buf),
            ActivePicker::Session(ref state) => PickerWidget::new(state).render(area, buf),
            ActivePicker::Profile(ref state) => PickerWidget::new(state).render(area, buf),
        }
    }
}

/// Build a profile picker populated with all configured profiles.
pub fn build_profile_picker() -> PickerState<String> {
    use omni_core::auth::profiles;

    let all_profiles = profiles::list_profiles();
    let active_name = profiles::get_active_profile_name();

    let items: Vec<PickerItem<String>> = all_profiles
        .iter()
        .map(|p| {
            let is_active = active_name.as_deref() == Some(p.name.as_str());
            let status = p.status_label(active_name.as_deref());
            let badge_text = match status {
                "active" => Some("Active".to_string()),
                "expired" => Some("Expired".to_string()),
                _ => None,
            };
            let sub = match p.subscription_type.as_str() {
                "pro" => "Pro",
                "max" => "Max",
                "team" => "Team",
                "enterprise" => "Enterprise",
                "api" => "API",
                _ => &p.subscription_type,
            };
            PickerItem {
                label: p.name.clone(),
                description: Some(format!("{} - {}", p.email, sub)),
                value: p.name.clone(),
                is_current: is_active,
                badge: badge_text,
            }
        })
        .collect();

    PickerState::new("Switch Profile", items)
}

/// Build a model picker populated with known Claude models.
pub fn build_model_picker(current_model: &str) -> PickerState<String> {
    use omni_core::utils::model;

    let models: Vec<(&str, &str, &str, bool)> = vec![
        (
            model::OPUS_46,
            "200K context, 128K output",
            "Extra usage",
            true,
        ),
        (
            model::SONNET_46,
            "200K context, 128K output",
            "",
            false,
        ),
        (
            model::OPUS_45,
            "200K context, 64K output",
            "Extra usage",
            false,
        ),
        (
            model::SONNET_45,
            "200K context, 64K output",
            "",
            false,
        ),
        (
            model::SONNET_40,
            "200K context, 64K output",
            "",
            false,
        ),
        (
            model::OPUS_40,
            "200K context, 32K output",
            "Extra usage",
            false,
        ),
        (
            model::HAIKU_45,
            "200K context, 64K output",
            "",
            false,
        ),
        (
            model::HAIKU_35,
            "200K context, 8K output",
            "",
            false,
        ),
        (
            model::SONNET_37,
            "200K context, 64K output (deprecated)",
            "",
            false,
        ),
        (
            model::SONNET_35,
            "200K context, 8K output (deprecated)",
            "",
            false,
        ),
    ];

    let canonical_current = model::get_canonical_name(current_model);

    let items: Vec<PickerItem<String>> = models
        .into_iter()
        .map(|(id, desc, pricing, is_expensive)| {
            let display_name = model::get_public_model_display_name(id)
                .map(|s| s.to_string())
                .unwrap_or_else(|| id.to_string());
            let is_current = model::get_canonical_name(id) == canonical_current;
            let mut badge_parts = Vec::new();
            if is_current {
                badge_parts.push("[Current]".to_string());
            }
            if is_expensive && !pricing.is_empty() {
                badge_parts.push(format!("[{}]", pricing));
            }
            let badge = if badge_parts.is_empty() {
                None
            } else {
                Some(badge_parts.join(" "))
            };
            PickerItem {
                label: display_name,
                description: Some(desc.to_string()),
                value: id.to_string(),
                is_current,
                badge,
            }
        })
        .collect();

    PickerState::new("Select Model", items)
        .with_description("Choose a model for this session")
}

/// Build a theme picker.
pub fn build_theme_picker(current_theme: &str) -> PickerState<String> {
    let themes = vec![
        ("auto", "Detect from terminal"),
        ("dark", "Dark background, light text"),
        ("light", "Light background, dark text"),
        ("dark-daltonized", "Dark theme, color-blind friendly"),
        ("light-daltonized", "Light theme, color-blind friendly"),
        ("dark-ansi", "Dark theme, ANSI colors only"),
        ("light-ansi", "Light theme, ANSI colors only"),
    ];

    let items: Vec<PickerItem<String>> = themes
        .into_iter()
        .map(|(name, desc)| {
            let is_current = name == current_theme;
            PickerItem {
                label: name.to_string(),
                description: Some(desc.to_string()),
                value: name.to_string(),
                is_current,
                badge: if is_current {
                    Some("[Current]".to_string())
                } else {
                    None
                },
            }
        })
        .collect();

    PickerState::new("Select Theme", items)
        .with_description("Choose a color theme")
}

/// Build a session picker from a list of session summaries.
pub fn build_session_picker(
    sessions: &[omni_core::session::SessionSummary],
) -> PickerState<String> {
    let items: Vec<PickerItem<String>> = sessions
        .iter()
        .map(|s| {
            let title = s
                .display_title()
                .unwrap_or("(untitled)")
                .to_string();
            // Truncate long titles
            let label = if title.len() > 60 {
                format!("{}...", &title[..57])
            } else {
                title
            };
            let model_info = s.model.as_deref().unwrap_or("unknown");
            let date = s.updated_at.format("%Y-%m-%d %H:%M");
            let desc = format!("{} | {} | {} msgs", date, model_info, s.message_count);
            let tag_badge = s.tag.as_ref().map(|t| format!("[{}]", t));
            PickerItem {
                label,
                description: Some(desc),
                value: s.id.clone(),
                is_current: false,
                badge: tag_badge,
            }
        })
        .collect();

    PickerState::new("Resume Session", items)
        .with_description("Select a session to resume")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn test_picker_navigation() {
        let items = vec![
            PickerItem {
                label: "Alpha".into(),
                description: None,
                value: "a".to_string(),
                is_current: false,
                badge: None,
            },
            PickerItem {
                label: "Beta".into(),
                description: None,
                value: "b".to_string(),
                is_current: false,
                badge: None,
            },
            PickerItem {
                label: "Gamma".into(),
                description: None,
                value: "c".to_string(),
                is_current: false,
                badge: None,
            },
        ];
        let mut picker = PickerState::new("Test", items);
        assert_eq!(picker.selected, 0);

        picker.handle_key(make_key(KeyCode::Down));
        assert_eq!(picker.selected, 1);

        picker.handle_key(make_key(KeyCode::Down));
        assert_eq!(picker.selected, 2);

        // Should not go past end
        picker.handle_key(make_key(KeyCode::Down));
        assert_eq!(picker.selected, 2);

        picker.handle_key(make_key(KeyCode::Up));
        assert_eq!(picker.selected, 1);
    }

    #[test]
    fn test_picker_select() {
        let items = vec![
            PickerItem {
                label: "Alpha".into(),
                description: None,
                value: "a".to_string(),
                is_current: false,
                badge: None,
            },
            PickerItem {
                label: "Beta".into(),
                description: None,
                value: "b".to_string(),
                is_current: true,
                badge: Some("[Current]".into()),
            },
        ];
        let mut picker = PickerState::new("Test", items);
        // Should pre-select the current item
        assert_eq!(picker.selected, 1);

        let action = picker.handle_key(make_key(KeyCode::Enter));
        match action {
            PickerAction::Selected(val) => assert_eq!(val, "b"),
            _ => panic!("Expected Selected action"),
        }
    }

    #[test]
    fn test_picker_cancel() {
        let items = vec![PickerItem {
            label: "Alpha".into(),
            description: None,
            value: "a".to_string(),
            is_current: false,
            badge: None,
        }];
        let mut picker = PickerState::new("Test", items);
        let action = picker.handle_key(make_key(KeyCode::Esc));
        assert!(matches!(action, PickerAction::Cancelled));
    }

    #[test]
    fn test_picker_search() {
        let items = vec![
            PickerItem {
                label: "Opus 4.6".into(),
                description: None,
                value: "opus".to_string(),
                is_current: false,
                badge: None,
            },
            PickerItem {
                label: "Sonnet 4.6".into(),
                description: None,
                value: "sonnet".to_string(),
                is_current: false,
                badge: None,
            },
            PickerItem {
                label: "Haiku 4.5".into(),
                description: None,
                value: "haiku".to_string(),
                is_current: false,
                badge: None,
            },
        ];
        let mut picker = PickerState::new("Test", items);
        assert_eq!(picker.filtered_count(), 3);

        // Type "son" to filter
        picker.handle_key(make_key(KeyCode::Char('s')));
        picker.handle_key(make_key(KeyCode::Char('o')));
        picker.handle_key(make_key(KeyCode::Char('n')));
        assert_eq!(picker.filtered_count(), 1);
        assert_eq!(picker.filtered_items()[0].value, "sonnet");

        // Backspace to widen
        picker.handle_key(make_key(KeyCode::Backspace));
        picker.handle_key(make_key(KeyCode::Backspace));
        picker.handle_key(make_key(KeyCode::Backspace));
        assert_eq!(picker.filtered_count(), 3);
    }

    #[test]
    fn test_fuzzy_match() {
        assert!(fuzzy_match("ops", "opus 4.6"));
        assert!(fuzzy_match("snt", "sonnet"));
        assert!(!fuzzy_match("xyz", "opus"));
        assert!(fuzzy_match("", "anything"));
    }
}
