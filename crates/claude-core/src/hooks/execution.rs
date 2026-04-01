//! Hook execution: running shell commands, HTTP hooks, and processing results.
//!
//! Mirrors `execCommandHook`, `execHttpHook`, `processHookJSONOutput`,
//! `executeHooks`, and `executeHooksOutsideREPL` from the TypeScript `hooks.ts`.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::future::join_all;
use serde_json::Value;
use tokio::process::Command as TokioCommand;
use tracing::{debug, warn};

use super::matching::get_matching_hooks;
use super::registry::HookRegistry;
use super::types::*;

/// Default timeout for tool-related hook execution (10 minutes).
const TOOL_HOOK_EXECUTION_TIMEOUT_MS: u64 = 10 * 60 * 1000;

/// Default timeout for SessionEnd hooks (1.5 seconds).
const SESSION_END_HOOK_TIMEOUT_MS: u64 = 1500;

/// Get the session end hook timeout, optionally overridden by env var.
pub fn get_session_end_hook_timeout_ms() -> u64 {
    std::env::var("CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(SESSION_END_HOOK_TIMEOUT_MS)
}

/// Execute all matching hooks for a given event, collect and aggregate results.
///
/// This is the main entry point for running hooks within the REPL loop.
/// Hooks are matched, deduplicated, run in parallel, and results aggregated.
pub async fn execute_hooks_for_event(
    registry: &HookRegistry,
    input: &HookInput,
    timeout_ms: Option<u64>,
) -> AggregatedHookResult {
    if registry.is_all_disabled() {
        return AggregatedHookResult::default();
    }

    let matching = get_matching_hooks(registry, input);
    if matching.is_empty() {
        return AggregatedHookResult::default();
    }

    let event = input.hook_event_name();
    let match_query = input.match_query();
    let hook_name = match match_query {
        Some(q) => format!("{event}:{q}"),
        None => event.to_string(),
    };

    let timeout = timeout_ms.unwrap_or(TOOL_HOOK_EXECUTION_TIMEOUT_MS);
    let batch_start = Instant::now();

    // Serialize hookInput once for all command/http hooks
    let json_input = match serde_json::to_string(input) {
        Ok(v) => v,
        Err(e) => {
            warn!("failed to serialize hook input for {hook_name}: {e}");
            return AggregatedHookResult::default();
        }
    };
    let input_value: Value = match serde_json::to_value(input) {
        Ok(v) => v,
        Err(e) => {
            warn!("failed to convert hook input to value for {hook_name}: {e}");
            return AggregatedHookResult::default();
        }
    };

    // Execute all hooks in parallel
    let futures: Vec<_> = matching
        .iter()
        .map(|hook_config| {
            let json_input = json_input.clone();
            let input_value = input_value.clone();
            let hook_name = hook_name.clone();
            async move {
                execute_single_hook(&hook_config.config, &json_input, &input_value, timeout, &hook_name, event).await
            }
        })
        .collect();

    let results = join_all(futures).await;

    // Aggregate results
    let mut aggregated = AggregatedHookResult::default();
    let mut permission_behavior: Option<PermissionBehavior> = None;

    for result in results {
        match result.outcome {
            HookOutcome::Success => aggregated.outcomes.success += 1,
            HookOutcome::Blocking => aggregated.outcomes.blocking += 1,
            HookOutcome::NonBlockingError => aggregated.outcomes.non_blocking_error += 1,
            HookOutcome::Cancelled => aggregated.outcomes.cancelled += 1,
        }

        if result.prevent_continuation {
            aggregated.prevent_continuation = true;
            if result.stop_reason.is_some() {
                aggregated.stop_reason = result.stop_reason;
            }
        }

        if let Some(err) = result.blocking_error {
            aggregated.blocking_errors.push(err);
        }

        if let Some(ctx) = result.additional_context {
            aggregated.additional_contexts.push(ctx);
        }

        if result.initial_user_message.is_some() {
            aggregated.initial_user_message = result.initial_user_message;
        }

        if let Some(paths) = result.watch_paths {
            aggregated.watch_paths.extend(paths);
        }

        if result.updated_mcp_tool_output.is_some() {
            aggregated.updated_mcp_tool_output = result.updated_mcp_tool_output;
        }

        if let Some(msg) = result.system_message {
            aggregated.system_messages.push(msg);
        }

        if result.retry.is_some() {
            aggregated.retry = result.retry;
        }

        if result.permission_request_result.is_some() {
            aggregated.permission_request_result = result.permission_request_result;
        }

        // Permission behavior precedence: deny > ask > allow
        if let Some(pb) = result.permission_behavior {
            match pb {
                PermissionBehavior::Deny => {
                    permission_behavior = Some(PermissionBehavior::Deny);
                }
                PermissionBehavior::Ask => {
                    if permission_behavior != Some(PermissionBehavior::Deny) {
                        permission_behavior = Some(PermissionBehavior::Ask);
                    }
                }
                PermissionBehavior::Allow => {
                    if permission_behavior.is_none() {
                        permission_behavior = Some(PermissionBehavior::Allow);
                    }
                }
                PermissionBehavior::Passthrough => {}
            }

            if result.hook_permission_decision_reason.is_some() {
                aggregated.hook_permission_decision_reason =
                    result.hook_permission_decision_reason;
            }
        }

        // Carry updatedInput for allow/ask behaviors
        if let Some(updated) = result.updated_input {
            if matches!(
                result.permission_behavior,
                Some(PermissionBehavior::Allow) | Some(PermissionBehavior::Ask) | None
            ) {
                aggregated.updated_input = Some(updated);
            }
        }
    }

    aggregated.permission_behavior = permission_behavior;

    let total_ms = batch_start.elapsed().as_millis() as u64;
    debug!(
        "{hook_name}: {} hooks in {total_ms}ms (success={}, blocking={}, error={}, cancelled={})",
        aggregated.outcomes.success
            + aggregated.outcomes.blocking
            + aggregated.outcomes.non_blocking_error
            + aggregated.outcomes.cancelled,
        aggregated.outcomes.success,
        aggregated.outcomes.blocking,
        aggregated.outcomes.non_blocking_error,
        aggregated.outcomes.cancelled,
    );

    aggregated
}

/// Execute hooks outside the REPL loop (e.g., notifications, session end, file changes).
///
/// Returns simplified results suitable for non-interactive use.
pub async fn execute_hooks_outside_repl(
    registry: &HookRegistry,
    input: &HookInput,
    timeout_ms: Option<u64>,
) -> Vec<HookOutsideReplResult> {
    if registry.is_all_disabled() {
        return Vec::new();
    }

    let matching = get_matching_hooks(registry, input);
    if matching.is_empty() {
        return Vec::new();
    }

    let event = input.hook_event_name();
    let match_query = input.match_query();
    let hook_name = match match_query {
        Some(q) => format!("{event}:{q}"),
        None => event.to_string(),
    };

    let timeout = timeout_ms.unwrap_or(TOOL_HOOK_EXECUTION_TIMEOUT_MS);

    let json_input = match serde_json::to_string(input) {
        Ok(v) => v,
        Err(e) => {
            warn!("failed to serialize hook input for {hook_name}: {e}");
            return Vec::new();
        }
    };
    let input_value: Value = match serde_json::to_value(input) {
        Ok(v) => v,
        Err(e) => {
            warn!("failed to convert hook input to value for {hook_name}: {e}");
            return Vec::new();
        }
    };

    let futures: Vec<_> = matching
        .iter()
        .map(|hook_config| {
            let json_input = json_input.clone();
            let input_value = input_value.clone();
            let hook_name = hook_name.clone();
            let command_display = hook_config.config.display_text().to_string();
            async move {
                let result = execute_single_hook(
                    &hook_config.config,
                    &json_input,
                    &input_value,
                    timeout,
                    &hook_name,
                    event,
                )
                .await;
                HookOutsideReplResult {
                    command: command_display,
                    succeeded: result.outcome == HookOutcome::Success,
                    output: if !result.stderr.is_empty() {
                        result.stderr
                    } else {
                        result.stdout
                    },
                    blocked: result.blocking_error.is_some(),
                    watch_paths: result.watch_paths.unwrap_or_default(),
                    system_message: result.system_message,
                }
            }
        })
        .collect();

    join_all(futures).await
}

/// Execute a single hook command and process its output.
async fn execute_single_hook(
    config: &HookCommand,
    json_input: &str,
    input_value: &Value,
    timeout_ms: u64,
    hook_name: &str,
    hook_event: HookEvent,
) -> HookResult {
    match config {
        HookCommand::Command {
            command,
            shell,
            timeout,
            ..
        } => {
            let effective_timeout = timeout.map(|t| t * 1000).unwrap_or(timeout_ms);
            match execute_shell_hook(command, shell, json_input, effective_timeout).await {
                Ok(raw) => process_command_result(raw, command, hook_name, hook_event),
                Err(e) => {
                    warn!("hook {hook_name} error: {e}");
                    HookResult {
                        stderr: format!("Failed to run: {e}"),
                        outcome: HookOutcome::NonBlockingError,
                        ..Default::default()
                    }
                }
            }
        }
        HookCommand::Http {
            url, timeout, ..
        } => {
            let effective_timeout = timeout.map(|t| t * 1000).unwrap_or(timeout_ms);
            match execute_http_hook(url, input_value, effective_timeout).await {
                Ok(raw) => process_http_result(raw, url, hook_name, hook_event),
                Err(e) => {
                    warn!("HTTP hook {hook_name} error: {e}");
                    HookResult {
                        stderr: format!("Failed to run: {e}"),
                        outcome: HookOutcome::NonBlockingError,
                        ..Default::default()
                    }
                }
            }
        }
        HookCommand::Prompt { .. } | HookCommand::Agent { .. } => {
            // Prompt and agent hooks require a full query context which isn't
            // available at this level. They are handled by higher-level code
            // that has access to the LLM. Return success as a no-op placeholder.
            debug!("prompt/agent hook {hook_name} executed as no-op (requires query context)");
            HookResult::default()
        }
    }
}

/// Raw result from a shell command execution.
struct RawCommandResult {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    aborted: bool,
}

/// Execute a shell command hook via the specified shell.
async fn execute_shell_hook(
    command: &str,
    shell: &str,
    json_input: &str,
    timeout_ms: u64,
) -> Result<RawCommandResult> {
    let timeout = Duration::from_millis(timeout_ms);
    debug!(command, shell, "executing shell hook");

    let mut child = TokioCommand::new(shell)
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn hook via {shell}"))?;

    // Write JSON input to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        // Trailing newline matches TS behavior so `read -r line` in bash works correctly
        let _ = stdin.write_all(json_input.as_bytes()).await;
        let _ = stdin.write_all(b"\n").await;
        drop(stdin);
    }

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let wait_result = tokio::time::timeout(timeout, async {
        let stdout_task = tokio::spawn(async move {
            if let Some(stdout) = stdout_handle {
                use tokio::io::AsyncReadExt;
                let mut buf = String::new();
                let _ = tokio::io::BufReader::new(stdout)
                    .read_to_string(&mut buf)
                    .await;
                buf
            } else {
                String::new()
            }
        });
        let stderr_task = tokio::spawn(async move {
            if let Some(stderr) = stderr_handle {
                use tokio::io::AsyncReadExt;
                let mut buf = String::new();
                let _ = tokio::io::BufReader::new(stderr)
                    .read_to_string(&mut buf)
                    .await;
                buf
            } else {
                String::new()
            }
        });
        let status = child.wait().await?;
        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        Ok::<_, anyhow::Error>((status, stdout, stderr))
    })
    .await;

    match wait_result {
        Ok(Ok((status, stdout, stderr))) => Ok(RawCommandResult {
            stdout,
            stderr,
            exit_code: status.code(),
            aborted: false,
        }),
        Ok(Err(e)) => anyhow::bail!("hook process error: {e}"),
        Err(_) => Ok(RawCommandResult {
            stdout: String::new(),
            stderr: format!("hook timed out after {timeout_ms}ms"),
            exit_code: None,
            aborted: true,
        }),
    }
}

