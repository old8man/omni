//! Display formatting utilities: durations, token counts, byte sizes,
//! ANSI stripping, and middle-truncation.

// ---------------------------------------------------------------------------
// Duration formatting
// ---------------------------------------------------------------------------

/// Format a duration given in milliseconds to a human-readable string.
///
/// Examples: `"0s"`, `"3s"`, `"2m 30s"`, `"1h 5m 10s"`, `"2d 3h 15m"`.
pub fn format_duration(ms: u64) -> String {
    format_duration_opts(ms, false, false)
}

/// Format a duration, optionally hiding trailing zeros or showing only the
/// most significant unit.
pub fn format_duration_opts(
    ms: u64,
    hide_trailing_zeros: bool,
    most_significant_only: bool,
) -> String {
    if ms == 0 {
        return "0s".to_string();
    }

    if ms < 60_000 {
        let s = ms / 1000;
        return format!("{s}s");
    }

    let total_seconds = ms as f64 / 1000.0;
    let mut days = (total_seconds / 86400.0).floor() as u64;
    let mut hours = ((total_seconds % 86400.0) / 3600.0).floor() as u64;
    let mut minutes = ((total_seconds % 3600.0) / 60.0).floor() as u64;
    let mut seconds = ((total_seconds % 60.0) + 0.5).floor() as u64; // round

    // Carry-over from rounding.
    if seconds == 60 {
        seconds = 0;
        minutes += 1;
    }
    if minutes == 60 {
        minutes = 0;
        hours += 1;
    }
    if hours == 24 {
        hours = 0;
        days += 1;
    }

    if most_significant_only {
        return if days > 0 {
            format!("{days}d")
        } else if hours > 0 {
            format!("{hours}h")
        } else if minutes > 0 {
            format!("{minutes}m")
        } else {
            format!("{seconds}s")
        };
    }

    if days > 0 {
        if hide_trailing_zeros && hours == 0 && minutes == 0 {
            return format!("{days}d");
        }
        if hide_trailing_zeros && minutes == 0 {
            return format!("{days}d {hours}h");
        }
        return format!("{days}d {hours}h {minutes}m");
    }
    if hours > 0 {
        if hide_trailing_zeros && minutes == 0 && seconds == 0 {
            return format!("{hours}h");
        }
        if hide_trailing_zeros && seconds == 0 {
            return format!("{hours}h {minutes}m");
        }
        return format!("{hours}h {minutes}m {seconds}s");
    }
    if minutes > 0 {
        if hide_trailing_zeros && seconds == 0 {
            return format!("{minutes}m");
        }
        return format!("{minutes}m {seconds}s");
    }

    format!("{seconds}s")
}

