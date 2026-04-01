//! Welcome screen widget displayed when starting a new session.
//!
//! Shows a centered bordered box with product info, model name, and tips.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// State for the welcome screen.
pub struct WelcomeState {
    /// The active model name.
    pub model_name: String,
}

impl WelcomeState {
    pub fn new(model_name: String) -> Self {
        Self { model_name }
    }
}

/// Widget that renders the welcome screen.
pub struct WelcomeWidget<'a> {
    state: &'a WelcomeState,
}

impl<'a> WelcomeWidget<'a> {
    pub fn new(state: &'a WelcomeState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for WelcomeWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 12 || area.width < 44 {
            // Fallback for very small terminals
            let line = Line::from(Span::styled(
                "Claude Code (Rust Edition) - Type a message or /help",
                Style::default().fg(Color::Cyan),
            ));
            let x = area.x + area.width.saturating_sub(line.width() as u16) / 2;
            let y = area.y + area.height / 2;
            buf.set_line(x, y, &line, area.width);
            return;
        }

        // Box dimensions
        let box_width: u16 = 44;
        let box_height: u16 = 11;
        let x0 = area.x + (area.width.saturating_sub(box_width)) / 2;
        let y0 = area.y + (area.height.saturating_sub(box_height)) / 2;

        let border = Style::default().fg(Color::DarkGray);
        let title_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let label_style = Style::default().fg(Color::DarkGray);
        let value_style = Style::default().fg(Color::White);
        let tip_bullet = Style::default().fg(Color::Cyan);
        let tip_text = Style::default().fg(Color::DarkGray);
        let tip_key = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        // Inner width (excluding border chars and padding)
        let inner_w = box_width.saturating_sub(4) as usize;

        // Draw the box
        let mut y = y0;

        // Top border: ╭──...──╮
        let top = format!(
            "\u{256D}{}\u{256E}",
            "\u{2500}".repeat(box_width as usize - 2)
        );
        buf.set_line(x0, y, &Line::from(Span::styled(top, border)), box_width);
        y += 1;

        // Title line
        let title = "Claude Code (Rust Edition)";
        let pad = inner_w.saturating_sub(title.len());
        let left_pad = pad / 2;
        let right_pad = pad - left_pad;
        let title_line = Line::from(vec![
            Span::styled("\u{2502} ", border),
            Span::raw(" ".repeat(left_pad)),
            Span::styled(title, title_style),
            Span::raw(" ".repeat(right_pad)),
            Span::styled(" \u{2502}", border),
        ]);
        buf.set_line(x0, y, &title_line, box_width);
        y += 1;

        // Empty line
        let empty = format!(
            "\u{2502}{}\u{2502}",
            " ".repeat(box_width as usize - 2)
        );
        buf.set_line(
            x0,
            y,
            &Line::from(Span::styled(empty.clone(), border)),
            box_width,
        );
        y += 1;

        // Model line
        let model_label = "Model: ";
        let model_val = &self.state.model_name;
        let model_rest =
            inner_w.saturating_sub(model_label.len() + model_val.len());
        let model_line = Line::from(vec![
            Span::styled("\u{2502}  ", border),
            Span::styled(model_label, label_style),
            Span::styled(model_val.to_string(), value_style),
            Span::raw(" ".repeat(model_rest)),
            Span::styled(" \u{2502}", border),
        ]);
        buf.set_line(x0, y, &model_line, box_width);
        y += 1;

        // Instruction line
        let instr = "Type your message or /help";
        let instr_pad = inner_w.saturating_sub(instr.len());
        let instr_line = Line::from(vec![
            Span::styled("\u{2502}  ", border),
            Span::styled(instr, value_style),
            Span::raw(" ".repeat(instr_pad)),
            Span::styled(" \u{2502}", border),
        ]);
        buf.set_line(x0, y, &instr_line, box_width);
        y += 1;

        // Empty line
        buf.set_line(
            x0,
            y,
            &Line::from(Span::styled(empty.clone(), border)),
            box_width,
        );
        y += 1;

        // Tips header
        let tips_header = "Tips:";
        let tips_pad = inner_w.saturating_sub(tips_header.len());
        let tips_line = Line::from(vec![
            Span::styled("\u{2502}  ", border),
            Span::styled(
                tips_header,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(tips_pad)),
            Span::styled(" \u{2502}", border),
        ]);
        buf.set_line(x0, y, &tips_line, box_width);
        y += 1;

        // Tip entries
        let tips: &[(&str, &str)] = &[
            ("/help", "show available commands"),
            ("/model", "change model"),
            ("Ctrl+C", "cancel current request"),
            ("Ctrl+D", "quit"),
        ];

        // We can fit the first few tips
        let max_tips = (y0 + box_height - 1).saturating_sub(y) as usize;
        for (key, desc) in tips.iter().take(max_tips) {
            let entry = format!("{} \u{2014} {}", key, desc);
            let entry_pad = inner_w.saturating_sub(entry.len() + 2);
            let tip_line = Line::from(vec![
                Span::styled("\u{2502}  ", border),
                Span::styled("\u{2022} ", tip_bullet),
                Span::styled(key.to_string(), tip_key),
                Span::styled(format!(" \u{2014} {}", desc), tip_text),
                Span::raw(" ".repeat(entry_pad)),
                Span::styled(" \u{2502}", border),
            ]);
            buf.set_line(x0, y, &tip_line, box_width);
            y += 1;
        }

        // Fill remaining lines before bottom border
        while y < y0 + box_height - 1 {
            buf.set_line(
                x0,
                y,
                &Line::from(Span::styled(empty.clone(), border)),
                box_width,
            );
            y += 1;
        }

        // Bottom border: ╰──...──╯
        let bottom = format!(
            "\u{2570}{}\u{256F}",
            "\u{2500}".repeat(box_width as usize - 2)
        );
        buf.set_line(x0, y, &Line::from(Span::styled(bottom, border)), box_width);
    }
}
