//! Auto-mode state tracking.
//!
//! Manages session-level auto-approval state, denial tracking, and
//! dynamic permission escalation for auto-mode sessions.
//!
//! Mirrors the TypeScript `autoModeState.ts` and denial-tracking logic
//! from `denialTracking.ts`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Auto-mode state
// ---------------------------------------------------------------------------

/// Session-level auto-mode state.
///
/// Thread-safe via atomics; shared across concurrent tool evaluations
/// in the same session.
pub struct AutoModeState {
    /// Whether auto-mode is currently active.
    active: AtomicBool,
    /// Whether auto-mode was requested via the CLI flag.
    flag_cli: AtomicBool,
    /// Circuit breaker: set when a remote gate or kill-switch disables auto-mode.
    circuit_broken: AtomicBool,
    /// Consecutive denial counter for escalation tracking.
    consecutive_denials: AtomicU32,
    /// Total denials in this session.
    total_denials: AtomicU32,
    /// Total approvals in this session.
    total_approvals: AtomicU32,
}

impl AutoModeState {
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            flag_cli: AtomicBool::new(false),
            circuit_broken: AtomicBool::new(false),
            consecutive_denials: AtomicU32::new(0),
            total_denials: AtomicU32::new(0),
            total_approvals: AtomicU32::new(0),
        }
    }

    // -- active -----------------------------------------------------------

    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::SeqCst);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    // -- CLI flag ---------------------------------------------------------

    pub fn set_flag_cli(&self, passed: bool) {
        self.flag_cli.store(passed, Ordering::SeqCst);
    }

    pub fn flag_cli(&self) -> bool {
        self.flag_cli.load(Ordering::SeqCst)
    }

    // -- circuit breaker --------------------------------------------------

    pub fn set_circuit_broken(&mut self, broken: bool) {
        self.circuit_broken.store(broken, Ordering::SeqCst);
    }

    pub fn is_circuit_broken(&self) -> bool {
        self.circuit_broken.load(Ordering::SeqCst)
    }

    // -- denial tracking --------------------------------------------------

    /// Record a denial. Increments both consecutive and total counters.
    pub fn record_denial(&self) {
        self.consecutive_denials.fetch_add(1, Ordering::SeqCst);
        self.total_denials.fetch_add(1, Ordering::SeqCst);
    }

    /// Record a successful (allowed) tool use. Resets the consecutive counter.
    pub fn record_success(&self) {
        self.consecutive_denials.store(0, Ordering::SeqCst);
        self.total_approvals.fetch_add(1, Ordering::SeqCst);
    }

    pub fn consecutive_denials(&self) -> u32 {
        self.consecutive_denials.load(Ordering::SeqCst)
    }

    pub fn total_denials(&self) -> u32 {
        self.total_denials.load(Ordering::SeqCst)
    }

    pub fn total_approvals(&self) -> u32 {
        self.total_approvals.load(Ordering::SeqCst)
    }

    /// Whether the session should fall back to interactive prompting.
    ///
    /// After `threshold` consecutive denials, the classifier is likely
    /// stuck in a loop and the user should be consulted.
    pub fn should_fallback_to_prompting(&self, threshold: u32) -> bool {
        self.consecutive_denials() >= threshold
    }

    // -- reset ------------------------------------------------------------

    /// Reset all state (useful for testing).
    pub fn reset(&mut self) {
        self.active.store(false, Ordering::SeqCst);
        self.flag_cli.store(false, Ordering::SeqCst);
        self.circuit_broken.store(false, Ordering::SeqCst);
        self.consecutive_denials.store(0, Ordering::SeqCst);
        self.total_denials.store(0, Ordering::SeqCst);
        self.total_approvals.store(0, Ordering::SeqCst);
    }
}

impl Default for AutoModeState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Denial tracking configuration
// ---------------------------------------------------------------------------

