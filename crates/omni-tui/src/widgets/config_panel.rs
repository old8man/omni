//! Full-screen config panel widget matching the TypeScript `/config` command.
//!
//! Renders a tabbed overlay with a searchable, scrollable list of settings
//! that the user can toggle, cycle, or pick via sub-dialogs.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::theme;

// ── Setting value types ────────────────────────────────────────────────

/// The kind of picker a `ManagedEnum` opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerType {
    Theme,
    Model,
    OutputStyle,
    Language,
}

/// Current value of a single setting.
#[derive(Debug, Clone)]
pub enum SettingValue {
    Bool(bool),
    Enum {
        current: String,
        options: Vec<String>,
    },
    ManagedEnum {
        current: String,
        options: Vec<String>,
        picker_type: PickerType,
    },
    ReadOnly(String),
}

/// One row in the settings list.
#[derive(Debug, Clone)]
pub struct SettingItem {
    pub key: String,
    pub label: String,
    pub value: SettingValue,
    pub category: &'static str,
}

// ── Sub-dialog (picker overlay) ────────────────────────────────────────

/// A small selection list that opens on top of the config panel for
/// `ManagedEnum` settings.
#[derive(Debug, Clone)]
pub struct PickerDialog {
    pub title: String,
    pub options: Vec<String>,
    pub selected: usize,
    /// Index of the parent setting item so we can write back.
    pub setting_index: usize,
}

impl PickerDialog {
    fn new(title: String, options: Vec<String>, current: &str, setting_index: usize) -> Self {
        let selected = options
            .iter()
            .position(|o| o == current)
            .unwrap_or(0);
        Self {
            title,
            options,
            selected,
            setting_index,
        }
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.options.len() {
            self.selected += 1;
        }
    }

    fn current_value(&self) -> &str {
        &self.options[self.selected]
    }
}

// ── Tab enum ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigTab {
    Config,
    Status,
    Usage,
}

impl ConfigTab {
    const ALL: [ConfigTab; 3] = [ConfigTab::Config, ConfigTab::Status, ConfigTab::Usage];

    fn label(self) -> &'static str {
        match self {
            ConfigTab::Config => "Config",
            ConfigTab::Status => "Status",
            ConfigTab::Usage => "Usage",
        }
    }

    fn next(self) -> Self {
        match self {
            ConfigTab::Config => ConfigTab::Status,
            ConfigTab::Status => ConfigTab::Usage,
            ConfigTab::Usage => ConfigTab::Config,
        }
    }

    fn prev(self) -> Self {
        match self {
            ConfigTab::Config => ConfigTab::Usage,
            ConfigTab::Status => ConfigTab::Config,
            ConfigTab::Usage => ConfigTab::Status,
        }
    }
}

// ── Key event result ───────────────────────────────────────────────────

/// What the caller should do after a key event.
pub enum ConfigPanelAction {
    /// The panel consumed the key; keep it open.
    Consumed,
    /// The user confirmed; close the panel and apply changes.
    Close { changes: Vec<(String, String)> },
    /// The user cancelled; close the panel and revert.
    Cancel,
}

// ── Main panel state ───────────────────────────────────────────────────

pub struct ConfigPanel {
    /// Active tab.
    pub tab: ConfigTab,
    /// All setting items (unfiltered).
    items: Vec<SettingItem>,
    /// Snapshot of initial values (for revert on Esc).
    snapshot: Vec<SettingValue>,
    /// Index of the highlighted item in the *filtered* list.
    selected: usize,
    /// Scroll offset for the visible window.
    scroll_offset: usize,
    /// Whether we are in search mode.
    search_active: bool,
    /// Current search query.
    search_query: String,
    /// Picker dialog overlay (if open).
    picker: Option<PickerDialog>,
    /// Status tab text (pre-rendered).
    status_text: String,
    /// Usage tab text (pre-rendered).
    usage_text: String,
}

impl ConfigPanel {
    /// Create a new config panel, reading current values from the
    /// provided parameters.
    pub fn new(
        model: &str,
        vim_mode: bool,
        plan_mode: bool,
    ) -> Self {
        let items = default_settings(model, vim_mode, plan_mode);
        let snapshot: Vec<SettingValue> = items.iter().map(|i| i.value.clone()).collect();
        Self {
            tab: ConfigTab::Config,
            items,
            snapshot,
            selected: 0,
            scroll_offset: 0,
            search_active: false,
            search_query: String::new(),
            picker: None,
            status_text: String::new(),
            usage_text: String::new(),
        }
    }

