//! Markdown to ratatui styled text converter.
//!
//! Full GFM-compatible rendering including:
//! - `**bold**`, `*italic*`, `` `inline code` ``, `~~strikethrough~~`
//! - Fenced code blocks with language-aware syntax highlighting (15+ languages)
//! - `# H1` through `###### H6` headers with visual hierarchy
//! - `- unordered` and `1. ordered` lists with nesting
//! - `> blockquotes` with nesting
//! - `[links](url)` with OSC 8 hyperlink sequences
//! - Pipe tables with column alignment (`:---`, `:---:`, `---:`)
//! - Horizontal rules (`---`, `***`, `___`)
//! - Image placeholders: `![alt](url)` -> `[image: alt]`
//! - HTML entity decoding (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#123;`, `&#x1F;`)
//! - Diff display: colored `+`/`-` lines in code blocks with `diff` language
//! - ANSI escape passthrough in code blocks

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::syntax::highlight_code_block;

/// Convert a markdown string to styled ratatui Lines.
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();
    let raw_lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < raw_lines.len() {
        let line = raw_lines[i];

        // Fenced code blocks
        if line.trim_start().starts_with("```") {
            if in_code_block {
                // End of code block — render accumulated code
                render_code_block_lines(&code_lang, &code_buffer, &mut lines);
                lines.push(Line::from(Span::styled(
                    "\u{2500}".repeat(40),
                    Style::default().fg(Color::DarkGray),
                )));
                code_buffer.clear();
                code_lang.clear();
                in_code_block = false;
            } else {
                // Start of code block
                let trimmed = line.trim_start();
                code_lang = trimmed[3..].trim().to_string();
                in_code_block = true;
                let mut header_spans = vec![Span::styled(
                    "\u{2500}".repeat(40),
                    Style::default().fg(Color::DarkGray),
                )];
                if !code_lang.is_empty() {
                    header_spans.push(Span::styled(
                        format!(" {} ", code_lang),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ));
                }
                lines.push(Line::from(header_spans));
            }
            i += 1;
            continue;
        }

        if in_code_block {
            if !code_buffer.is_empty() {
                code_buffer.push('\n');
            }
            code_buffer.push_str(line);
            i += 1;
            continue;
        }

        // Horizontal rule
        if is_horizontal_rule(line) {
            lines.push(Line::from(Span::styled(
                "\u{2500}".repeat(40),
                Style::default().fg(Color::DarkGray),
            )));
            i += 1;
            continue;
        }

        // Pipe table: detect by checking if this line and the next look like table rows
        if is_table_row(line) && i + 1 < raw_lines.len() && is_table_separator(raw_lines[i + 1]) {
            let separator_line = raw_lines[i + 1];
            let alignments = parse_table_alignments(separator_line);

            let mut table_rows = Vec::new();
            while i < raw_lines.len()
                && (is_table_row(raw_lines[i]) || is_table_separator(raw_lines[i]))
            {
                if !is_table_separator(raw_lines[i]) {
                    table_rows.push(raw_lines[i]);
                }
                i += 1;
            }
            render_table(&table_rows, &alignments, &mut lines);
            continue;
        }

        // Headers H1-H6
        if let Some(header) = parse_header(line) {
            lines.push(render_header_line(header.level, header.text));
            i += 1;
            continue;
        }

        // Nested blockquote
        if line.starts_with('>') {
            let (depth, content) = parse_blockquote(line);
            let mut spans: Vec<Span<'static>> = Vec::new();
            for _ in 0..depth {
                spans.push(Span::styled(
                    "\u{2502} ",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            spans.extend(render_inline_spans(content));
            lines.push(Line::from(spans));
            i += 1;
            continue;
        }

        // Task lists (- [ ] / - [x])
        if let Some((indent_level, checked, rest)) = parse_task_list(line) {
            let indent = "  ".repeat(indent_level);
            let (checkbox, color) = if checked {
                ("☑ ", Color::Green)
            } else {
                ("☐ ", Color::DarkGray)
            };
            let mut spans = vec![
                Span::styled(
                    format!("{}", indent),
                    Style::default(),
                ),
                Span::styled(
                    checkbox.to_string(),
                    Style::default().fg(color),
                ),
            ];
            spans.extend(render_inline_spans(rest));
            lines.push(Line::from(spans));
            i += 1;
            continue;
        }

        // Nested unordered list items
        if let Some((indent_level, rest)) = parse_unordered_list(line) {
            let indent = "  ".repeat(indent_level);
            let mut spans = vec![Span::styled(
                format!("{}\u{00b7} ", indent),
                Style::default().fg(Color::DarkGray),
            )];
            spans.extend(render_inline_spans(rest));
            lines.push(Line::from(spans));
            i += 1;
            continue;
        }

        // Nested numbered list items
        if let Some((indent_level, num, rest)) = parse_numbered_list(line) {
            let indent = "  ".repeat(indent_level);
            let mut spans = vec![Span::styled(
                format!("{}{}. ", indent, num),
                Style::default().fg(Color::DarkGray),
            )];
            spans.extend(render_inline_spans(rest));
            lines.push(Line::from(spans));
            i += 1;
            continue;
        }

        // Regular text with inline formatting
        lines.push(Line::from(render_inline_spans(line)));
        i += 1;
    }

    // Handle unclosed code block
    if in_code_block && !code_buffer.is_empty() {
        render_code_block_lines(&code_lang, &code_buffer, &mut lines);
    }

    lines
}

/// Render accumulated code block lines with syntax highlighting or diff coloring.
fn render_code_block_lines(lang: &str, code: &str, lines: &mut Vec<Line<'static>>) {
    if lang == "diff" {
        // Diff display: colored +/- lines
        for code_line in code.lines() {
            let style = if code_line.starts_with('+') {
                Style::default().fg(Color::Green)
            } else if code_line.starts_with('-') {
                Style::default().fg(Color::Red)
            } else if code_line.starts_with("@@") {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::from(Span::styled(
                format!("  {}", code_line),
                style,
            )));
        }
    } else if !lang.is_empty() {
        // Language-aware syntax highlighting
        let highlighted = highlight_code_block(lang, code);
        for hl_line in highlighted {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(
                hl_line
                    .spans
                    .into_iter()
                    .map(|s| Span::styled(s.content.to_string(), s.style)),
            );
            lines.push(Line::from(spans));
        }
    } else {
        // Plain code (no language specified)
        for code_line in code.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {}", code_line),
                Style::default().fg(Color::Green),
            )));
        }
    }
}