/// Raw result from an HTTP hook call.
struct RawHttpResult {
    status_code: u16,
    body: String,
    ok: bool,
    error: Option<String>,
    aborted: bool,
}

/// Execute an HTTP hook by POSTing input JSON to the configured URL.
async fn execute_http_hook(url: &str, input_json: &Value, timeout_ms: u64) -> Result<RawHttpResult> {
    let timeout = Duration::from_millis(timeout_ms);
    let client = reqwest::Client::builder().timeout(timeout).build()?;

    debug!(url, "executing HTTP hook");

    match client.post(url).json(input_json).send().await {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Ok(RawHttpResult {
                status_code: status.as_u16(),
                body,
                ok: status.is_success(),
                error: None,
                aborted: false,
            })
        }
        Err(e) => {
            if e.is_timeout() {
                Ok(RawHttpResult {
                    status_code: 0,
                    body: String::new(),
                    ok: false,
                    error: None,
                    aborted: true,
                })
            } else {
                Ok(RawHttpResult {
                    status_code: 0,
                    body: String::new(),
                    ok: false,
                    error: Some(format!("{e}")),
                    aborted: false,
                })
            }
        }
    }
}

/// Process the raw result of a shell command hook into a HookResult.
///
/// Implements the same logic as `parseHookOutput` + `processHookJSONOutput` in TS:
/// - Exit code 0 with JSON stdout: parse JSON output and extract fields
/// - Exit code 0 with plain text: success with stdout content
/// - Exit code 2: blocking error
/// - Other non-zero: non-blocking error
/// - Aborted: cancelled
fn process_command_result(
    raw: RawCommandResult,
    command: &str,
    hook_name: &str,
    hook_event: HookEvent,
) -> HookResult {
    if raw.aborted {
        return HookResult {
            stdout: raw.stdout,
            stderr: raw.stderr,
            exit_code: raw.exit_code,
            outcome: HookOutcome::Cancelled,
            ..Default::default()
        };
    }

    // Try to parse stdout as JSON first
    let trimmed = raw.stdout.trim();
    if trimmed.starts_with('{') {
        if let Ok(json_output) = serde_json::from_str::<HookJsonOutput>(trimmed) {
            if json_output.is_async() {
                // Async hooks: treat as success (background processing)
                return HookResult {
                    stdout: raw.stdout,
                    stderr: raw.stderr,
                    exit_code: raw.exit_code,
                    outcome: HookOutcome::Success,
                    ..Default::default()
                };
            }

            if let Some(sync_output) = json_output.as_sync() {
                return process_sync_json_output(
                    sync_output,
                    command,
                    hook_name,
                    hook_event,
                    &raw.stdout,
                    &raw.stderr,
                    raw.exit_code,
                );
            }
        }
        // JSON parse failed but started with '{' — treat as validation error
        // but fall through to normal exit code handling rather than hard failing
        debug!("hook {hook_name}: stdout looks like JSON but failed to parse");
    }

    // Plain text or non-JSON output: use exit code logic
    match raw.exit_code {
        Some(0) => HookResult {
            stdout: raw.stdout,
            stderr: raw.stderr,
            exit_code: raw.exit_code,
            outcome: HookOutcome::Success,
            ..Default::default()
        },
        Some(2) => {
            // Exit code 2 = blocking error
            let error_text = if raw.stderr.is_empty() {
                "No stderr output".to_string()
            } else {
                raw.stderr.clone()
            };
            HookResult {
                stdout: raw.stdout,
                stderr: raw.stderr.clone(),
                exit_code: raw.exit_code,
                outcome: HookOutcome::Blocking,
                blocking_error: Some(HookBlockingError {
                    blocking_error: format!("[{command}]: {error_text}"),
                    command: command.to_string(),
                }),
                ..Default::default()
            }
        }
        _ => {
            // Any other non-zero exit code = non-blocking error
            let error_text = if raw.stderr.trim().is_empty() {
                "No stderr output".to_string()
            } else {
                raw.stderr.trim().to_string()
            };
            HookResult {
                stdout: raw.stdout,
                stderr: format!("Failed with non-blocking status code: {error_text}"),
                exit_code: raw.exit_code,
                outcome: HookOutcome::NonBlockingError,
                ..Default::default()
            }
        }
    }
}

