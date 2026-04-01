//! Built-in skills shipped with claude-rs.
//!
//! These mirror the bundled skills from the original Claude Code TypeScript
//! implementation. Each skill provides a prompt template that gets injected
//! into the conversation when invoked.

use super::types::Skill;

const SIMPLIFY_PROMPT: &str = r#"# Simplify: Code Review and Cleanup

Review all changed files for reuse, quality, and efficiency. Fix any issues found.

## Phase 1: Identify Changes

Run `git diff` (or `git diff HEAD` if there are staged changes) to see what changed. If there are no git changes, review the most recently modified files that the user mentioned or that you edited earlier in this conversation.

## Phase 2: Review

For each change, check:

### Code Reuse
1. Search for existing utilities and helpers that could replace newly written code
2. Flag any new function that duplicates existing functionality
3. Flag any inline logic that could use an existing utility

### Code Quality
1. Redundant state that duplicates existing state
2. Parameter sprawl — adding new parameters instead of restructuring
3. Copy-paste with slight variation — near-duplicate code blocks
4. Leaky abstractions — exposing internal details
5. Unnecessary comments — comments explaining WHAT instead of WHY

### Efficiency
1. Unnecessary work — redundant computations, repeated file reads
2. Missed concurrency — independent operations run sequentially
3. Hot-path bloat — blocking work added to startup or hot paths
4. Memory — unbounded data structures, missing cleanup

## Phase 3: Fix Issues

Fix each issue directly. If a finding is a false positive, skip it.
When done, briefly summarize what was fixed (or confirm the code was already clean).
"#;

const UPDATE_CONFIG_PROMPT: &str = r#"# Update Config Skill

Modify Claude Code configuration by updating settings.json files.

## When Hooks Are Required (Not Memory)

If the user wants something to happen automatically in response to an EVENT, they need a **hook** configured in settings.json.

## CRITICAL: Read Before Write

**Always read the existing settings file before making changes.** Merge new settings with existing ones — never replace the entire file.

## Settings File Locations

| File | Scope | Use For |
|------|-------|---------|
| `~/.claude/settings.json` | Global | Personal preferences for all projects |
| `.claude/settings.json` | Project | Team-wide hooks, permissions, plugins |
| `.claude/settings.local.json` | Project | Personal overrides for this project |

## Common Mistakes to Avoid

1. **Replacing instead of merging** — Always preserve existing settings
2. **Wrong file** — Ask user if scope is unclear
3. **Invalid JSON** — Validate syntax after changes
4. **Forgetting to read first** — Always read before write
"#;

const KEYBINDINGS_PROMPT: &str = r#"# Keybindings Skill

Create or modify `~/.claude/keybindings.json` to customize keyboard shortcuts.

## CRITICAL: Read Before Write

**Always read `~/.claude/keybindings.json` first** (it may not exist yet). Merge changes with existing bindings — never replace the entire file.

## Keystroke Syntax

**Modifiers** (combine with `+`): `ctrl`, `alt`, `shift`, `meta`/`cmd`

**Special keys**: `escape`, `enter`, `tab`, `space`, `backspace`, `delete`, `up`, `down`, `left`, `right`

**Chords**: Space-separated keystrokes, e.g. `ctrl+k ctrl+s`

## Available Shortcuts

### Global
- Ctrl+C / Ctrl+D — Quit
- Escape — Cancel / close dialog

### Input
- Enter — Submit prompt
- Shift+Enter — New line
- Up/Down — Navigate history

### Navigation
- Ctrl+Up/Down — Scroll messages
- Page Up/Down — Scroll by page

### Commands
- / — Start a slash command
- /help — Show all commands
"#;

const CLAUDE_API_PROMPT: &str = r#"# Claude API Assistance

Help the user build applications with the Claude API or Anthropic SDK.

TRIGGER when: code imports `anthropic`/`@anthropic-ai/sdk`/`claude_agent_sdk`, or user asks to use Claude API, Anthropic SDKs, or Agent SDK.

## Key Resources
- Latest model family: Claude 4.5/4.6
- Model IDs: claude-opus-4-6, claude-sonnet-4-6, claude-haiku-4-5-20251001
- Max output tokens: 16384 (default), 64000 (extended thinking)

## Best Practices
- Use streaming for long responses
- Implement proper error handling with retries for rate limits
- Use system prompts for consistent behavior
- Prefer structured tool use over free-form text parsing
"#;

