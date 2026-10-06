// Ported from CodexBar Sources/CodexBarCore/ProviderCostSnapshot.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use serde::{Deserialize, Serialize};

use crate::model::window::RateWindow;

/// Provider-specific spend/budget snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCostSnapshot {
    pub used: f64,
    pub limit: f64,
    pub currency_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub resets_at: Option<jiff::Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_regen_amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personal_used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub balance_updated_at: Option<jiff::Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_is_workspace: Option<bool>,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub updated_at: jiff::Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_is_unavailable: Option<bool>,
}

impl ProviderCostSnapshot {
    pub fn new(
        used: f64,
        limit: f64,
        currency_code: impl Into<String>,
        updated_at: jiff::Timestamp,
    ) -> Self {
        Self {
            used,
            limit,
            currency_code: currency_code.into(),
            period: None,
            resets_at: None,
            next_regen_amount: None,
            personal_used: None,
            balance: None,
            balance_updated_at: None,
            balance_is_workspace: None,
            updated_at,
            balance_is_unavailable: None,
        }
    }

    /// Projects a positive spend budget into a quota meter without assigning it a time-window cadence.
    pub fn spend_limit_window(&self) -> Option<RateWindow> {
        if self.limit > 0.0 && self.limit.is_finite() && self.used.is_finite() {
            let used_percent = ((self.used / self.limit) * 100.0).clamp(0.0, 100.0);
            RateWindow::new(used_percent, None, self.resets_at, None, None, false).ok()
        } else {
            None
        }
    }
}
