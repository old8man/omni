/// Generate a short session recap ("while you were away" card) when the user
/// returns after being idle.
///
/// Takes the recent conversation context and session memory, then asks a small
/// fast model for 1-3 concise sentences covering the high-level task and
/// concrete next step.
///
/// Port of `services/awaySummary.ts`.
use crate::types::message::Message;

use tracing::{debug, warn};

// ── Constants ──────────────────────────────────────────────────────────────

/// Recap only needs recent context. Truncate to avoid "prompt too long" on
/// large sessions. 30 messages is roughly 15 user/assistant exchanges.
const RECENT_MESSAGE_WINDOW: usize = 30;

// ── Prompt ─────────────────────────────────────────────────────────────────

/// Build the system-level instruction for away-summary generation.
pub fn build_away_summary_prompt(memory: Option<&str>) -> String {
    let memory_block = match memory {
        Some(m) if !m.is_empty() => format!("Session memory (broader context):\n{}\n\n", m),
        _ => String::new(),
    };

    format!(
        "{}The user stepped away and is coming back. Write exactly 1-3 short sentences. \
         Start by stating the high-level task — what they are building or debugging, \
         not implementation details. Next: the concrete next step. Skip status reports \
         and commit recaps.",
        memory_block
    )
}

// ── Generation (model-agnostic) ────────────────────────────────────────────

/// Extract the most recent messages for summary context.
///
/// Returns at most [`RECENT_MESSAGE_WINDOW`] messages from the tail of the
/// conversation. Returns `None` if the conversation is empty.
pub fn prepare_away_summary_context(messages: &[Message]) -> Option<Vec<&Message>> {
    if messages.is_empty() {
        return None;
    }

    let start = messages.len().saturating_sub(RECENT_MESSAGE_WINDOW);
    Some(messages[start..].iter().collect())
}

/// Result of an away summary generation attempt.
#[derive(Debug, Clone)]
pub enum AwaySummaryResult {
    /// Successfully generated a summary.
    Success(String),
    /// No messages to summarize.
    Empty,
    /// Generation was aborted (e.g. user cancelled).
    Aborted,
    /// An API or other error occurred.
    Error(String),
}

/// Generate the away summary given the conversation transcript and optional
/// session memory.
///
/// This function prepares the prompt and context. The actual model call is
/// delegated to the `query_fn` callback so that the caller can use whatever
/// API layer is appropriate (streaming, non-streaming, etc.).
///
/// `query_fn` receives:
/// 1. The recent messages (up to `RECENT_MESSAGE_WINDOW`)
/// 2. The summary prompt (to be appended as a user message)
///
/// It should return `Ok(text)` with the model's response, or `Err` on failure.
pub async fn generate_away_summary<F, Fut>(
    messages: &[Message],
    session_memory: Option<&str>,
    query_fn: F,
) -> AwaySummaryResult
where
    F: FnOnce(Vec<&Message>, String) -> Fut,
    Fut: std::future::Future<Output = Result<String, AwaySummaryError>>,
{
    if messages.is_empty() {
        return AwaySummaryResult::Empty;
    }

    let recent = match prepare_away_summary_context(messages) {
        Some(r) => r,
        None => return AwaySummaryResult::Empty,
    };

    let prompt = build_away_summary_prompt(session_memory);

    match query_fn(recent, prompt).await {
        Ok(text) => {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                debug!("[awaySummary] model returned empty response");
                AwaySummaryResult::Error("empty response".to_string())
            } else {
                AwaySummaryResult::Success(trimmed)
            }
        }
        Err(AwaySummaryError::Aborted) => {
            debug!("[awaySummary] generation aborted");
            AwaySummaryResult::Aborted
        }
        Err(AwaySummaryError::ApiError(msg)) => {
            debug!(error = %msg, "[awaySummary] API error");
            AwaySummaryResult::Error(msg)
        }
        Err(AwaySummaryError::Other(msg)) => {
            warn!(error = %msg, "[awaySummary] generation failed");
            AwaySummaryResult::Error(msg)
        }
    }
}

/// Errors that can occur during away summary generation.
#[derive(Debug, Clone)]
pub enum AwaySummaryError {
    /// The operation was aborted by the user or a signal.
    Aborted,
    /// An API-level error (rate limit, auth, etc.).
    ApiError(String),
    /// Any other error.
    Other(String),
}