const LOOP_PROMPT: &str = r#"# Loop Skill

Run a prompt or slash command on a recurring interval.

Usage: /loop [interval] [command]
- Default interval: 10 minutes
- Example: /loop 5m /status — run /status every 5 minutes

## Execution

1. Execute the specified command or prompt
2. Wait for the specified interval
3. Repeat until stopped by the user

Report progress after each iteration. If an iteration fails, log the error and continue.
"#;

const SCHEDULE_PROMPT: &str = r#"# Schedule Skill

Create, update, list, or run scheduled remote agents (triggers) that execute on a cron schedule.

## Commands
- Create a new schedule: specify the task, cron expression, and any constraints
- List existing schedules
- Update or delete a schedule
- Run a scheduled task immediately

## Cron Expression Format
```
┌───── minute (0-59)
│ ┌───── hour (0-23)
│ │ ┌───── day of month (1-31)
│ │ │ ┌───── month (1-12)
│ │ │ │ ┌───── day of week (0-6, Sun=0)
│ │ │ │ │
* * * * *
```

Common patterns:
- `0 9 * * 1-5` — 9am weekdays
- `*/15 * * * *` — every 15 minutes
- `0 0 * * *` — midnight daily
"#;

const VERIFY_PROMPT: &str = r#"# Verify Skill

Verify that recent changes work correctly by running the project's test suite and checking for common issues.

## Steps

1. Identify the test framework and commands for this project
2. Run the relevant test suite
3. If tests fail, analyze the failures and fix them
4. Run linting/type checking if available
5. Report results
"#;

const DEBUG_PROMPT: &str = r#"# Debug Skill

Help diagnose and fix a bug or unexpected behavior.

## Steps

1. **Reproduce**: Understand and reproduce the issue
2. **Isolate**: Narrow down to the smallest failing case
3. **Diagnose**: Read relevant code, add logging if needed, trace the execution path
4. **Fix**: Apply the minimal fix that resolves the issue
5. **Verify**: Confirm the fix works and doesn't break other things
"#;

const REMEMBER_PROMPT: &str = r#"# Remember Skill

Save important context to CLAUDE.md memory files so it persists across conversations.

## Steps

1. Identify what the user wants to remember
2. Determine the appropriate CLAUDE.md file:
   - Project root `CLAUDE.md` for project-wide conventions
   - `~/.claude/CLAUDE.md` for personal preferences
3. Read the existing file content
4. Add the new information in the appropriate section
5. Write the updated file
"#;

const BATCH_PROMPT: &str = r#"# Batch Skill

Run a command or prompt across multiple inputs (files, directories, items).

## Usage

/batch [command] [inputs...]

## Steps

1. Parse the command template and input list
2. For each input, substitute it into the command and execute
3. Collect results and report successes/failures
4. If any execution fails, continue with the rest and report errors at the end

## Tips

- Use `{}` as a placeholder for the current input in the command template
- Inputs can be file paths, strings, or any other values
- Results are streamed as they complete
"#;

const SKILLIFY_PROMPT: &str = r#"# Skillify: Convert a Prompt into a Reusable Skill

Turn a user's prompt, workflow, or multi-step process into a reusable Claude Code skill file.

## Steps

1. **Understand** the user's prompt or workflow they want to convert
2. **Design** the skill structure:
   - Choose a clear, descriptive name (lowercase, hyphens)
   - Write a concise description
   - Identify when the skill should trigger (`when-to-use`)
   - Determine which tools the skill needs (`allowed-tools`)
   - Decide if it should be user-invocable (slash command)
3. **Write** the skill file with proper frontmatter:
   ```markdown
   ---
   name: skill-name
   description: What the skill does
   when-to-use: When to trigger this skill
   allowed-tools: [Tool1, Tool2]
   user-invocable: true
   argument-hint: "[optional args]"
   ---

   # Skill Title

   Skill body with instructions...
   ```
4. **Save** to the appropriate location:
   - `.claude/skills/skill-name/SKILL.md` for project skills
   - `~/.claude/skills/skill-name/SKILL.md` for personal skills
5. **Test** by invoking the skill
"#;

const LOREM_IPSUM_PROMPT: &str = r#"# Lorem Ipsum Generator

Generate placeholder text for testing and development.

When asked for lorem ipsum, placeholder text, or dummy content, generate the appropriate amount of text. Support different formats:

