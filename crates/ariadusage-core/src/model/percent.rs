// Ported from CodexBar Sources/CodexBarCore/UsagePercent.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use crate::error::ModelError;

/// A provider-reported usage percentage before and after display normalization.
///
/// Keeps `raw` when over-quota values carry meaning for provider-specific details or pace diagnostics.
/// Uses `display_clamped` when projecting a percentage into a headline or `RateWindow` display value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsagePercent {
    pub raw: f64,
}

impl UsagePercent {
    pub fn new(raw: f64) -> Self {
        Self { raw }
    }

    /// Computes an unbounded percentage from a numeric quota ratio.
    ///
    /// Requires a positive limit.
    pub fn from_ratio(used: f64, limit: f64) -> Result<Self, ModelError> {
        if limit <= 0.0 || !limit.is_finite() || !used.is_finite() {
            return Err(ModelError::NonFiniteNumber(
                "Usage percent requires a positive finite limit and finite used value".to_string(),
            ));
        }
        Ok(Self {
            raw: (used / limit) * 100.0,
        })
    }

    /// Unchecked ratio constructor mirroring Swift's `init(used:limit:)` with assertion.
    pub fn from_ratio_unclamped(used: f64, limit: f64) -> Self {
        assert!(limit > 0.0, "Usage percent requires a positive limit");
        Self {
            raw: (used / limit) * 100.0,
        }
    }

    pub fn display_clamped(&self) -> f64 {
        self.raw.clamp(0.0, 100.0)
    }
}
