//! Unicode-aware string width calculation for terminal display.
//!
//! Handles CJK double-width characters, emoji (including ZWJ sequences and
//! flag pairs), combining marks, variation selectors, and zero-width
//! characters.  This is the Rust equivalent of the TypeScript
//! `stringWidth.ts` in the original Claude Code ink layer.
//!
//! The primary entry point is [`display_width`], which returns the number of
//! terminal columns a string occupies.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// Return the display width (in terminal columns) of a string.
///
/// Correctly handles:
/// - ASCII (fast path)
/// - CJK ideographs (width 2)
/// - Emoji sequences including ZWJ, skin tone modifiers, flag pairs (width 2)
/// - Combining diacritical marks (width 0)
/// - Variation selectors (width 0)
/// - ANSI escape codes (stripped, width 0)
/// - Control characters (width 0)
pub fn display_width(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }

    // Fast path: pure ASCII with no escape sequences
    if s.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return s.len();
    }

    // Strip ANSI escape sequences if present
    let s = if s.contains('\x1b') {
        strip_ansi(s)
    } else {
        s.to_string()
    };

    if s.is_empty() {
        return 0;
    }

    // Check if we need grapheme segmentation (emoji, ZWJ, variation selectors)
    if !needs_segmentation(&s) {
        return simple_width(&s);
    }

    // Full grapheme cluster segmentation path
    let mut width = 0;
    for grapheme in s.graphemes(true) {
        width += grapheme_width(grapheme);
    }
    width
}

/// Calculate width without grapheme segmentation — just iterate code points.
fn simple_width(s: &str) -> usize {
    let mut width = 0;
    for ch in s.chars() {
        if is_zero_width(ch) {
            continue;
        }
        width += UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    width
}

/// Calculate the display width of a single grapheme cluster.
fn grapheme_width(grapheme: &str) -> usize {
    let mut chars = grapheme.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return 0,
    };

    // Check for emoji: if the grapheme starts with an emoji code point
    // or contains ZWJ/variation selectors, treat it as an emoji cluster.
    if is_emoji_start(first) || grapheme.chars().any(|c| c == '\u{200D}' || c == '\u{FE0F}') {
        return emoji_width(grapheme);
    }

    // For non-emoji grapheme clusters (e.g., Devanagari conjuncts),
    // count only the first non-zero-width character's width.
    for ch in grapheme.chars() {
        if !is_zero_width(ch) {
            return UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    0
}

/// Return the display width of an emoji grapheme cluster.
fn emoji_width(grapheme: &str) -> usize {
    let first = grapheme.chars().next().unwrap_or('\0');

    // Regional indicator flags: single = 1, pair = 2
    if ('\u{1F1E6}'..='\u{1F1FF}').contains(&first) {
        let count = grapheme.chars().count();
        return if count == 1 { 1 } else { 2 };
    }

    // Incomplete keycap: digit/symbol + VS16 without U+20E3
    let chars: Vec<char> = grapheme.chars().collect();
    if chars.len() == 2
        && chars[1] == '\u{FE0F}'
        && (first.is_ascii_digit() || first == '#' || first == '*')
    {
        return 1;
    }

    // Most emoji sequences occupy 2 terminal columns
    2
}

/// Check if a character is the start of an emoji sequence.
fn is_emoji_start(c: char) -> bool {
    let cp = c as u32;
    // Common emoji ranges
    (0x1F300..=0x1FAFF).contains(&cp)
        || (0x2600..=0x27BF).contains(&cp)
        || (0x1F1E6..=0x1F1FF).contains(&cp)
        || (0xFE00..=0xFE0F).contains(&cp)
        || (0x2702..=0x27B0).contains(&cp)
        || cp == 0x200D
}

/// Check if a character is zero-width (combining marks, control chars, etc).
fn is_zero_width(c: char) -> bool {
    let cp = c as u32;

    // Fast path for common printable ASCII
    if (0x20..0x7F).contains(&cp) {
        return false;
    }

    // Control characters
    if cp <= 0x1F || (0x7F..=0x9F).contains(&cp) {
        return true;
    }

    // Soft hyphen
    if cp == 0x00AD {
        return true;
    }

    // Zero-width and invisible characters
    if (0x200B..=0x200D).contains(&cp) // ZW space/joiner
        || cp == 0xFEFF // BOM
        || (0x2060..=0x2064).contains(&cp) // Word joiner etc.
    {
        return true;
    }

    // Variation selectors
    if (0xFE00..=0xFE0F).contains(&cp) || (0xE0100..=0xE01EF).contains(&cp) {
        return true;
    }

    // Combining diacritical marks
    if (0x0300..=0x036F).contains(&cp)
        || (0x1AB0..=0x1AFF).contains(&cp)
        || (0x1DC0..=0x1DFF).contains(&cp)
        || (0x20D0..=0x20FF).contains(&cp)
        || (0xFE20..=0xFE2F).contains(&cp)
    {
        return true;
    }

    // Tag characters
    if (0xE0000..=0xE007F).contains(&cp) {
        return true;
    }

    false
}

/// Check if a string needs full grapheme segmentation.
///
/// Returns true if the string contains emoji, variation selectors, or ZWJ
/// that require cluster-level analysis.
fn needs_segmentation(s: &str) -> bool {
    for ch in s.chars() {
        let cp = ch as u32;
        // Emoji ranges
        if (0x1F300..=0x1FAFF).contains(&cp) {
            return true;
        }
        if (0x2600..=0x27BF).contains(&cp) {
            return true;
        }
        if (0x1F1E6..=0x1F1FF).contains(&cp) {
            return true;
        }
        // Variation selectors, ZWJ
        if (0xFE00..=0xFE0F).contains(&cp) {
            return true;
        }
        if cp == 0x200D {
            return true;
        }
    }
    false
}

/// Strip ANSI escape sequences from a string.
///
/// Handles both CSI sequences (`ESC [ ... final_byte`) and OSC sequences
/// (`ESC ] ... ST`).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == 0x1B {
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                b'[' => {
                    // CSI sequence: skip until final byte (0x40-0x7E)
                    i += 1;
                    while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1; // skip final byte
                    }
                }
                b']' => {
                    // OSC sequence: skip until ST (ESC \ or BEL)
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            // BEL
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Other escape: skip just the next byte
                    i += 1;
                }
            }
        } else {
            // Safe because we only push at ASCII byte boundaries or valid UTF-8
            // positions. Reconstruct character from bytes.
            let ch_start = i;
            // Advance past the UTF-8 character
            if bytes[i] < 0x80 {
                out.push(bytes[i] as char);
                i += 1;
            } else {
                // Multi-byte UTF-8: find the char at this position
                let remaining = &s[ch_start..];
                if let Some(ch) = remaining.chars().next() {
                    out.push(ch);
                    i += ch.len_utf8();
                } else {
                    i += 1;
                }
            }
        }
    }

    out
}