/// Thresholds for denial-based escalation.
pub struct DenialLimits {
    /// After this many consecutive denials, fall back to prompting.
    pub fallback_threshold: u32,
    /// After this many total denials, consider disabling auto-mode.
    pub disable_threshold: u32,
}

impl Default for DenialLimits {
    fn default() -> Self {
        Self {
            fallback_threshold: 3,
            disable_threshold: 10,
        }
    }
}

/// Standalone denial-tracking state (for sub-agents that don't share
/// the top-level `AutoModeState`).
#[derive(Clone, Debug)]
pub struct DenialTrackingState {
    pub consecutive_denials: u32,
    pub total_denials: u32,
    pub total_approvals: u32,
}

impl DenialTrackingState {
    pub fn new() -> Self {
        Self {
            consecutive_denials: 0,
            total_denials: 0,
            total_approvals: 0,
        }
    }

    /// Record a denial and return the updated state.
    pub fn record_denial(&self) -> Self {
        Self {
            consecutive_denials: self.consecutive_denials + 1,
            total_denials: self.total_denials + 1,
            total_approvals: self.total_approvals,
        }
    }

    /// Record a success and return the updated state.
    pub fn record_success(&self) -> Self {
        Self {
            consecutive_denials: 0,
            total_denials: self.total_denials,
            total_approvals: self.total_approvals + 1,
        }
    }

    /// Whether the session should fall back to prompting.
    pub fn should_fallback_to_prompting(&self, limits: &DenialLimits) -> bool {
        self.consecutive_denials >= limits.fallback_threshold
    }
}

impl Default for DenialTrackingState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_active_toggle() {
        let state = AutoModeState::new();
        assert!(!state.is_active());
        state.set_active(true);
        assert!(state.is_active());
        state.set_active(false);
        assert!(!state.is_active());
    }

    #[test]
    fn circuit_breaker() {
        let mut state = AutoModeState::new();
        assert!(!state.is_circuit_broken());
        state.set_circuit_broken(true);
        assert!(state.is_circuit_broken());
    }

    #[test]
    fn denial_tracking() {
        let state = AutoModeState::new();
        state.record_denial();
        state.record_denial();
        assert_eq!(state.consecutive_denials(), 2);
        assert_eq!(state.total_denials(), 2);

        state.record_success();
        assert_eq!(state.consecutive_denials(), 0);
        assert_eq!(state.total_denials(), 2);
        assert_eq!(state.total_approvals(), 1);
    }

    #[test]
    fn fallback_threshold() {
        let state = AutoModeState::new();
        assert!(!state.should_fallback_to_prompting(3));
        state.record_denial();
        state.record_denial();
        state.record_denial();
        assert!(state.should_fallback_to_prompting(3));

        // Success resets the streak.
        state.record_success();
        assert!(!state.should_fallback_to_prompting(3));
    }

    #[test]
    fn reset() {
        let mut state = AutoModeState::new();
        state.set_active(true);
        state.set_circuit_broken(true);
        state.record_denial();
        state.reset();
        assert!(!state.is_active());
        assert!(!state.is_circuit_broken());
        assert_eq!(state.consecutive_denials(), 0);
    }

    // -- DenialTrackingState (standalone) ---------------------------------

    #[test]
    fn standalone_denial_tracking() {
        let s = DenialTrackingState::new();
        let s = s.record_denial();
        let s = s.record_denial();
        assert_eq!(s.consecutive_denials, 2);
        assert_eq!(s.total_denials, 2);

        let s = s.record_success();
        assert_eq!(s.consecutive_denials, 0);
        assert_eq!(s.total_approvals, 1);
    }

    #[test]
    fn standalone_fallback() {
        let limits = DenialLimits::default();
        let s = DenialTrackingState::new();
        assert!(!s.should_fallback_to_prompting(&limits));

        let s = s.record_denial().record_denial().record_denial();
        assert!(s.should_fallback_to_prompting(&limits));
    }
}