    /// Provide text content for the Status tab.
    pub fn set_status_text(&mut self, text: String) {
        self.status_text = text;
    }

    /// Provide text content for the Usage tab.
    pub fn set_usage_text(&mut self, text: String) {
        self.usage_text = text;
    }

    // ── Filtered view helpers ──────────────────────────────────────────

    fn filtered_indices(&self) -> Vec<usize> {
        if self.search_query.is_empty() {
            (0..self.items.len()).collect()
        } else {
            let q = self.search_query.to_lowercase();
            self.items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.label.to_lowercase().contains(&q)
                        || item.key.to_lowercase().contains(&q)
                        || item.category.to_lowercase().contains(&q)
                })
                .map(|(i, _)| i)
                .collect()
        }
    }

    fn filtered_len(&self) -> usize {
        self.filtered_indices().len()
    }

    fn selected_real_index(&self) -> Option<usize> {
        let indices = self.filtered_indices();
        indices.get(self.selected).copied()
    }

    // ── Navigation ─────────────────────────────────────────────────────

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    fn move_down(&mut self) {
        let len = self.filtered_len();
        if len > 0 && self.selected + 1 < len {
            self.selected += 1;
            // Approximate visible height; fine-tuned at render time is not possible
            // since render takes &self. Use a reasonable default.
            self.ensure_visible(20);
        }
    }

    fn ensure_visible(&mut self, visible_height: usize) {
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_height {
            self.scroll_offset = self.selected.saturating_sub(visible_height.saturating_sub(1));
        }
    }

    // ── Toggle / cycle the selected item ───────────────────────────────

    fn toggle_selected(&mut self) {
        if let Some(idx) = self.selected_real_index() {
            match &mut self.items[idx].value {
                SettingValue::Bool(b) => {
                    *b = !*b;
                }
                SettingValue::Enum {
                    current,
                    options,
                } => {
                    if let Some(pos) = options.iter().position(|o| o == current) {
                        *current = options[(pos + 1) % options.len()].clone();
                    }
                }
                _ => {}
            }
        }
    }

    fn open_picker_for_selected(&mut self) {
        if let Some(idx) = self.selected_real_index() {
            match &self.items[idx].value {
                SettingValue::ManagedEnum {
                    current,
                    options,
                    ..
                } => {
                    self.picker = Some(PickerDialog::new(
                        self.items[idx].label.clone(),
                        options.clone(),
                        current,
                        idx,
                    ));
                }
                SettingValue::Enum { current, options } => {
                    self.picker = Some(PickerDialog::new(
                        self.items[idx].label.clone(),
                        options.clone(),
                        current,
                        idx,
                    ));
                }
                _ => {}
            }
        }
    }

    // ── Collect changes ────────────────────────────────────────────────

    fn collect_changes(&self) -> Vec<(String, String)> {
        let mut changes = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            let old = &self.snapshot[i];
            let new_val = value_display_string(&item.value);
            let old_val = value_display_string(old);
            if new_val != old_val {
                changes.push((item.key.clone(), new_val));
            }
        }
        changes
    }

    // ── Key handling ───────────────────────────────────────────────────

    pub fn handle_key(
        &mut self,
        code: crossterm::event::KeyCode,
        _modifiers: crossterm::event::KeyModifiers,
    ) -> ConfigPanelAction {
        use crossterm::event::KeyCode;

        // If a picker dialog is open, route keys to it.
        if let Some(ref mut picker) = self.picker {
            match code {
                KeyCode::Up | KeyCode::Char('k') => picker.move_up(),
                KeyCode::Down | KeyCode::Char('j') => picker.move_down(),
                KeyCode::Enter => {
                    let val = picker.current_value().to_string();
                    let idx = picker.setting_index;
                    match &mut self.items[idx].value {
                        SettingValue::ManagedEnum {
                            current, ..
                        } => {
                            *current = val;
                        }
                        SettingValue::Enum {
                            current, ..
                        } => {
                            *current = val;
                        }
                        _ => {}
                    }
                    self.picker = None;
                }
                KeyCode::Esc => {
                    self.picker = None;
                }
                _ => {}
            }
            return ConfigPanelAction::Consumed;
        }

        // Search mode input.
        if self.search_active {
            match code {
                KeyCode::Esc => {
                    self.search_active = false;
                    self.search_query.clear();
                    self.selected = 0;
                    self.scroll_offset = 0;
                }
                KeyCode::Enter => {
                    self.search_active = false;
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.selected = 0;
                    self.scroll_offset = 0;
                }
                KeyCode::Char(c) => {
                    self.search_query.push(c);
                    self.selected = 0;
                    self.scroll_offset = 0;
                }
                KeyCode::Up => self.move_up(),
                KeyCode::Down => self.move_down(),
                _ => {}
            }
            return ConfigPanelAction::Consumed;
        }

        // Normal mode.
        match code {
            KeyCode::Esc => {
                // Revert all changes.
                for (i, snap) in self.snapshot.iter().enumerate() {
                    self.items[i].value = snap.clone();
                }
                return ConfigPanelAction::Cancel;
            }
            KeyCode::Enter => {
                // If on a ManagedEnum, open the picker instead of closing.
                if let Some(idx) = self.selected_real_index() {
                    if matches!(self.items[idx].value, SettingValue::ManagedEnum { .. }) {
                        self.open_picker_for_selected();
                        return ConfigPanelAction::Consumed;
                    }
                }
                let changes = self.collect_changes();
                return ConfigPanelAction::Close { changes };
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Char(' ') => {
                if let Some(idx) = self.selected_real_index() {
                    match &self.items[idx].value {
                        SettingValue::ManagedEnum { .. } => {
                            self.open_picker_for_selected();
                        }
                        _ => self.toggle_selected(),
                    }
                }
            }
            KeyCode::Tab => {
                // For enum-like items, cycle forward; otherwise treat as toggle.
                self.toggle_selected();
            }
            KeyCode::Char('/') => {
                self.search_active = true;
                self.search_query.clear();
            }
            KeyCode::Left => {
                self.tab = self.tab.prev();
            }
            KeyCode::Right => {
                self.tab = self.tab.next();
            }
            KeyCode::Char('q') => {
                let changes = self.collect_changes();
                return ConfigPanelAction::Close { changes };
            }
            _ => {}
        }

        ConfigPanelAction::Consumed
    }
}

