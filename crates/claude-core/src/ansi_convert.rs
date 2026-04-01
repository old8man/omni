//! ANSI escape code conversion utilities.
//!
//! Provides:
//! - `ansi_to_svg`: Parse ANSI-escaped terminal text and render as SVG
//! - `ansi_to_png_via_command`: Shell out to `rsvg-convert` or `convert` for PNG
//! - `save_svg_to_file`: Write SVG output to a file
//! - `Asciicast`: asciicast v2 format recording (.cast files, NDJSON)

use std::fmt::Write as FmtWrite;
use std::io::Write;
use std::path::Path;

// ---------------------------------------------------------------------------
// Color types
// ---------------------------------------------------------------------------

/// An RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnsiColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl AnsiColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Format as `rgb(r, g, b)` for SVG.
    pub fn to_rgb_string(&self) -> String {
        format!("rgb({}, {}, {})", self.r, self.g, self.b)
    }
}

/// Default foreground color (light gray, matching typical terminal defaults).
pub const DEFAULT_FG: AnsiColor = AnsiColor::new(229, 229, 229);

/// Default background color (dark gray).
pub const DEFAULT_BG: AnsiColor = AnsiColor::new(30, 30, 30);

// ---------------------------------------------------------------------------
// Standard ANSI color palette
// ---------------------------------------------------------------------------

/// Standard 16-color ANSI palette, indexed by SGR code (30-37, 90-97).
fn ansi_color_by_sgr(code: u8) -> Option<AnsiColor> {
    match code {
        30 => Some(AnsiColor::new(0, 0, 0)),
        31 => Some(AnsiColor::new(205, 49, 49)),
        32 => Some(AnsiColor::new(13, 188, 121)),
        33 => Some(AnsiColor::new(229, 229, 16)),
        34 => Some(AnsiColor::new(36, 114, 200)),
        35 => Some(AnsiColor::new(188, 63, 188)),
        36 => Some(AnsiColor::new(17, 168, 205)),
        37 => Some(AnsiColor::new(229, 229, 229)),
        90 => Some(AnsiColor::new(102, 102, 102)),
        91 => Some(AnsiColor::new(241, 76, 76)),
        92 => Some(AnsiColor::new(35, 209, 139)),
        93 => Some(AnsiColor::new(245, 245, 67)),
        94 => Some(AnsiColor::new(59, 142, 234)),
        95 => Some(AnsiColor::new(214, 112, 214)),
        96 => Some(AnsiColor::new(41, 184, 219)),
        97 => Some(AnsiColor::new(255, 255, 255)),
        _ => None,
    }
}

/// 256-color palette lookup.
fn get_256_color(index: u8) -> AnsiColor {
    let idx = index as usize;
    if idx < 16 {
        // Standard 16 colors.
        const STANDARD: [AnsiColor; 16] = [
            AnsiColor::new(0, 0, 0),
            AnsiColor::new(128, 0, 0),
            AnsiColor::new(0, 128, 0),
            AnsiColor::new(128, 128, 0),
            AnsiColor::new(0, 0, 128),
            AnsiColor::new(128, 0, 128),
            AnsiColor::new(0, 128, 128),
            AnsiColor::new(192, 192, 192),
            AnsiColor::new(128, 128, 128),
            AnsiColor::new(255, 0, 0),
            AnsiColor::new(0, 255, 0),
            AnsiColor::new(255, 255, 0),
            AnsiColor::new(0, 0, 255),
            AnsiColor::new(255, 0, 255),
            AnsiColor::new(0, 255, 255),
            AnsiColor::new(255, 255, 255),
        ];
        return STANDARD[idx];
    }

    if idx < 232 {
        // 6x6x6 color cube (indices 16-231).
        let i = idx - 16;
        let ri = i / 36;
        let gi = (i % 36) / 6;
        let bi = i % 6;
        let r = if ri == 0 { 0 } else { (55 + ri * 40) as u8 };
        let g = if gi == 0 { 0 } else { (55 + gi * 40) as u8 };
        let b = if bi == 0 { 0 } else { (55 + bi * 40) as u8 };
        return AnsiColor::new(r, g, b);
    }

    // Grayscale (indices 232-255).
    let gray = ((idx - 232) * 10 + 8) as u8;
    AnsiColor::new(gray, gray, gray)
}

// ---------------------------------------------------------------------------
// ANSI parser
// ---------------------------------------------------------------------------

