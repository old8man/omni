//! Memory age tracking and staleness detection.
//!
//! Provides utilities to compute the age of a memory from its modification
//! time, render human-readable age strings, and generate staleness caveats
//! for memories that may be outdated.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds per day.
const MS_PER_DAY: u64 = 86_400_000;

/// Days elapsed since `mtime_ms` (milliseconds since Unix epoch).
///
/// Floor-rounded: 0 for today, 1 for yesterday, 2+ for older.
/// Negative inputs (future mtime, clock skew) clamp to 0.
pub fn memory_age_days(mtime_ms: u64) -> u64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now_ms.saturating_sub(mtime_ms) / MS_PER_DAY
}

/// Human-readable age string.
///
/// Models are poor at date arithmetic -- a raw ISO timestamp doesn't trigger
/// staleness reasoning the way "47 days ago" does.
pub fn memory_age(mtime_ms: u64) -> String {
    let d = memory_age_days(mtime_ms);
    match d {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        n => format!("{n} days ago"),
    }
}

/// Plain-text staleness caveat for memories > 1 day old.
///
/// Returns an empty string for fresh (today/yesterday) memories -- warning
/// there is noise.
///
/// Use this when the consumer already provides its own wrapping
/// (e.g. messages relevant_memories -> system reminder wrapper).
pub fn memory_freshness_text(mtime_ms: u64) -> String {
    let d = memory_age_days(mtime_ms);
    if d <= 1 {
        return String::new();
    }
    format!(
        "This memory is {d} days old. \
         Memories are point-in-time observations, not live state — \
         claims about code behavior or file:line citations may be outdated. \
         Verify against current code before asserting as fact."
    )
}

/// Per-memory staleness note wrapped in `<system-reminder>` tags.
///
/// Returns an empty string for memories <= 1 day old.
/// Use this for callers that don't add their own system-reminder wrapper
/// (e.g. FileReadTool output).
pub fn memory_freshness_note(mtime_ms: u64) -> String {
    let text = memory_freshness_text(mtime_ms);
    if text.is_empty() {
        return String::new();
    }
    format!("<system-reminder>{text}</system-reminder>\n")
}

/// Determine if a memory is considered stale.
///
/// A memory is stale if it is older than the given threshold in days.
/// Useful for bulk filtering or flagging during memory scans.
pub fn is_stale(mtime_ms: u64, threshold_days: u64) -> bool {
    memory_age_days(mtime_ms) > threshold_days
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    #[test]
    fn test_memory_age_days_today() {
        let now = now_ms();
        assert_eq!(memory_age_days(now), 0);
    }

    #[test]
    fn test_memory_age_days_yesterday() {
        let yesterday = now_ms() - MS_PER_DAY - 1000; // slightly more than 1 day
        assert_eq!(memory_age_days(yesterday), 1);
    }

    #[test]
    fn test_memory_age_days_old() {
        let ten_days_ago = now_ms() - (10 * MS_PER_DAY);
        assert_eq!(memory_age_days(ten_days_ago), 10);
    }

    #[test]
    fn test_memory_age_days_future_clamps() {
        let future = now_ms() + MS_PER_DAY;
        assert_eq!(memory_age_days(future), 0);
    }

    #[test]
    fn test_memory_age_string_today() {
        assert_eq!(memory_age(now_ms()), "today");
    }

    #[test]
    fn test_memory_age_string_yesterday() {
        let yesterday = now_ms() - MS_PER_DAY - 1000;
        assert_eq!(memory_age(yesterday), "yesterday");
    }

    #[test]
    fn test_memory_age_string_old() {
        let five_days = now_ms() - (5 * MS_PER_DAY);
        assert_eq!(memory_age(five_days), "5 days ago");
    }

    #[test]
    fn test_freshness_text_fresh() {
        assert!(memory_freshness_text(now_ms()).is_empty());
    }

    #[test]
    fn test_freshness_text_yesterday() {
        let yesterday = now_ms() - MS_PER_DAY - 1000;
        assert!(memory_freshness_text(yesterday).is_empty());
    }

    #[test]
    fn test_freshness_text_stale() {
        let old = now_ms() - (5 * MS_PER_DAY);
        let text = memory_freshness_text(old);
        assert!(text.contains("5 days old"));
        assert!(text.contains("Verify against current code"));
    }

    #[test]
    fn test_freshness_note_fresh() {
        assert!(memory_freshness_note(now_ms()).is_empty());
    }

    #[test]
    fn test_freshness_note_stale() {
        let old = now_ms() - (5 * MS_PER_DAY);
        let note = memory_freshness_note(old);
        assert!(note.starts_with("<system-reminder>"));
        assert!(note.contains("5 days old"));
        assert!(note.ends_with("</system-reminder>\n"));
    }

    #[test]
    fn test_is_stale() {
        let recent = now_ms() - MS_PER_DAY; // 1 day old
        assert!(!is_stale(recent, 7));

        let old = now_ms() - (10 * MS_PER_DAY);
        assert!(is_stale(old, 7));
    }

    #[test]
    fn test_is_stale_boundary() {
        let exactly_threshold = now_ms() - (7 * MS_PER_DAY);
        // At exactly the threshold, not stale (> not >=)
        assert!(!is_stale(exactly_threshold, 7));

        let one_day_past = now_ms() - (8 * MS_PER_DAY);
        assert!(is_stale(one_day_past, 7));
    }
}