- **Paragraphs**: Standard lorem ipsum paragraphs
- **Words**: A specific word count of placeholder text
- **Sentences**: A specific number of sentences
- **Lists**: Bullet or numbered lists with placeholder items

Default to one paragraph if no amount is specified.
"#;

const STUCK_PROMPT: &str = r#"# Stuck Skill

When you're going in circles or keep hitting the same error, use this skill to break out.

## Steps

1. **Stop and reflect**: What have you tried so far? What keeps failing?
2. **Re-read the error**: Often the answer is in the error message. Read it carefully.
3. **Check assumptions**: List your assumptions. Which ones can you verify?
4. **Try a different approach**:
   - If fixing code: try reading more context, check git history
   - If a command fails: try a simpler version first
   - If tests fail: run just one test in isolation
5. **Simplify**: Remove complexity until you have something that works, then build back up.
"#;

/// Return all bundled skills.
pub fn get_bundled_skills() -> Vec<Skill> {
    vec![
        Skill::bundled(
            "simplify",
            "Review changed code for reuse, quality, and efficiency, then fix any issues found.",
            SIMPLIFY_PROMPT,
        )
        .with_user_invocable(None),
        Skill::bundled(
            "update-config",
            "Configure Claude Code via settings.json. Use for hooks, permissions, env vars, and settings changes.",
            UPDATE_CONFIG_PROMPT,
        )
        .with_allowed_tools(vec!["Read".to_string(), "Write".to_string()])
        .with_user_invocable(None),
        Skill::bundled(
            "keybindings-help",
            "Customize keyboard shortcuts, rebind keys, or modify ~/.claude/keybindings.json.",
            KEYBINDINGS_PROMPT,
        )
        .with_when_to_use("When the user wants to customize keyboard shortcuts, rebind keys, add chord bindings, or modify ~/.claude/keybindings.json")
        .with_allowed_tools(vec!["Read".to_string()]),
        Skill::bundled(
            "claude-api",
            "Build apps with the Claude API or Anthropic SDK.",
            CLAUDE_API_PROMPT,
        )
        .with_when_to_use("When code imports anthropic/@anthropic-ai/sdk, or user asks to use Claude API or Anthropic SDKs"),
        Skill::bundled(
            "loop",
            "Run a prompt or slash command on a recurring interval.",
            LOOP_PROMPT,
        )
        .with_when_to_use("When the user wants to set up a recurring task or run something repeatedly on an interval")
        .with_user_invocable(Some("[interval] [command]")),
        Skill::bundled(
            "schedule",
            "Create, update, list, or run scheduled remote agents on a cron schedule.",
            SCHEDULE_PROMPT,
        )
        .with_when_to_use("When the user wants to schedule a recurring remote agent or manage scheduled agents")
        .with_user_invocable(None),
        Skill::bundled(
            "verify",
            "Run tests and checks to verify recent changes work correctly.",
            VERIFY_PROMPT,
        )
        .with_user_invocable(None),
        Skill::bundled(
            "debug",
            "Diagnose and fix a bug or unexpected behavior.",
            DEBUG_PROMPT,
        )
        .with_user_invocable(None),
        Skill::bundled(
            "remember",
            "Save important context to CLAUDE.md memory files.",
            REMEMBER_PROMPT,
        )
        .with_when_to_use("When the user asks to remember something or save context for future conversations")
        .with_user_invocable(None),
        Skill::bundled(
            "stuck",
            "Break out of a loop when you keep hitting the same error.",
            STUCK_PROMPT,
        )
        .with_when_to_use("When you are going in circles or keep hitting the same error"),
        Skill::bundled(
            "batch",
            "Run a command or prompt across multiple inputs in bulk.",
            BATCH_PROMPT,
        )
        .with_when_to_use(
            "When the user wants to run the same command on multiple files or inputs",
        )
        .with_user_invocable(Some("[command] [inputs...]")),
        Skill::bundled(
            "skillify",
            "Convert a prompt or workflow into a reusable Claude Code skill.",
            SKILLIFY_PROMPT,
        )
        .with_when_to_use(
            "When the user wants to save a prompt as a reusable skill",
        )
        .with_allowed_tools(vec!["Read".to_string(), "Write".to_string()])
        .with_user_invocable(None),
        Skill::bundled(
            "loremIpsum",
            "Generate placeholder lorem ipsum text for testing.",
            LOREM_IPSUM_PROMPT,
        )
        .with_when_to_use("When the user needs placeholder or dummy text"),
    ]
}