/// Header metadata parsed from a markdown line.
struct HeaderInfo<'a> {
    level: u8,
    text: &'a str,
}

/// Parse a header line, returning level (1-6) and text.
fn parse_header(line: &str) -> Option<HeaderInfo<'_>> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let mut level = 0u8;
    for ch in trimmed.chars() {
        if ch == '#' {
            level += 1;
        } else {
            break;
        }
    }
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level as usize..];
    if !rest.starts_with(' ') && !rest.is_empty() {
        return None;
    }
    let text = rest.trim_start();
    Some(HeaderInfo { level, text })
}

/// Render a header line with visual hierarchy.
fn render_header_line(level: u8, text: &str) -> Line<'static> {
    let style = match level {
        1 => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        3 => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        4 => Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
        5 => Style::default().fg(Color::Blue),
        6 => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
        _ => Style::default(),
    };
    Line::from(Span::styled(text.to_string(), style))
}

/// Parse a blockquote line, returning (nesting depth, remaining content).
fn parse_blockquote(line: &str) -> (usize, &str) {
    let mut depth = 0;
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start();
        if let Some(stripped) = trimmed.strip_prefix("> ") {
            depth += 1;
            rest = stripped;
        } else if let Some(stripped) = trimmed.strip_prefix('>') {
            depth += 1;
            rest = stripped;
        } else {
            break;
        }
    }
    (depth, rest)
}

/// Parse an unordered list item, returning (indent level, remaining text).
fn parse_unordered_list(line: &str) -> Option<(usize, &str)> {
    let stripped = line.trim_end();
    let leading_spaces = stripped.len() - stripped.trim_start().len();
    let trimmed = stripped.trim_start();

    if let Some(rest) = trimmed.strip_prefix("- ") {
        Some((leading_spaces / 2, rest))
    } else if let Some(rest) = trimmed.strip_prefix("* ") {
        Some((leading_spaces / 2, rest))
    } else if let Some(rest) = trimmed.strip_prefix("+ ") {
        Some((leading_spaces / 2, rest))
    } else {
        None
    }
}

