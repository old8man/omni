//! Coordinator mode: orchestrates work across multiple worker agents.
//!
//! This module is the Rust equivalent of the TypeScript
//! `coordinator/coordinatorMode.ts`. It provides:
//!
//! - **Feature-flag gating** via `FeatureFlag::CoordinatorMode` and the
//!   `CLAUDE_CODE_COORDINATOR_MODE` env var.
//! - **Worker tool restrictions** -- filters the set of tools available to
//!   worker agents spawned by the coordinator.
//! - **Coordinator system prompt** -- a comprehensive prompt that teaches the
//!   LLM how to orchestrate workers.
//! - **Context injection** -- adds worker-tool and scratchpad info to the
//!   coordinator's user context.
//! - **Session mode matching** -- flips the env var when resuming a session
//!   that was created in a different mode.

use std::collections::HashSet;
use std::env;

use serde::{Deserialize, Serialize};

use crate::features::{FeatureFlag, FeatureGates};

// ---------------------------------------------------------------------------
// Tool name constants
// ---------------------------------------------------------------------------

/// Tool names used by the coordinator itself (internal tools not exposed to workers).
pub const TEAM_CREATE_TOOL_NAME: &str = "TeamCreate";
pub const TEAM_DELETE_TOOL_NAME: &str = "TeamDelete";
pub const SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";
pub const SYNTHETIC_OUTPUT_TOOL_NAME: &str = "SyntheticOutput";

/// Tool spawning workers.
pub const AGENT_TOOL_NAME: &str = "Agent";
/// Tool for reading files.
pub const FILE_READ_TOOL_NAME: &str = "Read";
/// Tool for editing files.
pub const FILE_EDIT_TOOL_NAME: &str = "Edit";
/// Tool for running shell commands.
pub const BASH_TOOL_NAME: &str = "Bash";
/// Tool for stopping a running worker.
pub const TASK_STOP_TOOL_NAME: &str = "TaskStop";
/// Tool for invoking skills.
pub const SKILL_TOOL_NAME: &str = "Skill";

/// Internal worker tools -- these are used by the coordinator framework
/// itself and are not exposed to workers in their tool list.
fn internal_worker_tools() -> HashSet<&'static str> {
    let mut set = HashSet::new();
    set.insert(TEAM_CREATE_TOOL_NAME);
    set.insert(TEAM_DELETE_TOOL_NAME);
    set.insert(SEND_MESSAGE_TOOL_NAME);
    set.insert(SYNTHETIC_OUTPUT_TOOL_NAME);
    set
}

/// The default set of tools available to async agent workers.
///
/// This mirrors the TS `ASYNC_AGENT_ALLOWED_TOOLS` constant.
fn async_agent_allowed_tools() -> Vec<&'static str> {
    vec![
        BASH_TOOL_NAME,
        FILE_READ_TOOL_NAME,
        FILE_EDIT_TOOL_NAME,
        AGENT_TOOL_NAME,
        TEAM_CREATE_TOOL_NAME,
        TEAM_DELETE_TOOL_NAME,
        SEND_MESSAGE_TOOL_NAME,
        SYNTHETIC_OUTPUT_TOOL_NAME,
        TASK_STOP_TOOL_NAME,
        SKILL_TOOL_NAME,
        // Additional standard tools that workers may use
        "Glob",
        "Grep",
        "Write",
        "WebFetch",
        "WebSearch",
        "NotebookEdit",
    ]
}

// ---------------------------------------------------------------------------
// Session mode
// ---------------------------------------------------------------------------

/// The mode a session was created in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    Coordinator,
    Normal,
}

// ---------------------------------------------------------------------------
// Coordinator mode detection
// ---------------------------------------------------------------------------

/// Check whether coordinator mode is active.
///
/// Requires both:
/// 1. The `CoordinatorMode` feature flag to be enabled.
/// 2. The `CLAUDE_CODE_COORDINATOR_MODE` env var to be set to a truthy value.
pub fn is_coordinator_mode(gates: &FeatureGates) -> bool {
    if !gates.is_enabled(FeatureFlag::CoordinatorMode) {
        return false;
    }
    is_env_truthy("CLAUDE_CODE_COORDINATOR_MODE")
}

