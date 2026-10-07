// Ported from CodexBar Sources/CodexBar/UsageStore+Refresh.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Shared/UsageStore+BrowserSession.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use crate::projection::RefreshState;
use crate::refresh::failure_gate::FailureGate;

/// Provider-chosen surfacing policy for refresh failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfacePolicy {
    /// Default consecutive failure gate: hides the first failure if prior data exists.
    #[default]
    Gate,
    /// Browser-session providers and restored history: surfaces immediately when prior data exists.
    AlwaysWithPrior,
    /// Claude probe timeout / CLI rate limit with preserved data: clears error without surfacing.
    NeverWhenPreserved,
}

/// Inputs for evaluating provider failure policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureInput {
    pub had_prior: bool,
    pub preservable: bool,
    pub policy: SurfacePolicy,
    pub restored_history: bool,
}

impl FailureInput {
    /// Creates a new failure evaluation input.
    pub const fn new(
        had_prior: bool,
        preservable: bool,
        policy: SurfacePolicy,
        restored_history: bool,
    ) -> Self {
        Self {
            had_prior,
            preservable,
            policy,
            restored_history,
        }
    }
}

/// Resulting decision for a provider refresh failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureDecision {
    /// Whether the error should be surfaced to clients.
    pub surface_error: bool,
    /// Whether any prior snapshot must be dropped from cache.
    pub drop_snapshot: bool,
    /// Whether an update envelope should be published to clients.
    pub publish: bool,
}

impl FailureDecision {
    /// Maps this failure decision and prior snapshot presence to projection `(RefreshState, kept_snapshot)`.
    pub fn to_projection_refresh(&self, had_prior: bool) -> (RefreshState, bool) {
        if self.drop_snapshot || !had_prior {
            (RefreshState::Error, false)
        } else if self.surface_error {
            (RefreshState::Error, true)
        } else {
            (RefreshState::Fresh, true)
        }
    }
}

/// Pure decision function for provider refresh failures.
///
/// Evaluates the consecutive failure gate and provider policy overrides:
/// - NeverWhenPreserved: clears the error and keeps snapshot when prior data is preserved.
/// - AlwaysWithPrior: surfaces immediately when prior data exists.
/// - Restored history: surfaces immediately when prior data exists.
/// - Gate (default): hides the first failure with prior data; surfaces on repeat failures.
///
/// When surfaced, non-preservable errors drop the snapshot, while preservable ones keep it.
pub fn failure_decision(input: FailureInput, gate: &mut FailureGate) -> FailureDecision {
    let gate_surfaces = gate.should_surface(input.had_prior);
    let surface_error = match input.policy {
        SurfacePolicy::NeverWhenPreserved if input.had_prior && input.preservable => false,
        SurfacePolicy::AlwaysWithPrior if input.had_prior => true,
        _ if input.restored_history && input.had_prior => true,
        _ => gate_surfaces,
    };
    let drop_snapshot = surface_error && !input.preservable;
    let publish = surface_error || drop_snapshot;

    FailureDecision {
        surface_error,
        drop_snapshot,
        publish,
    }
}

/// Decision rendered on an account change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountChangeDecision {
    pub drop_snapshot: bool,
}

/// Handles account change: drops existing snapshot first, then resets failure gate.
pub fn on_account_change(gate: &mut FailureGate) -> AccountChangeDecision {
    gate.reset();
    AccountChangeDecision {
        drop_snapshot: true,
    }
}