/// Parse a task list item (`- [ ]` or `- [x]`), returning (indent level, checked, remaining text).
fn parse_task_list(line: &str) -> Option<(usize, bool, &str)> {
    let stripped = line.trim_end();
    let leading_spaces = stripped.len() - stripped.trim_start().len();
    let trimmed = stripped.trim_start();

    if let Some(rest) = trimmed.strip_prefix("- [x] ").or_else(|| trimmed.strip_prefix("- [X] ")) {
        Some((leading_spaces / 2, true, rest))
    } else if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
        Some((leading_spaces / 2, false, rest))
    } else {
        None
    }
}

/// Parse a numbered list item, returning (indent level, number string, remaining text).
fn parse_numbered_list(line: &str) -> Option<(usize, &str, &str)> {
    let stripped = line.trim_end();
    let leading_spaces = stripped.len() - stripped.trim_start().len();
    let trimmed = stripped.trim_start();

    if let Some(dot_pos) = trimmed.find(". ") {
        let num_part = &trimmed[..dot_pos];
        if !num_part.is_empty() && num_part.chars().all(|c| c.is_ascii_digit()) {
            let indent_level = leading_spaces / 2;
            let rest = &trimmed[dot_pos + 2..];
            return Some((indent_level, num_part, rest));
        }
    }
    None
}

/// Column alignment for table rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Alignment {
    Left,
    Center,
    Right,
}

/// Parse table separator row to extract column alignments.
fn parse_table_alignments(separator: &str) -> Vec<Alignment> {
    separator
        .trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| {
            let trimmed = cell.trim();
            let starts_colon = trimmed.starts_with(':');
            let ends_colon = trimmed.ends_with(':');
            match (starts_colon, ends_colon) {
                (true, true) => Alignment::Center,
                (false, true) => Alignment::Right,
                _ => Alignment::Left,
            }
        })
        .collect()
}

/// Parse inline markdown and return a Vec of styled Spans.
///
/// Handles: `**bold**`, `*italic*`, `` `code` ``, `[text](url)`,
/// `![alt](url)` (image placeholder), `~~strikethrough~~`, and HTML entities.
fn render_inline_spans(text: &str) -> Vec<Span<'static>> {
    // Decode HTML entities first
    let decoded = decode_html_entities(text);
    let text = &decoded;

    let mut spans = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Inline code
        if chars[i] == '`' {
            flush_current(&mut current, &mut spans);
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != '`' {
                i += 1;
            }
            let code: String = chars[start..i].iter().collect();
            spans.push(Span::styled(code, Style::default().fg(Color::Green)));
            if i < chars.len() {
                i += 1;
            }
        }
        // Image: ![alt](url)
        else if chars[i] == '!' && i + 1 < chars.len() && chars[i + 1] == '[' {
            flush_current(&mut current, &mut spans);
            i += 2; // skip ![
            let alt_start = i;
            while i < chars.len() && chars[i] != ']' {
                i += 1;
            }
            let alt_text: String = chars[alt_start..i].iter().collect();
            if i < chars.len()
                && chars[i] == ']'
                && i + 1 < chars.len()
                && chars[i + 1] == '('
            {
                i += 2; // skip ](
                while i < chars.len() && chars[i] != ')' {
                    i += 1;
                }
                if i < chars.len() {
                    i += 1; // skip )
                }
                spans.push(Span::styled(
                    format!("[image: {}]", alt_text),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ));
            } else {
                // Not a valid image — treat as literal
                current.push('!');
                current.push('[');
                current.push_str(&alt_text);
                if i < chars.len() {
                    current.push(chars[i]);
                    i += 1;
                }
            }
        }
        // Link: [text](url) with OSC 8 hyperlink
        else if chars[i] == '[' {
            let link_start = i;
            i += 1;
            let text_start = i;
            let mut depth = 1;
            while i < chars.len() && depth > 0 {
                if chars[i] == '[' {
                    depth += 1;
                } else if chars[i] == ']' {
                    depth -= 1;
                }
                if depth > 0 {
                    i += 1;
                }
            }
            if i < chars.len()
                && chars[i] == ']'
                && i + 1 < chars.len()
                && chars[i + 1] == '('
            {
                let link_text: String = chars[text_start..i].iter().collect();
                i += 2; // skip ](
                let url_start = i;
                while i < chars.len() && chars[i] != ')' {
                    i += 1;
                }
                let url: String = chars[url_start..i].iter().collect();
                if i < chars.len() {
                    i += 1;
                }
                flush_current(&mut current, &mut spans);
                // OSC 8 hyperlink: \e]8;;url\e\\ text \e]8;;\e\\
                let osc_link = format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", url, link_text);
                spans.push(Span::styled(
                    osc_link,
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::UNDERLINED),
                ));
            } else {
                // Not a valid link — treat as literal
                i = link_start;
                current.push(chars[i]);
                i += 1;
            }
        }
        // Strikethrough: ~~text~~
        else if i + 1 < chars.len() && chars[i] == '~' && chars[i + 1] == '~' {
            flush_current(&mut current, &mut spans);
            i += 2;
            let start = i;
            while i + 1 < chars.len() && !(chars[i] == '~' && chars[i + 1] == '~') {
                i += 1;
            }
            let struck: String = chars[start..i].iter().collect();
            spans.push(Span::styled(
                struck,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::CROSSED_OUT),
            ));
            if i + 1 < chars.len() {
                i += 2;
            } else {
                i = chars.len();
            }
        }
        // Bold: **text**
        else if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            flush_current(&mut current, &mut spans);
            i += 2;
            let start = i;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '*') {
                i += 1;
            }
            let bold: String = chars[start..i].iter().collect();
            spans.push(Span::styled(
                bold,
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if i + 1 < chars.len() {
                i += 2;
            } else {
                i = chars.len();
            }
        }
        // Italic: *text*
        else if chars[i] == '*' {
            flush_current(&mut current, &mut spans);
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != '*' {
                i += 1;
            }
            let italic: String = chars[start..i].iter().collect();
            spans.push(Span::styled(
                italic,
                Style::default().add_modifier(Modifier::ITALIC),
            ));
            if i < chars.len() {
                i += 1;
            }
        } else {
            current.push(chars[i]);
            i += 1;
        }
    }

    flush_current(&mut current, &mut spans);
    spans
}

