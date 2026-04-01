//! Memory age tracking and staleness detection.

use std::time::{SystemTime, UNIX_EPOCH};

const MS_PER_DAY: u64 = 86_400_000;

/// Days elapsed since `mtime_ms` (milliseconds since Unix epoch).
pub fn memory_age_days(mtime_ms: u64) -> u64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now_ms.saturating_sub(mtime_ms) / MS_PER_DAY
}

/// Human-readable age string.
pub fn memory_age(mtime_ms: u64) -> String {
    let d = memory_age_days(mtime_ms);
    match d {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        n => format!("{n} days ago"),
    }
}

/// Plain-text staleness caveat for memories > 1 day old.
pub fn memory_freshness_text(mtime_ms: u64) -> String {
    let d = memory_age_days(mtime_ms);
    if d <= 1 { return String::new(); }
    format!(
        "This memory is {d} days old. \
         Memories are point-in-time observations, not live state \u{2014} \
         claims about code behavior or file:line citations may be outdated. \
         Verify against current code before asserting as fact."
    )
}

/// Per-memory staleness note wrapped in `<system-reminder>` tags.
pub fn memory_freshness_note(mtime_ms: u64) -> String {
    let text = memory_freshness_text(mtime_ms);
    if text.is_empty() { return String::new(); }
    format!("<system-reminder>{text}</system-reminder>\n")
}

/// Determine if a memory is considered stale.
pub fn is_stale(mtime_ms: u64, threshold_days: u64) -> bool {
    memory_age_days(mtime_ms) > threshold_days
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_ms() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
    }

    #[test]
    fn test_memory_age_days_today() { assert_eq!(memory_age_days(now_ms()), 0); }

    #[test]
    fn test_memory_age_days_yesterday() {
        assert_eq!(memory_age_days(now_ms() - MS_PER_DAY - 1000), 1);
    }

    #[test]
    fn test_memory_age_days_old() {
        assert_eq!(memory_age_days(now_ms() - 10 * MS_PER_DAY), 10);
    }

    #[test]
    fn test_memory_age_days_future_clamps() {
        assert_eq!(memory_age_days(now_ms() + MS_PER_DAY), 0);
    }

    #[test]
    fn test_memory_age_string() {
        assert_eq!(memory_age(now_ms()), "today");
        assert_eq!(memory_age(now_ms() - MS_PER_DAY - 1000), "yesterday");
        assert_eq!(memory_age(now_ms() - 5 * MS_PER_DAY), "5 days ago");
    }

    #[test]
    fn test_freshness_text_fresh() { assert!(memory_freshness_text(now_ms()).is_empty()); }

    #[test]
    fn test_freshness_text_stale() {
        let text = memory_freshness_text(now_ms() - 5 * MS_PER_DAY);
        assert!(text.contains("5 days old"));
        assert!(text.contains("Verify against current code"));
    }

    #[test]
    fn test_freshness_note_fresh() { assert!(memory_freshness_note(now_ms()).is_empty()); }

    #[test]
    fn test_freshness_note_stale() {
        let note = memory_freshness_note(now_ms() - 5 * MS_PER_DAY);
        assert!(note.starts_with("<system-reminder>"));
        assert!(note.ends_with("</system-reminder>\n"));
    }

    #[test]
    fn test_is_stale() {
        assert!(!is_stale(now_ms() - MS_PER_DAY, 7));
        assert!(is_stale(now_ms() - 10 * MS_PER_DAY, 7));
    }
}