/// Format milliseconds as seconds with 1 decimal place, e.g. `"1.2s"`.
pub fn format_seconds_short(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

// ---------------------------------------------------------------------------
// Number / token formatting
// ---------------------------------------------------------------------------

/// Format a number in compact notation: `900` → `"900"`, `1321` → `"1.3k"`, `1_500_000` → `"1.5m"`.
pub fn format_number(n: u64) -> String {
    if n >= 1_000_000_000 {
        let v = n as f64 / 1_000_000_000.0;
        format!("{:.1}b", v).to_lowercase()
    } else if n >= 1_000_000 {
        let v = n as f64 / 1_000_000.0;
        format!("{:.1}m", v).to_lowercase()
    } else if n >= 1_000 {
        let v = n as f64 / 1_000.0;
        format!("{:.1}k", v).to_lowercase()
    } else {
        n.to_string()
    }
}

/// Format a token count: like `format_number` but strips `.0` trailing decimals.
/// E.g. `1000` → `"1k"`, `1500` → `"1.5k"`.
pub fn format_tokens(count: u64) -> String {
    format_number(count).replace(".0", "")
}

// ---------------------------------------------------------------------------
// Byte / file-size formatting
// ---------------------------------------------------------------------------

/// Format a byte count to human-readable: `"1.5KB"`, `"2.3MB"`, `"1.1GB"`.
pub fn format_bytes(size_in_bytes: u64) -> String {
    format_file_size(size_in_bytes)
}

/// Alias matching the TS `formatFileSize`.
pub fn format_file_size(size_in_bytes: u64) -> String {
    let kb = size_in_bytes as f64 / 1024.0;
    if kb < 1.0 {
        return format!("{size_in_bytes} bytes");
    }
    if kb < 1024.0 {
        return strip_trailing_zero(format!("{kb:.1}")) + "KB";
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return strip_trailing_zero(format!("{mb:.1}")) + "MB";
    }
    let gb = mb / 1024.0;
    strip_trailing_zero(format!("{gb:.1}")) + "GB"
}

/// Remove a trailing `.0` from a formatted float string.
fn strip_trailing_zero(s: String) -> String {
    if s.ends_with(".0") {
        s[..s.len() - 2].to_string()
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// ANSI stripping
// ---------------------------------------------------------------------------

/// Strip ANSI escape sequences from text, returning plain text.
pub fn strip_ansi(text: &str) -> String {
    // Match CSI sequences (\x1b[...X), OSC sequences (\x1b]...ST), and other
    // simple 2-byte escapes (\x1bX).
    lazy_regex::regex!(r"\x1b(?:\[[0-9;]*[A-Za-z]|\][^\x07\x1b]*(?:\x07|\x1b\\)|\[[0-9;]*m|.)")
        .replace_all(text, "")
        .to_string()
}

// ---------------------------------------------------------------------------
// Text truncation
// ---------------------------------------------------------------------------

/// Truncate text in the middle, preserving the start and end.
///
/// If `text.len() <= max_len` the original string is returned unchanged.
/// Otherwise returns `"start...end"` where total length equals `max_len`.
pub fn truncate_middle(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    if max_len <= 3 {
        return "...".to_string();
    }

    let ellipsis = "...";
    let available = max_len - ellipsis.len();
    let start_len = available.div_ceil(2);
    let end_len = available / 2;

    format!(
        "{}{}{}",
        &text[..start_len],
        ellipsis,
        &text[text.len() - end_len..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Duration
    #[test]
    fn test_format_duration_zero() {
        assert_eq!(format_duration(0), "0s");
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(3000), "3s");
        assert_eq!(format_duration(59_000), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(150_000), "2m 30s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3_661_000), "1h 1m 1s");
    }

    #[test]
    fn test_format_duration_days() {
        assert_eq!(format_duration(90_000_000), "1d 1h 0m");
    }

    #[test]
    fn test_format_duration_hide_trailing() {
        assert_eq!(format_duration_opts(3_600_000, true, false), "1h");
        assert_eq!(format_duration_opts(7_200_000 + 60_000, true, false), "2h 1m");
    }

    #[test]
    fn test_format_duration_most_significant() {
        assert_eq!(format_duration_opts(90_000_000, false, true), "1d");
        assert_eq!(format_duration_opts(3_661_000, false, true), "1h");
    }

    // Tokens / numbers
    #[test]
    fn test_format_number() {
        assert_eq!(format_number(900), "900");
        assert_eq!(format_number(1321), "1.3k");
        assert_eq!(format_number(1_500_000), "1.5m");
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(1000), "1k");
        assert_eq!(format_tokens(1500), "1.5k");
    }

    // Bytes
    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 bytes");
        assert_eq!(format_bytes(1536), "1.5KB");
        assert_eq!(format_bytes(1_048_576), "1MB");
        assert_eq!(format_bytes(1_572_864), "1.5MB");
    }

    // ANSI stripping
    #[test]
    fn test_strip_ansi() {
        assert_eq!(strip_ansi("\x1b[31mhello\x1b[0m"), "hello");
        assert_eq!(strip_ansi("no escapes"), "no escapes");
    }

    // Middle truncation
    #[test]
    fn test_truncate_middle_short() {
        assert_eq!(truncate_middle("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_middle_exact() {
        assert_eq!(truncate_middle("abcdefghij", 7), "ab...ij");
    }

    #[test]
    fn test_truncate_middle_tiny() {
        assert_eq!(truncate_middle("abcdef", 3), "...");
    }
}
