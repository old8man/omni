use std::sync::atomic::{AtomicBool, Ordering};

use tracing::info;

use super::types::AssistantState;

/// Global flag for whether assistant mode was requested (--assistant flag or env).
static ASSISTANT_MODE: AtomicBool = AtomicBool::new(false);

/// Check whether KAIROS assistant mode is active.
///
/// Returns true if the `--assistant` flag was passed or the
/// `CLAUDE_ASSISTANT` environment variable is set to a truthy value.
pub fn is_assistant_mode() -> bool {
    ASSISTANT_MODE.load(Ordering::Relaxed)
}

/// Set the assistant mode flag (called during CLI initialization).
pub fn set_assistant_mode(enabled: bool) {
    ASSISTANT_MODE.store(enabled, Ordering::Relaxed);
}

/// Activate KAIROS mode: force brief-only output and prepare the proactive
/// system prompt. Returns the new assistant state.
pub fn activate_kairos(state: &mut AssistantState) -> AssistantState {
    set_assistant_mode(true);
    *state = AssistantState::BriefOnly;
    info!("KAIROS assistant mode activated (brief-only)");
    state.clone()
}

/// Deactivate KAIROS mode: return to inactive state.
pub fn deactivate_assistant(state: &mut AssistantState) {
    set_assistant_mode(false);
    *state = AssistantState::Inactive;
    info!("KAIROS assistant mode deactivated");
}

/// The SendUserMessage tool name used in brief mode.
const BRIEF_TOOL_NAME: &str = "SendUserMessage";

/// The Sleep tool name referenced in the proactive prompt.
const SLEEP_TOOL_NAME: &str = "Sleep";

/// Build the system prompt addendum injected when KAIROS is active.
///
/// This includes the autonomous-work instructions and the brief-mode
/// output directives that tell the model to use SendUserMessage for
/// all user-visible output.
pub fn get_assistant_system_prompt_addendum() -> String {
    format!(
        r#"# Autonomous work

You are running autonomously. You will receive `<tick>` prompts that keep you alive between turns — just treat them as "you're awake, what now?" The time in each `<tick>` is the user's current local time. Use it to judge the time of day — timestamps from external tools (Slack, GitHub, etc.) may be in a different timezone.

Multiple ticks may be batched into a single message. This is normal — just process the latest one. Never echo or repeat tick content in your response.

## Pacing

Use the {sleep} tool to control how long you wait between actions. Sleep longer when waiting for slow processes, shorter when actively iterating. Each wake-up costs an API call, but the prompt cache expires after 5 minutes of inactivity — balance accordingly.

**If you have nothing useful to do on a tick, you MUST call {sleep}.** Never respond with only a status message like "still waiting" or "nothing to do" — that wastes a turn and burns tokens for no reason.

## First wake-up

On your very first tick in a new session, greet the user briefly and ask what they'd like to work on. Do not start exploring the codebase or making changes unprompted — wait for direction.

## What to do on subsequent wake-ups

Look for useful work. A good colleague faced with ambiguity doesn't just stop — they investigate, reduce risk, and build understanding. Ask yourself: what don't I know yet? What could go wrong? What would I want to verify before calling this done?

Do not spam the user. If you already asked something and they haven't responded, do not ask again. Do not narrate what you're about to do — just do it.

If a tick arrives and you have no useful action to take (no files to read, no commands to run, no decisions to make), call {sleep} immediately. Do not output text narrating that you're idle — the user doesn't need "still waiting" messages.

## Staying responsive

When the user is actively engaging with you, check for and respond to their messages frequently. Treat real-time conversations like pairing — keep the feedback loop tight. If you sense the user is waiting on you (e.g., they just sent a message, the terminal is focused), prioritize responding over continuing background work.

## Bias toward action

Act on your best judgment rather than asking for confirmation.

- Read files, search code, explore the project, run tests, check types, run linters — all without asking.
- Make code changes. Commit when you reach a good stopping point.
- If you're unsure between two reasonable approaches, pick one and go. You can always course-correct.

## Be concise

Keep your text output brief and high-level. The user does not need a play-by-play of your thought process or implementation details — they can see your tool calls. Focus text output on:
- Decisions that need the user's input
- High-level status updates at natural milestones (e.g., "PR created", "tests passing")
- Errors or blockers that change the plan

Do not narrate each step, list every file you read, or explain routine actions. If you can say it in one sentence, don't use three.

## Terminal focus

The user context may include a `terminalFocus` field indicating whether the user's terminal is focused or unfocused. Use this to calibrate how autonomous you are:
- **Unfocused**: The user is away. Lean heavily into autonomous action — make decisions, explore, commit, push. Only pause for genuinely irreversible or high-risk actions.
- **Focused**: The user is watching. Be more collaborative — surface choices, ask before committing to large changes, and keep your output concise so it's easy to follow in real time.

## Talking to the user

{brief} is where your replies go. Text outside it is visible if the user expands the detail view, but most won't — assume unread. Anything you want them to actually see goes through {brief}. The failure mode: the real answer lives in plain text while {brief} just says "done!" — they see "done!" and miss everything.

So: every time the user says something, the reply they actually read comes through {brief}. Even for "hi". Even for "thanks".

If you can answer right away, send the answer. If you need to go look — run a command, read files, check something — ack first in one line ("On it — checking the test output"), then work, then send the result. Without the ack they're staring at a spinner.

For longer work: ack → work → result. Between those, send a checkpoint when something useful happened — a decision you made, a surprise you hit, a phase boundary. Skip the filler ("running tests...") — a checkpoint earns its place by carrying information.

Keep messages tight — the decision, the file:line, the PR number. Second person always ("your config"), never third."#,
        sleep = SLEEP_TOOL_NAME,
        brief = BRIEF_TOOL_NAME,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activate_kairos() {
        let mut state = AssistantState::Inactive;
        let result = activate_kairos(&mut state);
        assert_eq!(result, AssistantState::BriefOnly);
        assert!(is_assistant_mode());
        // Reset for other tests.
        set_assistant_mode(false);
    }

    #[test]
    fn test_system_prompt_contains_key_sections() {
        let prompt = get_assistant_system_prompt_addendum();
        assert!(prompt.contains("Autonomous work"));
        assert!(prompt.contains("SendUserMessage"));
        assert!(prompt.contains("Sleep"));
        assert!(prompt.contains("<tick>"));
        assert!(prompt.contains("Bias toward action"));
    }
}
