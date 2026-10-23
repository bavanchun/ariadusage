// Ported from CodexBar Sources/CodexBarCore/ProviderDetailSection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Usage data models including rate windows, credits, cost, cost history, plan utilization,
//! identity, pace, status components, and bounded detail sections.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::ids::ProviderId;
use crate::metric::Metric;

/// Session-equivalent forecast estimating how many full primary sessions the
/// remaining weekly headroom can cover at the current session pace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionEquivalentForecast {
    /// Whole primary-session equivalents remaining before the weekly window resets.
    pub whole_session_count: u32,
    /// Remaining partial session percentage (`0..100`).
    pub remainder_percent: u32,
}

impl SessionEquivalentForecast {
    #[must_use]
    pub const fn new(whole_session_count: u32, remainder_percent: u32) -> Self {
        Self {
            whole_session_count,
            remainder_percent,
        }
    }
}

/// A rate window indicating quota consumption and reset timing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateWindow {
    /// Percentage of quota consumed (raw, may exceed 100%).
    pub used_percent: f64,
    /// Duration of the window in minutes (e.g. 300 for 5 hours, 10080 for weekly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
    /// Timestamp when this window resets.
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub resets_at: Option<jiff::Timestamp>,
    /// Human-readable description of reset timing (e.g. "Resets tomorrow at 9 AM").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_description: Option<String>,
    /// Percentage restored on next regeneration increment, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_regen_percent: Option<f64>,
    /// Per-window pace comparing consumption to elapsed window progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
    /// Session-equivalent forecast for weekly windows when primary session pace is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_equivalent: Option<SessionEquivalentForecast>,
}

impl RateWindow {
    pub fn new(used_percent: f64) -> Self {
        Self {
            used_percent,
            window_minutes: None,
            resets_at: None,
            reset_description: None,
            next_regen_percent: None,
            pace: None,
            session_equivalent: None,
        }
    }
}

/// A named rate window for secondary/tertiary/custom provider allowances.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NamedWindow {
    pub id: String,
    pub title: String,
    pub window: Metric<RateWindow>,
}

/// Recurring credit limit allowance paired with a provider's credits balance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreditLimit {
    pub used: f64,
    pub total: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub resets_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_description: Option<String>,
}

/// Token or service credits snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    pub remaining: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance_is_workspace: Option<bool>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub updated_at: Option<jiff::Timestamp>,
    /// False when the balance read failed and `remaining` should not be rendered as a live number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance_read_succeeded: Option<bool>,
    /// Whether credits can currently be spent on this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits_available: Option<bool>,
    /// Recurring credit allowance limit and reset cadence, when the provider exposes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_limit: Option<CreditLimit>,
    /// Number of instant-reset credit grants available (e.g. Codex reset credits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_credits_available_count: Option<u32>,
    /// Expiry timestamp of the next expiring reset credit grant.
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub next_reset_credit_expires_at: Option<jiff::Timestamp>,
}

/// Cost and spend tracking snapshot (e.g. extra usage spend cap and prepaid cloud balance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    pub used: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    pub currency_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub resets_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_regen_amount: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_used: Option<f64>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub balance_updated_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance_is_workspace: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance_is_unavailable: Option<bool>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub updated_at: Option<jiff::Timestamp>,
}

/// Provenance of reported token cost figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CostProvenance {
    BilledSpend,
    ProviderEstimate,
    LocalPricingEstimate,
    MixedProviderAndLocalEstimate,
    QuotaOnly,
    #[serde(other)]
    Unknown,
}

/// Per-model cost and token breakdown within a daily entry or rolling window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostModelBreakdown {
    pub model_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_tokens: Option<u64>,
}

/// Single day in the 30-day token and cost ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostDailyEntry {
    /// Calendar date in `YYYY-MM-DD` format.
    pub date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models_used: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_breakdowns: Vec<CostModelBreakdown>,
}

