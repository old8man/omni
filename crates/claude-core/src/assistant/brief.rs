use serde::{Deserialize, Serialize};

use super::types::AssistantState;

/// Status label for a brief message indicating intent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BriefStatus {
    /// Replying to something the user just said.
    #[default]
    Normal,
    /// Surfacing something the user hasn't asked for — task completion,
    /// a blocker, an unsolicited status update.
    Proactive,
}

/// A message sent through the SendUserMessage (Brief) tool.
///
/// In brief mode this is the sole channel for user-visible output.
/// Text outside this tool is visible only in the detail view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefMessage {
    /// The markdown-formatted message for the user.
    pub message: String,
    /// Optional file paths to attach alongside the message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// Intent label: 'normal' for replies, 'proactive' for unsolicited.
    #[serde(default)]
    pub status: BriefStatus,
}

/// Tool name for the brief/SendUserMessage tool.
pub const BRIEF_TOOL_NAME: &str = "SendUserMessage";

/// Legacy alias for backward compatibility.
pub const LEGACY_BRIEF_TOOL_NAME: &str = "Brief";

/// Description of the SendUserMessage tool.
pub const BRIEF_TOOL_DESCRIPTION: &str = "Send a message to the user";

/// Full prompt for the SendUserMessage tool.
pub const BRIEF_TOOL_PROMPT: &str = "\
Send a message the user will read. Text outside this tool is visible in \
the detail view, but most won't open it — the answer lives here.\n\
\n\
`message` supports markdown. `attachments` takes file paths (absolute or \
cwd-relative) for images, diffs, logs.\n\
\n\
`status` labels intent: 'normal' when replying to what they just asked; \
'proactive' when you're initiating — a scheduled task finished, a blocker \
surfaced during background work, you need input on something they haven't \
asked about. Set it honestly; downstream routing uses it.";

/// The system prompt section explaining brief-mode output rules.
///
/// Injected when brief mode is active and proactive mode is not
/// (proactive mode includes this inline in getProactiveSection).
pub const BRIEF_PROACTIVE_SECTION: &str = "\
## Talking to the user\n\
\n\
SendUserMessage is where your replies go. Text outside it is visible if \
the user expands the detail view, but most won't — assume unread. Anything \
you want them to actually see goes through SendUserMessage. The failure \
mode: the real answer lives in plain text while SendUserMessage just says \
\"done!\" — they see \"done!\" and miss everything.\n\
\n\
So: every time the user says something, the reply they actually read comes \
through SendUserMessage. Even for \"hi\". Even for \"thanks\".\n\
\n\
If you can answer right away, send the answer. If you need to go look — run \
a command, read files, check something — ack first in one line (\"On it — \
checking the test output\"), then work, then send the result. Without the \
ack they're staring at a spinner.\n\
\n\
For longer work: ack -> work -> result. Between those, send a checkpoint \
when something useful happened — a decision you made, a surprise you hit, \
a phase boundary. Skip the filler (\"running tests...\") — a checkpoint \
earns its place by carrying information.\n\
\n\
Keep messages tight — the decision, the file:line, the PR number. Second \
person always (\"your config\"), never third.";

/// Format a brief message for display in the detail view.
pub fn format_brief_result(_message: &str, attachment_count: usize) -> String {
    if attachment_count == 0 {
        "Message delivered to user.".to_string()
    } else {
        let plural = if attachment_count == 1 {
            "attachment"
        } else {
            "attachments"
        };
        format!("Message delivered to user. ({attachment_count} {plural} included)")
    }
}

/// Format content for brief mode: strip verbose output, keep it ultra-concise.
///
/// In brief mode, the model's text output is hidden from the user (they only
/// see SendUserMessage calls). This formats content for the detail/debug view.
pub fn format_brief_output(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // In brief mode, keep detail-view text minimal. Truncate long outputs
    // to a reasonable preview length.
    const MAX_BRIEF_DETAIL_CHARS: usize = 500;
    if trimmed.len() <= MAX_BRIEF_DETAIL_CHARS {
        trimmed.to_string()
    } else {
        let mut truncated = trimmed[..MAX_BRIEF_DETAIL_CHARS].to_string();
        truncated.push_str("...");
        truncated
    }
}

/// Check whether the given assistant state means brief mode is active.
pub fn is_brief_mode(state: &AssistantState) -> bool {
    state.is_brief_only()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_brief_status_default() {
        let status = BriefStatus::default();
        assert_eq!(status, BriefStatus::Normal);
    }

    #[test]
    fn test_format_brief_result() {
        assert_eq!(format_brief_result("hi", 0), "Message delivered to user.");
        assert_eq!(
            format_brief_result("hi", 1),
            "Message delivered to user. (1 attachment included)"
        );
        assert_eq!(
            format_brief_result("hi", 3),
            "Message delivered to user. (3 attachments included)"
        );
    }

    #[test]
    fn test_brief_message_serde() {
        let msg = BriefMessage {
            message: "Hello".to_string(),
            attachments: vec![],
            status: BriefStatus::Proactive,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"proactive\""));
        let parsed: BriefMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, BriefStatus::Proactive);
    }
}