/// Decode common HTML entities in text.
///
/// Handles named entities (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;`,
/// `&nbsp;`, `&mdash;`, `&ndash;`, `&hellip;`, `&copy;`, `&reg;`,
/// `&trade;`), decimal (`&#123;`), and hex (`&#x1F;`) numeric entities.
fn decode_html_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '&' {
            result.push(ch);
            continue;
        }

        // Collect entity text until ';' or give up
        let mut entity = String::new();
        let mut found_semi = false;

        for _ in 0..12 {
            match chars.peek() {
                Some(&';') => {
                    chars.next();
                    found_semi = true;
                    break;
                }
                Some(&c) if c.is_alphanumeric() || c == '#' => {
                    entity.push(c);
                    chars.next();
                }
                _ => break,
            }
        }

        if !found_semi {
            // Not a valid entity — output literally
            result.push('&');
            result.push_str(&entity);
            continue;
        }

        // Named entities
        let decoded = match entity.as_str() {
            "amp" => "&",
            "lt" => "<",
            "gt" => ">",
            "quot" => "\"",
            "apos" => "'",
            "nbsp" => "\u{00A0}",
            "mdash" => "\u{2014}",
            "ndash" => "\u{2013}",
            "hellip" => "\u{2026}",
            "copy" => "\u{00A9}",
            "reg" => "\u{00AE}",
            "trade" => "\u{2122}",
            "laquo" => "\u{00AB}",
            "raquo" => "\u{00BB}",
            "bull" => "\u{2022}",
            "middot" => "\u{00B7}",
            "times" => "\u{00D7}",
            "divide" => "\u{00F7}",
            _ => {
                // Numeric entity
                if let Some(num_str) = entity.strip_prefix('#') {
                    let code_point = if let Some(hex_str) = num_str
                        .strip_prefix('x')
                        .or_else(|| num_str.strip_prefix('X'))
                    {
                        u32::from_str_radix(hex_str, 16).ok()
                    } else {
                        num_str.parse::<u32>().ok()
                    };
                    if let Some(cp) = code_point {
                        if let Some(c) = char::from_u32(cp) {
                            result.push(c);
                            continue;
                        }
                    }
                }
                // Unknown entity — output literally
                result.push('&');
                result.push_str(&entity);
                result.push(';');
                continue;
            }
        };

        result.push_str(decoded);
    }

    result
}