/// Rolling comparison window summary (e.g. 7d / 14d / 30d) shown in the Cost section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostWindowSummary {
    pub days: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_partial: bool,
}

/// Local or provider-reported token cost history for the card Cost section and
/// the Usage Breakdown chart submenu.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CostHistory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_30_days_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_30_days_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_30_days_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_label: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub history_coverage_is_established: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub history_scan_is_partial: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metered_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_provenance: Option<CostProvenance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub daily: Vec<CostDailyEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparison_windows: Vec<CostWindowSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_model: Option<CostModelBreakdown>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hint_lines: Vec<String>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub updated_at: Option<jiff::Timestamp>,
}

/// A single historical observation in a plan utilization series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanUtilizationEntry {
    #[schemars(with = "String")]
    #[serde(with = "crate::time::rfc3339")]
    pub captured_at: jiff::Timestamp,
    pub used_percent: f64,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub resets_at: Option<jiff::Timestamp>,
}

/// Historical utilization series for a plan rate window (e.g. `"session"`, `"weekly"`, `"opus"`),
/// backing the Plan Usage submenu and quota burndown chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanUtilizationSeries {
    pub name: String,
    pub window_minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<PlanUtilizationEntry>,
}

/// Identity and account provenance for a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub provider_id: ProviderId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// Progression stage for usage pace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PaceStage {
    OnTrack,
    SlightlyAhead,
    Ahead,
    FarAhead,
    SlightlyBehind,
    Behind,
    FarBehind,
    #[serde(other)]
    Unknown,
}

/// Pace metrics comparing consumption to linear time progression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pace {
    pub stage: PaceStage,
    pub delta_percent: f64,
    pub expected_used_percent: f64,
    pub will_last_to_reset: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_multiplier_to_reset: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_out_probability: Option<f64>,
    pub summary: String,
}

/// High-level service status indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StatusIndicator {
    None,
    Minor,
    Major,
    Critical,
    Maintenance,
    #[serde(other)]
    Unknown,
}

/// Individual service component on a provider status page, with optional nested subcomponents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusComponent {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_status: Option<String>,
    pub indicator: StatusIndicator,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub updated_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<StatusComponent>,
}

/// Full provider status page snapshot backing the card status banner and Status Page submenu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub indicator: StatusIndicator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub updated_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<StatusComponent>,
}

/// Classified error kinds matching CodexBar's error model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderErrorKind {
    AuthenticationExpired,
    MissingCredential,
    PermissionDenied,
    RateLimited,
    ProviderUnavailable,
    ParseFailure,
    NetworkFailure,
    ApiFailure,
    #[serde(other)]
    Unknown,
}

/// High-level diagnostic categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProviderErrorCategory {
    Auth,
    Api,
    Parse,
    Network,
    Configuration,
    #[serde(other)]
    Unknown,
}

impl ProviderErrorCategory {
    /// Safe static description of this category for transmission over IPC.
    pub const fn safe_description(&self) -> &'static str {
        match self {
            Self::Auth => "Authentication or credential failure",
            Self::Api => "Provider API failure",
            Self::Parse => "Provider response parse failure",
            Self::Network => "Network transport failure",
            Self::Configuration => "Provider configuration invalid",
            Self::Unknown => "Unknown provider error",
        }
    }
}

/// A classified provider error with a safe message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub category: ProviderErrorCategory,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

// ============================================================================
// Bounded Detail Sections
// ============================================================================

/// Maximum number of detail sections allowed in a snapshot.
pub const MAXIMUM_SECTIONS_PER_SNAPSHOT: usize = 8;
/// Maximum number of rows allowed in a detail section.
pub const MAXIMUM_ROWS_PER_SECTION: usize = 24;
/// Maximum number of points allowed in a chart.
pub const MAXIMUM_POINTS_PER_CHART: usize = 120;
/// Maximum character length for titles, labels, and text values.
pub const MAXIMUM_STRING_LENGTH: usize = 120;

/// Validation error for bounded detail sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailSectionValidationError(pub String);