// ── Rendering ──────────────────────────────────────────────────────────

impl Widget for &ConfigPanel {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear the full overlay area.
        Clear.render(area, buf);

        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled("Settings", theme::STYLE_BOLD_CYAN),
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

        let mut y = inner.y;
        let content_width = inner.width.saturating_sub(2) as usize;

        // ── Tab bar ────────────────────────────────────────────────────
        {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(Span::raw(" "));
            for tab in &ConfigTab::ALL {
                if *tab == self.tab {
                    spans.push(Span::styled(
                        format!(" {} ", tab.label()),
                        Style::new()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::styled(
                        format!(" {} ", tab.label()),
                        theme::STYLE_DARK_GRAY,
                    ));
                }
                spans.push(Span::raw(" "));
            }
            buf.set_line(inner.x + 1, y, &Line::from(spans), inner.width.saturating_sub(2));
            y += 1;
        }

        // Separator below tabs.
        {
            let sep: String = "\u{2500}".repeat(content_width);
            let sep_line = Line::from(Span::styled(sep, theme::STYLE_DARK_GRAY));
            buf.set_line(inner.x + 1, y, &sep_line, inner.width.saturating_sub(2));
            y += 1;
        }

        // ── Tab content ────────────────────────────────────────────────
        match self.tab {
            ConfigTab::Config => {
                self.render_config_tab(inner, buf, y, content_width);
            }
            ConfigTab::Status => {
                render_text_tab(&self.status_text, inner, buf, y, content_width);
            }
            ConfigTab::Usage => {
                render_text_tab(&self.usage_text, inner, buf, y, content_width);
            }
        }

        // ── Picker dialog overlay ──────────────────────────────────────
        if let Some(ref picker) = self.picker {
            render_picker(picker, area, buf);
        }
    }
}