/// Process the raw result of an HTTP hook into a HookResult.
fn process_http_result(
    raw: RawHttpResult,
    url: &str,
    hook_name: &str,
    hook_event: HookEvent,
) -> HookResult {
    if raw.aborted {
        return HookResult {
            outcome: HookOutcome::Cancelled,
            ..Default::default()
        };
    }

    if let Some(error) = raw.error {
        return HookResult {
            stderr: error,
            outcome: HookOutcome::NonBlockingError,
            ..Default::default()
        };
    }

    if !raw.ok {
        let stderr = format!("HTTP {} from {url}", raw.status_code);
        return HookResult {
            stdout: raw.body,
            stderr,
            exit_code: Some(raw.status_code as i32),
            outcome: HookOutcome::NonBlockingError,
            ..Default::default()
        };
    }

    // HTTP hooks must return JSON
    let trimmed = raw.body.trim();
    if trimmed.is_empty() {
        // Empty body = success with no output
        return HookResult {
            outcome: HookOutcome::Success,
            ..Default::default()
        };
    }

    if !trimmed.starts_with('{') {
        return HookResult {
            stderr: format!(
                "HTTP hook must return JSON, but got non-JSON response: {}",
                if trimmed.len() > 200 {
                    format!("{}...", &trimmed[..200])
                } else {
                    trimmed.to_string()
                }
            ),
            outcome: HookOutcome::NonBlockingError,
            ..Default::default()
        };
    }

    match serde_json::from_str::<HookJsonOutput>(trimmed) {
        Ok(json_output) => {
            if json_output.is_async() {
                return HookResult {
                    stdout: raw.body,
                    outcome: HookOutcome::Success,
                    ..Default::default()
                };
            }
            if let Some(sync_output) = json_output.as_sync() {
                return process_sync_json_output(
                    sync_output,
                    url,
                    hook_name,
                    hook_event,
                    &raw.body,
                    "",
                    Some(raw.status_code as i32),
                );
            }
            HookResult {
                stdout: raw.body,
                outcome: HookOutcome::Success,
                ..Default::default()
            }
        }
        Err(e) => HookResult {
            stderr: format!("HTTP hook JSON validation failed: {e}"),
            stdout: raw.body,
            outcome: HookOutcome::NonBlockingError,
            ..Default::default()
        },
    }
}

