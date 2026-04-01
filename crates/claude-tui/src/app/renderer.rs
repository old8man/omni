use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;

use crate::theme;

use crate::widgets::message_list::MessageListWidget;
use crate::widgets::notification::NotificationWidget;
use crate::widgets::prompt_input::PromptInputWidget;
use crate::widgets::search_overlay::SearchOverlayWidget;
use crate::widgets::status_bar::{StatusBarState, StatusBarWidget};
use crate::widgets::welcome::{WelcomeState, WelcomeWidget};

use super::App;

impl App {
    pub(crate) fn render(&mut self) -> Result<()> {
        // Capture vim mode indicator for rendering
        let vim_mode_str: Option<String> = if self.vim_mode {
            Some(self.input_handler.mode().to_string())
        } else {
            None
        };
        // Compute dynamic input area height based on line count
        let _input_area_height = (self.prompt.line_count() as u16).clamp(1, 10) + 1;
        // Prune expired notifications before rendering
        self.notifications.prune();

        // Auto-dismiss expired flash messages and capture for render
        if let Some(ref fm) = self.flash_message {
            if fm.is_expired() {
                self.flash_message = None;
            }
        }

        // Context window usage warning: suggest /compact when over 80%
        if !self.context_warning_shown && self.flash_message.is_none() {
            let ctx_window =
                claude_core::utils::context::get_context_window_for_model(&self.model_name);
            if ctx_window > 0 {
                let ctx_pct = (self.total_tokens as f64 / ctx_window as f64) * 100.0;
                if ctx_pct > 80.0 {
                    self.context_warning_shown = true;
                    self.flash(crate::widgets::status_bar::FlashMessage::warning(
                        "Context window >80% full. Consider using /compact to free space.",
                    ));
                }
                // Show contextual hint notification for high context usage
                if let Some(hint) = self.notifications.hint_tracker.check_context_usage(ctx_pct) {
                    self.notifications.show_hint(hint);
                }
            }
        }

        let state_flash = self.flash_message.clone();

        let theme = &self.theme;
        let spinner = &self.spinner;
        let message_list = &self.message_list;
        let prompt = &self.prompt;
        let permission_dialog = &self.permission_dialog;
        let action_menu = &self.action_menu;
        let search_overlay = &self.search_overlay;
        let notifications = &self.notifications;
        let input_handler = &self.input_handler;
        let selection = &self.selection;
        let show_welcome = message_list.is_empty() && !spinner.active;
        // Read live state from AppStateStore (non-blocking try_read to avoid
        // stalling the 60fps render loop if the engine holds a write lock).
        let (state_model, state_tokens, state_cost, state_plan, state_vim, state_agent_name) =
            if let Some(ref state) = self.app_state {
                if let Some(s) = state.try_read() {
                    (
                        s.active_model().to_string(),
                        s.total_input_tokens() + s.total_output_tokens(),
                        s.total_cost_usd(),
                        s.is_plan_mode(),
                        s.vim_mode,
                        s.agent_name.clone(),
                    )
                } else {
                    (
                        self.model_name.clone(),
                        self.total_tokens,
                        self.total_cost,
                        self.plan_mode,
                        self.vim_mode,
                        None,
                    )
                }
            } else {
                (
                    self.model_name.clone(),
                    self.total_tokens,
                    self.total_cost,
                    self.plan_mode,
                    self.vim_mode,
                    None,
                )
            };

        // Compute input line count for dynamic input area sizing
        let input_line_count = prompt.line_count().max(1) as u16;
        let spinner_height = spinner.render_height();

        self.terminal.draw(|frame| {
            let area = frame.area();

            // Use the new layout system: Header | Separator | Messages | Spinner | Input | StatusBar
            let layout = crate::layout::TuiLayout::compute(
                area,
                spinner_height,
                input_line_count,
                true, // always show status bar
            );

            // Header: "Claude Code (Rust) | model"
            let header_spans = vec![
                ratatui::text::Span::styled(
                    " Claude Code (Rust)",
                    ratatui::style::Style::new()
                        .fg(theme.accent)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ),
                ratatui::text::Span::styled(
                    " | ",
                    ratatui::style::Style::new().fg(theme.border),
                ),
                ratatui::text::Span::styled(
                    &state_model,
                    ratatui::style::Style::new().fg(theme.muted),
                ),
            ];
            let header = Paragraph::new(ratatui::text::Line::from(header_spans));
            frame.render_widget(header, layout.header);

            // Header separator
            let border_line = "\u{2500}".repeat(area.width as usize);
            let header_sep = Paragraph::new(border_line)
                .style(ratatui::style::Style::new().fg(theme.border));
            frame.render_widget(header_sep, layout.header_separator);

            // Messages area (or welcome screen for new sessions)
            if show_welcome {
                let welcome_state = WelcomeState::new(state_model.clone());
                let welcome_widget = WelcomeWidget::new(&welcome_state);
                frame.render_widget(welcome_widget, layout.messages);
            } else {
                let msg_widget = MessageListWidget::new(message_list);
                frame.render_widget(msg_widget, layout.messages);

                // Apply selection highlighting over the message area
                if selection.has_selection() {
                    let ((sc, sr), (ec, er)) = selection.normalized();
                    let msg_area = layout.messages;
                    for row in sr..=er {
                        if row < msg_area.y || row >= msg_area.y + msg_area.height {
                            continue;
                        }
                        let col_start = if row == sr { sc } else { msg_area.x };
                        let col_end = if row == er { ec } else { msg_area.x + msg_area.width - 1 };
                        for col in col_start..=col_end {
                            if col >= msg_area.x + msg_area.width {
                                break;
                            }
                            if let Some(cell) = frame.buffer_mut().cell_mut(ratatui::layout::Position::new(col, row)) {
                                // Invert foreground/background for selection highlight
                                let fg = cell.fg;
                                let bg = cell.bg;
                                cell.fg = if bg == ratatui::style::Color::Reset {
                                    ratatui::style::Color::Black
                                } else {
                                    bg
                                };
                                cell.bg = if fg == ratatui::style::Color::Reset {
                                    ratatui::style::Color::White
                                } else {
                                    fg
                                };
                            }
                        }
                    }
                }
            }

            // Spinner
            if spinner.active {
                frame.render_widget(spinner, layout.spinner);
            }

            // Input
            let mut input_widget = PromptInputWidget::new(prompt);
            if let Some(ref mode_str) = vim_mode_str {
                input_widget = input_widget.vim_mode(mode_str);
            }
            frame.render_widget(input_widget, layout.input);

            // Set cursor position in the input area so it's visible
            if permission_dialog.is_none() && !search_overlay.active {
                let cursor_pos = prompt.cursor_pos();
                let prompt_prefix_len: u16 = 2; // "> "
                // Convert byte offset to display column width (handles UTF-8 properly)
                let text_before_cursor = &prompt.lines()[cursor_pos.row][..cursor_pos.col];
                let display_col = crate::unicode_width::display_width(text_before_cursor) as u16;
                let cursor_x = layout.input.x + prompt_prefix_len + display_col;
                // +1 for the top border of the input block
                let cursor_y = layout.input.y + 1 + cursor_pos.row as u16;
                if cursor_x < layout.input.x + layout.input.width
                    && cursor_y < layout.input.y + layout.input.height
                {
                    frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
                }
            }

            // Completion popup (rendered above input area so it's not clipped)
            if let Some(ref comp) = prompt.completion {
                let max_visible = comp.items.len().min(10);
                let popup_height = max_visible as u16;
                // Position popup above the input area
                let popup_y = layout.input.y.saturating_sub(popup_height);
                let popup_x = layout.input.x + 2; // align with "> " prefix

                // Scroll window so selected item is always visible
                let scroll_start = if comp.selected >= max_visible {
                    comp.selected - max_visible + 1
                } else {
                    0
                };

                // Compute widths from ALL items (not just visible window)
                let max_label_w = comp.items.iter()
                    .map(|item| item.label.len())
                    .max()
                    .unwrap_or(10)
                    .min(30);
                let max_desc_w = comp.items.iter()
                    .filter_map(|item| item.description.as_ref())
                    .map(|d| d.len())
                    .max()
                    .unwrap_or(0)
                    .min(40);
                let entry_width = (max_label_w + max_desc_w + 4).min(area.width as usize - 4) as u16;

                for (display_idx, item_idx) in (scroll_start..comp.items.len())
                    .enumerate()
                    .take(max_visible)
                {
                    let item = &comp.items[item_idx];
                    let y = popup_y + display_idx as u16;
                    if y >= layout.input.y {
                        break;
                    }
                    let is_selected = item_idx == comp.selected;
                    let label_style = if is_selected {
                        ratatui::style::Style::new()
                            .fg(ratatui::style::Color::Black)
                            .bg(ratatui::style::Color::Cyan)
                            .add_modifier(ratatui::style::Modifier::BOLD)
                    } else {
                        ratatui::style::Style::new()
                            .fg(ratatui::style::Color::White)
                            .bg(ratatui::style::Color::Rgb(40, 40, 50))
                    };
                    let desc_style = if is_selected {
                        ratatui::style::Style::new()
                            .fg(ratatui::style::Color::DarkGray)
                            .bg(ratatui::style::Color::Cyan)
                    } else {
                        ratatui::style::Style::new()
                            .fg(ratatui::style::Color::Gray)
                            .bg(ratatui::style::Color::Rgb(40, 40, 50))
                    };

                    let label = format!(" {:<width$}", item.label, width = max_label_w);
                    let desc = item.description.as_deref()
                        .map(|d| format!("  {}", d))
                        .unwrap_or_default();
                    let pad_len = entry_width as usize - label.len().min(entry_width as usize) - desc.len().min(entry_width as usize);
                    let padding = " ".repeat(pad_len.min(20));

                    let line = ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(label, label_style),
                        ratatui::text::Span::styled(desc, desc_style),
                        ratatui::text::Span::styled(padding, label_style),
                    ]);
                    frame.render_widget(
                        ratatui::widgets::Paragraph::new(line),
                        ratatui::layout::Rect::new(popup_x, y, entry_width, 1),
                    );
                }

                // Scroll indicators
                let indicator_x = popup_x + entry_width;
                if indicator_x < area.width {
                    if scroll_start > 0 {
                        // Show "up arrow" at top
                        let up_span = ratatui::text::Span::styled(
                            "\u{25b2}",
                            theme::STYLE_DARK_GRAY,
                        );
                        frame.render_widget(
                            ratatui::widgets::Paragraph::new(ratatui::text::Line::from(up_span)),
                            ratatui::layout::Rect::new(indicator_x, popup_y, 2, 1),
                        );
                    }
                    if scroll_start + max_visible < comp.items.len() {
                        // Show "down arrow" at bottom
                        let down_y = popup_y + popup_height.saturating_sub(1);
                        let down_span = ratatui::text::Span::styled(
                            "\u{25bc}",
                            theme::STYLE_DARK_GRAY,
                        );
                        frame.render_widget(
                            ratatui::widgets::Paragraph::new(ratatui::text::Line::from(down_span)),
                            ratatui::layout::Rect::new(indicator_x, down_y, 2, 1),
                        );
                    }
                }
            }

            // Status bar (full-featured, bottom of screen)
            let status_state = StatusBarState {
                model_name: state_model.clone(),
                total_tokens: state_tokens,
                total_cost: state_cost,
                input_mode: input_handler.mode(),
                vim_enabled: state_vim,
                plan_mode: state_plan,
                context_percent: {
                    let ctx_window = claude_core::utils::context::get_context_window_for_model(&state_model);
                    if ctx_window > 0 {
                        (state_tokens as f64 / ctx_window as f64) * 100.0
                    } else {
                        0.0
                    }
                },
                session_name: state_agent_name.clone(),
                rate_limited: false,
                flash: state_flash.clone(),
            };
            let status_widget = StatusBarWidget::new(&status_state);
            frame.render_widget(status_widget, layout.status_bar);

            // Permission dialog overlay (sized for JSON preview)
            if let Some(dialog) = permission_dialog {
                let dialog_height = (area.height * 60 / 100).max(12).min(area.height);
                let dialog_area = crate::layout::centered_rect(70, dialog_height, area);
                frame.render_widget(dialog, dialog_area);
            }

            // Action menu overlay (Ctrl+E popup)
            if action_menu.is_some() {
                let menu_width = 40u16.min(area.width.saturating_sub(4));
                let menu_height = 7u16.min(area.height.saturating_sub(4));
                let menu_x = area.width.saturating_sub(menu_width) / 2;
                let menu_y = area.height.saturating_sub(menu_height) / 2;
                let menu_area = Rect::new(menu_x, menu_y, menu_width, menu_height);

                // Render a Clear widget first to blank the area
                frame.render_widget(ratatui::widgets::Clear, menu_area);

                let block = ratatui::widgets::Block::default()
                    .title(" Message Actions ")
                    .borders(ratatui::widgets::Borders::ALL)
                    .border_style(theme::STYLE_CYAN)
                    .style(ratatui::style::Style::new().bg(theme::STATUS_BG));

                let menu_lines = vec![
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [C] ", theme::STYLE_BOLD_CYAN),
                        ratatui::text::Span::raw("Copy to clipboard"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [E] ", theme::STYLE_BOLD_CYAN),
                        ratatui::text::Span::raw("Edit (put in input)"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [R] ", theme::STYLE_BOLD_CYAN),
                        ratatui::text::Span::raw("Rewind to here"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [S] ", theme::STYLE_BOLD_CYAN),
                        ratatui::text::Span::raw("Summarize"),
                    ]),
                    ratatui::text::Line::from(ratatui::text::Span::styled(
                        " Esc to close",
                        theme::STYLE_DARK_GRAY,
                    )),
                ];
                let menu_paragraph = ratatui::widgets::Paragraph::new(menu_lines).block(block);
                frame.render_widget(menu_paragraph, menu_area);
            }

            // Search overlay (at bottom of messages area)
            if search_overlay.active {
                let search_area = Rect::new(
                    layout.messages.x,
                    layout.messages.y + layout.messages.height.saturating_sub(1),
                    layout.messages.width,
                    1,
                );
                let search_widget = SearchOverlayWidget::new(search_overlay);
                frame.render_widget(search_widget, search_area);
            }

            // Notification popups (top-right corner, below header)
            if notifications.has_active() {
                let notif_lines: u16 = notifications.visible().iter()
                    .filter(|n| !n.is_expired())
                    .map(|n| n.line_count() as u16)
                    .sum::<u16>()
                    + if notifications.queued_count() > 0 { 1 } else { 0 };
                let notif_height = notif_lines.max(1).min(area.height.saturating_sub(2));
                let notif_area = Rect::new(
                    area.x,
                    area.y + 2, // below header + separator
                    area.width,
                    notif_height,
                );
                let notif_widget = NotificationWidget::new(notifications);
                frame.render_widget(notif_widget, notif_area);
            }
        })?;
        Ok(())
    }
}