/// Flush accumulated plain text into the spans vec.
fn flush_current(current: &mut String, spans: &mut Vec<Span<'static>>) {
    if !current.is_empty() {
        spans.push(Span::raw(std::mem::take(current)));
    }
}

/// Check if a line is a horizontal rule (`---`, `***`, `___`).
fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let first = trimmed.chars().next().unwrap_or(' ');
    matches!(first, '-' | '*' | '_') && trimmed.chars().all(|c| c == first || c == ' ')
}

/// Check if a line looks like a pipe table row (starts and/or contains `|`).
fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && !trimmed.starts_with("```")
}

/// Check if a line is a table separator row (e.g. `|---|---|`).
fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|')
        && trimmed
            .chars()
            .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
}

/// Render pipe table rows as styled lines with alignment support.
fn render_table(rows: &[&str], alignments: &[Alignment], lines: &mut Vec<Line<'static>>) {
    if rows.is_empty() {
        return;
    }

    // Parse cells from each row
    let parsed: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect()
        })
        .collect();

    // Compute column widths
    let col_count = parsed.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; col_count];
    for row in &parsed {
        for (j, cell) in row.iter().enumerate() {
            if j < widths.len() {
                widths[j] = widths[j].max(cell.len());
            }
        }
    }

    // Render header row (first row) in bold
    if let Some(header) = parsed.first() {
        let mut spans = Vec::new();
        spans.push(Span::styled("  ", Style::default()));
        for (j, cell) in header.iter().enumerate() {
            let w = widths.get(j).copied().unwrap_or(cell.len());
            let align = alignments.get(j).copied().unwrap_or(Alignment::Left);
            let formatted = align_text(cell, w, align);
            spans.push(Span::styled(
                formatted,
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if j + 1 < header.len() {
                spans.push(Span::styled(
                    " \u{2502} ",
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
        lines.push(Line::from(spans));

        // Separator
        let sep_width: usize = widths.iter().sum::<usize>() + (col_count.saturating_sub(1)) * 3;
        lines.push(Line::from(Span::styled(
            format!("  {}", "\u{2500}".repeat(sep_width)),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Render data rows
    for row in parsed.iter().skip(1) {
        let mut spans = Vec::new();
        spans.push(Span::styled("  ", Style::default()));
        for (j, cell) in row.iter().enumerate() {
            let w = widths.get(j).copied().unwrap_or(cell.len());
            let align = alignments.get(j).copied().unwrap_or(Alignment::Left);
            let formatted = align_text(cell, w, align);
            spans.push(Span::raw(formatted));
            if j + 1 < row.len() {
                spans.push(Span::styled(
                    " \u{2502} ",
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
        lines.push(Line::from(spans));
    }
}

/// Align text within a given width according to alignment spec.
fn align_text(text: &str, width: usize, align: Alignment) -> String {
    let text_len = text.len();
    if text_len >= width {
        return text.to_string();
    }
    let padding = width - text_len;
    match align {
        Alignment::Left => format!("{}{}", text, " ".repeat(padding)),
        Alignment::Right => format!("{}{}", " ".repeat(padding), text),
        Alignment::Center => {
            let left = padding / 2;
            let right = padding - left;
            format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_header_h1() {
        let lines = render_markdown("# Hello");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_render_header_h4() {
        let lines = render_markdown("#### Level 4");
        assert_eq!(lines.len(), 1);
        let span = &lines[0].spans[0];
        assert_eq!(span.style.fg, Some(Color::Blue));
    }

    #[test]
    fn test_render_header_h6() {
        let lines = render_markdown("###### Level 6");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_render_code_block_with_lang() {
        let lines = render_markdown("```rust\nfn main() {}\n```");
        // open separator (with lang label), highlighted code line, close separator
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_render_diff_block() {
        let md = "```diff\n+ added line\n- removed line\n unchanged\n```";
        let lines = render_markdown(md);
        // separator, 3 diff lines, separator
        assert_eq!(lines.len(), 5);
        // The added line should be green
        let added_span = &lines[1].spans[0];
        assert_eq!(added_span.style.fg, Some(Color::Green));
    }

    #[test]
    fn test_render_list() {
        let lines = render_markdown("- item one\n- item two");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_render_nested_list() {
        let lines = render_markdown("- item\n  - nested\n    - deep");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_render_numbered_list() {
        let lines = render_markdown("1. first\n2. second");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_render_horizontal_rule() {
        let lines = render_markdown("---");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_render_inline_bold() {
        let spans = render_inline_spans("hello **world**");
        assert!(spans.len() >= 2);
    }

    #[test]
    fn test_render_inline_link() {
        let spans = render_inline_spans("click [here](http://example.com) now");
        assert!(spans.len() >= 3);
        let has_underlined = spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(has_underlined);
    }

    #[test]
    fn test_render_inline_strikethrough() {
        let spans = render_inline_spans("~~removed~~");
        assert!(!spans.is_empty());
    }

    #[test]
    fn test_render_image_placeholder() {
        let spans = render_inline_spans("![logo](http://example.com/logo.png)");
        assert_eq!(spans.len(), 1);
        assert!(spans[0].content.contains("[image: logo]"));
    }

    #[test]
    fn test_render_table() {
        let md = "| Name | Age |\n|------|-----|\n| Alice | 30 |\n| Bob | 25 |";
        let lines = render_markdown(md);
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn test_render_table_alignment() {
        let md = "| Left | Center | Right |\n|:-----|:------:|------:|\n| a | b | c |";
        let lines = render_markdown(md);
        assert_eq!(lines.len(), 3); // header + separator + 1 data row
    }

    #[test]
    fn test_render_blockquote() {
        let lines = render_markdown("> quoted text");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_render_nested_blockquote() {
        let lines = render_markdown("> > nested");
        assert_eq!(lines.len(), 1);
        let pipe_count = lines[0]
            .spans
            .iter()
            .filter(|s| s.content.contains('\u{2502}'))
            .count();
        assert_eq!(pipe_count, 2);
    }

    #[test]
    fn test_html_entity_decoding() {
        assert_eq!(decode_html_entities("&amp;"), "&");
        assert_eq!(decode_html_entities("&lt;div&gt;"), "<div>");
        assert_eq!(decode_html_entities("&#65;"), "A");
        assert_eq!(decode_html_entities("&#x41;"), "A");
        assert_eq!(decode_html_entities("no entities"), "no entities");
    }

    #[test]
    fn test_is_horizontal_rule() {
        assert!(is_horizontal_rule("---"));
        assert!(is_horizontal_rule("***"));
        assert!(is_horizontal_rule("___"));
        assert!(is_horizontal_rule("- - -"));
        assert!(!is_horizontal_rule("--"));
        assert!(!is_horizontal_rule("hello"));
    }

    #[test]
    fn test_is_table_separator() {
        assert!(is_table_separator("|---|---|"));
        assert!(is_table_separator("| --- | --- |"));
        assert!(is_table_separator("|:---:|---:|"));
        assert!(!is_table_separator("| hello | world |"));
    }

    #[test]
    fn test_parse_table_alignments() {
        let aligns = parse_table_alignments("|:---|:---:|---:|");
        assert_eq!(
            aligns,
            vec![Alignment::Left, Alignment::Center, Alignment::Right]
        );
    }

    #[test]
    fn test_code_block_plain() {
        let lines = render_markdown("```\nplain code\n```");
        // separator, code line (green), separator
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_task_list_unchecked() {
        let lines = render_markdown("- [ ] todo item");
        assert_eq!(lines.len(), 1);
        let has_checkbox = lines[0].spans.iter().any(|s| s.content.contains('☐'));
        assert!(has_checkbox);
    }

    #[test]
    fn test_task_list_checked() {
        let lines = render_markdown("- [x] done item");
        assert_eq!(lines.len(), 1);
        let has_checkbox = lines[0].spans.iter().any(|s| s.content.contains('☑'));
        assert!(has_checkbox);
    }

    #[test]
    fn test_task_list_mixed() {
        let lines = render_markdown("- [x] done\n- [ ] todo\n- [X] also done");
        assert_eq!(lines.len(), 3);
    }
}