/// Process a synchronous hook JSON output into a HookResult.
///
/// This is the Rust equivalent of the TypeScript `processHookJSONOutput`.
fn process_sync_json_output(
    json: &SyncHookJsonOutput,
    command: &str,
    _hook_name: &str,
    expected_event: HookEvent,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> HookResult {
    let mut result = HookResult {
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        exit_code,
        outcome: HookOutcome::Success,
        ..Default::default()
    };

    // Handle `continue: false`
    if json.r#continue == Some(false) {
        result.prevent_continuation = true;
        result.stop_reason = json.stop_reason.clone();
    }

    // Handle `decision` field
    if let Some(decision) = &json.decision {
        match decision.as_str() {
            "approve" => {
                result.permission_behavior = Some(PermissionBehavior::Allow);
            }
            "block" => {
                result.permission_behavior = Some(PermissionBehavior::Deny);
                result.blocking_error = Some(HookBlockingError {
                    blocking_error: json
                        .reason
                        .clone()
                        .unwrap_or_else(|| "Blocked by hook".to_string()),
                    command: command.to_string(),
                });
                result.outcome = HookOutcome::Blocking;
            }
            other => {
                warn!("unknown hook decision type: {other}");
            }
        }
    }

    // Handle `systemMessage` field
    if json.system_message.is_some() {
        result.system_message = json.system_message.clone();
    }

    // Handle `hookSpecificOutput`
    if let Some(specific) = &json.hook_specific_output {
        process_hook_specific_output(
            specific,
            &mut result,
            command,
            json.reason.as_deref(),
            expected_event,
        );
    }

    result
}

