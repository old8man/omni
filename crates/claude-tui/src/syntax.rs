//! Syntax highlighting for code blocks using `syntect`.
//!
//! Provides language-aware highlighting for fenced code blocks.  Converts
//! syntect's styled tokens into ratatui [`Span`]s so they integrate directly
//! with the TUI rendering pipeline.
//!
//! Supported languages (at minimum): Rust, Python, JavaScript, TypeScript, Go,
//! Bash/Shell, JSON, YAML, TOML, HTML, CSS, SQL, C, C++, Java, Ruby.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::highlighting::{
    FontStyle, HighlightState, Theme, ThemeSet,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use std::sync::OnceLock;

/// Global syntax set — loaded once, reused across all highlight calls.
static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
/// Global dark theme.
static THEME: OnceLock<Theme> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        let ts = ThemeSet::load_defaults();
        ts.themes
            .get("base16-ocean.dark")
            .cloned()
            .unwrap_or_else(|| {
                ts.themes.values().next().cloned().unwrap_or_default()
            })
    })
}

/// Map a fenced code block language tag to a syntect [`SyntaxReference`].
///
/// Handles common aliases: `js` -> JavaScript, `ts` -> TypeScript,
/// `sh` -> Shell, `py` -> Python, `yml` -> YAML, `rb` -> Ruby, etc.
fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let ss = syntax_set();
    let lang_lower = lang.to_lowercase();

    // Try direct lookup first
    if let Some(syn) = ss.find_syntax_by_token(&lang_lower) {
        return Some(syn);
    }

    // Common aliases
    let mapped = match lang_lower.as_str() {
        "js" | "jsx" => "JavaScript",
        "ts" | "tsx" => "JavaScript",
        "py" => "Python",
        "rb" => "Ruby",
        "sh" | "zsh" | "fish" | "shell" => "Bourne Again Shell (bash)",
        "bash" => "Bourne Again Shell (bash)",
        "yml" => "YAML",
        "md" | "markdown" => "Markdown",
        "rs" => "Rust",
        "cpp" | "c++" | "cxx" | "cc" | "hpp" => "C++",
        "h" => "C",
        "cs" | "csharp" => "C#",
        "dockerfile" => "Dockerfile",
        "makefile" | "make" => "Makefile",
        "tf" | "hcl" => "HCL",
        "proto" | "protobuf" => "Protocol Buffers",
        _ => return None,
    };

    ss.find_syntax_by_name(mapped)
        .or_else(|| ss.find_syntax_by_token(mapped))
}

/// Convert a syntect RGBA color to a ratatui [`Color`].
fn syn_color_to_ratatui(c: syntect::highlighting::Color) -> Color {
    // Skip near-black / transparent backgrounds that are just the theme default
    if c.a == 0 {
        return Color::Reset;
    }
    Color::Rgb(c.r, c.g, c.b)
}

/// Convert syntect [`FontStyle`] flags to ratatui [`Modifier`].
fn syn_font_to_modifier(fs: FontStyle) -> Modifier {
    let mut m = Modifier::empty();
    if fs.contains(FontStyle::BOLD) {
        m |= Modifier::BOLD;
    }
    if fs.contains(FontStyle::ITALIC) {
        m |= Modifier::ITALIC;
    }
    if fs.contains(FontStyle::UNDERLINE) {
        m |= Modifier::UNDERLINED;
    }
    m
}

/// Highlight a single line of code and return a vector of styled [`Span`]s.
fn highlight_line_spans(
    line: &str,
    _syntax: &SyntaxReference,
    state: &mut syntect::parsing::ParseState,
    highlighter: &syntect::highlighting::Highlighter<'_>,
) -> Vec<Span<'static>> {
    let ss = syntax_set();
    let ops = state.parse_line(line, ss).unwrap_or_default();
    let mut highlight_state =
        HighlightState::new(highlighter, syntect::parsing::ScopeStack::new());
    let styled = syntect::highlighting::HighlightIterator::new(
        &mut highlight_state,
        &ops,
        line,
        highlighter,
    );

    let mut spans = Vec::new();
    for (style, text) in styled {
        let fg = syn_color_to_ratatui(style.foreground);
        let modifier = syn_font_to_modifier(style.font_style);
        let ratatui_style = Style::default().fg(fg).add_modifier(modifier);
        spans.push(Span::styled(text.to_string(), ratatui_style));
    }
    spans
}

/// Highlight a code block and return styled [`Line`]s.
///
/// If the language is not recognized or highlighting fails, falls back to
/// a plain green style (matching the non-highlighted code block style).
///
/// # Arguments
/// * `lang` - The language tag from the fenced code block (e.g. "rust", "py").
/// * `code` - The raw code content (newline-separated lines).
///
/// # Returns
/// A vector of styled [`Line`]s ready for rendering.
pub fn highlight_code_block(lang: &str, code: &str) -> Vec<Line<'static>> {
    let syntax = match find_syntax(lang) {
        Some(s) => s,
        None => {
            // Fallback: plain green
            return code
                .lines()
                .map(|l| {
                    Line::from(Span::styled(
                        l.to_string(),
                        Style::default().fg(Color::Green),
                    ))
                })
                .collect();
        }
    };

    let highlighter = syntect::highlighting::Highlighter::new(theme());
    let mut parse_state = syntect::parsing::ParseState::new(syntax);
    let mut lines = Vec::new();

    for line in code.lines() {
        let spans = highlight_line_spans(line, syntax, &mut parse_state, &highlighter);
        if spans.is_empty() {
            lines.push(Line::from(Span::raw(String::new())));
        } else {
            lines.push(Line::from(spans));
        }
    }

    lines
}

/// Check whether a language tag is recognized for highlighting.
pub fn is_supported_language(lang: &str) -> bool {
    find_syntax(lang).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_rust() {
        let code = "fn main() {\n    println!(\"hello\");\n}";
        let lines = highlight_code_block("rust", code);
        assert_eq!(lines.len(), 3);
        // Each line should have at least one span
        for line in &lines {
            assert!(!line.spans.is_empty());
        }
    }

    #[test]
    fn test_highlight_python() {
        let code = "def hello():\n    print('hello')";
        let lines = highlight_code_block("python", code);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_highlight_unknown_language() {
        let code = "some unknown code\nline two";
        let lines = highlight_code_block("xyzlang_unknown", code);
        assert_eq!(lines.len(), 2);
        // Should be plain green fallback
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn test_aliases() {
        assert!(is_supported_language("js"));
        assert!(is_supported_language("ts"));
        assert!(is_supported_language("py"));
        assert!(is_supported_language("sh"));
        assert!(is_supported_language("bash"));
        assert!(is_supported_language("rb"));
        assert!(is_supported_language("yml"));
        assert!(is_supported_language("rs"));
        assert!(is_supported_language("cpp"));
    }

    #[test]
    fn test_highlight_json() {
        let code = "{\n  \"key\": \"value\",\n  \"num\": 42\n}";
        let lines = highlight_code_block("json", code);
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn test_empty_code() {
        let lines = highlight_code_block("rust", "");
        // Empty string has no lines from .lines()
        assert!(lines.is_empty());
    }

    #[test]
    fn test_highlight_bash() {
        let code = "#!/bin/bash\necho \"hello $USER\"\nls -la | grep foo";
        let lines = highlight_code_block("bash", code);
        assert_eq!(lines.len(), 3);
    }
}