impl ConfigPanel {
    fn render_config_tab(
        &self,
        inner: Rect,
        buf: &mut Buffer,
        start_y: u16,
        content_width: usize,
    ) {
        let mut y = start_y;

        // Search box.
        if self.search_active {
            let search_line = Line::from(vec![
                Span::styled(" / ", theme::STYLE_BOLD_YELLOW),
                Span::styled(&self.search_query, theme::STYLE_WHITE),
                Span::styled("\u{2588}", theme::STYLE_YELLOW), // cursor block
            ]);
            buf.set_line(inner.x + 1, y, &search_line, inner.width.saturating_sub(2));
            y += 1;
        } else if !self.search_query.is_empty() {
            let filter_line = Line::from(vec![
                Span::styled(" Filter: ", theme::STYLE_DARK_GRAY),
                Span::styled(&self.search_query, theme::STYLE_YELLOW),
            ]);
            buf.set_line(inner.x + 1, y, &filter_line, inner.width.saturating_sub(2));
            y += 1;
        }

        // Reserve footer (2 lines: separator + keybindings).
        let footer_height: u16 = 2;
        let list_bottom = inner.y + inner.height.saturating_sub(footer_height);
        let visible_height = list_bottom.saturating_sub(y) as usize;

        // Filtered items.
        let indices = self.filtered_indices();

        // Render settings rows.
        let scroll = self.scroll_offset;
        let mut current_category: Option<&str> = None;
        let mut rendered: usize = 0;
        let mut logical_row: usize = 0;

        for &real_idx in &indices {
            let item = &self.items[real_idx];

            // Category header (counts toward logical rows).
            let needs_header = current_category != Some(item.category);
            if needs_header {
                // Skip rows before scroll offset.
                if logical_row >= scroll && rendered < visible_height {
                    if rendered > 0 {
                        // blank line before category
                        y += 1;
                        rendered += 1;
                        if rendered >= visible_height {
                            break;
                        }
                    }
                    let cat_line = Line::from(Span::styled(
                        format!(" {}", item.category),
                        theme::STYLE_BOLD_CYAN,
                    ));
                    buf.set_line(inner.x + 1, y, &cat_line, inner.width.saturating_sub(2));
                    y += 1;
                    rendered += 1;
                }
                logical_row += 1;
                current_category = Some(item.category);
            }

            if logical_row < scroll {
                logical_row += 1;
                continue;
            }
            if rendered >= visible_height {
                break;
            }

            // Find logical position in filtered list to determine if selected.
            let filtered_pos = indices
                .iter()
                .position(|&i| i == real_idx)
                .unwrap_or(usize::MAX);
            let is_selected = filtered_pos == self.selected;

            // Row: pointer + label + value
            let pointer = if is_selected { "\u{25b8} " } else { "  " };
            let label = &item.label;
            let (val_text, val_style) = value_display(&item.value);

            let label_width = 44.min(content_width.saturating_sub(4));
            let padded_label = if label.len() >= label_width {
                label[..label_width].to_string()
            } else {
                format!("{:<width$}", label, width = label_width)
            };

            let row_style = if is_selected {
                Style::new().bg(Color::Rgb(40, 40, 60))
            } else {
                Style::default()
            };

            let line = Line::from(vec![
                Span::styled(
                    pointer,
                    if is_selected {
                        theme::STYLE_BOLD_CYAN
                    } else {
                        theme::STYLE_DARK_GRAY
                    },
                ),
                Span::styled(padded_label, row_style.fg(Color::White)),
                Span::styled(val_text, val_style),
            ]);

            buf.set_line(inner.x + 1, y, &line, inner.width.saturating_sub(2));
            y += 1;
            rendered += 1;
            logical_row += 1;
        }

        // Footer separator.
        let footer_sep_y = inner.y + inner.height.saturating_sub(footer_height);
        {
            let sep: String = "\u{2500}".repeat(content_width);
            buf.set_line(
                inner.x + 1,
                footer_sep_y,
                &Line::from(Span::styled(sep, theme::STYLE_DARK_GRAY)),
                inner.width.saturating_sub(2),
            );
        }

        // Footer keybindings.
        let hint_y = inner.y + inner.height - 1;
        let hints = if self.search_active {
            " Esc: clear search  \u{2191}\u{2193}: navigate  Enter: done "
        } else {
            " \u{2191}\u{2193}/jk: navigate  Space: toggle  Enter: confirm  /: search  Esc: cancel  \u{2190}\u{2192}: tabs "
        };
        buf.set_line(
            inner.x + 1,
            hint_y,
            &Line::from(Span::styled(hints, theme::STYLE_DARK_GRAY)),
            inner.width.saturating_sub(2),
        );
    }
}