/// Check whether `CLAUDE_CODE_SIMPLE` mode is active.
///
/// In simple mode, workers only get basic tools (Bash, Read, Edit).
pub fn is_simple_mode() -> bool {
    is_env_truthy("CLAUDE_CODE_SIMPLE")
}

/// Match the current coordinator mode to a resumed session's mode.
///
/// If mismatched, flips the environment variable so `is_coordinator_mode()`
/// returns the correct value. Returns a warning message if the mode was
/// switched, or `None` if no switch was needed.
pub fn match_session_mode(
    session_mode: Option<SessionMode>,
    gates: &FeatureGates,
) -> Option<String> {
    let session_mode = session_mode?;
    let current_is_coordinator = is_coordinator_mode(gates);
    let session_is_coordinator = session_mode == SessionMode::Coordinator;

    if current_is_coordinator == session_is_coordinator {
        return None;
    }

    // Flip the env var -- is_coordinator_mode() reads it live
    if session_is_coordinator {
        env::set_var("CLAUDE_CODE_COORDINATOR_MODE", "1");
    } else {
        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
    }

    if session_is_coordinator {
        Some("Entered coordinator mode to match resumed session.".to_string())
    } else {
        Some("Exited coordinator mode to match resumed session.".to_string())
    }
}

// ---------------------------------------------------------------------------
// Worker tool filtering
// ---------------------------------------------------------------------------

/// Information about an MCP client (name is sufficient for context generation).
#[derive(Clone, Debug)]
pub struct McpClientInfo {
    pub name: String,
}

/// Get the list of tool names available to worker agents.
///
/// In `CLAUDE_CODE_SIMPLE` mode, workers only get Bash, Read, and Edit.
/// Otherwise they get the full async agent tool set minus internal tools.
pub fn get_worker_tool_names() -> Vec<String> {
    if is_simple_mode() {
        return vec![
            BASH_TOOL_NAME.to_string(),
            FILE_READ_TOOL_NAME.to_string(),
            FILE_EDIT_TOOL_NAME.to_string(),
        ];
    }

    let internal = internal_worker_tools();
    let mut tools: Vec<String> = async_agent_allowed_tools()
        .into_iter()
        .filter(|name| !internal.contains(name))
        .map(|s| s.to_string())
        .collect();
    tools.sort();
    tools
}