/// Process hook-specific output fields into the result.
fn process_hook_specific_output(
    specific: &HookSpecificOutput,
    result: &mut HookResult,
    command: &str,
    reason: Option<&str>,
    _expected_event: HookEvent,
) {
    match specific {
        HookSpecificOutput::PreToolUse {
            permission_decision,
            permission_decision_reason,
            updated_input,
            additional_context,
        } => {
            if let Some(pd) = permission_decision {
                match pd.as_str() {
                    "allow" => {
                        result.permission_behavior = Some(PermissionBehavior::Allow);
                    }
                    "deny" => {
                        result.permission_behavior = Some(PermissionBehavior::Deny);
                        result.blocking_error = Some(HookBlockingError {
                            blocking_error: permission_decision_reason
                                .clone()
                                .or_else(|| reason.map(|s| s.to_string()))
                                .unwrap_or_else(|| "Blocked by hook".to_string()),
                            command: command.to_string(),
                        });
                        result.outcome = HookOutcome::Blocking;
                    }
                    "ask" => {
                        result.permission_behavior = Some(PermissionBehavior::Ask);
                    }
                    other => {
                        warn!("unknown hook permissionDecision: {other}");
                    }
                }
            }
            result.hook_permission_decision_reason = permission_decision_reason.clone();
            if let Some(input) = updated_input {
                result.updated_input = Some(input.clone());
            }
            result.additional_context = additional_context.clone();
        }
        HookSpecificOutput::UserPromptSubmit {
            additional_context, ..
        }
        | HookSpecificOutput::Setup {
            additional_context, ..
        }
        | HookSpecificOutput::SubagentStart {
            additional_context, ..
        }
        | HookSpecificOutput::PostToolUseFailure {
            additional_context, ..
        }
        | HookSpecificOutput::Notification {
            additional_context, ..
        } => {
            result.additional_context = additional_context.clone();
        }
        HookSpecificOutput::SessionStart {
            additional_context,
            initial_user_message,
            watch_paths,
        } => {
            result.additional_context = additional_context.clone();
            result.initial_user_message = initial_user_message.clone();
            result.watch_paths = watch_paths.clone();
        }
        HookSpecificOutput::PostToolUse {
            additional_context,
            updated_mcp_tool_output,
        } => {
            result.additional_context = additional_context.clone();
            if updated_mcp_tool_output.is_some() {
                result.updated_mcp_tool_output = updated_mcp_tool_output.clone();
            }
        }
        HookSpecificOutput::PermissionDenied { retry } => {
            result.retry = *retry;
        }
        HookSpecificOutput::PermissionRequest { decision } => {
            match decision {
                PermissionRequestDecision::Allow { updated_input, .. } => {
                    result.permission_behavior = Some(PermissionBehavior::Allow);
                    if let Some(input) = updated_input {
                        result.updated_input = Some(input.clone());
                    }
                }
                PermissionRequestDecision::Deny { .. } => {
                    result.permission_behavior = Some(PermissionBehavior::Deny);
                }
            }
            result.permission_request_result = Some(decision.clone());
        }
        HookSpecificOutput::Elicitation { action, .. } => {
            if let Some(action) = action {
                if action == "decline" {
                    result.blocking_error = Some(HookBlockingError {
                        blocking_error: reason
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "Elicitation denied by hook".to_string()),
                        command: command.to_string(),
                    });
                    result.outcome = HookOutcome::Blocking;
                }
            }
        }
        HookSpecificOutput::ElicitationResult { action, .. } => {
            if let Some(action) = action {
                if action == "decline" {
                    result.blocking_error = Some(HookBlockingError {
                        blocking_error: reason
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "Elicitation result blocked by hook".to_string()),
                        command: command.to_string(),
                    });
                    result.outcome = HookOutcome::Blocking;
                }
            }
        }
        HookSpecificOutput::CwdChanged { watch_paths }
        | HookSpecificOutput::FileChanged { watch_paths } => {
            result.watch_paths = watch_paths.clone();
        }
        HookSpecificOutput::WorktreeCreate { .. } => {
            // Informational only
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_command_result_exit_0() {
        let raw = RawCommandResult {
            stdout: "hello\n".to_string(),
            stderr: String::new(),
            exit_code: Some(0),
            aborted: false,
        };
        let result = process_command_result(raw, "echo hello", "test", HookEvent::Stop);
        assert_eq!(result.outcome, HookOutcome::Success);
        assert!(result.blocking_error.is_none());
    }

    #[test]
    fn test_process_command_result_exit_2_blocking() {
        let raw = RawCommandResult {
            stdout: String::new(),
            stderr: "plan not complete".to_string(),
            exit_code: Some(2),
            aborted: false,
        };
        let result = process_command_result(raw, "check.sh", "Stop:check", HookEvent::Stop);
        assert_eq!(result.outcome, HookOutcome::Blocking);
        assert!(result.blocking_error.is_some());
        let err = result.blocking_error.unwrap();
        assert!(err.blocking_error.contains("plan not complete"));
    }

    #[test]
    fn test_process_command_result_exit_1_non_blocking() {
        let raw = RawCommandResult {
            stdout: String::new(),
            stderr: "some warning".to_string(),
            exit_code: Some(1),
            aborted: false,
        };
        let result = process_command_result(raw, "warn.sh", "test", HookEvent::Stop);
        assert_eq!(result.outcome, HookOutcome::NonBlockingError);
        assert!(result.blocking_error.is_none());
    }

    #[test]
    fn test_process_command_result_json_decision_block() {
        let json_stdout =
            r#"{"decision": "block", "reason": "forbidden file"}"#.to_string();
        let raw = RawCommandResult {
            stdout: json_stdout,
            stderr: String::new(),
            exit_code: Some(0),
            aborted: false,
        };
        let result = process_command_result(raw, "guard.sh", "PreToolUse:Write", HookEvent::PreToolUse);
        assert_eq!(result.outcome, HookOutcome::Blocking);
        let err = result.blocking_error.unwrap();
        assert_eq!(err.blocking_error, "forbidden file");
    }

    #[test]
    fn test_process_command_result_json_continue_false() {
        let json_stdout =
            r#"{"continue": false, "stopReason": "done"}"#.to_string();
        let raw = RawCommandResult {
            stdout: json_stdout,
            stderr: String::new(),
            exit_code: Some(0),
            aborted: false,
        };
        let result = process_command_result(raw, "stop.sh", "Stop", HookEvent::Stop);
        assert!(result.prevent_continuation);
        assert_eq!(result.stop_reason.as_deref(), Some("done"));
    }

    #[test]
    fn test_process_command_result_aborted() {
        let raw = RawCommandResult {
            stdout: String::new(),
            stderr: "timeout".to_string(),
            exit_code: None,
            aborted: true,
        };
        let result = process_command_result(raw, "slow.sh", "test", HookEvent::Stop);
        assert_eq!(result.outcome, HookOutcome::Cancelled);
    }

    #[tokio::test]
    async fn test_execute_shell_hook_echo() {
        let result = execute_shell_hook("echo hello", "bash", "{}", 5000).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.stdout.trim(), "hello");
        assert_eq!(r.exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_execute_shell_hook_stdin() {
        let result =
            execute_shell_hook("cat", "bash", r#"{"test": true}"#, 5000).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.stdout.contains(r#""test""#));
    }

    #[tokio::test]
    async fn test_execute_shell_hook_timeout() {
        let result = execute_shell_hook("sleep 60", "bash", "{}", 100).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.aborted);
    }
}