// ── Plain text tab rendering (Status / Usage) ──────────────────────────

fn render_text_tab(text: &str, inner: Rect, buf: &mut Buffer, start_y: u16, content_width: usize) {
    let mut y = start_y;
    for line_text in text.lines() {
        if y >= inner.y + inner.height.saturating_sub(1) {
            break;
        }
        let truncated = if line_text.len() > content_width {
            &line_text[..content_width]
        } else {
            line_text
        };
        buf.set_line(
            inner.x + 1,
            y,
            &Line::from(Span::styled(truncated, theme::STYLE_WHITE)),
            inner.width.saturating_sub(2),
        );
        y += 1;
    }
}

// ── Picker dialog rendering ────────────────────────────────────────────

fn render_picker(picker: &PickerDialog, parent_area: Rect, buf: &mut Buffer) {
    let item_count = picker.options.len() as u16;
    let width = 50u16.min(parent_area.width.saturating_sub(8));
    let height = (item_count + 4).min(parent_area.height.saturating_sub(6));
    let dialog_area = parent_area.centered(
        Constraint::Length(width),
        Constraint::Length(height),
    );

    Clear.render(dialog_area, buf);

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(&picker.title, theme::STYLE_BOLD_YELLOW),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_style(theme::STYLE_YELLOW)
        .style(Style::new().bg(Color::Rgb(25, 25, 35)));

    let inner = block.inner(dialog_area);
    block.render(dialog_area, buf);

    if inner.height < 2 || inner.width < 10 {
        return;
    }

    let mut y = inner.y;
    for (i, opt) in picker.options.iter().enumerate() {
        if y >= inner.y + inner.height.saturating_sub(1) {
            break;
        }
        let is_sel = i == picker.selected;
        let pointer = if is_sel { "\u{25b8} " } else { "  " };
        let style = if is_sel {
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            theme::STYLE_WHITE
        };
        let line = Line::from(vec![
            Span::styled(
                pointer,
                if is_sel {
                    theme::STYLE_BOLD_CYAN
                } else {
                    theme::STYLE_DARK_GRAY
                },
            ),
            Span::styled(opt.as_str(), style),
        ]);
        buf.set_line(inner.x + 1, y, &line, inner.width.saturating_sub(2));
        y += 1;
    }

    // Hint line at bottom.
    let hint_y = inner.y + inner.height - 1;
    buf.set_line(
        inner.x + 1,
        hint_y,
        &Line::from(Span::styled(
            " \u{2191}\u{2193}: select  Enter: confirm  Esc: cancel ",
            theme::STYLE_DARK_GRAY,
        )),
        inner.width.saturating_sub(2),
    );
}

// ── Helpers ────────────────────────────────────────────────────────────

fn value_display(val: &SettingValue) -> (String, Style) {
    match val {
        SettingValue::Bool(b) => {
            if *b {
                ("true".to_string(), theme::STYLE_GREEN)
            } else {
                ("false".to_string(), theme::STYLE_RED)
            }
        }
        SettingValue::Enum { current, .. } => (current.clone(), theme::STYLE_YELLOW),
        SettingValue::ManagedEnum { current, .. } => {
            (format!("{} \u{25be}", current), theme::STYLE_MAGENTA)
        }
        SettingValue::ReadOnly(s) => (s.clone(), theme::STYLE_DARK_GRAY),
    }
}

fn value_display_string(val: &SettingValue) -> String {
    match val {
        SettingValue::Bool(b) => b.to_string(),
        SettingValue::Enum { current, .. } => current.clone(),
        SettingValue::ManagedEnum { current, .. } => current.clone(),
        SettingValue::ReadOnly(s) => s.clone(),
    }
}

// ── Default settings list ──────────────────────────────────────────────