/// Wrap text to fit within `max_width` terminal columns, respecting
/// Unicode character widths (CJK double-width, emoji, etc).
///
/// Returns a vector of lines, each fitting within `max_width` columns.
/// Words are broken at whitespace when possible; long words are broken
/// mid-word if they exceed the line width.
pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut result = Vec::new();

    for line in text.lines() {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }

        let mut current_line = String::new();
        let mut current_width = 0;

        for word in line.split_whitespace() {
            let word_width = display_width(word);

            if word_width > max_width {
                // Word is too long — break it character by character
                if !current_line.is_empty() {
                    result.push(current_line);
                    current_line = String::new();
                    current_width = 0;
                }
                for ch in word.chars() {
                    let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if current_width + ch_width > max_width && !current_line.is_empty() {
                        result.push(current_line);
                        current_line = String::new();
                        current_width = 0;
                    }
                    current_line.push(ch);
                    current_width += ch_width;
                }
            } else if current_width == 0 {
                // First word on a new line
                current_line.push_str(word);
                current_width = word_width;
            } else if current_width + 1 + word_width <= max_width {
                // Fits with a space
                current_line.push(' ');
                current_line.push_str(word);
                current_width += 1 + word_width;
            } else {
                // Start a new line
                result.push(current_line);
                current_line = word.to_string();
                current_width = word_width;
            }
        }

        if !current_line.is_empty() {
            result.push(current_line);
        }
    }

    if result.is_empty() {
        result.push(String::new());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii_width() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("abc 123"), 7);
    }

    #[test]
    fn test_cjk_width() {
        // CJK characters are double-width
        assert_eq!(display_width("\u{4E16}\u{754C}"), 4); // 世界
        assert_eq!(display_width("a\u{4E16}b"), 4); // a世b
    }

    #[test]
    fn test_combining_marks() {
        // e + combining acute accent = 1 column
        assert_eq!(display_width("e\u{0301}"), 1);
    }

    #[test]
    fn test_zero_width_chars() {
        assert_eq!(display_width("\u{200B}"), 0); // zero-width space
        assert_eq!(display_width("a\u{200B}b"), 2); // a + ZWS + b
    }

    #[test]
    fn test_ansi_stripping() {
        assert_eq!(display_width("\x1b[31mhello\x1b[0m"), 5);
        assert_eq!(display_width("\x1b[1;32mtest\x1b[0m"), 4);
    }

    #[test]
    fn test_strip_ansi_osc() {
        let s = "\x1b]8;;http://example.com\x1b\\link\x1b]8;;\x1b\\";
        assert_eq!(strip_ansi(s), "link");
    }

    #[test]
    fn test_wrap_text_simple() {
        let wrapped = wrap_text("hello world", 5);
        assert_eq!(wrapped, vec!["hello", "world"]);
    }

    #[test]
    fn test_wrap_text_long_word() {
        let wrapped = wrap_text("abcdefghij", 5);
        assert_eq!(wrapped, vec!["abcde", "fghij"]);
    }

    #[test]
    fn test_wrap_text_empty() {
        let wrapped = wrap_text("", 10);
        assert_eq!(wrapped, vec![""]);
    }

    #[test]
    fn test_wrap_text_multiline() {
        let wrapped = wrap_text("line one\nline two", 20);
        assert_eq!(wrapped, vec!["line one", "line two"]);
    }

    #[test]
    fn test_emoji_width() {
        // Most emoji should be width 2
        assert_eq!(display_width("\u{1F600}"), 2); // 😀
    }
}