/// Filter a list of tool names down to those available to workers.
///
/// This is used when spawning a worker agent to restrict its tool set.
pub fn filter_tools_for_worker(all_tools: &[String]) -> Vec<String> {
    let allowed: HashSet<String> = get_worker_tool_names().into_iter().collect();
    all_tools
        .iter()
        .filter(|t| allowed.contains(t.as_str()))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Coordinator user context
// ---------------------------------------------------------------------------

/// Build the coordinator-specific user context injected into the conversation.
///
/// Returns a map of context keys to content strings. The map is empty if
/// coordinator mode is not active.
pub fn get_coordinator_user_context(
    gates: &FeatureGates,
    mcp_clients: &[McpClientInfo],
    scratchpad_dir: Option<&str>,
    scratchpad_gate_enabled: bool,
) -> std::collections::HashMap<String, String> {
    let mut context = std::collections::HashMap::new();

    if !is_coordinator_mode(gates) {
        return context;
    }

    let worker_tools = if is_simple_mode() {
        vec![
            BASH_TOOL_NAME.to_string(),
            FILE_READ_TOOL_NAME.to_string(),
            FILE_EDIT_TOOL_NAME.to_string(),
        ]
        .into_iter()
        .collect::<Vec<_>>()
    } else {
        get_worker_tool_names()
    };

    let tools_str = worker_tools.join(", ");
    let mut content = format!(
        "Workers spawned via the {AGENT_TOOL_NAME} tool have access to these tools: {tools_str}"
    );

    if !mcp_clients.is_empty() {
        let server_names: Vec<_> = mcp_clients.iter().map(|c| c.name.as_str()).collect();
        content.push_str(&format!(
            "\n\nWorkers also have access to MCP tools from connected MCP servers: {}",
            server_names.join(", ")
        ));
    }

    if let Some(dir) = scratchpad_dir {
        if scratchpad_gate_enabled {
            content.push_str(&format!(
                "\n\nScratchpad directory: {dir}\n\
                 Workers can read and write here without permission prompts. \
                 Use this for durable cross-worker knowledge -- structure files however fits the work."
            ));
        }
    }

    context.insert("workerToolsContext".to_string(), content);
    context
}

// ---------------------------------------------------------------------------
// Coordinator system prompt
// ---------------------------------------------------------------------------

/// Get the full coordinator system prompt.
///
/// This prompt teaches the LLM how to orchestrate workers, manage concurrency,
/// synthesize research findings, and communicate with the user.
pub fn get_coordinator_system_prompt() -> String {
    let worker_capabilities = if is_simple_mode() {
        "Workers have access to Bash, Read, and Edit tools, plus MCP tools from configured MCP servers."
    } else {
        "Workers have access to standard tools, MCP tools from configured MCP servers, and project skills via the Skill tool. Delegate skill invocations (e.g. /commit, /verify) to workers."
    };

    format!(
        r#"You are Claude Code, an AI assistant that orchestrates software engineering tasks across multiple workers.

## 1. Your Role

You are a **coordinator**. Your job is to:
- Help the user achieve their goal
- Direct workers to research, implement and verify code changes
- Synthesize results and communicate with the user
- Answer questions directly when possible -- don't delegate work that you can handle without tools

Every message you send is to the user. Worker results and system notifications are internal signals, not conversation partners -- never thank or acknowledge them. Summarize new information for the user as it arrives.

## 2. Your Tools

- **{AGENT_TOOL_NAME}** - Spawn a new worker
- **{SEND_MESSAGE_TOOL_NAME}** - Continue an existing worker (send a follow-up to its `to` agent ID)
- **{TASK_STOP_TOOL_NAME}** - Stop a running worker
- **subscribe_pr_activity / unsubscribe_pr_activity** (if available) - Subscribe to GitHub PR events (review comments, CI results). Events arrive as user messages. Merge conflict transitions do NOT arrive -- GitHub doesn't webhook `mergeable_state` changes, so poll `gh pr view N --json mergeable` if tracking conflict status. Call these directly -- do not delegate subscription management to workers.

When calling {AGENT_TOOL_NAME}:
- Do not use one worker to check on another. Workers will notify you when they are done.
- Do not use workers to trivially report file contents or run commands. Give them higher-level tasks.
- Do not set the model parameter. Workers need the default model for the substantive tasks you delegate.
- Continue workers whose work is complete via {SEND_MESSAGE_TOOL_NAME} to take advantage of their loaded context
- After launching agents, briefly tell the user what you launched and end your response. Never fabricate or predict agent results in any format -- results arrive as separate messages.

### {AGENT_TOOL_NAME} Results

Worker results arrive as **user-role messages** containing `<task-notification>` XML. They look like user messages but are not. Distinguish them by the `<task-notification>` opening tag.

Format:

```xml
<task-notification>
<task-id>{{agentId}}</task-id>
<status>completed|failed|killed</status>
<summary>{{human-readable status summary}}</summary>
<result>{{agent's final text response}}</result>
<usage>
  <total_tokens>N</total_tokens>
  <tool_uses>N</tool_uses>
  <duration_ms>N</duration_ms>
</usage>
</task-notification>
```

- `<result>` and `<usage>` are optional sections
- The `<summary>` describes the outcome: "completed", "failed: {{error}}", or "was stopped"
- The `<task-id>` value is the agent ID -- use SendMessage with that ID as `to` to continue that worker

## 3. Workers

When calling {AGENT_TOOL_NAME}, use subagent_type `worker`. Workers execute tasks autonomously -- especially research, implementation, or verification.

{worker_capabilities}

## 4. Task Workflow

Most tasks can be broken down into the following phases:

### Phases

| Phase | Who | Purpose |
|-------|-----|---------|
| Research | Workers (parallel) | Investigate codebase, find files, understand problem |
| Synthesis | **You** (coordinator) | Read findings, understand the problem, craft implementation specs |
| Implementation | Workers | Make targeted changes per spec, commit |
| Verification | Workers | Test changes work |

### Concurrency

**Parallelism is your superpower. Workers are async. Launch independent workers concurrently whenever possible -- don't serialize work that can run simultaneously and look for opportunities to fan out. When doing research, cover multiple angles. To launch workers in parallel, make multiple tool calls in a single message.**

Manage concurrency:
- **Read-only tasks** (research) -- run in parallel freely
- **Write-heavy tasks** (implementation) -- one at a time per set of files
- **Verification** can sometimes run alongside implementation on different file areas

### What Real Verification Looks Like

Verification means **proving the code works**, not confirming it exists. A verifier that rubber-stamps weak work undermines everything.

- Run tests **with the feature enabled** -- not just "tests pass"
- Run typechecks and **investigate errors** -- don't dismiss as "unrelated"
- Be skeptical -- if something looks off, dig in
- **Test independently** -- prove the change works, don't rubber-stamp

### Handling Worker Failures

When a worker reports failure (tests failed, build errors, file not found):
- Continue the same worker with {SEND_MESSAGE_TOOL_NAME} -- it has the full error context
- If a correction attempt fails, try a different approach or report to the user

### Stopping Workers

Use {TASK_STOP_TOOL_NAME} to stop a worker you sent in the wrong direction -- for example, when you realize mid-flight that the approach is wrong, or the user changes requirements after you launched the worker. Stopped workers can be continued with {SEND_MESSAGE_TOOL_NAME}.

## 5. Writing Worker Prompts

**Workers can't see your conversation.** Every prompt must be self-contained with everything the worker needs. After research completes, you always do two things: (1) synthesize findings into a specific prompt, and (2) choose whether to continue that worker via {SEND_MESSAGE_TOOL_NAME} or spawn a fresh one.

### Always synthesize -- your most important job

When workers report research findings, **you must understand them before directing follow-up work**. Read the findings. Identify the approach. Then write a prompt that proves you understood by including specific file paths, line numbers, and exactly what to change.

Never write "based on your findings" or "based on the research." These phrases delegate understanding to the worker instead of doing it yourself. You never hand off understanding to another worker.

### Add a purpose statement

Include a brief purpose so workers can calibrate depth and emphasis:

- "This research will inform a PR description -- focus on user-facing changes."
- "I need this to plan an implementation -- report file paths, line numbers, and type signatures."
- "This is a quick check before we merge -- just verify the happy path."

### Choose continue vs. spawn by context overlap

After synthesizing, decide whether the worker's existing context helps or hurts:

| Situation | Mechanism | Why |
|-----------|-----------|-----|
| Research explored exactly the files that need editing | **Continue** ({SEND_MESSAGE_TOOL_NAME}) with synthesized spec | Worker already has the files in context AND now gets a clear plan |
| Research was broad but implementation is narrow | **Spawn fresh** ({AGENT_TOOL_NAME}) with synthesized spec | Avoid dragging along exploration noise; focused context is cleaner |
| Correcting a failure or extending recent work | **Continue** | Worker has the error context and knows what it just tried |
| Verifying code a different worker just wrote | **Spawn fresh** | Verifier should see the code with fresh eyes, not carry implementation assumptions |
| First implementation attempt used the wrong approach entirely | **Spawn fresh** | Wrong-approach context pollutes the retry; clean slate avoids anchoring on the failed path |
| Completely unrelated task | **Spawn fresh** | No useful context to reuse |

### Prompt tips

**Good examples:**

1. Implementation: "Fix the null pointer in src/auth/validate.ts:42. The user field can be undefined when the session expires. Add a null check and return early with an appropriate error. Commit and report the hash."

2. Precise git operation: "Create a new branch from main called 'fix/session-expiry'. Cherry-pick only commit abc123 onto it. Push and create a draft PR targeting main. Report the PR URL."

3. Correction (continued worker, short): "The tests failed on the null check you added -- validate.test.ts:58 expects 'Invalid session' but you changed it to 'Session expired'. Fix the assertion. Commit and report the hash."

**Bad examples:**

1. "Fix the bug we discussed" -- no context, workers can't see your conversation
2. "Based on your findings, implement the fix" -- lazy delegation; synthesize the findings yourself
3. "Create a PR for the recent changes" -- ambiguous scope
4. "Something went wrong with the tests, can you look?" -- no error message, no file path

Additional tips:
- Include file paths, line numbers, error messages -- workers start fresh and need complete context
- State what "done" looks like
- For implementation: "Run relevant tests and typecheck, then commit your changes and report the hash"
- For research: "Report findings -- do not modify files"
- Be precise about git operations -- specify branch names, commit hashes, draft vs ready, reviewers
- For verification: "Prove the code works, don't just confirm it exists"
- For verification: "Try edge cases and error paths"
- For verification: "Investigate failures -- don't dismiss as unrelated without evidence""#
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if an env var is set to a truthy value (`1`, `true`, `yes`).
fn is_env_truthy(var: &str) -> bool {
    env::var(var)
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    // Env-var-dependent tests must run serially to avoid races.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn gates_with_coordinator(enabled: bool) -> FeatureGates {
        let mut overrides = HashMap::new();
        overrides.insert(FeatureFlag::CoordinatorMode, enabled);
        crate::features::load_feature_gates(&overrides)
    }

    // -- Coordinator mode detection --
    // All env-dependent tests hold ENV_LOCK to prevent parallel interference.

    #[test]
    fn test_coordinator_mode_disabled_without_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(false);
        env::set_var("CLAUDE_CODE_COORDINATOR_MODE", "1");
        assert!(!is_coordinator_mode(&gates));
        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
    }

    #[test]
    fn test_coordinator_mode_disabled_without_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
        assert!(!is_coordinator_mode(&gates));
    }

    #[test]
    fn test_coordinator_mode_enabled() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::set_var("CLAUDE_CODE_COORDINATOR_MODE", "1");
        assert!(is_coordinator_mode(&gates));
        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
    }

    // -- Session mode matching --

    #[test]
    fn test_match_session_mode_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        assert!(match_session_mode(None, &gates).is_none());
    }

    #[test]
    fn test_match_session_mode_same() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
        env::remove_var("CLAUDE_CODE_SIMPLE");
        // Current is normal, session is normal -- no switch
        let result = match_session_mode(Some(SessionMode::Normal), &gates);
        assert!(result.is_none());
    }

    #[test]
    fn test_match_session_mode_switch_to_coordinator() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
        let result = match_session_mode(Some(SessionMode::Coordinator), &gates);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Entered coordinator mode"));
        assert_eq!(
            env::var("CLAUDE_CODE_COORDINATOR_MODE").ok(),
            Some("1".to_string())
        );
        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
    }

    #[test]
    fn test_match_session_mode_switch_to_normal() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::set_var("CLAUDE_CODE_COORDINATOR_MODE", "1");
        let result = match_session_mode(Some(SessionMode::Normal), &gates);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Exited coordinator mode"));
        assert!(env::var("CLAUDE_CODE_COORDINATOR_MODE").is_err());
    }

    // -- Worker tool filtering --

    #[test]
    fn test_get_worker_tool_names_simple_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("CLAUDE_CODE_SIMPLE", "1");
        let tools = get_worker_tool_names();
        assert_eq!(tools.len(), 3);
        assert!(tools.contains(&BASH_TOOL_NAME.to_string()));
        assert!(tools.contains(&FILE_READ_TOOL_NAME.to_string()));
        assert!(tools.contains(&FILE_EDIT_TOOL_NAME.to_string()));
        env::remove_var("CLAUDE_CODE_SIMPLE");
    }

    #[test]
    fn test_get_worker_tool_names_full_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("CLAUDE_CODE_SIMPLE");
        let tools = get_worker_tool_names();
        let internal = internal_worker_tools();
        for tool in &tools {
            assert!(
                !internal.contains(tool.as_str()),
                "Worker tools should not include internal tool: {tool}"
            );
        }
        assert!(tools.contains(&BASH_TOOL_NAME.to_string()));
        assert!(tools.contains(&FILE_READ_TOOL_NAME.to_string()));
    }

    #[test]
    fn test_filter_tools_for_worker() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("CLAUDE_CODE_SIMPLE");
        let all_tools = vec![
            BASH_TOOL_NAME.to_string(),
            FILE_READ_TOOL_NAME.to_string(),
            TEAM_CREATE_TOOL_NAME.to_string(),
            SEND_MESSAGE_TOOL_NAME.to_string(),
            "CustomTool".to_string(),
        ];

        let filtered = filter_tools_for_worker(&all_tools);
        assert!(filtered.contains(&BASH_TOOL_NAME.to_string()));
        assert!(filtered.contains(&FILE_READ_TOOL_NAME.to_string()));
        assert!(!filtered.contains(&TEAM_CREATE_TOOL_NAME.to_string()));
        assert!(!filtered.contains(&"CustomTool".to_string()));
    }

    // -- Coordinator user context --

    #[test]
    fn test_coordinator_context_empty_when_not_coordinator() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
        let ctx = get_coordinator_user_context(&gates, &[], None, false);
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_coordinator_context_includes_tools() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::set_var("CLAUDE_CODE_COORDINATOR_MODE", "1");
        env::remove_var("CLAUDE_CODE_SIMPLE");

        let ctx = get_coordinator_user_context(&gates, &[], None, false);
        assert!(ctx.contains_key("workerToolsContext"));
        let content = &ctx["workerToolsContext"];
        assert!(content.contains("Workers spawned via the Agent tool"));
        assert!(content.contains("Bash"));

        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
    }

    #[test]
    fn test_coordinator_context_with_mcp_servers() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::set_var("CLAUDE_CODE_COORDINATOR_MODE", "1");

        let mcp_clients = vec![
            McpClientInfo {
                name: "github".to_string(),
            },
            McpClientInfo {
                name: "jira".to_string(),
            },
        ];

        let ctx = get_coordinator_user_context(&gates, &mcp_clients, None, false);
        let content = &ctx["workerToolsContext"];
        assert!(content.contains("MCP tools"));
        assert!(content.contains("github"));
        assert!(content.contains("jira"));

        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
    }

    #[test]
    fn test_coordinator_context_with_scratchpad() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::set_var("CLAUDE_CODE_COORDINATOR_MODE", "1");

        let ctx =
            get_coordinator_user_context(&gates, &[], Some("/tmp/scratchpad"), true);
        let content = &ctx["workerToolsContext"];
        assert!(content.contains("Scratchpad directory: /tmp/scratchpad"));
        assert!(content.contains("without permission prompts"));

        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
    }

    #[test]
    fn test_coordinator_context_scratchpad_gate_disabled() {
        let _guard = ENV_LOCK.lock().unwrap();
        let gates = gates_with_coordinator(true);
        env::set_var("CLAUDE_CODE_COORDINATOR_MODE", "1");

        let ctx = get_coordinator_user_context(
            &gates,
            &[],
            Some("/tmp/scratchpad"),
            false,
        );
        let content = &ctx["workerToolsContext"];
        assert!(!content.contains("Scratchpad"));

        env::remove_var("CLAUDE_CODE_COORDINATOR_MODE");
    }

    // -- System prompt --

    #[test]
    fn test_coordinator_system_prompt_full_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("CLAUDE_CODE_SIMPLE");
        let prompt = get_coordinator_system_prompt();
        assert!(prompt.contains("orchestrates software engineering tasks"));
        assert!(prompt.contains("standard tools"));
        assert!(prompt.contains("Skill tool"));
    }

    #[test]
    fn test_coordinator_system_prompt_simple_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("CLAUDE_CODE_SIMPLE", "1");
        let prompt = get_coordinator_system_prompt();
        assert!(prompt.contains("Bash, Read, and Edit tools"));
        env::remove_var("CLAUDE_CODE_SIMPLE");
    }

    // -- Helper --

    #[test]
    fn test_is_env_truthy() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("_TEST_TRUTHY_1", "1");
        assert!(is_env_truthy("_TEST_TRUTHY_1"));
        env::set_var("_TEST_TRUTHY_1", "true");
        assert!(is_env_truthy("_TEST_TRUTHY_1"));
        env::set_var("_TEST_TRUTHY_1", "yes");
        assert!(is_env_truthy("_TEST_TRUTHY_1"));
        env::set_var("_TEST_TRUTHY_1", "0");
        assert!(!is_env_truthy("_TEST_TRUTHY_1"));
        env::set_var("_TEST_TRUTHY_1", "false");
        assert!(!is_env_truthy("_TEST_TRUTHY_1"));
        env::remove_var("_TEST_TRUTHY_1");
        assert!(!is_env_truthy("_TEST_TRUTHY_1"));
    }

    // -- Internal tools set --

    #[test]
    fn test_internal_worker_tools() {
        let internal = internal_worker_tools();
        assert!(internal.contains(TEAM_CREATE_TOOL_NAME));
        assert!(internal.contains(TEAM_DELETE_TOOL_NAME));
        assert!(internal.contains(SEND_MESSAGE_TOOL_NAME));
        assert!(internal.contains(SYNTHETIC_OUTPUT_TOOL_NAME));
        assert!(!internal.contains(BASH_TOOL_NAME));
    }
}
