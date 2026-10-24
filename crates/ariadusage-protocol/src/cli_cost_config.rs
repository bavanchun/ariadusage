// Ported from CodexBar Sources/CodexBarCLI/CLIPayloads.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIConfigCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! CLI JSON payload types for `ariadusage cost` and `ariadusage config`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Root JSON payload emitted by `ariadusage cost --format json` (serialized as a list of provider payloads).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostPayload {
    pub provider: String,
    pub source: String,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub updated_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_tokens: Option<i64>,
    #[serde(
        rename = "sessionCostUSD",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub session_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_days: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_period: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_coverage_is_established: Option<bool>,
    #[serde(
        rename = "last30DaysTokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_30_days_tokens: Option<i64>,
    #[serde(
        rename = "last30DaysCostUSD",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_30_days_cost_usd: Option<f64>,
    #[serde(
        rename = "meteredCostUSD",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub metered_cost_usd: Option<f64>,
    pub daily: Vec<CostDailyEntryPayload>,
    #[serde(default)]
    pub projects: Vec<CostProjectPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totals: Option<CostTotalsPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CostUsageCoverageCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_request_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CostErrorPayload>,
}

/// Daily token and cost row within [`CostPayload`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostDailyEntryPayload {
    pub date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    #[serde(rename = "totalCost", default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_used: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_request_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_breakdowns: Option<Vec<CostModelBreakdownPayload>>,
}

/// Per-model cost breakdown entry within [`CostDailyEntryPayload`], [`CostProjectPayload`], or [`CostProjectSourcePayload`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostModelBreakdownPayload {
    pub model_name: String,
    #[serde(rename = "cost", default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_request_count: Option<i64>,
}

/// Per-project rollup within [`CostPayload`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostProjectPayload {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    #[serde(rename = "totalCost", default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    pub daily: Vec<CostDailyEntryPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_breakdowns: Option<Vec<CostModelBreakdownPayload>>,
    #[serde(default)]
    pub sources: Vec<CostProjectSourcePayload>,
}

/// Per-source rollup within [`CostProjectPayload`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostProjectSourcePayload {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    #[serde(rename = "totalCost", default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    pub daily: Vec<CostDailyEntryPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_breakdowns: Option<Vec<CostModelBreakdownPayload>>,
}

/// Aggregate window totals within [`CostPayload`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostTotalsPayload {
    #[serde(
        rename = "inputTokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub total_input_tokens: Option<i64>,
    #[serde(
        rename = "outputTokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub total_output_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    #[serde(rename = "totalCost", default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_request_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CostUsageCoverageCounts>,
}

/// Request/row coverage counts for a cost window (`CostUsageCoverageCounts` in `CostProvenance.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostUsageCoverageCounts {
    pub priced: i64,
    pub unpriced: i64,
    pub unmetered: i64,
    pub estimated: i64,
}

impl CostUsageCoverageCounts {
    #[must_use]
    pub const fn new(priced: i64, unpriced: i64, unmetered: i64, estimated: i64) -> Self {
        Self {
            priced: if priced > 0 { priced } else { 0 },
            unpriced: if unpriced > 0 { unpriced } else { 0 },
            unmetered: if unmetered > 0 { unmetered } else { 0 },
            estimated: if estimated > 0 { estimated } else { 0 },
        }
    }

    #[must_use]
    pub const fn total(&self) -> i64 {
        self.priced
            .saturating_add(self.unpriced)
            .saturating_add(self.unmetered)
            .saturating_add(self.estimated)
    }

    pub fn merge(&mut self, other: Self) {
        self.priced = self.priced.saturating_add(other.priced);
        self.unpriced = self.unpriced.saturating_add(other.unpriced);
        self.unmetered = self.unmetered.saturating_add(other.unmetered);
        self.estimated = self.estimated.saturating_add(other.estimated);
    }
}

/// Structured error payload embedded in [`CostPayload`] or emitted on CLI failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostErrorPayload {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// Provider row emitted by `ariadusage config providers --format json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProviderStatusResult {
    pub provider: String,
    pub display_name: String,
    pub enabled: bool,
    pub default_enabled: bool,
}

/// Result payload emitted by `ariadusage config enable|disable --format json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProviderToggleResult {
    pub provider: String,
    pub display_name: String,
    pub enabled: bool,
    pub config_path: String,
}

/// Result payload emitted by `ariadusage config set-source --format json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSetSourceResult {
    pub provider: String,
    pub display_name: String,
    pub enabled: bool,
    pub source: String,
    pub config_path: String,
}
