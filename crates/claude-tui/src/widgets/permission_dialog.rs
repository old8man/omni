//! Permission dialog widget for tool use approval.
//!
//! Renders a bordered popup centered on screen with tool name, description,
//! syntax-highlighted JSON input preview, and action buttons.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

/// Permission dialog state.
pub struct PermissionDialog {
    pub tool_name: String,
    pub description: String,
    pub input_preview: String,
    pub selected_button: usize, // 0=Allow, 1=Deny, 2=Always
    /// Scroll offset into the input preview (for long JSON).
    pub scroll_offset: u16,
    /// Total number of lines in the pretty-printed input.
    input_lines: Vec<String>,
}

impl PermissionDialog {
    pub fn new(tool_name: String, description: String, input_preview: String) -> Self {
        // Pretty-print JSON if valid, otherwise use as-is
        let pretty = if let Ok(val) = serde_json::from_str::<serde_json::Value>(&input_preview) {
            serde_json::to_string_pretty(&val).unwrap_or(input_preview)
        } else {
            input_preview
        };
        let input_lines: Vec<String> = pretty.lines().map(|l| l.to_string()).collect();
        Self {
            tool_name,
            description,
            input_preview: pretty,
            selected_button: 0,
            scroll_offset: 0,
            input_lines,
        }
    }

    pub fn next_button(&mut self) {
        self.selected_button = (self.selected_button + 1) % 3;
    }

    pub fn prev_button(&mut self) {
        self.selected_button = (self.selected_button + 2) % 3;
    }

    pub fn selected(&self) -> &str {
        match self.selected_button {
            0 => "allow",
            1 => "deny",
            2 => "always",
            _ => "allow",
        }
    }

    /// Scroll the input preview down by one line.
    pub fn scroll_down(&mut self) {
        let max = self.input_lines.len().saturating_sub(1) as u16;
        if self.scroll_offset < max {
            self.scroll_offset += 1;
        }
    }

    /// Scroll the input preview up by one line.
    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }
}

/// Color a JSON token for syntax highlighting.
fn json_highlight_line(line: &str) -> Line<'_> {
    let mut spans = Vec::new();
    let mut chars = line.chars().peekable();
    let mut buf = String::new();

    while let Some(&ch) = chars.peek() {
        match ch {
            '"' => {
                // Flush preceding buffer
                if !buf.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut buf),
                        Style::default().fg(Color::White),
                    ));
                }
                // Consume the quoted string
                let mut s = String::new();
                s.push(chars.next().unwrap()); // opening "
                let mut escaped = false;
                for c in chars.by_ref() {
                    s.push(c);
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        break;
                    }
                }
                // Determine if this is a key or a value by checking what comes after
                // Keys are followed by `: ` (after optional whitespace)
                let remaining: String = chars.clone().collect();
                let trimmed = remaining.trim_start();
                let color = if trimmed.starts_with(':') {
                    Color::Cyan // key
                } else {
                    Color::Green // string value
                };
                spans.push(Span::styled(s, Style::default().fg(color)));
            }
            '0'..='9' | '-' if buf.is_empty() || buf.ends_with(|c: char| !c.is_alphanumeric()) =>
            {
                if !buf.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut buf),
                        Style::default().fg(Color::White),
                    ));
                }
                let mut num = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' {
                        num.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
                spans.push(Span::styled(num, Style::default().fg(Color::Yellow)));
            }
            '{' | '}' | '[' | ']' => {
                if !buf.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut buf),
                        Style::default().fg(Color::White),
                    ));
                }
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ));
                chars.next();
            }
            _ => {
                buf.push(chars.next().unwrap());
            }
        }
    }

    if !buf.is_empty() {
        // Check for literal keywords
        let trimmed = buf.trim();
        let color = match trimmed {
            "true" | "false" => Color::Yellow,
            "null" => Color::Red,
            _ => Color::White,
        };
        spans.push(Span::styled(buf, Style::default().fg(color)));
    }

    Line::from(spans)
}