fn default_settings(model: &str, vim_mode: bool, plan_mode: bool) -> Vec<SettingItem> {
    vec![
        SettingItem {
            key: "theme".into(),
            label: "Theme".into(),
            value: SettingValue::ManagedEnum {
                current: "auto".into(),
                options: vec![
                    "auto".into(),
                    "dark".into(),
                    "light".into(),
                    "dark-daltonized".into(),
                    "light-daltonized".into(),
                    "dark-ansi".into(),
                    "light-ansi".into(),
                ],
                picker_type: PickerType::Theme,
            },
            category: "Appearance",
        },
        SettingItem {
            key: "editor_mode".into(),
            label: "Editor mode".into(),
            value: SettingValue::Enum {
                current: if vim_mode {
                    "vim".into()
                } else {
                    "normal".into()
                },
                options: vec!["normal".into(), "vim".into()],
            },
            category: "Appearance",
        },
        SettingItem {
            key: "reduce_motion".into(),
            label: "Reduce motion".into(),
            value: SettingValue::Bool(false),
            category: "Appearance",
        },
        SettingItem {
            key: "show_tips".into(),
            label: "Show tips".into(),
            value: SettingValue::Bool(true),
            category: "Appearance",
        },
        SettingItem {
            key: "language".into(),
            label: "Language".into(),
            value: SettingValue::ManagedEnum {
                current: "en".into(),
                options: vec![
                    "en".into(),
                    "es".into(),
                    "fr".into(),
                    "de".into(),
                    "it".into(),
                    "pt".into(),
                    "ja".into(),
                    "ko".into(),
                    "zh".into(),
                ],
                picker_type: PickerType::Language,
            },
            category: "Appearance",
        },
        SettingItem {
            key: "auto_compact".into(),
            label: "Auto-compact".into(),
            value: SettingValue::Bool(true),
            category: "Conversation",
        },
        SettingItem {
            key: "model".into(),
            label: "Model".into(),
            value: SettingValue::ManagedEnum {
                current: model.into(),
                options: vec![
                    "claude-sonnet-4-6".into(),
                    "claude-opus-4-6".into(),
                    "claude-haiku-3-5".into(),
                ],
                picker_type: PickerType::Model,
            },
            category: "Conversation",
        },
        SettingItem {
            key: "thinking_mode".into(),
            label: "Thinking mode".into(),
            value: SettingValue::Bool(true),
            category: "Conversation",
        },
        SettingItem {
            key: "fast_mode".into(),
            label: "Fast mode".into(),
            value: SettingValue::Bool(false),
            category: "Conversation",
        },
        SettingItem {
            key: "default_permission_mode".into(),
            label: "Default permission mode".into(),
            value: SettingValue::Enum {
                current: if plan_mode {
                    "plan".into()
                } else {
                    "default".into()
                },
                options: vec![
                    "default".into(),
                    "plan".into(),
                    "auto".into(),
                    "bypassPermissions".into(),
                ],
            },
            category: "Permissions",
        },
        SettingItem {
            key: "verbose_output".into(),
            label: "Verbose output".into(),
            value: SettingValue::Bool(false),
            category: "Output",
        },
        SettingItem {
            key: "show_turn_duration".into(),
            label: "Show turn duration".into(),
            value: SettingValue::Bool(false),
            category: "Output",
        },
        SettingItem {
            key: "notification_channel".into(),
            label: "Notification channel".into(),
            value: SettingValue::Enum {
                current: "auto".into(),
                options: vec![
                    "auto".into(),
                    "iterm2".into(),
                    "terminal_bell".into(),
                    "kitty".into(),
                    "ghostty".into(),
                    "disabled".into(),
                ],
            },
            category: "Output",
        },
        SettingItem {
            key: "rewind_code".into(),
            label: "Rewind code (checkpoints)".into(),
            value: SettingValue::Bool(true),
            category: "Tools",
        },
        SettingItem {
            key: "diff_tool".into(),
            label: "Diff tool".into(),
            value: SettingValue::Enum {
                current: "terminal".into(),
                options: vec!["terminal".into(), "auto".into()],
            },
            category: "Tools",
        },
        SettingItem {
            key: "copy_on_select".into(),
            label: "Copy on select".into(),
            value: SettingValue::Bool(true),
            category: "Tools",
        },
        SettingItem {
            key: "output_style".into(),
            label: "Output style".into(),
            value: SettingValue::ManagedEnum {
                current: "text".into(),
                options: vec![
                    "text".into(),
                    "json".into(),
                    "stream-json".into(),
                ],
                picker_type: PickerType::OutputStyle,
            },
            category: "Output",
        },
        SettingItem {
            key: "respect_gitignore".into(),
            label: "Respect .gitignore".into(),
            value: SettingValue::Bool(true),
            category: "Tools",
        },
    ]
}
