//! Bridge debug utilities.
//!
//! Provides logging helpers, secret redaction, fault injection for testing,
//! and debug handle management for the bridge.

use std::sync::Mutex;

use serde_json::Value;

/// Maximum length of debug messages before truncation.
const DEBUG_MSG_LIMIT: usize = 2000;

/// Minimum length for a secret value to show partial redaction
/// (prefix + suffix). Shorter values are fully redacted.
const REDACT_MIN_LENGTH: usize = 16;

/// Field names whose values should be redacted in debug output.
const SECRET_FIELD_NAMES: &[&str] = &[
    "session_ingress_token",
    "environment_secret",
    "access_token",
    "secret",
    "token",
];

/// Redact secret field values in a JSON string.
///
/// Replaces values of known secret fields with either a partial redaction
/// (first 8 + last 4 chars) or full `[REDACTED]` for short values.
pub fn redact_secrets(s: &str) -> String {
    let mut result = s.to_string();
    for field in SECRET_FIELD_NAMES {
        // Match "field":"value" patterns
        let pattern = format!("\"{field}\":\"");
        let mut search_from = 0;
        while let Some(start) = result[search_from..].find(&pattern) {
            let abs_start = search_from + start;
            let value_start = abs_start + pattern.len();

            // Find the closing quote
            if let Some(value_end) = result[value_start..].find('"') {
                let abs_value_end = value_start + value_end;
                let value = &result[value_start..abs_value_end];

                let redacted = if value.len() < REDACT_MIN_LENGTH {
                    "[REDACTED]".to_string()
                } else {
                    format!("{}...{}", &value[..8], &value[value.len() - 4..])
                };

                result = format!(
                    "{}\"{}\":\"{}\"{}",
                    &result[..abs_start],
                    field,
                    redacted,
                    &result[abs_value_end + 1..]
                );
                search_from = abs_start + field.len() + redacted.len() + 5;
            } else {
                break;
            }
        }
    }
    result
}

/// Truncate a string for debug logging, collapsing newlines.
pub fn debug_truncate(s: &str) -> String {
    let flat = s.replace('\n', "\\n");
    if flat.len() <= DEBUG_MSG_LIMIT {
        flat
    } else {
        format!(
            "{}... ({} chars)",
            &flat[..DEBUG_MSG_LIMIT],
            flat.len()
        )
    }
}

/// Truncate a JSON-serializable value for debug logging.
pub fn debug_body(data: &Value) -> String {
    let raw = serde_json::to_string(data).unwrap_or_else(|_| format!("{data:?}"));
    let s = redact_secrets(&raw);
    if s.len() <= DEBUG_MSG_LIMIT {
        s
    } else {
        format!("{}... ({} chars)", &s[..DEBUG_MSG_LIMIT], s.len())
    }
}

/// Extract a human-readable error detail from an HTTP error response.
///
/// Checks `data.message` first, then `data.error.message`.
pub fn extract_error_detail(data: &Value) -> Option<String> {
    // Try data.message
    if let Some(msg) = data.get("message").and_then(|m| m.as_str()) {
        return Some(msg.to_string());
    }
    // Try data.error.message
    if let Some(error) = data.get("error") {
        if let Some(msg) = error.get("message").and_then(|m| m.as_str()) {
            return Some(msg.to_string());
        }
    }
    None
}

/// Extract the HTTP status code from an error, if present.
pub fn extract_http_status(err: &anyhow::Error) -> Option<u16> {
    // Check for reqwest errors
    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
        return reqwest_err.status().map(|s| s.as_u16());
    }
    // Check for BridgeFatalError
    if let Some(fatal) = err.downcast_ref::<super::api::BridgeFatalError>() {
        return Some(fatal.status);
    }
    None
}

// ---------------------------------------------------------------------------
// Fault injection (debug/testing only)
// ---------------------------------------------------------------------------

/// A one-shot fault to inject on the next matching API call.
#[derive(Clone, Debug)]
pub struct BridgeFault {
    /// Which API method to intercept.
    pub method: FaultMethod,
    /// Whether the error should be fatal (teardown) or transient (retry).
    pub kind: FaultKind,
    /// HTTP status code to simulate.
    pub status: u16,
    /// Optional error type string.
    pub error_type: Option<String>,
    /// Remaining injection count. Decremented on consume; removed at 0.
    pub count: u32,
}

/// API methods that can be fault-injected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultMethod {
    PollForWork,
    RegisterEnvironment,
    ReconnectSession,
    HeartbeatWork,
}

/// Kind of fault to inject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// Fatal errors go through handleErrorStatus -> BridgeFatalError.
    Fatal,
    /// Transient errors surface as plain rejections (5xx / network).
    Transient,
}

/// Handle for debug operations on a running bridge.
pub struct BridgeDebugHandle {
    /// Fire the transport close handler directly (tests ws_closed recovery).
    pub fire_close: Box<dyn Fn(u16) + Send + Sync>,
    /// Force a reconnect via reconnectEnvironmentWithSession.
    pub force_reconnect: Box<dyn Fn() + Send + Sync>,
    /// Queue a fault for the next N calls to the named API method.
    pub inject_fault: Box<dyn Fn(BridgeFault) + Send + Sync>,
    /// Abort the at-capacity sleep immediately.
    pub wake_poll_loop: Box<dyn Fn() + Send + Sync>,
    /// Environment/session IDs for debug.log grep.
    pub describe: Box<dyn Fn() -> String + Send + Sync>,
}