impl Widget for &PermissionDialog {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear background
        Clear.render(area, buf);

        let border_style = Style::default().fg(Color::Yellow);
        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(" ", border_style),
                Span::styled(
                    &self.tool_name,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", border_style),
            ]))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 6 || inner.width < 10 {
            return;
        }

        let content_width = inner.width.saturating_sub(2) as usize;
        let mut y = inner.y;

        // ── Description ─────────────────────────────────────────────────
        let desc_text = if self.description.len() > content_width {
            format!("{}...", &self.description[..content_width.saturating_sub(3)])
        } else {
            self.description.clone()
        };
        let desc_line = Line::from(Span::styled(
            desc_text,
            Style::default().fg(Color::White),
        ));
        buf.set_line(inner.x + 1, y, &desc_line, inner.width.saturating_sub(2));
        y += 1;

        // Separator
        let sep = "\u{2500}"
            .repeat(content_width)
            .chars()
            .take(content_width)
            .collect::<String>();
        let sep_line = Line::from(Span::styled(sep, Style::default().fg(Color::DarkGray)));
        buf.set_line(inner.x + 1, y, &sep_line, inner.width.saturating_sub(2));
        y += 1;

        // ── JSON input preview (scrollable) ─────────────────────────────
        // Reserve 3 rows at bottom: separator + buttons + hint
        let json_height = inner.height.saturating_sub((y - inner.y) + 3) as usize;

        let start = self.scroll_offset as usize;
        let visible_lines = &self.input_lines
            [start..self.input_lines.len().min(start + json_height)];

        for line_text in visible_lines {
            if y >= inner.y + inner.height.saturating_sub(3) {
                break;
            }
            let truncated = if line_text.len() > content_width {
                format!("{}...", &line_text[..content_width.saturating_sub(3)])
            } else {
                line_text.clone()
            };
            let highlighted = json_highlight_line(&truncated);
            buf.set_line(inner.x + 1, y, &highlighted, inner.width.saturating_sub(2));
            y += 1;
        }

        // Scroll indicator
        if self.input_lines.len() > json_height {
            let indicator = format!(
                " [{}/{}] ",
                start + 1,
                self.input_lines.len()
            );
            let ind_line = Line::from(Span::styled(
                indicator,
                Style::default().fg(Color::DarkGray),
            ));
            // Place at right side of current y
            let ind_x = inner.x + inner.width.saturating_sub(ind_line.width() as u16 + 1);
            buf.set_line(ind_x, y.min(inner.y + inner.height - 3), &ind_line, 20);
        }

        // ── Separator before buttons ────────────────────────────────────
        let button_sep_y = inner.y + inner.height - 3;
        let sep2 = "\u{2500}".repeat(content_width);
        let sep2_line = Line::from(Span::styled(sep2, Style::default().fg(Color::DarkGray)));
        buf.set_line(
            inner.x + 1,
            button_sep_y,
            &sep2_line,
            inner.width.saturating_sub(2),
        );

        // ── Action buttons ──────────────────────────────────────────────
        let button_y = inner.y + inner.height - 2;
        let buttons = [
            ("[A]llow", Color::Green),
            ("[D]eny", Color::Red),
            ("[R]emember", Color::Blue),
        ];
        let mut x = inner.x + 2;
        for (i, (label, color)) in buttons.iter().enumerate() {
            let style = if i == self.selected_button {
                Style::default()
                    .fg(Color::Black)
                    .bg(*color)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(*color)
            };
            let text = format!(" {} ", label);
            let span = Span::styled(text, style);
            let w = span.width() as u16;
            buf.set_span(x, button_y, &span, w);
            x += w + 2;
        }

        // ── Hint line ───────────────────────────────────────────────────
        let hint_y = inner.y + inner.height - 1;
        let hint = Line::from(Span::styled(
            " Tab: switch  Enter: confirm  \u{2191}\u{2193}: scroll ",
            Style::default().fg(Color::DarkGray),
        ));
        buf.set_line(inner.x + 1, hint_y, &hint, inner.width.saturating_sub(2));
    }
}
