// Ported from CodexBar Sources/CodexBar/UsageStoreSupport.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

/// Tracks consecutive failures to hide single transient flakes when fresh data was present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FailureGate {
    pub streak: u32,
}

impl FailureGate {
    /// Creates a new failure gate with zero consecutive failures.
    pub const fn new() -> Self {
        Self { streak: 0 }
    }

    /// Resets consecutive failure streak to zero on success.
    pub fn record_success(&mut self) {
        self.streak = 0;
    }

    /// Resets consecutive failure streak to zero.
    pub fn reset(&mut self) {
        self.streak = 0;
    }

    /// Returns the current streak of consecutive failures.
    pub fn streak(&self) -> u32 {
        self.streak
    }

    /// Increments failure streak and returns whether the failure should be surfaced.
    ///
    /// The first consecutive failure is hidden when prior data exists; subsequent
    /// failures surface immediately.
    pub fn should_surface(&mut self, had_prior: bool) -> bool {
        self.streak = self.streak.saturating_add(1);
        !(had_prior && self.streak == 1)
    }
}
