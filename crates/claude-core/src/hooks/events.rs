//! Hook event system: broadcasting hook execution events for observability.
//!
//! Mirrors `hookEvents.ts`. Provides a generic event system separate from the
//! main message stream. Handlers register to receive events and decide what to
//! do with them (e.g., convert to SDK messages, log, etc.).

use std::sync::{Arc, Mutex};

use super::types::HookExecutionEvent;

/// Maximum number of pending events to buffer before a handler is registered.
const MAX_PENDING_EVENTS: usize = 100;

/// Events that are always emitted regardless of the `all_events_enabled` flag.
const ALWAYS_EMITTED_EVENTS: &[&str] = &["SessionStart", "Setup"];

/// Callback type for hook event handlers.
pub type HookEventHandler = Arc<dyn Fn(&HookExecutionEvent) + Send + Sync>;

/// Hook event emitter: collects events and dispatches to registered handlers.
pub struct HookEventEmitter {
    inner: Mutex<EmitterInner>,
}

struct EmitterInner {
    handler: Option<HookEventHandler>,
    pending_events: Vec<HookExecutionEvent>,
    all_events_enabled: bool,
}

impl HookEventEmitter {
    /// Create a new emitter with no handler registered.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(EmitterInner {
                handler: None,
                pending_events: Vec::new(),
                all_events_enabled: false,
            }),
        }
    }

    /// Register a handler for hook events. Flushes any pending events.
    pub fn register_handler(&self, handler: Option<HookEventHandler>) {
        let mut inner = self.inner.lock().unwrap();
        inner.handler = handler.clone();

        if let Some(ref h) = handler {
            for event in inner.pending_events.drain(..) {
                h(&event);
            }
        }
    }

    /// Enable emission of all hook event types (beyond SessionStart and Setup).
    pub fn set_all_events_enabled(&self, enabled: bool) {
        self.inner.lock().unwrap().all_events_enabled = enabled;
    }

    /// Emit a hook started event.
    pub fn emit_started(&self, hook_id: &str, hook_name: &str, hook_event: &str) {
        if !self.should_emit(hook_event) {
            return;
        }
        self.emit(HookExecutionEvent::Started {
            hook_id: hook_id.to_string(),
            hook_name: hook_name.to_string(),
            hook_event: hook_event.to_string(),
        });
    }

    /// Emit a hook progress event.
    pub fn emit_progress(
        &self,
        hook_id: &str,
        hook_name: &str,
        hook_event: &str,
        stdout: &str,
        stderr: &str,
        output: &str,
    ) {
        if !self.should_emit(hook_event) {
            return;
        }
        self.emit(HookExecutionEvent::Progress {
            hook_id: hook_id.to_string(),
            hook_name: hook_name.to_string(),
            hook_event: hook_event.to_string(),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            output: output.to_string(),
        });
    }

    /// Emit a hook response event.
    pub fn emit_response(
        &self,
        hook_id: &str,
        hook_name: &str,
        hook_event: &str,
        output: &str,
        stdout: &str,
        stderr: &str,
        exit_code: Option<i32>,
        outcome: &str,
    ) {
        if !self.should_emit(hook_event) {
            return;
        }
        self.emit(HookExecutionEvent::Response {
            hook_id: hook_id.to_string(),
            hook_name: hook_name.to_string(),
            hook_event: hook_event.to_string(),
            output: output.to_string(),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit_code,
            outcome: outcome.to_string(),
        });
    }

    /// Clear the handler and all pending events.
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.handler = None;
        inner.pending_events.clear();
        inner.all_events_enabled = false;
    }

    fn should_emit(&self, hook_event: &str) -> bool {
        if ALWAYS_EMITTED_EVENTS.contains(&hook_event) {
            return true;
        }
        let inner = self.inner.lock().unwrap();
        inner.all_events_enabled
    }

    fn emit(&self, event: HookExecutionEvent) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(ref handler) = inner.handler {
            handler(&event);
        } else {
            inner.pending_events.push(event);
            if inner.pending_events.len() > MAX_PENDING_EVENTS {
                inner.pending_events.remove(0);
            }
        }
    }
}

impl Default for HookEventEmitter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_always_emitted_events() {
        let emitter = HookEventEmitter::new();
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();
        emitter.register_handler(Some(Arc::new(move |_| {
            count_clone.fetch_add(1, Ordering::Relaxed);
        })));

        // SessionStart should always emit
        emitter.emit_started("1", "test", "SessionStart");
        assert_eq!(count.load(Ordering::Relaxed), 1);

        // PreToolUse should not emit when all_events_enabled is false
        emitter.emit_started("2", "test", "PreToolUse");
        assert_eq!(count.load(Ordering::Relaxed), 1);

        // Enable all events
        emitter.set_all_events_enabled(true);
        emitter.emit_started("3", "test", "PreToolUse");
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_pending_events_flushed_on_register() {
        let emitter = HookEventEmitter::new();

        // Emit before handler is registered
        emitter.emit_started("1", "test", "SessionStart");
        emitter.emit_started("2", "test", "Setup");

        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();
        emitter.register_handler(Some(Arc::new(move |_| {
            count_clone.fetch_add(1, Ordering::Relaxed);
        })));

        // Both pending events should have been flushed
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }
}