/// Global debug handle and fault queue.
static DEBUG_HANDLE: Mutex<Option<BridgeDebugHandle>> = Mutex::new(None);
static FAULT_QUEUE: Mutex<Vec<BridgeFault>> = Mutex::new(Vec::new());

/// Register a debug handle for the running bridge.
pub fn register_bridge_debug_handle(handle: BridgeDebugHandle) {
    let mut global = DEBUG_HANDLE.lock().unwrap();
    *global = Some(handle);
}

/// Clear the debug handle and fault queue.
pub fn clear_bridge_debug_handle() {
    let mut global = DEBUG_HANDLE.lock().unwrap();
    *global = None;
    let mut queue = FAULT_QUEUE.lock().unwrap();
    queue.clear();
}

/// Get a description of the current bridge for debugging.
pub fn describe_bridge() -> Option<String> {
    let global = DEBUG_HANDLE.lock().unwrap();
    global.as_ref().map(|h| (h.describe)())
}

/// Queue a fault for injection.
pub fn inject_bridge_fault(fault: BridgeFault) {
    tracing::debug!(
        "[bridge:debug] Queued fault: {:?} {:?}/{} x{}",
        fault.method,
        fault.kind,
        fault.status,
        fault.count
    );
    let mut queue = FAULT_QUEUE.lock().unwrap();
    queue.push(fault);
}

/// Try to consume a queued fault for the given method.
///
/// Returns the fault if one was queued, decrementing its count.
/// Removes the fault from the queue when count reaches 0.
pub fn consume_fault(method: FaultMethod) -> Option<BridgeFault> {
    let mut queue = FAULT_QUEUE.lock().unwrap();
    let idx = queue.iter().position(|f| f.method == method)?;
    let fault = queue[idx].clone();
    queue[idx].count -= 1;
    if queue[idx].count == 0 {
        queue.remove(idx);
    }
    Some(fault)
}

/// Fire the transport close handler (for debug testing).
pub fn fire_debug_close(code: u16) {
    let global = DEBUG_HANDLE.lock().unwrap();
    if let Some(h) = global.as_ref() {
        (h.fire_close)(code);
    }
}

/// Force a reconnect (for debug testing).
pub fn force_debug_reconnect() {
    let global = DEBUG_HANDLE.lock().unwrap();
    if let Some(h) = global.as_ref() {
        (h.force_reconnect)();
    }
}

/// Wake the poll loop (for debug testing).
pub fn wake_debug_poll_loop() {
    let global = DEBUG_HANDLE.lock().unwrap();
    if let Some(h) = global.as_ref() {
        (h.wake_poll_loop)();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_redact_secrets_long_token() {
        let input = r#"{"session_ingress_token":"sk-ant-si-abcdefghijklmnopqrstuvwxyz1234567890"}"#;
        let redacted = redact_secrets(input);
        assert!(redacted.contains("sk-ant-s...7890"));
        assert!(!redacted.contains("abcdefghijklmnopqrstuvwxyz1234567890"));
    }

    #[test]
    fn test_redact_secrets_short_token() {
        let input = r#"{"token":"short"}"#;
        let redacted = redact_secrets(input);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redact_secrets_no_secrets() {
        let input = r#"{"type":"user","content":"hello"}"#;
        let redacted = redact_secrets(input);
        assert_eq!(redacted, input);
    }

    #[test]
    fn test_debug_truncate_short() {
        let s = "hello world";
        assert_eq!(debug_truncate(s), "hello world");
    }

    #[test]
    fn test_debug_truncate_long() {
        let s = "x".repeat(3000);
        let result = debug_truncate(&s);
        assert!(result.len() < 3000);
        assert!(result.contains("3000 chars"));
    }

    #[test]
    fn test_debug_truncate_newlines() {
        let s = "line1\nline2\nline3";
        let result = debug_truncate(s);
        assert_eq!(result, "line1\\nline2\\nline3");
    }

    #[test]
    fn test_debug_body() {
        let data = json!({"type": "user", "token": "sk-ant-si-verylongtokenvalue12345678"});
        let result = debug_body(&data);
        assert!(result.contains("[REDACTED]") || result.contains("..."));
    }

    #[test]
    fn test_extract_error_detail_message() {
        let data = json!({"message": "something went wrong"});
        assert_eq!(
            extract_error_detail(&data),
            Some("something went wrong".to_string())
        );
    }

    #[test]
    fn test_extract_error_detail_nested() {
        let data = json!({"error": {"message": "nested error"}});
        assert_eq!(
            extract_error_detail(&data),
            Some("nested error".to_string())
        );
    }

    #[test]
    fn test_extract_error_detail_none() {
        let data = json!({"status": 500});
        assert_eq!(extract_error_detail(&data), None);
    }

    #[test]
    fn test_fault_queue() {
        // Clear any existing faults
        clear_bridge_debug_handle();

        let fault = BridgeFault {
            method: FaultMethod::PollForWork,
            kind: FaultKind::Transient,
            status: 500,
            error_type: None,
            count: 2,
        };

        inject_bridge_fault(fault);

        // First consume should succeed
        let f1 = consume_fault(FaultMethod::PollForWork);
        assert!(f1.is_some());
        assert_eq!(f1.unwrap().status, 500);

        // Second consume should succeed (count was 2)
        let f2 = consume_fault(FaultMethod::PollForWork);
        assert!(f2.is_some());

        // Third should be None (exhausted)
        let f3 = consume_fault(FaultMethod::PollForWork);
        assert!(f3.is_none());

        // Different method should also be None
        let f4 = consume_fault(FaultMethod::HeartbeatWork);
        assert!(f4.is_none());
    }
}
