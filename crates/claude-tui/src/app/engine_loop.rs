use std::path::Path;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use claude_core::permissions::evaluator::evaluate_permission_sync;
use claude_core::permissions::types::{PermissionBehavior, ToolPermissionContext};
use claude_core::types::events::{StreamEvent, ToolResultData};
use claude_tools::{ToolRegistry, ToolUseContext};

use crate::widgets::message_list::{MessageEntry, SystemSeverity, ToolUseStatus};
use crate::widgets::permission_dialog::PermissionDialog;
use crate::widgets::spinner::SpinnerMode;

use super::{AppEvent, PendingTool, App};

impl App {
    pub(crate) fn handle_stream_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::TextDelta { text } => {
                // Append to current assistant message, or create one
                if let Some(MessageEntry::Assistant { text: ref mut t }) =
                    self.message_list.messages_mut().last_mut()
                {
                    t.push_str(&text);
                } else {
                    self.message_list.push(MessageEntry::Assistant { text });
                }
            }
            StreamEvent::ThinkingDelta { text } => {
                if let Some(MessageEntry::Thinking { text: ref mut t, .. }) =
                    self.message_list.messages_mut().last_mut()
                {
                    t.push_str(&text);
                } else {
                    self.message_list.push(MessageEntry::Thinking {
                        text,
                        is_collapsed: true,
                    });
                }
            }
            StreamEvent::ToolStart {
                tool_use_id,
                name,
                input,
            } => {
                self.message_list.push(MessageEntry::ToolUse {
                    id: tool_use_id,
                    name: name.clone(),
                    input,
                    status: ToolUseStatus::Running,
                });
                self.spinner.start(SpinnerMode::Tool { name });
            }
            StreamEvent::ToolResult {
                tool_use_id,
                result,
            } => {
                // Update the corresponding ToolUse status
                let status = if result.is_error {
                    ToolUseStatus::Error
                } else {
                    ToolUseStatus::Complete
                };
                self.message_list.update_tool_status(&tool_use_id, status);
                self.message_list.push(MessageEntry::ToolResult {
                    id: tool_use_id,
                    name: "tool".to_string(),
                    output: truncate_result(
                        result.data.as_str().unwrap_or(&result.data.to_string()),
                    ),
                    is_error: result.is_error,
                    duration_ms: None,
                });
            }
            StreamEvent::Done { stop_reason: _ } => {
                self.spinner.stop();
                self.engine_busy = false;
            }
            StreamEvent::UsageUpdate(usage) => {
                self.spinner.tokens = usage.output_tokens;
                self.total_tokens = self.total_tokens.saturating_add(usage.output_tokens);
                // Sync cost from shared state (updated by engine via CostTracker)
                if let Some(ref state) = self.app_state {
                    if let Some(s) = state.try_read() {
                        self.total_cost = s.total_cost_usd();
                    }
                }
            }
            StreamEvent::RequestStart { request_id: _ } => {
                self.spinner.start(SpinnerMode::Thinking);
            }
            StreamEvent::Error(err) => {
                self.spinner.stop();
                self.engine_busy = false;
                self.message_list.push(MessageEntry::System {
                    text: format!("Error: {}", err),
                    severity: SystemSeverity::Error,
                });
            }
            StreamEvent::RetryWait {
                attempt,
                delay_ms,
                status,
            } => {
                if status == 529 && delay_ms == 0 {
                    // 529 fallback -- switching to non-streaming
                    self.spinner.start(SpinnerMode::Thinking);
                    self.message_list.push(MessageEntry::System {
                        text: "API overloaded, falling back to non-streaming request...".into(),
                        severity: SystemSeverity::Info,
                    });
                } else {
                    let status_label = if status == 0 {
                        "network error".to_string()
                    } else {
                        format!("HTTP {status}")
                    };
                    self.message_list.push(MessageEntry::System {
                        text: format!(
                            "Retrying ({status_label}, attempt {attempt}, waiting {delay_ms}ms)..."
                        ),
                        severity: SystemSeverity::Warning,
                    });
                }
            }
            StreamEvent::Compacted { summary } => {
                self.message_list.push(MessageEntry::CompactBoundary { summary });
            }
            _ => {}
        }
    }

    /// Check permissions for the next tool in the pending list.
    /// If the tool is auto-allowed, execute it immediately and advance.
    /// If it needs user permission, show the dialog.
    pub(crate) fn check_next_tool_permission(
        &mut self,
        pending_tools: &[PendingTool],
        pending_tool_index: &mut usize,
        perm_ctx: &ToolPermissionContext,
        tools: &ToolRegistry,
        tx: &mpsc::Sender<AppEvent>,
    ) {
        if *pending_tool_index < pending_tools.len() {
            let tool = &pending_tools[*pending_tool_index];
            let info = &tool.info;
            *pending_tool_index += 1;

            // Determine if tool is read-only
            let is_read_only = tools
                .get(&info.name)
                .map(|t| t.is_read_only(&info.input))
                .unwrap_or(false);

            let decision =
                evaluate_permission_sync(&info.name, &info.input, perm_ctx, is_read_only);

            match decision.behavior {
                PermissionBehavior::Allow => {
                    // Auto-execute: send an "allow" permission response immediately
                    let tx2 = tx.clone();
                    tokio::spawn(async move {
                        let _ = tx2
                            .send(AppEvent::PermissionResponse("allow".to_string()))
                            .await;
                    });
                }
                PermissionBehavior::Ask => {
                    let message = decision.message.unwrap_or_else(|| "Permission required".to_string());
                    let input_preview = serde_json::to_string_pretty(&info.input)
                        .unwrap_or_else(|_| info.input.to_string());
                    self.permission_dialog = Some(PermissionDialog::new(
                        info.name.clone(),
                        message,
                        input_preview,
                    ));
                }
                PermissionBehavior::Deny => {
                    let message = decision.message.unwrap_or_else(|| "Denied".to_string());
                    // Auto-deny, send deny response
                    let tx2 = tx.clone();
                    tokio::spawn(async move {
                        let _ = tx2
                            .send(AppEvent::PermissionResponse("deny".to_string()))
                            .await;
                    });
                    self.message_list.push(MessageEntry::System {
                        text: format!("Denied: {}", message),
                        severity: SystemSeverity::Warning,
                    });
                }
            }
        }
    }

    /// Check cost threshold and show warning if exceeded.
    pub(crate) fn check_cost_threshold(&mut self) {
        if self.total_cost >= self.cost_warning_threshold && !self.cost_warning_shown {
            self.cost_warning_shown = true;
            let cost = self.total_cost;
            let threshold = self.cost_warning_threshold;
            self.flash(crate::widgets::status_bar::FlashMessage::warning(
                format!(
                    "Session cost ${:.2} exceeded ${:.0} threshold",
                    cost, threshold
                ),
            ));
            self.message_list.push(MessageEntry::System {
                text: format!(
                    "Warning: session cost ${:.2} has exceeded the ${:.0} threshold",
                    cost, threshold
                ),
                severity: SystemSeverity::Warning,
            });
        }
    }
}

/// Execute a tool call.
pub(crate) async fn execute_tool(
    tools: &ToolRegistry,
    name: &str,
    input: &serde_json::Value,
    cwd: &Path,
    cancel: CancellationToken,
) -> Result<ToolResultData> {
    let executor = tools
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?;
    let ctx = ToolUseContext::with_working_directory(cwd.to_path_buf());
    executor.call(input, &ctx, cancel, None).await
}

/// Truncate long tool results for display.
pub(crate) fn truncate_result(s: &str) -> String {
    const MAX_DISPLAY: usize = 2000;
    if s.len() <= MAX_DISPLAY {
        s.to_string()
    } else {
        format!("{}... ({} chars total)", &s[..MAX_DISPLAY], s.len())
    }
}