impl std::fmt::Display for AwaySummaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AwaySummaryError::Aborted => write!(f, "aborted"),
            AwaySummaryError::ApiError(msg) => write!(f, "API error: {}", msg),
            AwaySummaryError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for AwaySummaryError {}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::{ApiMessage, AssistantMessage, Role, UserMessage};
    use crate::types::usage::Usage;
    use uuid::Uuid;

    fn make_user_msg(text: &str) -> Message {
        Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            content: vec![crate::types::content::ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: chrono::Utc::now(),
        })
    }

    fn make_assistant_msg(text: &str) -> Message {
        Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            message: ApiMessage {
                id: String::new(),
                model: String::new(),
                role: Role::Assistant,
                content: vec![crate::types::content::ContentBlock::Text {
                    text: text.to_string(),
                }],
                stop_reason: None,
                usage: Usage::default(),
            },
            request_id: None,
            timestamp: chrono::Utc::now(),
        })
    }

    #[test]
    fn test_build_prompt_no_memory() {
        let prompt = build_away_summary_prompt(None);
        assert!(prompt.starts_with("The user stepped away"));
        assert!(prompt.contains("1-3 short sentences"));
    }

    #[test]
    fn test_build_prompt_with_memory() {
        let prompt = build_away_summary_prompt(Some("Working on auth module"));
        assert!(prompt.contains("Session memory"));
        assert!(prompt.contains("Working on auth module"));
    }

    #[test]
    fn test_build_prompt_with_empty_memory() {
        let prompt = build_away_summary_prompt(Some(""));
        assert!(prompt.starts_with("The user stepped away"));
    }

    #[test]
    fn test_prepare_context_empty() {
        let messages: Vec<Message> = vec![];
        assert!(prepare_away_summary_context(&messages).is_none());
    }

    #[test]
    fn test_prepare_context_small() {
        let messages = vec![
            make_user_msg("hello"),
            make_assistant_msg("hi there"),
        ];
        let ctx = prepare_away_summary_context(&messages).unwrap();
        assert_eq!(ctx.len(), 2);
    }

    #[test]
    fn test_prepare_context_truncation() {
        let mut messages = Vec::new();
        for i in 0..50 {
            messages.push(make_user_msg(&format!("msg {}", i)));
        }
        let ctx = prepare_away_summary_context(&messages).unwrap();
        assert_eq!(ctx.len(), RECENT_MESSAGE_WINDOW);
    }

    #[tokio::test]
    async fn test_generate_away_summary_success() {
        let messages = vec![
            make_user_msg("Fix the auth bug"),
            make_assistant_msg("I'll look into the auth module."),
        ];

        let result = generate_away_summary(
            &messages,
            Some("Working on auth"),
            |_msgs, _prompt| async {
                Ok("You were debugging an auth issue. Next step: add token refresh.".to_string())
            },
        )
        .await;

        match result {
            AwaySummaryResult::Success(text) => {
                assert!(text.contains("auth"));
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_generate_away_summary_empty_messages() {
        let messages: Vec<Message> = vec![];

        let result = generate_away_summary(&messages, None, |_, _| async {
            Ok("should not be called".to_string())
        })
        .await;

        assert!(matches!(result, AwaySummaryResult::Empty));
    }

    #[tokio::test]
    async fn test_generate_away_summary_aborted() {
        let messages = vec![make_user_msg("hello")];

        let result = generate_away_summary(&messages, None, |_, _| async {
            Err(AwaySummaryError::Aborted)
        })
        .await;

        assert!(matches!(result, AwaySummaryResult::Aborted));
    }

    #[tokio::test]
    async fn test_generate_away_summary_api_error() {
        let messages = vec![make_user_msg("hello")];

        let result = generate_away_summary(&messages, None, |_, _| async {
            Err(AwaySummaryError::ApiError("rate limited".to_string()))
        })
        .await;

        match result {
            AwaySummaryResult::Error(msg) => assert!(msg.contains("rate limited")),
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_generate_away_summary_empty_response() {
        let messages = vec![make_user_msg("hello")];

        let result = generate_away_summary(&messages, None, |_, _| async {
            Ok("   \n  ".to_string())
        })
        .await;

        assert!(matches!(result, AwaySummaryResult::Error(_)));
    }

    #[test]
    fn test_away_summary_error_display() {
        assert_eq!(AwaySummaryError::Aborted.to_string(), "aborted");
        assert_eq!(
            AwaySummaryError::ApiError("oops".to_string()).to_string(),
            "API error: oops"
        );
        assert_eq!(
            AwaySummaryError::Other("fail".to_string()).to_string(),
            "fail"
        );
    }
}