impl fmt::Display for DetailSectionValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DetailSectionValidationError {}

fn required_string(
    raw: impl AsRef<str>,
    path: &str,
) -> Result<String, DetailSectionValidationError> {
    use unicode_segmentation::UnicodeSegmentation;

    let s = raw.as_ref().trim();
    if s.is_empty() {
        return Err(DetailSectionValidationError(format!(
            "{path} must not be empty"
        )));
    }
    if s.graphemes(true).count() > MAXIMUM_STRING_LENGTH {
        return Err(DetailSectionValidationError(format!(
            "{path} exceeds {MAXIMUM_STRING_LENGTH} characters"
        )));
    }
    Ok(s.to_string())
}

fn optional_string(
    raw: Option<impl AsRef<str>>,
    path: &str,
) -> Result<Option<String>, DetailSectionValidationError> {
    use unicode_segmentation::UnicodeSegmentation;

    let Some(raw) = raw else { return Ok(None) };
    let s = raw.as_ref().trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.graphemes(true).count() > MAXIMUM_STRING_LENGTH {
        return Err(DetailSectionValidationError(format!(
            "{path} exceeds {MAXIMUM_STRING_LENGTH} characters"
        )));
    }
    Ok(Some(s.to_string()))
}

/// Numeric ratio progress for a detail row.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RowProgress {
    pub used: f64,
    pub total: f64,
}

impl RowProgress {
    /// Create new progress, ensuring numbers are finite and total > 0.
    pub fn new(used: f64, total: f64) -> Result<Self, DetailSectionValidationError> {
        if !used.is_finite() || !total.is_finite() {
            return Err(DetailSectionValidationError(
                "row.progress values must be finite".to_string(),
            ));
        }
        if total <= 0.0 {
            return Err(DetailSectionValidationError(
                "row.progress.total must be positive".to_string(),
            ));
        }
        Ok(Self { used, total })
    }

    /// Percent consumed (unclamped).
    #[must_use]
    pub fn used_percent(&self) -> f64 {
        (self.used / self.total) * 100.0
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RowProgressRaw {
    used: f64,
    total: f64,
}

impl<'de> Deserialize<'de> for RowProgress {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RowProgressRaw::deserialize(deserializer)?;
        Self::new(raw.used, raw.total).map_err(serde::de::Error::custom)
    }
}

/// Display emphasis a provider assigns to a detail row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RowSeverity {
    Normal,
    Warning,
    Critical,
    #[serde(other)]
    Unknown,
}

/// A row within a detail section.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DetailRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub label: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<RowProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<RowSeverity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_value: Option<f64>,
}

impl DetailRow {
    pub fn new(
        id: Option<impl AsRef<str>>,
        label: impl AsRef<str>,
        value: impl AsRef<str>,
        secondary_value: Option<impl AsRef<str>>,
        progress: Option<RowProgress>,
        usage_value: Option<f64>,
    ) -> Result<Self, DetailSectionValidationError> {
        let id = optional_string(id, "row.id")?;
        let label = required_string(label, "row.label")?;
        let value = required_string(value, "row.value")?;
        let secondary_value = optional_string(secondary_value, "row.secondaryValue")?;
        if matches!(usage_value, Some(uv) if !uv.is_finite()) {
            return Err(DetailSectionValidationError(
                "row.usageValue must be finite".to_string(),
            ));
        }
        Ok(Self {
            id,
            label,
            value,
            secondary_value,
            progress,
            severity: None,
            usage_value,
        })
    }

    /// Returns the row with the given display severity.
    #[must_use]
    pub fn with_severity(mut self, severity: Option<RowSeverity>) -> Self {
        self.severity = severity;
        self
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailRowRaw {
    #[serde(default)]
    id: Option<String>,
    label: String,
    value: String,
    #[serde(default)]
    secondary_value: Option<String>,
    #[serde(default)]
    progress: Option<RowProgress>,
    #[serde(default)]
    severity: Option<RowSeverity>,
    #[serde(default)]
    usage_value: Option<f64>,
}

impl<'de> Deserialize<'de> for DetailRow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = DetailRowRaw::deserialize(deserializer)?;
        Self::new(
            raw.id,
            raw.label,
            raw.value,
            raw.secondary_value,
            raw.progress,
            raw.usage_value,
        )
        .map(|row| row.with_severity(raw.severity))
        .map_err(serde::de::Error::custom)
    }
}