/// A span of text with uniform styling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSpan {
    pub text: String,
    pub fg: AnsiColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// A parsed line is a sequence of styled text spans.
pub type ParsedLine = Vec<TextSpan>;

/// Current SGR (Select Graphic Rendition) state.
#[derive(Debug, Clone)]
struct SgrState {
    fg: AnsiColor,
    bold: bool,
    italic: bool,
    underline: bool,
}

impl Default for SgrState {
    fn default() -> Self {
        Self {
            fg: DEFAULT_FG,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

/// Parse ANSI escape sequences from text, returning styled lines.
///
/// Supports:
/// - Basic colors (SGR 30-37, 90-97)
/// - 256-color mode (38;5;n)
/// - 24-bit true color (38;2;r;g;b)
/// - Bold (1), italic (3), underline (4), reset (0)
/// - Default foreground (39)
pub fn parse_ansi(text: &str) -> Vec<ParsedLine> {
    let mut lines: Vec<ParsedLine> = Vec::new();
    let raw_lines: Vec<&str> = text.split('\n').collect();

    for raw_line in &raw_lines {
        let mut spans: Vec<TextSpan> = Vec::new();
        let mut state = SgrState::default();
        let bytes = raw_line.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            // Check for ESC [ sequence.
            if bytes[i] == 0x1b && i + 1 < len && bytes[i + 1] == b'[' {
                // Find the terminator character (a letter).
                let mut j = i + 2;
                while j < len && !bytes[j].is_ascii_alphabetic() {
                    j += 1;
                }
                if j < len && bytes[j] == b'm' {
                    // SGR sequence.
                    let params_str = &raw_line[i + 2..j];
                    let codes: Vec<u16> = params_str
                        .split(';')
                        .filter_map(|s| s.parse::<u16>().ok())
                        .collect();
                    apply_sgr_codes(&codes, &mut state);
                    i = j + 1;
                    continue;
                } else {
                    // Non-SGR escape (e.g., cursor movement) — skip it.
                    i = if j < len { j + 1 } else { len };
                    continue;
                }
            }

            // Regular character — accumulate same-styled text.
            let text_start = i;
            while i < len && !(bytes[i] == 0x1b && i + 1 < len && bytes[i + 1] == b'[') {
                i += 1;
            }

            let span_text = &raw_line[text_start..i];
            if !span_text.is_empty() {
                spans.push(TextSpan {
                    text: span_text.to_string(),
                    fg: state.fg,
                    bold: state.bold,
                    italic: state.italic,
                    underline: state.underline,
                });
            }
        }

        // Preserve empty lines.
        if spans.is_empty() {
            spans.push(TextSpan {
                text: String::new(),
                fg: DEFAULT_FG,
                bold: false,
                italic: false,
                underline: false,
            });
        }

        lines.push(spans);
    }

    lines
}

fn apply_sgr_codes(codes: &[u16], state: &mut SgrState) {
    let mut k = 0;
    while k < codes.len() {
        let code = codes[k];
        match code {
            0 => {
                // Reset.
                *state = SgrState::default();
            }
            1 => state.bold = true,
            3 => state.italic = true,
            4 => state.underline = true,
            22 => state.bold = false,
            23 => state.italic = false,
            24 => state.underline = false,
            30..=37 | 90..=97 => {
                if let Some(c) = ansi_color_by_sgr(code as u8) {
                    state.fg = c;
                }
            }
            39 => state.fg = DEFAULT_FG,
            38 => {
                // Extended foreground color.
                if k + 1 < codes.len() && codes[k + 1] == 5 && k + 2 < codes.len() {
                    // 256-color mode: 38;5;n
                    state.fg = get_256_color(codes[k + 2] as u8);
                    k += 2;
                } else if k + 1 < codes.len()
                    && codes[k + 1] == 2
                    && k + 4 < codes.len()
                {
                    // True color: 38;2;r;g;b
                    state.fg = AnsiColor::new(
                        codes[k + 2] as u8,
                        codes[k + 3] as u8,
                        codes[k + 4] as u8,
                    );
                    k += 4;
                }
            }
            _ => {} // Ignore unsupported codes.
        }
        k += 1;
    }
}

// ---------------------------------------------------------------------------
// XML escaping
// ---------------------------------------------------------------------------

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// ANSI -> SVG
// ---------------------------------------------------------------------------

/// Options for SVG rendering.
#[derive(Debug, Clone)]
pub struct AnsiToSvgOptions {
    pub font_family: String,
    pub font_size: u32,
    pub line_height: u32,
    pub padding_x: u32,
    pub padding_y: u32,
    pub background_color: String,
    pub border_radius: u32,
}

impl Default for AnsiToSvgOptions {
    fn default() -> Self {
        Self {
            font_family: "Menlo, Monaco, monospace".to_string(),
            font_size: 14,
            line_height: 22,
            padding_x: 24,
            padding_y: 24,
            background_color: DEFAULT_BG.to_rgb_string(),
            border_radius: 8,
        }
    }
}

/// Convert ANSI-escaped terminal text to SVG.
///
/// Parses ANSI escape codes for colors (16-color, 256-color, true color),
/// bold, italic, and underline. Generates SVG with `<text>` elements containing
/// `<tspan>` children for each styled segment.
pub fn ansi_to_svg(input: &str) -> String {
    ansi_to_svg_with_options(input, &AnsiToSvgOptions::default())
}

/// Convert ANSI-escaped terminal text to SVG with custom options.
pub fn ansi_to_svg_with_options(input: &str, options: &AnsiToSvgOptions) -> String {
    let mut lines = parse_ansi(input);

    // Trim trailing empty lines.
    while lines.len() > 1
        && lines
            .last()
            .map(|l| l.iter().all(|s| s.text.trim().is_empty()))
            .unwrap_or(false)
    {
        lines.pop();
    }

    if lines.is_empty() {
        lines.push(vec![TextSpan {
            text: String::new(),
            fg: DEFAULT_FG,
            bold: false,
            italic: false,
            underline: false,
        }]);
    }

    // Estimate width based on max line length (monospace: char width ~ 0.6 * font_size).
    let char_width_estimate = options.font_size as f64 * 0.6;
    let max_line_len = lines
        .iter()
        .map(|spans| spans.iter().map(|s| s.text.len()).sum::<usize>())
        .max()
        .unwrap_or(0);
    let width = (max_line_len as f64 * char_width_estimate + (options.padding_x * 2) as f64).ceil()
        as u32;
    let height = lines.len() as u32 * options.line_height + options.padding_y * 2;

    let mut svg = String::with_capacity(4096);

    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">\n",
        width, height, width, height
    );
    let _ = write!(
        svg,
        "  <rect width=\"100%\" height=\"100%\" fill=\"{}\" rx=\"{}\" ry=\"{}\"/>\n",
        options.background_color, options.border_radius, options.border_radius
    );
    let _ = write!(svg, "  <style>\n");
    let _ = write!(
        svg,
        "    text {{ font-family: {}; font-size: {}px; white-space: pre; }}\n",
        options.font_family, options.font_size
    );
    let _ = write!(svg, "    .b {{ font-weight: bold; }}\n");
    let _ = write!(svg, "    .i {{ font-style: italic; }}\n");
    let _ = write!(svg, "    .u {{ text-decoration: underline; }}\n");
    let _ = write!(svg, "  </style>\n");

    for (line_index, spans) in lines.iter().enumerate() {
        let y = options.padding_y as f64
            + (line_index as f64 + 1.0) * options.line_height as f64
            - (options.line_height as f64 - options.font_size as f64) / 2.0;

        let _ = write!(
            svg,
            "  <text x=\"{}\" y=\"{:.1}\" xml:space=\"preserve\">",
            options.padding_x, y
        );

        for span in spans {
            if span.text.is_empty() {
                continue;
            }
            let color_str = span.fg.to_rgb_string();

            // Build class attribute.
            let mut classes = Vec::new();
            if span.bold {
                classes.push("b");
            }
            if span.italic {
                classes.push("i");
            }
            if span.underline {
                classes.push("u");
            }
            let class_attr = if classes.is_empty() {
                String::new()
            } else {
                format!(" class=\"{}\"", classes.join(" "))
            };

            let _ = write!(
                svg,
                "<tspan fill=\"{}\"{}>{}</tspan>",
                color_str,
                class_attr,
                escape_xml(&span.text)
            );
        }

        let _ = write!(svg, "</text>\n");
    }

    svg.push_str("</svg>");
    svg
}

/// Save SVG content to a file.
pub fn save_svg_to_file(svg: &str, path: &Path) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(svg.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// ANSI -> PNG (via external command)
// ---------------------------------------------------------------------------

/// Convert ANSI text to PNG by first generating SVG and then rasterizing
/// via an external tool (`rsvg-convert` or ImageMagick `convert`).
///
/// Returns the PNG bytes, or an error if no rasterizer is available.
pub fn ansi_to_png(input: &str) -> Result<Vec<u8>, AnsiToPngError> {
    ansi_to_png_with_options(input, &AnsiToSvgOptions::default())
}

/// Convert ANSI text to PNG with custom SVG options.
pub fn ansi_to_png_with_options(
    input: &str,
    options: &AnsiToSvgOptions,
) -> Result<Vec<u8>, AnsiToPngError> {
    let svg = ansi_to_svg_with_options(input, options);
    ansi_to_png_via_command(&svg)
}

/// Error type for PNG conversion.
#[derive(Debug, thiserror::Error)]
pub enum AnsiToPngError {
    #[error("no SVG rasterizer found (install rsvg-convert or imagemagick)")]
    NoRasterizer,
    #[error("rasterizer failed: {0}")]
    RasterizerFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Rasterize an SVG string to PNG using an external command.
///
/// Tries `rsvg-convert` first, then falls back to ImageMagick `convert`.
pub fn ansi_to_png_via_command(svg: &str) -> Result<Vec<u8>, AnsiToPngError> {
    // Try rsvg-convert.
    if which::which("rsvg-convert").is_ok() {
        let mut child = std::process::Command::new("rsvg-convert")
            .args(["--format=png", "--dpi-x=144", "--dpi-y=144"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(svg.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AnsiToPngError::RasterizerFailed(format!(
            "rsvg-convert: {}",
            stderr
        )));
    }

    // Try ImageMagick convert.
    if which::which("convert").is_ok() {
        let mut child = std::process::Command::new("convert")
            .args(["svg:-", "png:-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(svg.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AnsiToPngError::RasterizerFailed(format!(
            "convert: {}",
            stderr
        )));
    }

    Err(AnsiToPngError::NoRasterizer)
}

// ---------------------------------------------------------------------------
// Asciicast v2 format
// ---------------------------------------------------------------------------

/// Asciicast v2 file header.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AsciicastHeader {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<AsciicastEnv>,
}

/// Environment info in asciicast header.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AsciicastEnv {
    #[serde(rename = "SHELL", skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(rename = "TERM", skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
}

impl Default for AsciicastHeader {
    fn default() -> Self {
        Self {
            version: 2,
            width: 80,
            height: 24,
            timestamp: None,
            title: None,
            env: None,
        }
    }
}

/// Event type in an asciicast recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AsciicastEventType {
    /// Output (data written to terminal).
    #[serde(rename = "o")]
    Output,
    /// Input (data read from terminal).
    #[serde(rename = "i")]
    Input,
    /// Resize event.
    #[serde(rename = "r")]
    Resize,
}

/// A single event in an asciicast recording.
#[derive(Debug, Clone)]
pub struct AsciicastEvent {
    /// Time offset in seconds from the start of the recording.
    pub timestamp: f64,
    /// Event type.
    pub event_type: AsciicastEventType,
    /// Event data (terminal output text, input text, or "WxH" for resize).
    pub data: String,
}

impl AsciicastEvent {
    /// Serialize to the NDJSON array format: `[timestamp, "type", "data"]`.
    pub fn to_json_line(&self) -> String {
        let type_str = match self.event_type {
            AsciicastEventType::Output => "o",
            AsciicastEventType::Input => "i",
            AsciicastEventType::Resize => "r",
        };
        // Use serde_json for proper string escaping.
        let data_json = serde_json::to_string(&self.data).unwrap_or_else(|_| "\"\"".to_string());
        format!("[{:.6}, \"{}\", {}]", self.timestamp, type_str, data_json)
    }
}

/// An asciicast recording, consisting of a header and a sequence of events.
#[derive(Debug, Clone)]
pub struct Asciicast {
    pub header: AsciicastHeader,
    pub events: Vec<AsciicastEvent>,
}

impl Asciicast {
    /// Create a new empty recording.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            header: AsciicastHeader {
                version: 2,
                width,
                height,
                timestamp: None,
                title: None,
                env: None,
            },
            events: Vec::new(),
        }
    }

    /// Add an output event.
    pub fn add_output(&mut self, timestamp: f64, data: impl Into<String>) {
        self.events.push(AsciicastEvent {
            timestamp,
            event_type: AsciicastEventType::Output,
            data: data.into(),
        });
    }

    /// Add an input event.
    pub fn add_input(&mut self, timestamp: f64, data: impl Into<String>) {
        self.events.push(AsciicastEvent {
            timestamp,
            event_type: AsciicastEventType::Input,
            data: data.into(),
        });
    }

    /// Add a resize event.
    pub fn add_resize(&mut self, timestamp: f64, width: u32, height: u32) {
        self.events.push(AsciicastEvent {
            timestamp,
            event_type: AsciicastEventType::Resize,
            data: format!("{}x{}", width, height),
        });
    }

    /// Record a sequence of events from a pre-built list.
    pub fn record_events(&mut self, events: Vec<AsciicastEvent>) {
        self.events.extend(events);
    }

    /// Serialize the recording to NDJSON (asciicast v2 format).
    ///
    /// The first line is the JSON header, followed by one JSON array per event.
    pub fn to_ndjson(&self) -> String {
        let mut out = String::with_capacity(1024);

        // Header line.
        if let Ok(header_json) = serde_json::to_string(&self.header) {
            out.push_str(&header_json);
            out.push('\n');
        }

        // Event lines.
        for event in &self.events {
            out.push_str(&event.to_json_line());
            out.push('\n');
        }

        out
    }

    /// Write the recording to a .cast file at the given path.
    pub fn write_to_file(&self, path: &Path) -> std::io::Result<()> {
        let content = self.to_ndjson();
        let mut file = std::fs::File::create(path)?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }

    /// Create an asciicast recording from frame-by-frame ANSI content.
    ///
    /// Each frame is a tuple of (duration_seconds, ansi_text). The ANSI text
    /// for each frame is recorded as an output event at the cumulative timestamp.
    pub fn from_ansi_frames(
        width: u32,
        height: u32,
        frames: &[(f64, &str)],
    ) -> Self {
        let mut cast = Self::new(width, height);
        let mut t = 0.0;
        for (duration, text) in frames {
            cast.add_output(t, *text);
            t += duration;
        }
        cast
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // -- Color tests --

    #[test]
    fn test_ansi_color_rgb_string() {
        let c = AnsiColor::new(255, 128, 0);
        assert_eq!(c.to_rgb_string(), "rgb(255, 128, 0)");
    }

    #[test]
    fn test_get_256_color_standard() {
        let c = get_256_color(0);
        assert_eq!(c, AnsiColor::new(0, 0, 0));
        let c = get_256_color(9);
        assert_eq!(c, AnsiColor::new(255, 0, 0));
    }

    #[test]
    fn test_get_256_color_cube() {
        // Index 16 = first cube entry (0,0,0).
        let c = get_256_color(16);
        assert_eq!(c, AnsiColor::new(0, 0, 0));
        // Index 196 = (5,0,0) => r = 55 + 5*40 = 255
        let c = get_256_color(196);
        assert_eq!(c, AnsiColor::new(255, 0, 0));
    }

    #[test]
    fn test_get_256_color_grayscale() {
        let c = get_256_color(232);
        assert_eq!(c, AnsiColor::new(8, 8, 8));
        let c = get_256_color(255);
        assert_eq!(c, AnsiColor::new(238, 238, 238));
    }

    // -- Parser tests --

    #[test]
    fn test_parse_plain_text() {
        let lines = parse_ansi("hello world");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text, "hello world");
        assert_eq!(lines[0][0].fg, DEFAULT_FG);
        assert!(!lines[0][0].bold);
    }

    #[test]
    fn test_parse_multiline() {
        let lines = parse_ansi("line1\nline2\nline3");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0][0].text, "line1");
        assert_eq!(lines[1][0].text, "line2");
        assert_eq!(lines[2][0].text, "line3");
    }

    #[test]
    fn test_parse_empty_lines() {
        let lines = parse_ansi("a\n\nb");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0][0].text, "a");
        assert_eq!(lines[1][0].text, ""); // empty line preserved
        assert_eq!(lines[2][0].text, "b");
    }

    #[test]
    fn test_parse_basic_color() {
        // Red text: ESC[31m
        let input = "\x1b[31mhello\x1b[0m world";
        let lines = parse_ansi(input);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[0][0].text, "hello");
        assert_eq!(lines[0][0].fg, AnsiColor::new(205, 49, 49)); // red
        assert_eq!(lines[0][1].text, " world");
        assert_eq!(lines[0][1].fg, DEFAULT_FG); // reset
    }

    #[test]
    fn test_parse_bright_color() {
        let input = "\x1b[92mbright green\x1b[0m";
        let lines = parse_ansi(input);
        assert_eq!(lines[0][0].fg, AnsiColor::new(35, 209, 139));
    }

    #[test]
    fn test_parse_bold() {
        let input = "\x1b[1mbold\x1b[0m normal";
        let lines = parse_ansi(input);
        assert!(lines[0][0].bold);
        assert!(!lines[0][1].bold);
    }

    #[test]
    fn test_parse_italic_underline() {
        let input = "\x1b[3mitalic\x1b[4m and underline\x1b[0m";
        let lines = parse_ansi(input);
        assert!(lines[0][0].italic);
        assert!(!lines[0][0].underline);
        assert!(lines[0][1].italic);
        assert!(lines[0][1].underline);
    }

    #[test]
    fn test_parse_256_color() {
        // 38;5;196 = cube index 180 = (5,0,0) => rgb(255,0,0)
        let input = "\x1b[38;5;196mred\x1b[0m";
        let lines = parse_ansi(input);
        assert_eq!(lines[0][0].fg, AnsiColor::new(255, 0, 0));
        assert_eq!(lines[0][0].text, "red");
    }

    #[test]
    fn test_parse_true_color() {
        // 38;2;100;200;50 = true color
        let input = "\x1b[38;2;100;200;50mgreen\x1b[0m";
        let lines = parse_ansi(input);
        assert_eq!(lines[0][0].fg, AnsiColor::new(100, 200, 50));
        assert_eq!(lines[0][0].text, "green");
    }

    #[test]
    fn test_parse_default_fg_reset() {
        let input = "\x1b[31mred\x1b[39mdefault";
        let lines = parse_ansi(input);
        assert_eq!(lines[0][0].fg, AnsiColor::new(205, 49, 49));
        assert_eq!(lines[0][1].fg, DEFAULT_FG);
    }

    #[test]
    fn test_parse_combined_sgr() {
        // Bold + red in one sequence: ESC[1;31m
        let input = "\x1b[1;31mbold red\x1b[0m";
        let lines = parse_ansi(input);
        assert!(lines[0][0].bold);
        assert_eq!(lines[0][0].fg, AnsiColor::new(205, 49, 49));
    }

    #[test]
    fn test_parse_non_sgr_escape_skipped() {
        // Cursor movement ESC[2J should be skipped.
        let input = "\x1b[2Jhello";
        let lines = parse_ansi(input);
        assert_eq!(lines[0][0].text, "hello");
    }

    // -- SVG tests --

    #[test]
    fn test_ansi_to_svg_basic() {
        let svg = ansi_to_svg("hello");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("hello"));
        assert!(svg.contains("Menlo"));
    }

    #[test]
    fn test_ansi_to_svg_escapes_xml() {
        let svg = ansi_to_svg("<script>&\"test\"</script>");
        assert!(svg.contains("&lt;script&gt;"));
        assert!(svg.contains("&amp;"));
        assert!(svg.contains("&quot;test&quot;"));
    }

    #[test]
    fn test_ansi_to_svg_with_colors() {
        let input = "\x1b[31mred\x1b[0m normal";
        let svg = ansi_to_svg(input);
        assert!(svg.contains("rgb(205, 49, 49)"));
        assert!(svg.contains("red"));
        assert!(svg.contains("normal"));
    }

    #[test]
    fn test_ansi_to_svg_bold_class() {
        let input = "\x1b[1mbold text\x1b[0m";
        let svg = ansi_to_svg(input);
        assert!(svg.contains("class=\"b\""));
    }

    #[test]
    fn test_ansi_to_svg_italic_underline_classes() {
        let input = "\x1b[3;4mfancy\x1b[0m";
        let svg = ansi_to_svg(input);
        assert!(svg.contains("class=\"i u\""));
    }

    #[test]
    fn test_ansi_to_svg_custom_options() {
        let options = AnsiToSvgOptions {
            font_family: "Courier".to_string(),
            font_size: 16,
            line_height: 24,
            padding_x: 10,
            padding_y: 10,
            background_color: "black".to_string(),
            border_radius: 0,
        };
        let svg = ansi_to_svg_with_options("test", &options);
        assert!(svg.contains("Courier"));
        assert!(svg.contains("16px"));
        assert!(svg.contains("fill=\"black\""));
    }

    #[test]
    fn test_ansi_to_svg_multiline() {
        let svg = ansi_to_svg("line1\nline2");
        // Should have two <text> elements.
        let text_count = svg.matches("<text ").count();
        assert_eq!(text_count, 2);
    }

    #[test]
    fn test_ansi_to_svg_trims_trailing_empty_lines() {
        let svg = ansi_to_svg("content\n\n\n");
        // Trailing empty lines should be trimmed, keeping just the content line.
        let text_count = svg.matches("<text ").count();
        assert_eq!(text_count, 1);
    }

    #[test]
    fn test_ansi_to_svg_preserves_whitespace() {
        let svg = ansi_to_svg("  indented");
        assert!(svg.contains("xml:space=\"preserve\""));
        assert!(svg.contains("  indented"));
    }

    // -- Save SVG tests --

    #[test]
    fn test_save_svg_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.svg");
        let svg = ansi_to_svg("hello");
        save_svg_to_file(&svg, &path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("<svg"));
        assert!(content.contains("hello"));
    }

    // -- Asciicast tests --

    #[test]
    fn test_asciicast_header_serialization() {
        let header = AsciicastHeader {
            version: 2,
            width: 80,
            height: 24,
            timestamp: Some(1700000000),
            title: Some("Test".to_string()),
            env: Some(AsciicastEnv {
                shell: Some("/bin/bash".to_string()),
                term: Some("xterm-256color".to_string()),
            }),
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(json.contains("\"version\":2"));
        assert!(json.contains("\"width\":80"));
        assert!(json.contains("\"SHELL\":\"/bin/bash\""));
        assert!(json.contains("\"TERM\":\"xterm-256color\""));
    }

    #[test]
    fn test_asciicast_header_optional_fields() {
        let header = AsciicastHeader::default();
        let json = serde_json::to_string(&header).unwrap();
        // Optional fields with None should be skipped.
        assert!(!json.contains("timestamp"));
        assert!(!json.contains("title"));
        assert!(!json.contains("env"));
    }

    #[test]
    fn test_asciicast_event_to_json_line() {
        let event = AsciicastEvent {
            timestamp: 1.234567,
            event_type: AsciicastEventType::Output,
            data: "hello\r\n".to_string(),
        };
        let line = event.to_json_line();
        assert!(line.starts_with("[1.234567, \"o\","));
        assert!(line.contains("hello\\r\\n"));
    }

    #[test]
    fn test_asciicast_event_types() {
        let output = AsciicastEvent {
            timestamp: 0.0,
            event_type: AsciicastEventType::Output,
            data: "x".to_string(),
        };
        assert!(output.to_json_line().contains("\"o\""));

        let input = AsciicastEvent {
            timestamp: 0.0,
            event_type: AsciicastEventType::Input,
            data: "y".to_string(),
        };
        assert!(input.to_json_line().contains("\"i\""));

        let resize = AsciicastEvent {
            timestamp: 0.0,
            event_type: AsciicastEventType::Resize,
            data: "120x40".to_string(),
        };
        assert!(resize.to_json_line().contains("\"r\""));
    }

    #[test]
    fn test_asciicast_new() {
        let cast = Asciicast::new(120, 40);
        assert_eq!(cast.header.version, 2);
        assert_eq!(cast.header.width, 120);
        assert_eq!(cast.header.height, 40);
        assert!(cast.events.is_empty());
    }

    #[test]
    fn test_asciicast_add_events() {
        let mut cast = Asciicast::new(80, 24);
        cast.add_output(0.0, "hello ");
        cast.add_output(0.5, "world\r\n");
        cast.add_input(1.0, "ls\r\n");
        cast.add_resize(2.0, 120, 40);
        assert_eq!(cast.events.len(), 4);
        assert_eq!(cast.events[0].event_type, AsciicastEventType::Output);
        assert_eq!(cast.events[2].event_type, AsciicastEventType::Input);
        assert_eq!(cast.events[3].event_type, AsciicastEventType::Resize);
        assert_eq!(cast.events[3].data, "120x40");
    }

    #[test]
    fn test_asciicast_record_events() {
        let mut cast = Asciicast::new(80, 24);
        let events = vec![
            AsciicastEvent {
                timestamp: 0.0,
                event_type: AsciicastEventType::Output,
                data: "first".to_string(),
            },
            AsciicastEvent {
                timestamp: 1.0,
                event_type: AsciicastEventType::Output,
                data: "second".to_string(),
            },
        ];
        cast.record_events(events);
        assert_eq!(cast.events.len(), 2);
    }

    #[test]
    fn test_asciicast_to_ndjson() {
        let mut cast = Asciicast::new(80, 24);
        cast.add_output(0.0, "hello");
        cast.add_output(1.5, "world");
        let ndjson = cast.to_ndjson();
        let lines: Vec<&str> = ndjson.trim().split('\n').collect();
        assert_eq!(lines.len(), 3); // header + 2 events

        // Verify header.
        let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["version"], 2);
        assert_eq!(header["width"], 80);

        // Verify events.
        let event0: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(event0[1], "o");
        assert_eq!(event0[2], "hello");

        let event1: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert!(event1[0].as_f64().unwrap() > 1.0);
        assert_eq!(event1[2], "world");
    }

    #[test]
    fn test_asciicast_write_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.cast");

        let mut cast = Asciicast::new(80, 24);
        cast.header.timestamp = Some(1700000000);
        cast.add_output(0.0, "$ echo hello\r\n");
        cast.add_output(0.1, "hello\r\n");
        cast.write_to_file(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 3);

        // Verify it's valid NDJSON.
        for line in &lines {
            let _: serde_json::Value = serde_json::from_str(line).unwrap();
        }
    }

    #[test]
    fn test_asciicast_from_ansi_frames() {
        let frames = vec![
            (0.5, "\x1b[31mred output\x1b[0m\r\n"),
            (1.0, "normal output\r\n"),
            (0.3, "\x1b[1mbold\x1b[0m\r\n"),
        ];
        let cast = Asciicast::from_ansi_frames(80, 24, &frames);
        assert_eq!(cast.events.len(), 3);
        assert!((cast.events[0].timestamp - 0.0).abs() < f64::EPSILON);
        assert!((cast.events[1].timestamp - 0.5).abs() < f64::EPSILON);
        assert!((cast.events[2].timestamp - 1.5).abs() < f64::EPSILON);
        assert!(cast.events[0].data.contains("\x1b[31m"));
    }

    #[test]
    fn test_asciicast_event_data_escaping() {
        let event = AsciicastEvent {
            timestamp: 0.0,
            event_type: AsciicastEventType::Output,
            data: "line with \"quotes\" and \t tab".to_string(),
        };
        let line = event.to_json_line();
        // Should be valid JSON.
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed[2], "line with \"quotes\" and \t tab");
    }

    // -- XML escaping tests --

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("hello"), "hello");
        assert_eq!(escape_xml("<>&\"'"), "&lt;&gt;&amp;&quot;&apos;");
        assert_eq!(escape_xml("a < b & c"), "a &lt; b &amp; c");
    }

    // -- Integration: ANSI -> Asciicast --

    #[test]
    fn test_roundtrip_ansi_to_asciicast_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roundtrip.cast");

        let frames = vec![
            (0.0, "$ \x1b[32mls\x1b[0m\r\n"),
            (0.5, "file1.txt  file2.txt\r\n"),
        ];
        let cast = Asciicast::from_ansi_frames(80, 24, &frames);
        cast.write_to_file(&path).unwrap();

        // Read back and verify.
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 3); // header + 2 events

        let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["version"], 2);

        let event: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        let data = event[2].as_str().unwrap();
        assert!(data.contains("\x1b[32m")); // ANSI preserved
    }

    // -- 256-color exhaustive --

    #[test]
    fn test_parse_all_256_colors() {
        for i in 0..=255u8 {
            let input = format!("\x1b[38;5;{}mX\x1b[0m", i);
            let lines = parse_ansi(&input);
            assert_eq!(lines[0][0].text, "X");
            let expected = get_256_color(i);
            assert_eq!(lines[0][0].fg, expected, "mismatch for 256-color index {}", i);
        }
    }

    // -- True color edge cases --

    #[test]
    fn test_parse_true_color_boundary_values() {
        let input = "\x1b[38;2;0;0;0mblack\x1b[0m";
        let lines = parse_ansi(input);
        assert_eq!(lines[0][0].fg, AnsiColor::new(0, 0, 0));

        let input = "\x1b[38;2;255;255;255mwhite\x1b[0m";
        let lines = parse_ansi(input);
        assert_eq!(lines[0][0].fg, AnsiColor::new(255, 255, 255));
    }
}
