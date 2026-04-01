use serde::{Deserialize, Serialize};

// ── Types ───────────────────────────────────────────────────────────────────

/// Variant of the suggestion prompt used for generation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptVariant {
    #[default]
    UserIntent,
    StatedIntent,
}

/// A generated prompt suggestion.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromptSuggestion {
    pub text: String,
    pub prompt_id: PromptVariant,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_request_id: Option<String>,
}

/// Reason why a suggestion was suppressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuppressReason {
    Disabled,
    PendingPermission,
    ElicitationActive,
    PlanMode,
    RateLimit,
    EarlyConversation,
    LastResponseError,
    CacheCold,
    Aborted,
    Empty,
}

impl SuppressReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::PendingPermission => "pending_permission",
            Self::ElicitationActive => "elicitation_active",
            Self::PlanMode => "plan_mode",
            Self::RateLimit => "rate_limit",
            Self::EarlyConversation => "early_conversation",
            Self::LastResponseError => "last_response_error",
            Self::CacheCold => "cache_cold",
            Self::Aborted => "aborted",
            Self::Empty => "empty",
        }
    }
}

// ── Suggestion prompt ───────────────────────────────────────────────────────

/// System prompt used to generate next-input suggestions.
pub const SUGGESTION_PROMPT: &str = r#"[SUGGESTION MODE: Suggest what the user might naturally type next into Claude Code.]

FIRST: Look at the user's recent messages and original request.

Your job is to predict what THEY would type - not what you think they should do.

THE TEST: Would they think "I was just about to type that"?

EXAMPLES:
User asked "fix the bug and run tests", bug is fixed → "run the tests"
After code written → "try it out"
Claude offers options → suggest the one the user would likely pick, based on conversation
Claude asks to continue → "yes" or "go ahead"
Task complete, obvious follow-up → "commit this" or "push it"
After error or misunderstanding → silence (let them assess/correct)

Be specific: "run the tests" beats "continue".

NEVER SUGGEST:
- Evaluative ("looks good", "thanks")
- Questions ("what about...?")
- Claude-voice ("Let me...", "I'll...", "Here's...")
- New ideas they didn't ask about
- Multiple sentences

Stay silent if the next step isn't obvious from what the user said.

Format: 2-12 words, match the user's style. Or nothing.

Reply with ONLY the suggestion, no quotes or explanation."#;

// ── Filtering ───────────────────────────────────────────────────────────────

/// Set of single-word inputs that are valid and should not be filtered out.
const ALLOWED_SINGLE_WORDS: &[&str] = &[
    "yes", "yeah", "yep", "yea", "yup", "sure", "ok", "okay", "push", "commit",
    "deploy", "stop", "continue", "check", "exit", "quit", "no",
];

/// Determine whether a suggestion should be filtered out before display.
///
/// Returns the filter reason if suppressed, or `None` if the suggestion is
/// acceptable.
pub fn should_filter_suggestion(suggestion: &str) -> Option<&'static str> {
    let lower = suggestion.to_lowercase();
    let trimmed = suggestion.trim();
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let word_count = words.len();

    // "done"
    if lower == "done" {
        return Some("done");
    }

    // Meta text indicating the model chose silence
    if lower == "nothing found"
        || lower == "nothing found."
        || lower.starts_with("nothing to suggest")
        || lower.starts_with("no suggestion")
        || lower.contains("silence is")
        || lower.contains("stay silent")
        || lower.contains("staying silent")
        || lower.contains("stays silent")
    {
        return Some("meta_text");
    }

    // Bare "silence" wrapped in punctuation
    let silence_re = trimmed
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    if silence_re == "silence" {
        return Some("meta_text");
    }

    // Wrapped in parens or brackets
    if (trimmed.starts_with('(') && trimmed.ends_with(')'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        return Some("meta_wrapped");
    }

    // Error messages
    if lower.starts_with("api error:")
        || lower.starts_with("prompt is too long")
        || lower.starts_with("request timed out")
        || lower.starts_with("invalid api key")
        || lower.starts_with("image was too large")
    {
        return Some("error_message");
    }

    // Prefixed label like "Suggestion: ..."
    if suggestion
        .find(": ")
        .map(|i| suggestion[..i].chars().all(|c| c.is_alphanumeric()))
        .unwrap_or(false)
    {
        return Some("prefixed_label");
    }

    // Too few words (unless slash command or allowed single word)
    if word_count < 2
        && !trimmed.starts_with('/')
        && !ALLOWED_SINGLE_WORDS.contains(&lower.as_str())
    {
        return Some("too_few_words");
    }

    // Too many words
    if word_count > 12 {
        return Some("too_many_words");
    }

    // Too long
    if suggestion.len() >= 100 {
        return Some("too_long");
    }

    // Multiple sentences
    if has_multiple_sentences(suggestion) {
        return Some("multiple_sentences");
    }

    // Formatting
    if suggestion.contains('\n') || suggestion.contains('*') || suggestion.contains("**") {
        return Some("has_formatting");
    }

    // Evaluative
    let evaluative = [
        "thanks",
        "thank you",
        "looks good",
        "sounds good",
        "that works",
        "that worked",
        "that's all",
        "nice",
        "great",
        "perfect",
        "makes sense",
        "awesome",
        "excellent",
    ];
    for word in &evaluative {
        if lower.contains(word) {
            return Some("evaluative");
        }
    }

    // Claude voice
    let claude_prefixes = [
        "let me",
        "i'll",
        "i've",
        "i'm",
        "i can",
        "i would",
        "i think",
        "i notice",
        "here's",
        "here is",
        "here are",
        "that's",
        "this is",
        "this will",
        "you can",
        "you should",
        "you could",
        "sure,",
        "of course",
        "certainly",
    ];
    for prefix in &claude_prefixes {
        if lower.starts_with(prefix) {
            return Some("claude_voice");
        }
    }

    None
}

/// Detect multiple sentences: period/bang/question followed by space+uppercase.
fn has_multiple_sentences(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    for i in 0..chars.len().saturating_sub(2) {
        if matches!(chars[i], '.' | '!' | '?') && chars[i + 1] == ' ' && chars[i + 2].is_uppercase()
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_basics() {
        assert_eq!(should_filter_suggestion("done"), Some("done"));
        assert_eq!(should_filter_suggestion("nothing found"), Some("meta_text"));
        assert!(should_filter_suggestion("run the tests").is_none());
        assert_eq!(should_filter_suggestion("yes"), None);
        assert_eq!(should_filter_suggestion("x"), Some("too_few_words"));
        assert_eq!(
            should_filter_suggestion("looks good"),
            Some("evaluative")
        );
        assert_eq!(
            should_filter_suggestion("Let me check that for you"),
            Some("claude_voice")
        );
    }
}