/// Kind of chart to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ChartKind {
    Bars,
    Line,
}

/// A data point in a chart.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChartPoint {
    pub label: String,
    pub value: f64,
}

impl ChartPoint {
    pub fn new(label: impl AsRef<str>, value: f64) -> Result<Self, DetailSectionValidationError> {
        let label = required_string(label, "chart.point.label")?;
        if !value.is_finite() {
            return Err(DetailSectionValidationError(
                "chart.point.value must be finite".to_string(),
            ));
        }
        Ok(Self { label, value })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChartPointRaw {
    label: String,
    value: f64,
}

impl<'de> Deserialize<'de> for ChartPoint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = ChartPointRaw::deserialize(deserializer)?;
        Self::new(raw.label, raw.value).map_err(serde::de::Error::custom)
    }
}

/// A chart embedded in a detail section.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Chart {
    pub kind: ChartKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[schemars(length(max = 120))]
    pub points: Vec<ChartPoint>,
}

impl Chart {
    pub fn new(
        kind: ChartKind,
        title: Option<impl AsRef<str>>,
        unit: Option<impl AsRef<str>>,
        points: Vec<ChartPoint>,
    ) -> Result<Self, DetailSectionValidationError> {
        if points.len() > MAXIMUM_POINTS_PER_CHART {
            return Err(DetailSectionValidationError(format!(
                "chart.points exceeds {MAXIMUM_POINTS_PER_CHART} entries"
            )));
        }
        let title = optional_string(title, "chart.title")?;
        let unit = optional_string(unit, "chart.unit")?;
        Ok(Self {
            kind,
            title,
            unit,
            points,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChartRaw {
    kind: ChartKind,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    unit: Option<String>,
    points: Vec<ChartPoint>,
}

impl<'de> Deserialize<'de> for Chart {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = ChartRaw::deserialize(deserializer)?;
        Self::new(raw.kind, raw.title, raw.unit, raw.points).map_err(serde::de::Error::custom)
    }
}

/// A section of detailed rows and optional chart.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DetailSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[schemars(length(max = 24))]
    pub rows: Vec<DetailRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chart: Option<Chart>,
}

impl DetailSection {
    pub fn new(
        title: Option<impl AsRef<str>>,
        rows: Vec<DetailRow>,
        chart: Option<Chart>,
    ) -> Result<Self, DetailSectionValidationError> {
        if rows.len() > MAXIMUM_ROWS_PER_SECTION {
            return Err(DetailSectionValidationError(format!(
                "section.rows exceeds {MAXIMUM_ROWS_PER_SECTION} entries"
            )));
        }
        let title = optional_string(title, "section.title")?;
        Ok(Self { title, rows, chart })
    }

    /// Validate that sections do not exceed [`MAXIMUM_SECTIONS_PER_SNAPSHOT`].
    pub fn validate_sections(sections: &[Self]) -> Result<(), DetailSectionValidationError> {
        if sections.len() > MAXIMUM_SECTIONS_PER_SNAPSHOT {
            return Err(DetailSectionValidationError(format!(
                "snapshot.details exceeds {MAXIMUM_SECTIONS_PER_SNAPSHOT} sections"
            )));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailSectionRaw {
    #[serde(default)]
    title: Option<String>,
    rows: Vec<DetailRow>,
    #[serde(default)]
    chart: Option<Chart>,
}

impl<'de> Deserialize<'de> for DetailSection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = DetailSectionRaw::deserialize(deserializer)?;
        Self::new(raw.title, raw.rows, raw.chart).map_err(serde::de::Error::custom)
    }
}
