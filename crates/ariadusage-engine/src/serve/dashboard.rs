// Ported from CodexBar Sources/CodexBarCLI/DashboardPayloads.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/DashboardSnapshotBuilder.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/DashboardAccountCollector.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/DashboardManagedCodexAccounts.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/DashboardUsageAccount.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIDashboardCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIPayloads.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLICostCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
//
// Dashboard v1 payloads, CLI usage/cost payloads, multi-account collection,
// and snapshot builder for `serve` and `dashboard`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::config::{Config, ProviderEntry, TokenAccountMeta};
use ariadusage_core::model::{ProviderCostSnapshot, RateWindow, UsageSnapshot};
use ariadusage_core::pace::{
    DEFAULT_SESSION_WINDOW_MINUTES, DEFAULT_WEEKLY_WINDOW_MINUTES, UsagePace,
};
use ariadusage_core::pipeline::BoxFuture;
use ariadusage_core::providers;
use ariadusage_protocol::{Confidence, PaceStage, StatusIndicator};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};

use super::accounts::{
    CodexActiveSource, FileManagedCodexAccountStore, ManagedCodexAccount,
    parse_opaque_token_accounts,
};
use super::coordinator::CliServeOperationProgressSink;
use crate::providers::codex::CreditsSnapshot;

fn is_false(value: &bool) -> bool {
    !*value
}

/// How much account identity a dashboard snapshot exposes (`DashboardIdentityMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DashboardIdentityMode {
    None,
    Redacted,
    Full,
}

impl DashboardIdentityMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Redacted => "redacted",
            Self::Full => "full",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "redacted" => Some(Self::Redacted),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DashboardSnapshotDetail {
    #[default]
    Full,
    Shell,
}

impl DashboardSnapshotDetail {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Shell => "shell",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::Full),
            "shell" => Some(Self::Shell),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshotPayload {
    pub schema_version: i32,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub generated_at: Timestamp,
    pub stale_after_seconds: i64,
    pub host: DashboardHostPayload,
    pub providers: Vec<DashboardProviderPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardHostPayload {
    #[serde(alias = "codexBarVersion")]
    pub ariadusage_version: Option<String>,
    pub refresh_interval_seconds: i64,
    pub usage_bars_show_used: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardProviderPayload {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub status: Option<DashboardStatusPayload>,
    #[serde(default)]
    pub identity: Option<DashboardIdentityPayload>,
    #[serde(default)]
    pub windows: Vec<DashboardWindowPayload>,
    #[serde(default)]
    pub credits: Option<DashboardCreditsPayload>,
    #[serde(default)]
    pub cost: Option<DashboardCostPayload>,
    pub display: DashboardDisplayPayload,
    #[serde(default)]
    pub error: Option<ProviderErrorPayload>,
    #[serde(default, with = "ariadusage_protocol::time::rfc3339_opt")]
    pub updated_at: Option<Timestamp>,
    #[serde(default)]
    pub accounts: Option<Vec<DashboardAccountPayload>>,
    #[serde(default)]
    pub accounts_error: Option<String>,
    #[serde(skip)]
    pub detail: DashboardSnapshotDetail,
}

impl Serialize for DashboardProviderPayload {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.detail == DashboardSnapshotDetail::Shell {
            let mut s = serializer.serialize_struct("DashboardProviderPayload", 4)?;
            s.serialize_field("id", &self.id)?;
            s.serialize_field("name", &self.name)?;
            s.serialize_field("enabled", &self.enabled)?;
            s.serialize_field("display", &self.display)?;
            return s.end();
        }
        let mut count = 12;
        if self.accounts.is_some() {
            count += 1;
        }
        if self.accounts_error.is_some() {
            count += 1;
        }
        let mut s = serializer.serialize_struct("DashboardProviderPayload", count)?;
        s.serialize_field("id", &self.id)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("enabled", &self.enabled)?;
        s.serialize_field("display", &self.display)?;
        s.serialize_field("source", &self.source)?;
        s.serialize_field("status", &self.status)?;
        s.serialize_field("identity", &self.identity)?;
        s.serialize_field("windows", &self.windows)?;
        s.serialize_field("credits", &self.credits)?;
        s.serialize_field("cost", &self.cost)?;
        s.serialize_field("error", &self.error)?;
        let updated_at_str = self.updated_at.map(DashboardSnapshotBuilder::iso_string);
        s.serialize_field("updatedAt", &updated_at_str)?;
        if let Some(accounts) = &self.accounts {
            s.serialize_field("accounts", accounts)?;
        }
        if let Some(accounts_error) = &self.accounts_error {
            s.serialize_field("accountsError", accounts_error)?;
        }
        s.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardAccountPayload {
    pub id: String,
    pub label: String,
    pub active: bool,
    pub identity: Option<DashboardIdentityPayload>,
    pub windows: Vec<DashboardWindowPayload>,
    pub pace: Option<ProviderPacePayload>,
    pub error: Option<String>,
    #[serde(with = "ariadusage_protocol::time::rfc3339_opt")]
    pub updated_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardStatusPayload {
    pub level: String,
    pub label: String,
    #[serde(with = "ariadusage_protocol::time::rfc3339_opt")]
    pub updated_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardIdentityPayload {
    pub account_email: Option<String>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardWindowPayload {
    pub kind: String,
    pub label: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    #[serde(with = "ariadusage_protocol::time::rfc3339_opt")]
    pub reset_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub idle: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardCreditsPayload {
    pub remaining: f64,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardCostPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today_incomplete_request_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_30_days_incomplete_request_count: Option<i64>,
    #[serde(rename = "todayUSD", alias = "todayUsd")]
    pub today_usd: Option<f64>,
    #[serde(rename = "last30DaysUSD", alias = "last30DaysUsd")]
    pub last_30_days_usd: Option<f64>,
}

impl DashboardCostPayload {
    #[must_use]
    pub fn new(
        today_usd: Option<f64>,
        last_30_days_usd: Option<f64>,
        today_incomplete_request_count: Option<i64>,
        last_30_days_incomplete_request_count: Option<i64>,
    ) -> Self {
        Self {
            today_usd,
            last_30_days_usd,
            today_incomplete_request_count: today_incomplete_request_count.filter(|&c| c > 0),
            last_30_days_incomplete_request_count: last_30_days_incomplete_request_count
                .filter(|&c| c > 0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardDisplayPayload {
    pub accent_color: String,
    pub sort_key: i32,
    pub priority: String,
}

/// Account presentation metadata travelling beside usage (`DashboardUsageAccount` in `DashboardUsageAccount.swift`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardUsageAccount {
    pub id: String,
    pub label: String,
    pub active: bool,
}

impl DashboardUsageAccount {
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>, active: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            active,
        }
    }

    #[must_use]
    pub fn token(account: &TokenAccountMeta, active: bool) -> Self {
        Self {
            id: format!("token:{}", account.id.to_ascii_lowercase()),
            label: account.label.clone(),
            active,
        }
    }

    #[must_use]
    pub fn codex(
        selection_source: &CodexActiveSource,
        stored_account_id: Option<uuid::Uuid>,
        workspace_account_id: Option<&str>,
        display_name: impl Into<String>,
        is_active: bool,
    ) -> Self {
        let id = match selection_source {
            CodexActiveSource::LiveSystem => stored_account_id
                .map(|id| {
                    format!(
                        "codex-managed:{}",
                        id.hyphenated().to_string().to_ascii_lowercase()
                    )
                })
                .unwrap_or_else(|| {
                    Self::opaque_codex_id(&format!(
                        "system:{}",
                        workspace_account_id.unwrap_or("default")
                    ))
                }),
            CodexActiveSource::ManagedAccount { id } => {
                format!(
                    "codex-managed:{}",
                    id.hyphenated().to_string().to_ascii_lowercase()
                )
            }
            CodexActiveSource::ProfileHome { path } => {
                let normalized = Self::normalized_home_path(path);
                Self::opaque_codex_id(&format!("profile:{normalized}"))
            }
        };
        Self {
            id,
            label: display_name.into(),
            active: is_active,
        }
    }

    #[must_use]
    pub fn codex_visible(account: &CodexVisibleAccount) -> Self {
        Self::codex(
            &account.selection_source,
            account.stored_account_id,
            account.workspace_account_id.as_deref(),
            &account.email,
            account.is_active,
        )
    }

    #[must_use]
    pub fn opaque_codex_id(source: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(source.as_bytes());
        let digest = hasher.finalize();
        let mut hex = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write;
            let _ = write!(&mut hex, "{b:02x}");
        }
        format!("codex:{hex}")
    }

    fn normalized_home_path(path: &str) -> String {
        let trimmed = path.trim();
        let stripped = trimmed.trim_end_matches('/');
        if stripped.is_empty() {
            "/".to_string()
        } else {
            stripped.to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAccountIdentity {
    pub source: String,
    pub opaque_id: String,
}

impl ProviderAccountIdentity {
    #[must_use]
    pub fn new(source: impl Into<String>, opaque_id: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            opaque_id: opaque_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderAccountUsageSnapshot {
    pub id: ProviderAccountIdentity,
    pub provider: String,
    pub display_label: String,
    pub account_email: Option<String>,
    pub is_active: bool,
    pub uses_last_known_usage: bool,
    pub snapshot: Option<UsageSnapshot>,
    pub error: Option<String>,
    pub source_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DashboardAccountsInput {
    pub accounts: Option<Vec<ProviderAccountUsageSnapshot>>,
    pub adapter_error: Option<String>,
    pub weekly_work_days: Option<u32>,
}

// ============================================================================
// CLI Usage & Cost Payloads (`CLIPayloads.swift` & `CLICostCommand.swift`)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderErrorPayload {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl ProviderErrorPayload {
    #[must_use]
    pub fn new(code: i32, message: impl Into<String>, kind: Option<&str>) -> Self {
        Self {
            code,
            message: message.into(),
            kind: kind.map(ToOwned::to_owned),
        }
    }

    #[must_use]
    pub fn provider(message: impl Into<String>) -> Self {
        Self::new(1, message, Some("provider"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatusPayload {
    pub indicator: StatusIndicator,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, with = "ariadusage_protocol::time::rfc3339_opt")]
    pub updated_at: Option<Timestamp>,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityPlanInfoSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_short_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPacePayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<PacePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<PacePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tertiary: Option<PacePayload>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PacePayload {
    pub stage: String,
    pub delta_percent: f64,
    pub expected_used_percent: f64,
    pub will_last_to_reset: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_out_probability: Option<f64>,
    pub summary: String,
}

/// Vendor-reported or local token cost history attached to a `UsageSnapshot` (`CostUsageTokenSnapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsageTokenSnapshot {
    pub session_tokens: Option<i64>,
    pub session_cost_usd: Option<f64>,
    pub last_30_days_tokens: Option<i64>,
    pub last_30_days_cost_usd: Option<f64>,
    pub currency_code: String,
    pub history_days: i32,
    pub history_label: Option<String>,
    pub daily: Vec<CostDailyEntryPayload>,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub updated_at: Timestamp,
}

impl CostUsageTokenSnapshot {
    #[must_use]
    pub fn new(
        last_30_days_cost_usd: Option<f64>,
        daily: Vec<CostDailyEntryPayload>,
        updated_at: Timestamp,
    ) -> Self {
        Self {
            session_tokens: None,
            session_cost_usd: None,
            last_30_days_tokens: None,
            last_30_days_cost_usd,
            currency_code: "USD".to_string(),
            history_days: 30,
            history_label: None,
            daily,
            updated_at,
        }
    }
}

/// Wire payload for `GET /usage` (`ProviderPayload` in `CLIPayloads.swift`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPayload {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ProviderStatusPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<ProviderPacePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<CreditsSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub antigravity_plan_info: Option<AntigravityPlanInfoSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_dashboard: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderErrorPayload>,

    /// Non-serialized account discriminator used to invalidate cached responses on account switch.
    #[serde(skip)]
    pub cache_account_key: Option<String>,
    /// Non-serialized multi-account metadata used by `DashboardSnapshotBuilder`.
    #[serde(skip)]
    pub dashboard_account: Option<DashboardUsageAccount>,
    /// Non-serialized flag indicating that multi-account discovery was incomplete.
    #[serde(skip)]
    pub dashboard_accounts_incomplete: bool,
    /// Non-serialized reported cost history (`UsageSnapshot.costUsage` in Swift).
    #[serde(skip)]
    pub cost_usage: Option<CostUsageTokenSnapshot>,
}

impl ProviderPayload {
    #[must_use]
    pub fn new(provider: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            account: None,
            version: None,
            source: source.into(),
            status: None,
            usage: None,
            pace: None,
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
            cache_account_key: None,
            dashboard_account: None,
            dashboard_accounts_incomplete: false,
            cost_usage: None,
        }
    }

    #[must_use]
    pub fn with_usage(mut self, usage: UsageSnapshot) -> Self {
        self.usage = Some(usage);
        self
    }

    #[must_use]
    pub fn with_cost_usage(mut self, cost_usage: Option<CostUsageTokenSnapshot>) -> Self {
        self.cost_usage = cost_usage;
        self
    }

    #[must_use]
    pub fn with_error(mut self, error: ProviderErrorPayload) -> Self {
        self.error = Some(error);
        self
    }
}

/// Collected usage output across one or more providers (`UsageCommandOutput`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageCommandOutput {
    pub sections: Vec<String>,
    pub payload: Vec<ProviderPayload>,
    pub exit_code: i32,
}

impl UsageCommandOutput {
    #[must_use]
    pub fn with_sections(sections: Vec<String>) -> Self {
        Self {
            sections,
            payload: Vec::new(),
            exit_code: 0,
        }
    }

    pub fn merge(&mut self, other: Self) {
        self.sections.extend(other.sections);
        self.payload.extend(other.payload);
        if other.exit_code != 0 {
            self.exit_code = other.exit_code;
        }
    }

    pub fn attach_dashboard_account(
        &mut self,
        account: Option<&DashboardUsageAccount>,
        inventory_incomplete: bool,
    ) {
        for item in &mut self.payload {
            if let Some(acc) = account {
                item.dashboard_account = Some(acc.clone());
            }
            item.dashboard_accounts_incomplete =
                item.dashboard_accounts_incomplete || inventory_incomplete;
        }
    }
}

/// Wire payload for `GET /cost` (`CostPayload` in `CLIPayloads.swift`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostPayload {
    pub provider: String,
    pub source: String,
    #[serde(default, with = "ariadusage_protocol::time::rfc3339_opt")]
    pub updated_at: Option<Timestamp>,
    pub session_tokens: Option<i64>,
    #[serde(rename = "sessionCostUSD", alias = "sessionCostUsd")]
    pub session_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_days: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_provenance: Option<String>,
    pub last_30_days_tokens: Option<i64>,
    #[serde(rename = "last30DaysCostUSD", alias = "last30DaysCostUsd")]
    pub last_30_days_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete_request_count: Option<i64>,
    pub daily: Vec<CostDailyEntryPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<CostProjectPayload>>,
    pub totals: Option<CostTotalsPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderErrorPayload>,
}

impl CostPayload {
    #[must_use]
    pub fn new(provider: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            source: source.into(),
            updated_at: None,
            session_tokens: None,
            session_cost_usd: None,
            history_days: None,
            history_label: None,
            cost_provenance: None,
            last_30_days_tokens: None,
            last_30_days_cost_usd: None,
            incomplete_request_count: None,
            daily: Vec::new(),
            projects: None,
            totals: None,
            error: None,
        }
    }

    #[must_use]
    pub fn from_token_snapshot(
        provider: impl Into<String>,
        snapshot: Option<CostUsageTokenSnapshot>,
        error: Option<ProviderErrorPayload>,
    ) -> Self {
        let provider_str = provider.into();
        let Some(snap) = snapshot else {
            let mut item = Self::new(provider_str, "local");
            item.error = error;
            return item;
        };
        let incomplete_sum: i64 = snap
            .daily
            .iter()
            .filter_map(|d| d.incomplete_request_count)
            .filter(|&c| c > 0)
            .sum();
        let total_cost: f64 = snap.daily.iter().filter_map(|d| d.cost_usd).sum();
        let has_any_cost = snap.daily.iter().any(|d| d.cost_usd.is_some());
        let total_tokens: i64 = snap.daily.iter().filter_map(|d| d.total_tokens).sum();
        let has_any_tokens = snap.daily.iter().any(|d| d.total_tokens.is_some());

        // For histories longer than 30 days, narrow `last_30_days_cost_usd` to the 30-day window ending at `snap.updated_at`.
        let last_30_days_cost_usd = if snap.history_days > 30 {
            let (amt, _) =
                DashboardSnapshotBuilder::narrow_thirty_day_entries(&snap.daily, snap.updated_at);
            amt
        } else {
            snap.last_30_days_cost_usd
        };

        Self {
            provider: provider_str,
            source: "local".to_string(),
            updated_at: Some(snap.updated_at),
            session_tokens: snap.session_tokens,
            session_cost_usd: snap.session_cost_usd,
            history_days: Some(snap.history_days),
            history_label: snap.history_label,
            cost_provenance: None,
            last_30_days_tokens: snap.last_30_days_tokens,
            last_30_days_cost_usd,
            incomplete_request_count: (incomplete_sum > 0).then_some(incomplete_sum),
            daily: snap.daily,
            projects: None,
            totals: (has_any_cost || has_any_tokens || incomplete_sum > 0).then(|| {
                CostTotalsPayload {
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    total_tokens: has_any_tokens.then_some(total_tokens),
                    total_cost_usd: has_any_cost.then_some(total_cost),
                    incomplete_request_count: (incomplete_sum > 0).then_some(incomplete_sum),
                }
            }),
            error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostDailyEntryPayload {
    pub date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    #[serde(
        rename = "costUSD",
        alias = "costUsd",
        skip_serializing_if = "Option::is_none"
    )]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete_request_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_used: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_breakdowns: Option<Vec<CostModelBreakdownPayload>>,
}

impl CostDailyEntryPayload {
    #[must_use]
    pub fn new(
        date: impl Into<String>,
        cost_usd: Option<f64>,
        total_tokens: Option<i64>,
        incomplete_request_count: Option<i64>,
    ) -> Self {
        Self {
            date: date.into(),
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            total_tokens,
            cost_usd,
            incomplete_request_count: incomplete_request_count.filter(|&c| c > 0),
            models_used: None,
            model_breakdowns: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostProjectPayload {
    pub name: String,
    pub path: String,
    pub total_tokens: Option<i64>,
    #[serde(rename = "totalCostUSD", alias = "totalCostUsd")]
    pub total_cost_usd: Option<f64>,
    pub daily: Vec<CostDailyEntryPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_breakdowns: Option<Vec<CostModelBreakdownPayload>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostModelBreakdownPayload {
    pub model_name: String,
    #[serde(
        rename = "costUSD",
        alias = "costUsd",
        skip_serializing_if = "Option::is_none"
    )]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostTotalsPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    #[serde(
        rename = "totalCostUSD",
        alias = "totalCostUsd",
        skip_serializing_if = "Option::is_none"
    )]
    pub total_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete_request_count: Option<i64>,
}

// ============================================================================
// DashboardSnapshotBuilder (`DashboardSnapshotBuilder.swift`)
// ============================================================================

struct ProviderPresentationMeta {
    id: String,
    name: String,
    enabled: bool,
    display: DashboardDisplayPayload,
}

pub struct DashboardSnapshotBuilder;

impl DashboardSnapshotBuilder {
    pub const SCHEMA_VERSION: i32 = 1;

    #[must_use]
    pub fn iso_string(ts: Timestamp) -> String {
        ts.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn make_snapshot(
        usage_payloads: &[ProviderPayload],
        cost_payloads: &[CostPayload],
        config: &Config,
        identity_mode: DashboardIdentityMode,
        generated_at: Timestamp,
        refresh_interval: f64,
        ariadusage_version: Option<String>,
        account_collections: &BTreeMap<String, DashboardAccountsInput>,
        usage_bars_show_used: bool,
        all_accounts: bool,
    ) -> DashboardSnapshotPayload {
        let mut cost_by_provider: HashMap<&str, &CostPayload> = HashMap::new();
        for cost in cost_payloads {
            cost_by_provider.insert(cost.provider.as_str(), cost);
        }

        let mut grouped: HashMap<String, Vec<ProviderPayload>> = HashMap::new();
        for item in usage_payloads {
            grouped
                .entry(item.provider.clone())
                .or_default()
                .push(item.clone());
        }

        let selected_payloads = Self::selected_payloads(usage_payloads, all_accounts);
        let mut attached_providers: HashSet<String> = HashSet::new();

        let providers = selected_payloads
            .iter()
            .enumerate()
            .map(|(index, payload)| {
                let row_accounts = if attached_providers.insert(payload.provider.clone()) {
                    account_collections.get(&payload.provider)
                } else {
                    None
                };
                let presentation =
                    Self::provider_presentation(&payload.provider, config, 10_000 + (index as i32));
                let group_slice = grouped
                    .get(&payload.provider)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let incomplete = all_accounts
                    && Self::account_collection_incomplete(&payload.provider, group_slice, config);
                Self::make_provider(
                    payload,
                    cost_by_provider.get(payload.provider.as_str()).copied(),
                    presentation,
                    identity_mode,
                    generated_at,
                    row_accounts,
                    incomplete,
                    all_accounts.then_some(group_slice),
                )
            })
            .collect();

        let refresh_seconds = Self::dashboard_refresh_seconds(refresh_interval);
        DashboardSnapshotPayload {
            schema_version: Self::SCHEMA_VERSION,
            generated_at,
            stale_after_seconds: (refresh_seconds.saturating_mul(3)).max(180),
            host: DashboardHostPayload {
                ariadusage_version,
                refresh_interval_seconds: refresh_seconds,
                usage_bars_show_used,
            },
            providers,
        }
    }

    #[must_use]
    pub fn make_shell_snapshot(
        config: &Config,
        requested_providers: Option<&[String]>,
        generated_at: Timestamp,
        refresh_interval: f64,
        ariadusage_version: Option<String>,
        usage_bars_show_used: bool,
    ) -> DashboardSnapshotPayload {
        let enabled_list: Vec<String> = match requested_providers {
            Some(req) => req.to_vec(),
            None => config.enabled_providers(),
        };
        let rows = enabled_list
            .iter()
            .enumerate()
            .map(|(index, provider_id)| {
                let presentation =
                    Self::provider_presentation(provider_id, config, 10_000 + (index as i32));
                DashboardProviderPayload {
                    id: presentation.id,
                    name: presentation.name,
                    enabled: presentation.enabled,
                    source: String::new(),
                    status: None,
                    identity: None,
                    windows: Vec::new(),
                    credits: None,
                    cost: None,
                    display: presentation.display,
                    error: None,
                    updated_at: None,
                    accounts: None,
                    accounts_error: None,
                    detail: DashboardSnapshotDetail::Shell,
                }
            })
            .collect();

        let refresh_seconds = Self::dashboard_refresh_seconds(refresh_interval);
        DashboardSnapshotPayload {
            schema_version: Self::SCHEMA_VERSION,
            generated_at,
            stale_after_seconds: (refresh_seconds.saturating_mul(3)).max(180),
            host: DashboardHostPayload {
                ariadusage_version,
                refresh_interval_seconds: refresh_seconds,
                usage_bars_show_used,
            },
            providers: rows,
        }
    }

    #[must_use]
    pub fn selected_payloads(
        payloads: &[ProviderPayload],
        all_accounts: bool,
    ) -> Vec<ProviderPayload> {
        if !all_accounts {
            return payloads.to_vec();
        }
        let mut groups: HashMap<&str, Vec<&ProviderPayload>> = HashMap::new();
        for p in payloads {
            groups.entry(p.provider.as_str()).or_default().push(p);
        }
        let mut seen: HashSet<&str> = HashSet::new();
        let mut result = Vec::new();
        for p in payloads {
            if !seen.insert(p.provider.as_str()) {
                continue;
            }
            let selected = groups
                .get(p.provider.as_str())
                .and_then(|list| {
                    list.iter()
                        .copied()
                        .find(|item| item.dashboard_account.as_ref().is_some_and(|a| a.active))
                })
                .unwrap_or(p);
            result.push(selected.clone());
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn make_provider(
        payload: &ProviderPayload,
        cost: Option<&CostPayload>,
        presentation: ProviderPresentationMeta,
        identity_mode: DashboardIdentityMode,
        generated_at: Timestamp,
        account_collection: Option<&DashboardAccountsInput>,
        account_collection_incomplete: bool,
        account_payloads: Option<&[ProviderPayload]>,
    ) -> DashboardProviderPayload {
        let provider_id = payload.provider.as_str();
        let raw_error = payload
            .error
            .as_ref()
            .or_else(|| cost.and_then(|c| c.error.as_ref()));
        let error = raw_error.map(|err| ProviderErrorPayload {
            code: err.code,
            message: if account_payloads.is_some() && identity_mode != DashboardIdentityMode::Full {
                "Account usage unavailable".to_string()
            } else {
                err.message.clone()
            },
            kind: err.kind.clone(),
        });

        let collected_accounts: Option<Vec<DashboardAccountPayload>> =
            account_payloads.map(|items| {
                items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        Self::make_usage_account(item, identity_mode, index + 1, generated_at)
                    })
                    .collect()
            });

        let adapter_accounts: Option<Vec<DashboardAccountPayload>> =
            account_collection.and_then(|coll| {
                if coll.adapter_error.is_some() {
                    return None;
                }
                coll.accounts.as_ref().map(|accs| {
                    accs.iter()
                        .filter(|a| a.provider == provider_id)
                        .enumerate()
                        .map(|(index, account)| {
                            let private_label = (account_payloads.is_some()
                                && identity_mode != DashboardIdentityMode::Full)
                                .then(|| format!("Account {}", index + 1));
                            Self::make_account(
                                account,
                                identity_mode,
                                coll.weekly_work_days,
                                generated_at,
                                private_label.as_deref(),
                            )
                        })
                        .collect()
                })
            });

        let accounts = if account_collection.is_some() {
            adapter_accounts
        } else {
            collected_accounts.filter(|v| !v.is_empty())
        };

        let accounts_error = if let Some(coll) = account_collection {
            coll.adapter_error.as_ref().map(|err_msg| {
                if account_payloads.is_some() && identity_mode != DashboardIdentityMode::Full {
                    "Account list unavailable".to_string()
                } else {
                    err_msg.clone()
                }
            })
        } else if account_collection_incomplete {
            Some("Account list incomplete".to_string())
        } else {
            None
        };

        let cost_payload = if let Some(c) = cost {
            Self::make_cost(c, generated_at)
        } else if let Some(reported) = payload.cost_usage.as_ref() {
            Self::make_reported_cost_token_snapshot(reported)
        } else {
            payload
                .usage
                .as_ref()
                .and_then(|u| u.provider_cost.as_ref())
                .and_then(Self::make_reported_cost)
        };

        let updated_at = Self::updated_at(payload, cost, error.as_ref(), generated_at);

        DashboardProviderPayload {
            id: presentation.id,
            name: presentation.name,
            enabled: presentation.enabled,
            source: Self::dashboard_source(&payload.source),
            status: payload.status.as_ref().map(Self::make_status),
            identity: Self::make_identity(provider_id, payload.usage.as_ref(), identity_mode),
            windows: Self::make_windows(provider_id, payload.usage.as_ref()),
            credits: payload.credits.as_ref().and_then(Self::make_credits),
            cost: cost_payload,
            display: presentation.display,
            error,
            updated_at,
            accounts,
            accounts_error,
            detail: DashboardSnapshotDetail::Full,
        }
    }

    fn account_collection_incomplete(
        provider: &str,
        payloads: &[ProviderPayload],
        config: &Config,
    ) -> bool {
        if payloads.iter().any(|p| p.dashboard_accounts_incomplete) {
            return true;
        }
        let supports_tokens = providers::find_by_id_str(provider)
            .is_some_and(|d| d.token_account_support)
            || matches!(provider, "claude" | "antigravity");
        if !supports_tokens {
            return false;
        }
        let accounts = Self::configured_token_accounts(config, provider);
        if accounts.is_empty() {
            return false;
        }
        let collected_ids: HashSet<&str> = payloads
            .iter()
            .filter_map(|p| p.dashboard_account.as_ref().map(|a| a.id.as_str()))
            .collect();
        accounts.iter().any(|(acc, _)| {
            let token_id = DashboardUsageAccount::token(acc, false).id;
            !collected_ids.contains(token_id.as_str())
        })
    }

    fn make_usage_account(
        payload: &ProviderPayload,
        identity_mode: DashboardIdentityMode,
        number: usize,
        generated_at: Timestamp,
    ) -> Option<DashboardAccountPayload> {
        let account = payload.dashboard_account.as_ref()?;
        let provider_id = payload.provider.as_str();
        let label = if identity_mode == DashboardIdentityMode::Full {
            if account.label.is_empty() {
                "Account".to_string()
            } else {
                account.label.clone()
            }
        } else {
            format!("Account {number}")
        };
        let _ = generated_at;
        let pace = payload.pace.clone();
        let error = payload.error.as_ref().map(|err| {
            if identity_mode == DashboardIdentityMode::Full {
                err.message.clone()
            } else {
                "Account usage unavailable".to_string()
            }
        });
        Some(DashboardAccountPayload {
            id: account.id.clone(),
            label,
            active: account.active,
            identity: Self::make_identity(provider_id, payload.usage.as_ref(), identity_mode),
            windows: Self::make_windows(provider_id, payload.usage.as_ref()),
            pace,
            error,
            updated_at: payload.usage.as_ref().map(|u| u.updated_at),
        })
    }

    fn make_account(
        account: &ProviderAccountUsageSnapshot,
        identity_mode: DashboardIdentityMode,
        weekly_work_days: Option<u32>,
        generated_at: Timestamp,
        private_label: Option<&str>,
    ) -> DashboardAccountPayload {
        let provider_id = account.provider.as_str();
        let source_email: Option<String> = account
            .account_email
            .as_ref()
            .filter(|e| e.contains('@'))
            .cloned()
            .or_else(|| {
                account
                    .snapshot
                    .as_ref()
                    .and_then(|s| Self::snapshot_identity_for(s, provider_id))
                    .and_then(|id| id.account_email.as_ref())
                    .filter(|e| e.contains('@'))
                    .cloned()
            });
        let presented_email = if identity_mode != DashboardIdentityMode::None
            && source_email.as_ref().is_some_and(|e| e.contains('@'))
        {
            Self::dashboard_email(source_email.as_deref(), identity_mode)
        } else {
            None
        };
        let plan = if provider_id == "codex" {
            Self::make_identity(provider_id, account.snapshot.as_ref(), identity_mode)
                .and_then(|i| i.plan)
        } else {
            None
        };
        let identity = presented_email
            .clone()
            .map(|email| DashboardIdentityPayload {
                account_email: Some(email),
                plan,
            });
        let trimmed_label = account.display_label.trim();
        let fallback_label = if trimmed_label.is_empty() {
            format!("Account {}", account.id.opaque_id)
        } else {
            trimmed_label.to_string()
        };
        let label = match private_label {
            Some(pl) => pl.to_string(),
            None => Self::dashboard_account_label(
                &fallback_label,
                source_email.as_deref(),
                presented_email.as_deref(),
                &account.id.opaque_id,
                identity_mode,
            ),
        };
        let pace = account.snapshot.as_ref().and_then(|s| {
            Self::provider_pace_payload(provider_id, s, weekly_work_days, generated_at)
        });
        let error = account.error.as_ref().map(|e| {
            if private_label.is_some() {
                "Account usage unavailable".to_string()
            } else {
                e.clone()
            }
        });
        DashboardAccountPayload {
            id: format!("{}:{}", account.id.source, account.id.opaque_id),
            label,
            active: account.is_active,
            identity,
            windows: Self::make_windows(provider_id, account.snapshot.as_ref()),
            pace,
            error,
            updated_at: account.snapshot.as_ref().map(|s| s.updated_at),
        }
    }

    fn dashboard_account_label(
        display_label: &str,
        source_email: Option<&str>,
        presented_email: Option<&str>,
        account_id: &str,
        identity_mode: DashboardIdentityMode,
    ) -> String {
        if let Some(presented) = presented_email {
            if let Some(source) = source_email
                && display_label.contains('@')
                && (display_label == source || display_label.starts_with(source))
            {
                let suffix = &display_label[source.len()..];
                return format!(
                    "{presented}{}",
                    Self::redact_email_shaped_text(suffix, identity_mode)
                );
            }
            return Self::redact_email_shaped_text(display_label, identity_mode);
        }
        if display_label.contains('@') {
            format!("Account {account_id}")
        } else {
            display_label.to_string()
        }
    }

    fn provider_presentation(
        id: &str,
        config: &Config,
        fallback_sort_key: i32,
    ) -> ProviderPresentationMeta {
        let descriptor = providers::find_by_id_str(id);
        let enabled_set: HashSet<String> = config.enabled_providers().into_iter().collect();
        let ordered = config.ordered_providers();
        let sort_key = ordered
            .iter()
            .position(|p| p == id)
            .map(|idx| (idx as i32) * 10)
            .unwrap_or(fallback_sort_key);
        let accent_override = config
            .provider_config(id)
            .and_then(|c| c.accent_color.as_deref())
            .and_then(Self::normalize_hex_color);
        let accent_color = accent_override.unwrap_or_else(|| {
            descriptor
                .and_then(|d| d.presentation().accent_color)
                .unwrap_or_else(|| Self::default_accent_color(id).to_string())
        });
        let name = descriptor
            .map(|d| d.display_name.to_string())
            .unwrap_or_else(|| Self::default_display_name(id));
        let enabled = if ordered.iter().any(|p| p == id) || descriptor.is_some() {
            enabled_set.contains(id)
        } else {
            true
        };
        ProviderPresentationMeta {
            id: id.to_string(),
            name,
            enabled,
            display: DashboardDisplayPayload {
                accent_color,
                sort_key,
                priority: "normal".to_string(),
            },
        }
    }

    fn normalize_hex_color(raw: &str) -> Option<String> {
        let trimmed = raw.trim().trim_start_matches('#');
        if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(format!("#{}", trimmed.to_ascii_uppercase()))
        } else {
            None
        }
    }

    fn default_accent_color(id: &str) -> &'static str {
        match id {
            "codex" => "#49A3B0",
            "claude" => "#CC7C5E",
            "antigravity" => "#60BA7E",
            _ => "#6E6E6E",
        }
    }

    fn default_display_name(id: &str) -> String {
        match id {
            "codex" => "Codex".to_string(),
            "claude" => "Claude".to_string(),
            "antigravity" => "Antigravity".to_string(),
            other => other.to_string(),
        }
    }

    fn dashboard_source(source: &str) -> String {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            "unknown".to_string()
        } else {
            trimmed.to_string()
        }
    }

    fn make_status(status: &ProviderStatusPayload) -> DashboardStatusPayload {
        let (level, default_label) = match status.indicator {
            StatusIndicator::None => ("ok", "Operational"),
            StatusIndicator::Minor => ("warning", "Partial outage"),
            StatusIndicator::Maintenance => ("warning", "Maintenance"),
            StatusIndicator::Major => ("critical", "Major outage"),
            StatusIndicator::Critical => ("critical", "Critical issue"),
            StatusIndicator::Unknown => ("unknown", "Status unknown"),
        };
        DashboardStatusPayload {
            level: level.to_string(),
            label: default_label.to_string(),
            updated_at: status.updated_at,
        }
    }

    fn snapshot_identity_for<'a>(
        usage: &'a UsageSnapshot,
        provider_id: &str,
    ) -> Option<&'a ariadusage_core::model::ProviderIdentitySnapshot> {
        let id = usage.identity.as_ref()?;
        match id.provider_id.as_ref() {
            Some(pid) if pid.as_str() == provider_id => Some(id),
            None => Some(id),
            _ => None,
        }
    }

    fn make_identity(
        provider_id: &str,
        usage: Option<&UsageSnapshot>,
        mode: DashboardIdentityMode,
    ) -> Option<DashboardIdentityPayload> {
        if mode == DashboardIdentityMode::None {
            return None;
        }
        let snapshot = usage?;
        let identity = Self::snapshot_identity_for(snapshot, provider_id)?;
        let email = Self::dashboard_email(identity.account_email.as_deref(), mode);
        let plan = Self::dashboard_plan(identity.login_method.as_deref(), provider_id);
        if email.is_none() && plan.is_none() {
            return None;
        }
        Some(DashboardIdentityPayload {
            account_email: email,
            plan,
        })
    }

    #[must_use]
    pub fn dashboard_email(email: Option<&str>, mode: DashboardIdentityMode) -> Option<String> {
        let trimmed = email?.trim();
        if trimmed.is_empty() {
            return None;
        }
        if mode == DashboardIdentityMode::None {
            return None;
        }
        if mode != DashboardIdentityMode::Redacted {
            return Some(trimmed.to_string());
        }
        let Some(at_pos) = trimmed.rfind('@') else {
            return Some("redacted".to_string());
        };
        let domain = &trimmed[at_pos + 1..];
        if domain.is_empty() {
            return Some("redacted".to_string());
        }
        Some(format!("redacted@{domain}"))
    }

    #[must_use]
    pub fn redact_email_shaped_text(text: &str, mode: DashboardIdentityMode) -> String {
        if mode != DashboardIdentityMode::Redacted {
            return text.to_string();
        }
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let mut output = String::new();
        let mut cursor_byte = 0;
        let mut i = 0;
        while i < chars.len() {
            if chars[i].1 == '@' {
                let local_start_idx = Self::email_token_start(&chars, i);
                let domain_end_idx = Self::email_token_end(&chars, i);
                if local_start_idx < i && domain_end_idx > i + 1 {
                    let start_byte = chars[local_start_idx].0;
                    let end_byte = if domain_end_idx < chars.len() {
                        chars[domain_end_idx].0
                    } else {
                        text.len()
                    };
                    output.push_str(&text[cursor_byte..start_byte]);
                    let candidate = &text[start_byte..end_byte];
                    output.push_str(
                        &Self::dashboard_email(Some(candidate), DashboardIdentityMode::Redacted)
                            .unwrap_or_else(|| "redacted".to_string()),
                    );
                    cursor_byte = end_byte;
                    i = domain_end_idx;
                    continue;
                }
            }
            i += 1;
        }
        output.push_str(&text[cursor_byte..]);
        output
    }

    fn email_token_start(chars: &[(usize, char)], at_idx: usize) -> usize {
        let mut idx = at_idx;
        let mut in_quoted = false;
        while idx > 0 {
            let prev = idx - 1;
            let ch = chars[prev].1;
            if ch == '"' {
                in_quoted = !in_quoted;
                idx = prev;
                continue;
            }
            if !in_quoted && Self::is_email_boundary(ch) {
                break;
            }
            idx = prev;
        }
        idx
    }

    fn email_token_end(chars: &[(usize, char)], at_idx: usize) -> usize {
        let mut idx = at_idx + 1;
        if idx >= chars.len() {
            return idx;
        }
        if chars[idx].1 == '[' {
            while idx < chars.len() {
                let ch = chars[idx].1;
                idx += 1;
                if ch == ']' {
                    break;
                }
            }
            return idx;
        }
        while idx < chars.len() && !Self::is_email_boundary(chars[idx].1) {
            idx += 1;
        }
        idx
    }

    fn is_email_boundary(ch: char) -> bool {
        ch.is_whitespace() || matches!(ch, '(' | ')' | '<' | '>' | ':' | ',' | ';' | '·' | '/')
    }

    fn dashboard_plan(raw: Option<&str>, provider_id: &str) -> Option<String> {
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            return None;
        }
        if provider_id == "codex" {
            return Some(Self::codex_plan_display_name(trimmed));
        }
        Some(Self::clean_plan_name(trimmed))
    }

    fn codex_plan_display_name(raw: &str) -> String {
        match raw.trim().to_ascii_lowercase().as_str() {
            "plus" => "Plus".to_string(),
            "pro" => "Pro 20x".to_string(),
            "prolite" | "pro_lite" | "pro-lite" => "Pro 5x".to_string(),
            "team" => "Team".to_string(),
            "enterprise" => "Enterprise".to_string(),
            "edu" => "Edu".to_string(),
            "free" => "Free".to_string(),
            _ => Self::clean_plan_name(raw),
        }
    }

    fn clean_plan_name(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let mut chars = trimmed.chars();
        match chars.next() {
            Some(first) if first.is_ascii_lowercase() => {
                format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
            }
            _ => trimmed.to_string(),
        }
    }

    #[must_use]
    pub fn make_windows(
        provider_id: &str,
        usage: Option<&UsageSnapshot>,
    ) -> Vec<DashboardWindowPayload> {
        let Some(usage) = usage else {
            return Vec::new();
        };
        if provider_id == "antigravity"
            && let Some(windows) = Self::antigravity_quota_summary_windows(usage)
        {
            return windows;
        }
        let (primary_label, secondary_label, tertiary_label) =
            Self::rate_window_labels(provider_id);
        let mut windows = Vec::new();

        if let Some(primary) = &usage.primary {
            windows.push(Self::make_window("session", &primary_label, primary, false));
        }
        if let Some(secondary) = &usage.secondary {
            windows.push(Self::make_window(
                "weekly",
                &secondary_label,
                secondary,
                false,
            ));
        }
        if let Some(tertiary) = &usage.tertiary {
            windows.push(Self::make_window(
                "tertiary",
                &tertiary_label,
                tertiary,
                false,
            ));
        }
        if let Some(extras) = &usage.extra_rate_windows {
            for extra in extras {
                windows.push(Self::make_window(
                    &extra.id,
                    &extra.title,
                    &extra.window,
                    false,
                ));
            }
        }
        windows
    }

    fn antigravity_quota_summary_windows(
        usage: &UsageSnapshot,
    ) -> Option<Vec<DashboardWindowPayload>> {
        let extras = usage.extra_rate_windows.as_ref()?;
        if !extras
            .iter()
            .any(|w| w.id.starts_with("antigravity-quota-summary-"))
        {
            return None;
        }
        let idle_ids = Self::antigravity_idle_window_ids(extras);
        Some(
            extras
                .iter()
                .map(|w| {
                    Self::make_window(&w.id, &w.title, &w.window, idle_ids.contains(w.id.as_str()))
                })
                .collect(),
        )
    }

    fn antigravity_idle_window_ids(
        extras: &[ariadusage_core::model::NamedRateWindow],
    ) -> HashSet<&str> {
        let mut family_buckets: BTreeMap<&str, Vec<&ariadusage_core::model::NamedRateWindow>> =
            BTreeMap::new();
        for w in extras {
            if let Some(rest) = w.id.strip_prefix("antigravity-quota-summary-") {
                let family = rest
                    .strip_suffix("-5h")
                    .or_else(|| rest.strip_suffix("-weekly"))
                    .unwrap_or(rest);
                family_buckets.entry(family).or_default().push(w);
            }
        }
        let any_family_used = family_buckets
            .values()
            .any(|lanes| lanes.iter().any(|l| l.window.used_percent > 0.0));
        if !any_family_used {
            return HashSet::new();
        }
        let mut idle = HashSet::new();
        for lanes in family_buckets.values() {
            let all_known_zero = lanes
                .iter()
                .all(|l| l.usage_known && l.window.used_percent <= 0.0);
            if all_known_zero {
                for l in lanes {
                    idle.insert(l.id.as_str());
                }
            }
        }
        idle
    }

    fn rate_window_labels(provider_id: &str) -> (String, String, String) {
        if let Some(desc) = providers::find_by_id_str(provider_id) {
            return (
                desc.session_label.to_string(),
                desc.weekly_label.to_string(),
                desc.opus_label.unwrap_or("Tertiary").to_string(),
            );
        }
        (
            "Session".to_string(),
            "Weekly".to_string(),
            "Tertiary".to_string(),
        )
    }

    fn make_window(
        kind: &str,
        label: &str,
        window: &RateWindow,
        idle: bool,
    ) -> DashboardWindowPayload {
        let used = window.used_percent.clamp(0.0, 100.0);
        let remaining = (100.0 - used).clamp(0.0, 100.0);
        DashboardWindowPayload {
            kind: kind.to_string(),
            label: label.to_string(),
            used_percent: used,
            remaining_percent: remaining,
            reset_at: window.resets_at,
            idle,
        }
    }

    fn make_credits(credits: &CreditsSnapshot) -> Option<DashboardCreditsPayload> {
        if !credits.remaining.is_finite() {
            return None;
        }
        Some(DashboardCreditsPayload {
            remaining: credits.remaining,
            unit: "credits".to_string(),
        })
    }

    fn make_reported_cost_token_snapshot(
        snapshot: &CostUsageTokenSnapshot,
    ) -> Option<DashboardCostPayload> {
        if snapshot.currency_code != "USD" || snapshot.history_days != 30 {
            return None;
        }
        let incomplete_count: i64 = snapshot
            .daily
            .iter()
            .filter_map(|d| d.incomplete_request_count)
            .filter(|&c| c > 0)
            .sum();
        if snapshot.last_30_days_cost_usd.is_none() && incomplete_count <= 0 {
            return None;
        }
        Some(DashboardCostPayload::new(
            None,
            snapshot.last_30_days_cost_usd,
            None,
            (incomplete_count > 0).then_some(incomplete_count),
        ))
    }

    fn make_reported_cost(snapshot: &ProviderCostSnapshot) -> Option<DashboardCostPayload> {
        if snapshot.currency_code != "USD" {
            return None;
        }
        if snapshot
            .period
            .as_deref()
            .is_some_and(|p| p != "30d" && p != "Last 30 days" && p != "Last 30 days (UTC)")
        {
            return None;
        }
        if !snapshot.used.is_finite() {
            return None;
        }
        Some(DashboardCostPayload::new(
            None,
            Some(snapshot.used),
            None,
            None,
        ))
    }

    fn make_cost(cost: &CostPayload, reference_date: Timestamp) -> Option<DashboardCostPayload> {
        let today = Self::today_cost_entry(cost, reference_date);
        let today_usd = today.and_then(|e| e.cost_usd);
        let today_incomplete = today
            .and_then(|e| e.incomplete_request_count)
            .filter(|&c| c > 0);
        let (history_amount, history_incomplete) = Self::thirty_day_cost(cost, reference_date);
        if today_usd.is_none()
            && history_amount.is_none()
            && today_incomplete.is_none()
            && history_incomplete.is_none()
        {
            return None;
        }
        Some(DashboardCostPayload::new(
            today_usd,
            history_amount,
            today_incomplete,
            history_incomplete,
        ))
    }

    fn thirty_day_cost(
        cost: &CostPayload,
        reference_date: Timestamp,
    ) -> (Option<f64>, Option<i64>) {
        if cost.history_days.unwrap_or(30) <= 30 {
            let inc = cost
                .incomplete_request_count
                .filter(|&c| c > 0)
                .or_else(|| {
                    let sum: i64 = cost
                        .daily
                        .iter()
                        .filter_map(|d| d.incomplete_request_count)
                        .filter(|&c| c > 0)
                        .sum();
                    (sum > 0).then_some(sum)
                });
            return (cost.last_30_days_cost_usd, inc);
        }
        Self::narrow_thirty_day_entries(&cost.daily, reference_date)
    }

    #[must_use]
    pub fn narrow_thirty_day_entries(
        daily: &[CostDailyEntryPayload],
        reference_date: Timestamp,
    ) -> (Option<f64>, Option<i64>) {
        let tz = TimeZone::system();
        let end_date = reference_date.to_zoned(tz).date();
        let Ok(start_date) = end_date.checked_sub(jiff::Span::new().days(29)) else {
            return (None, None);
        };
        let start_key = start_date.strftime("%Y-%m-%d").to_string();
        let end_key = end_date.strftime("%Y-%m-%d").to_string();
        let entries: Vec<&CostDailyEntryPayload> = daily
            .iter()
            .filter(|e| {
                let key = e.date.get(..10).unwrap_or(e.date.as_str());
                key >= start_key.as_str() && key <= end_key.as_str()
            })
            .collect();
        let amounts: Vec<f64> = entries.iter().filter_map(|e| e.cost_usd).collect();
        let inc_sum: i64 = entries
            .iter()
            .filter_map(|e| e.incomplete_request_count)
            .filter(|&c| c > 0)
            .sum();
        let amount = (!amounts.is_empty()).then(|| amounts.into_iter().sum());
        let inc = (inc_sum > 0).then_some(inc_sum);
        (amount, inc)
    }

    fn today_cost_entry(
        cost: &CostPayload,
        reference_date: Timestamp,
    ) -> Option<&CostDailyEntryPayload> {
        let day_key = Self::gregorian_day_key(reference_date);
        cost.daily
            .iter()
            .find(|e| e.date.get(..10).unwrap_or(e.date.as_str()) == day_key)
    }

    #[must_use]
    pub fn gregorian_day_key(date: Timestamp) -> String {
        let tz = TimeZone::system();
        date.to_zoned(tz).strftime("%Y-%m-%d").to_string()
    }

    fn updated_at(
        payload: &ProviderPayload,
        cost: Option<&CostPayload>,
        error: Option<&ProviderErrorPayload>,
        generated_at: Timestamp,
    ) -> Option<Timestamp> {
        let newest = [
            payload.status.as_ref().and_then(|s| s.updated_at),
            payload.usage.as_ref().map(|u| u.updated_at),
            payload.credits.as_ref().map(|c| c.updated_at),
            cost.and_then(|c| c.updated_at),
        ]
        .into_iter()
        .flatten()
        .max();
        if newest.is_some() {
            return newest;
        }
        error.map(|_| generated_at)
    }

    fn dashboard_refresh_seconds(refresh_interval: f64) -> i64 {
        if !refresh_interval.is_finite() || refresh_interval <= 0.0 {
            if refresh_interval.is_sign_positive() && refresh_interval.is_infinite() {
                return i64::MAX / 3;
            }
            return 0;
        }
        let maximum = i64::MAX / 3;
        if refresh_interval >= maximum as f64 {
            return maximum;
        }
        (refresh_interval.ceil() as i64).min(maximum)
    }

    #[must_use]
    pub fn provider_pace_payload(
        provider_id: &str,
        snapshot: &UsageSnapshot,
        weekly_work_days: Option<u32>,
        now: Timestamp,
    ) -> Option<ProviderPacePayload> {
        if snapshot.data_confidence == Confidence::Estimated {
            return None;
        }
        let tz = TimeZone::system();
        let shows_headroom = providers::find_by_id_str(provider_id)
            .is_some_and(|d| d.pace_capability.shows_headroom_hint);
        let primary = snapshot.primary.as_ref().and_then(|w| {
            Self::compute_pace_payload(
                w,
                DEFAULT_SESSION_WINDOW_MINUTES,
                None,
                &tz,
                now,
                true,
                shows_headroom,
            )
        });
        let secondary = snapshot.secondary.as_ref().and_then(|w| {
            Self::compute_pace_payload(
                w,
                DEFAULT_WEEKLY_WINDOW_MINUTES,
                weekly_work_days,
                &tz,
                now,
                false,
                shows_headroom,
            )
        });
        let tertiary = snapshot.tertiary.as_ref().and_then(|w| {
            Self::compute_pace_payload(
                w,
                DEFAULT_WEEKLY_WINDOW_MINUTES,
                weekly_work_days,
                &tz,
                now,
                false,
                shows_headroom,
            )
        });
        if primary.is_none() && secondary.is_none() && tertiary.is_none() {
            return None;
        }
        Some(ProviderPacePayload {
            primary,
            secondary,
            tertiary,
        })
    }

    fn compute_pace_payload(
        window: &RateWindow,
        default_minutes: i64,
        weekly_work_days: Option<u32>,
        tz: &TimeZone,
        now: Timestamp,
        is_session: bool,
        shows_headroom: bool,
    ) -> Option<PacePayload> {
        let pace = UsagePace::weekly(window, now, Some(default_minutes), weekly_work_days, tz)?;
        if pace.expected_used_percent < 3.0 {
            return None;
        }
        let stage = match pace.stage {
            PaceStage::FarAhead => "farAhead",
            PaceStage::Ahead => "ahead",
            PaceStage::SlightlyAhead => "slightlyAhead",
            PaceStage::OnTrack => "onTrack",
            PaceStage::SlightlyBehind => "slightlyBehind",
            PaceStage::Behind => "behind",
            PaceStage::FarBehind => "farBehind",
            PaceStage::Unknown => return None,
        };
        let delta_val = pace.delta_percent.abs().round() as i64;
        let left_label = match pace.stage {
            PaceStage::OnTrack => "On pace".to_string(),
            PaceStage::SlightlyAhead | PaceStage::Ahead | PaceStage::FarAhead => {
                format!("{delta_val}% in deficit")
            }
            PaceStage::SlightlyBehind | PaceStage::Behind | PaceStage::FarBehind => {
                format!("{delta_val}% in reserve")
            }
            PaceStage::Unknown => return None,
        };
        let expected_int = pace.expected_used_percent.round() as i64;
        let right_label = if pace.will_last_to_reset {
            if shows_headroom
                && pace.delta_percent < -15.0
                && pace.speed_multiplier_to_reset.is_some_and(|m| m >= 1.5)
            {
                Some("Lasts until reset | 1.5× headroom".to_string())
            } else {
                Some("Lasts until reset".to_string())
            }
        } else {
            pace.eta_seconds.map(|secs| {
                if secs <= 0.0 {
                    if is_session {
                        "Projected empty now".to_string()
                    } else {
                        "Runs out now".to_string()
                    }
                } else {
                    let hours = (secs / 3600.0).floor() as i64;
                    let mins = ((secs % 3600.0) / 60.0).round() as i64;
                    let dur = if hours > 0 {
                        format!("{hours}h {mins}m")
                    } else {
                        format!("{mins}m")
                    };
                    if is_session {
                        format!("Projected empty in {dur}")
                    } else {
                        format!("Runs out in {dur}")
                    }
                }
            })
        };
        let mut parts = vec![left_label, format!("Expected {expected_int}% used")];
        if let Some(r) = right_label {
            parts.push(r);
        }
        Some(PacePayload {
            stage: stage.to_string(),
            delta_percent: pace.delta_percent.round(),
            expected_used_percent: pace.expected_used_percent.round(),
            will_last_to_reset: pace.will_last_to_reset,
            eta_seconds: pace.eta_seconds.map(f64::round),
            run_out_probability: pace.run_out_probability,
            summary: parts.join(" | "),
        })
    }

    #[must_use]
    pub fn configured_token_accounts(
        config: &Config,
        provider_id: &str,
    ) -> Vec<(TokenAccountMeta, bool)> {
        let Some(entry) = config.provider_entry(provider_id) else {
            return Vec::new();
        };
        match entry {
            ProviderEntry::Typed(pc) => {
                let Some(ta) = pc.token_accounts.as_ref() else {
                    return Vec::new();
                };
                if ta.accounts.is_empty() {
                    return Vec::new();
                }
                let active_idx = ta.clamped_active_index();
                ta.accounts
                    .iter()
                    .enumerate()
                    .map(|(idx, acc)| (acc.clone(), idx == active_idx))
                    .collect()
            }
            ProviderEntry::Opaque { raw, .. } => {
                let Some(ta) = parse_opaque_token_accounts(raw.get()) else {
                    return Vec::new();
                };
                if ta.accounts.is_empty() {
                    return Vec::new();
                }
                let active_idx = ta.clamped_active_index();
                ta.accounts
                    .into_iter()
                    .enumerate()
                    .map(|(idx, acc)| (acc, idx == active_idx))
                    .collect()
            }
        }
    }
}

// ============================================================================
// DashboardManagedCodexAccounts (`DashboardManagedCodexAccounts.swift`)
// ============================================================================

#[derive(Deserialize)]
struct SavedCodexSnapshotsCache {
    version: i32,
    records: Vec<SavedCodexSnapshotRecord>,
}

struct SavedCodexSnapshotRecord {
    account_identity: Option<SavedCodexRecordIdentity>,
    snapshot: Option<UsageSnapshot>,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedCodexRecordIdentity {
    normalized_email: Option<String>,
    #[serde(rename = "workspaceAccountID", alias = "workspaceAccountId")]
    workspace_account_id: Option<String>,
    #[serde(
        default,
        rename = "storedAccountID",
        alias = "storedAccountId",
        with = "opt_serde_uuid"
    )]
    stored_account_id: Option<uuid::Uuid>,
}

mod opt_serde_uuid {
    use serde::{Deserialize, Deserializer};
    use uuid::Uuid;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Uuid>, D::Error> {
        let opt = Option::<String>::deserialize(d)?;
        match opt {
            None => Ok(None),
            Some(s) => Uuid::parse_str(s.trim())
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

impl<'de> Deserialize<'de> for SavedCodexSnapshotRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let account_identity = value
            .get("accountIdentity")
            .and_then(|v| serde_json::from_value::<SavedCodexRecordIdentity>(v.clone()).ok());
        let snapshot = value
            .get("snapshot")
            .and_then(|v| serde_json::from_value::<UsageSnapshot>(v.clone()).ok());
        let error = value
            .get("error")
            .and_then(|v| v.as_str().map(ToOwned::to_owned));
        Ok(Self {
            account_identity,
            snapshot,
            error,
        })
    }
}

impl SavedCodexSnapshotRecord {
    fn matches(&self, account: &ManagedCodexAccount) -> bool {
        let Some(identity) = &self.account_identity else {
            return false;
        };
        if identity.stored_account_id != Some(account.id) {
            return false;
        }
        let Some(norm_email) = identity
            .normalized_email
            .as_deref()
            .map(ManagedCodexAccount::normalize_email)
            .filter(|s| !s.is_empty())
        else {
            return false;
        };
        if norm_email != account.email {
            return false;
        }
        let Some(expected_ws) = account.effective_workspace_account_id() else {
            return false;
        };
        let record_ws = ManagedCodexAccount::normalize_workspace_account_id(
            identity.workspace_account_id.as_deref(),
        );
        if record_ws != Some(expected_ws) {
            return false;
        }
        match self
            .snapshot
            .as_ref()
            .and_then(|s| s.identity.as_ref())
            .and_then(|i| i.provider_id.as_ref())
        {
            None => true,
            Some(pid) => pid.as_str() == "codex",
        }
    }
}

pub struct DashboardManagedCodexAccounts;

impl DashboardManagedCodexAccounts {
    #[must_use]
    pub fn collect(config: &Config, account_store_path: &Path) -> Option<DashboardAccountsInput> {
        if !config.enabled_providers().iter().any(|p| p == "codex") {
            return None;
        }
        let accounts =
            match FileManagedCodexAccountStore::new(account_store_path).load_account_metadata() {
                Ok(set) => set.accounts,
                Err(_) => {
                    return Some(DashboardAccountsInput {
                        accounts: None,
                        adapter_error: Some(
                            "Could not read managed Codex account metadata.".to_string(),
                        ),
                        weekly_work_days: None,
                    });
                }
            };
        if accounts.is_empty() {
            return None;
        }
        let cache_path = account_store_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("codex-account-snapshots.json");
        let records = if cache_path.exists() {
            let Ok(bytes) = std::fs::read(&cache_path) else {
                return Some(DashboardAccountsInput {
                    accounts: None,
                    adapter_error: Some("Could not read saved Codex account usage.".to_string()),
                    weekly_work_days: None,
                });
            };
            let Ok(cache) = serde_json::from_slice::<SavedCodexSnapshotsCache>(&bytes) else {
                return Some(DashboardAccountsInput {
                    accounts: None,
                    adapter_error: Some("Could not read saved Codex account usage.".to_string()),
                    weekly_work_days: None,
                });
            };
            if cache.version != 1 {
                return Some(DashboardAccountsInput {
                    accounts: None,
                    adapter_error: Some("Unsupported saved Codex usage version.".to_string()),
                    weekly_work_days: None,
                });
            }
            cache.records
        } else {
            Vec::new()
        };

        let active_source = super::accounts::codex_active_source_from_config(config);
        let email_counts = {
            let mut m: HashMap<&str, usize> = HashMap::new();
            for acc in &accounts {
                *m.entry(acc.email.as_str()).or_insert(0) += 1;
            }
            m
        };

        let projected = accounts
            .iter()
            .map(|account| {
                let record = records
                    .iter()
                    .filter(|r| r.matches(account))
                    .max_by_key(|r| r.snapshot.as_ref().map(|s| s.updated_at));
                let error = if record.is_some_and(|r| r.error.is_some()) {
                    Some(
                        "Saved usage refresh failed. Refresh this account in AriadUsage."
                            .to_string(),
                    )
                } else if record.and_then(|r| r.snapshot.as_ref()).is_none() {
                    Some("No saved usage for this account. Refresh it in AriadUsage.".to_string())
                } else {
                    None
                };
                let display_label = Self::visible_account_display_name(
                    account,
                    email_counts
                        .get(account.email.as_str())
                        .copied()
                        .unwrap_or(1)
                        > 1,
                );
                let is_active =
                    active_source == CodexActiveSource::ManagedAccount { id: account.id };
                ProviderAccountUsageSnapshot {
                    id: ProviderAccountIdentity::new(
                        "codex-managed",
                        account.id.hyphenated().to_string().to_ascii_lowercase(),
                    ),
                    provider: "codex".to_string(),
                    display_label,
                    account_email: Some(account.email.clone()),
                    is_active,
                    uses_last_known_usage: true,
                    snapshot: record.and_then(|r| r.snapshot.clone()),
                    error,
                    source_label: None,
                }
            })
            .collect();

        Some(DashboardAccountsInput {
            accounts: Some(projected),
            adapter_error: None,
            weekly_work_days: None,
        })
    }

    fn visible_account_display_name(
        account: &ManagedCodexAccount,
        has_duplicate_email: bool,
    ) -> String {
        if let Some(ws_label) = account
            .workspace_label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return format!("{} — {ws_label}", account.email);
        }
        if has_duplicate_email
            && let Some(ws_id) = account
                .workspace_account_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        {
            return format!("{} — {ws_id}", account.email);
        }
        account.email.clone()
    }
}

// ============================================================================
// DashboardSnapshotProducer & Account Collection (`DashboardAccountCollector.swift` & `CLIDashboardCommand.swift`)
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct DashboardSnapshotResult {
    pub payload: DashboardSnapshotPayload,
    pub usage_cache_keys: Vec<Option<String>>,
}

pub type CollectUsageFn = Arc<
    dyn Fn(Vec<String>) -> BoxFuture<'static, Result<UsageCommandOutput, String>> + Send + Sync,
>;
pub type CollectCostFn =
    Arc<dyn Fn(Vec<String>, Config) -> BoxFuture<'static, Vec<CostPayload>> + Send + Sync>;
pub type CollectAccountsFn =
    Arc<dyn Fn(Config, String) -> BoxFuture<'static, Option<DashboardAccountsInput>> + Send + Sync>;

#[derive(Clone)]
pub struct DashboardSnapshotProducer {
    pub collect_usage: CollectUsageFn,
    pub collect_cost: CollectCostFn,
    pub now: Arc<dyn Fn() -> Timestamp + Send + Sync>,
    pub collect_accounts: CollectAccountsFn,
    pub weekly_work_days: Arc<dyn Fn() -> Option<u32> + Send + Sync>,
    pub usage_bars_show_used: Arc<dyn Fn() -> bool + Send + Sync>,
    pub all_accounts: bool,
}

impl DashboardSnapshotProducer {
    pub fn new(
        collect_usage: CollectUsageFn,
        collect_cost: CollectCostFn,
        now: Arc<dyn Fn() -> Timestamp + Send + Sync>,
    ) -> Self {
        Self {
            collect_usage,
            collect_cost,
            now,
            collect_accounts: Arc::new(|_, _| Box::pin(async { None })),
            weekly_work_days: Arc::new(|| None),
            usage_bars_show_used: Arc::new(|| false),
            all_accounts: false,
        }
    }

    pub async fn collect(
        &self,
        config: &Config,
        refresh_interval: f64,
        ariadusage_version: Option<String>,
        identity_mode: Option<DashboardIdentityMode>,
        requested_providers: Option<&[String]>,
    ) -> Result<DashboardSnapshotResult, String> {
        let selection: Vec<String> = match requested_providers {
            Some(req) => req.to_vec(),
            None => provider_selection_list(&config.enabled_providers()),
        };
        let usage_output = (self.collect_usage)(selection.clone()).await?;
        let cost_providers = cost_providers_from_selection(&selection);
        let cost_payloads = (self.collect_cost)(cost_providers, config.clone()).await;

        let mut account_collections: BTreeMap<String, DashboardAccountsInput> = BTreeMap::new();
        for provider in &selection {
            if let Some(mut collection) =
                (self.collect_accounts)(config.clone(), provider.clone()).await
            {
                if collection.weekly_work_days.is_none() {
                    collection.weekly_work_days = (self.weekly_work_days)();
                }
                account_collections.insert(provider.clone(), collection);
            }
        }
        let generated_at = (self.now)();
        let resolved_mode = identity_mode.unwrap_or(if self.all_accounts {
            DashboardIdentityMode::None
        } else {
            DashboardIdentityMode::Full
        });

        let payload = DashboardSnapshotBuilder::make_snapshot(
            &usage_output.payload,
            &cost_payloads,
            config,
            resolved_mode,
            generated_at,
            refresh_interval,
            ariadusage_version,
            &account_collections,
            (self.usage_bars_show_used)(),
            self.all_accounts,
        );
        let usage_cache_keys =
            DashboardSnapshotBuilder::selected_payloads(&usage_output.payload, self.all_accounts)
                .into_iter()
                .map(|p| p.cache_account_key)
                .collect();
        Ok(DashboardSnapshotResult {
            payload,
            usage_cache_keys,
        })
    }
}

/// Mirrors `CodexBarCLI.providerSelection(rawOverride: nil, enabled:)` (`CLIHelpers.swift:26`).
#[must_use]
pub fn provider_selection_list(enabled: &[String]) -> Vec<String> {
    if enabled.len() == 2 {
        let has_codex = enabled.iter().any(|p| p == "codex");
        let has_claude = enabled.iter().any(|p| p == "claude");
        if has_codex && has_claude {
            return vec!["codex".to_string(), "claude".to_string()];
        }
    }
    enabled.to_vec()
}

/// Mirrors `CodexBarCLI.costProviders(from:)` (`CLICostCommand.swift`).
#[must_use]
pub fn cost_providers_from_selection(selection: &[String]) -> Vec<String> {
    selection.to_vec()
}

/// Mirrors `CodexBarCLI.serveIncludesConfiguredAccounts` (`DashboardAccountCollector.swift:85`).
#[must_use]
pub fn serve_includes_configured_accounts(
    provider: &str,
    config: &Config,
    all_accounts: bool,
) -> bool {
    if !all_accounts {
        return false;
    }
    if provider == "claude" && dashboard_claude_swap_is_eligible(config) {
        return false;
    }
    let supports_tokens = providers::find_by_id_str(provider)
        .map_or(provider != "codex", |d| d.token_account_support);
    supports_tokens
        && !DashboardSnapshotBuilder::configured_token_accounts(config, provider).is_empty()
}

#[must_use]
pub fn dashboard_claude_swap_is_eligible(config: &Config) -> bool {
    config.enabled_providers().iter().any(|p| p == "claude")
        && config
            .provider_config("claude")
            .and_then(|c| c.extra.get("claudeSwapEnabled"))
            .and_then(|raw| serde_json::from_str::<bool>(raw.get()).ok())
            .unwrap_or(false)
}

/// Mirrors `CodexBarCLI.collectAccountUsage` (`DashboardAccountCollector.swift:97`).
pub async fn collect_account_usage<F, Fut>(
    provider: &str,
    accounts: &[Option<DashboardUsageAccount>],
    inventory_incomplete: bool,
    minimum_delay: Option<Duration>,
    publish_partial: Option<&CliServeOperationProgressSink<UsageCommandOutput>>,
    mut fetch: F,
) -> UsageCommandOutput
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = UsageCommandOutput>,
{
    let mut output = UsageCommandOutput::default();
    for index in 0..accounts.len() {
        let mut fallback = output.clone();
        if publish_partial.is_some() {
            for account in &accounts[index..] {
                let mut timeout = super::serve_provider_timeout_output(provider);
                timeout.attach_dashboard_account(account.as_ref(), inventory_incomplete);
                fallback.merge(timeout);
            }
        }
        if let Some(sink) = publish_partial {
            sink.publish(fallback.clone()).await;
            if sink.is_cancelled() {
                return fallback;
            }
        }
        if index > 0
            && let Some(delay) = minimum_delay
        {
            if let Some(sink) = publish_partial {
                let token = sink.cancel_token();
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = token.cancelled() => {
                        return fallback;
                    }
                }
                if sink.is_cancelled() {
                    return fallback;
                }
            } else {
                tokio::time::sleep(delay).await;
            }
        }
        let mut result = fetch(index).await;
        result.attach_dashboard_account(accounts[index].as_ref(), inventory_incomplete);
        output.merge(result);
    }
    output
}

/// Mirrors `CodexBarCLI.collectConfiguredCostPayloads` (`CLICostCommand.swift`).
pub async fn collect_configured_cost_payloads<F>(
    providers: &[String],
    _config: &Config,
    context: &super::ServeCostCollectionContext,
    fetch: F,
) -> Vec<CostPayload>
where
    F: Fn(String) -> Pin<Box<dyn Future<Output = CostPayload> + Send>> + Send + Sync + 'static,
{
    let fetch = Arc::new(fetch);
    super::serve_collect_cost_payloads(providers, context, move |prov, _sink| {
        let fetch = Arc::clone(&fetch);
        async move { fetch(prov).await }
    })
    .await
}

/// Visible Codex account projection used for stable cache account keys (`CodexVisibleAccount`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexVisibleAccount {
    pub id: String,
    pub email: String,
    pub workspace_label: Option<String>,
    pub workspace_account_id: Option<String>,
    pub auth_fingerprint: Option<String>,
    pub stored_account_id: Option<uuid::Uuid>,
    pub selection_source: CodexActiveSource,
    pub is_active: bool,
    pub is_live: bool,
    pub can_reauthenticate: bool,
    pub can_remove: bool,
}

/// Computes the stable cache account key for a provider/token/Codex visible account (`CodexBarCLI.usageCacheAccountKey`).
#[must_use]
pub fn codex_usage_cache_account_key(
    provider_id: &str,
    account: Option<&TokenAccountMeta>,
    codex_visible_account: Option<&CodexVisibleAccount>,
) -> Option<String> {
    if let Some(acc) = account {
        return Some(format!("token:{}", acc.id.to_ascii_lowercase()));
    }
    if provider_id == "codex"
        && let Some(vis) = codex_visible_account
    {
        if let Some(stored_id) = vis.stored_account_id {
            return Some(format!(
                "codex-stored:{}",
                stored_id.hyphenated().to_string().to_ascii_lowercase()
            ));
        }
        if let Some(auth_fp) = vis
            .auth_fingerprint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(format!("codex-auth:{auth_fp}"));
        }
        if let Some(ws_id) = vis
            .workspace_account_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(format!(
                "codex-workspace:{}:{ws_id}",
                vis.email.trim().to_ascii_lowercase()
            ));
        }
    }
    None
}

/// Computes an opaque account key for cache invalidation without storing raw credentials.
#[must_use]
pub fn usage_cache_account_key(provider_id: &str, payload: &ProviderPayload) -> Option<String> {
    if let Some(key) = &payload.cache_account_key {
        return Some(key.clone());
    }
    let id = payload
        .usage
        .as_ref()
        .and_then(|u| DashboardSnapshotBuilder::snapshot_identity_for(u, provider_id))
        .and_then(|i| {
            i.account_id
                .as_deref()
                .or(i.account_email.as_deref())
                .or(i.account_organization.as_deref())
        })
        .or(payload.account.as_deref())?;
    let trimmed = id.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(format!("{provider_id}:{trimmed}"))
    }
}

/// Validates and atomically writes `bytes` to `output_path` with mode `0644`, without creating parent directories (`CLIDashboardCommand.writeDashboardSnapshotAtomically`).
pub fn write_dashboard_snapshot_atomically(bytes: &[u8], output_path: &Path) -> Result<(), String> {
    let parent = output_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match std::fs::metadata(parent) {
        Ok(meta) if meta.is_dir() => {}
        _ => {
            return Err(format!(
                "--output directory does not exist: {} (parent directories are not created)",
                parent.display()
            ));
        }
    }
    let file_name = output_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("snapshot.json");
    let staged = parent.join(format!(
        ".{file_name}.ariadusage-dashboard-{}",
        uuid::Uuid::new_v4().hyphenated()
    ));

    #[cfg(unix)]
    {
        use std::io::Write;
        let fd = rustix::fs::open(
            &staged,
            rustix::fs::OFlags::RDWR
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_bits_retain(0o644),
        )
        .map_err(|err| {
            format!(
                "Could not write --output file {}: {}",
                staged.display(),
                std::io::Error::from(err)
            )
        })?;
        if let Err(err) = rustix::fs::fchmod(&fd, rustix::fs::Mode::from_bits_retain(0o644)) {
            drop(fd);
            let _ = std::fs::remove_file(&staged);
            return Err(format!(
                "Could not write --output file {}: {}",
                staged.display(),
                std::io::Error::from(err)
            ));
        }
        let mut file = std::fs::File::from(fd);
        if let Err(err) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&staged);
            return Err(format!(
                "Could not write --output file {}: {err}",
                staged.display()
            ));
        }
        drop(file);
    }

    #[cfg(not(unix))]
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(|err| format!("Could not write --output file {}: {err}", staged.display()))?;
        if let Err(err) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&staged);
            return Err(format!(
                "Could not write --output file {}: {err}",
                staged.display()
            ));
        }
        drop(file);
    }

    if let Err(err) = std::fs::rename(&staged, output_path) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "Could not write --output file {}: {err}",
            output_path.display()
        ));
    }

    #[cfg(unix)]
    if let Ok(parent_dir) = std::fs::File::open(parent) {
        let _ = parent_dir.sync_all();
    }

    Ok(())
}

/// Mirrors `CodexBarCLI.makeCostPayload` (`CLICostCommand.swift`).
#[must_use]
pub fn make_cost_payload(
    provider: &str,
    snapshot: Option<CostUsageTokenSnapshot>,
    error: Option<ProviderErrorPayload>,
) -> CostPayload {
    CostPayload::from_token_snapshot(provider, snapshot, error)
}

/// Resolves configured token accounts for `provider` under `all_accounts` (`TokenAccountCLIContext.resolvedAccounts`).
#[must_use]
pub fn resolved_token_accounts(
    config: &Config,
    provider: &str,
    all_accounts: bool,
) -> Vec<TokenAccountMeta> {
    let configured = DashboardSnapshotBuilder::configured_token_accounts(config, provider);
    if all_accounts {
        configured.into_iter().map(|(acc, _)| acc).collect()
    } else {
        configured
            .into_iter()
            .filter_map(|(acc, active)| active.then_some(acc))
            .collect()
    }
}

/// Mirrors `ServeUsageContext` (`CLIServeCommand.swift` & `DashboardAccountCollector.swift`).
#[derive(Clone)]
pub struct ServeUsageContext {
    pub config: Config,
    pub config_fingerprint: String,
    pub refresh_interval: f64,
    pub provider_timeout: Option<f64>,
    pub provider_deadline: Option<std::time::Instant>,
    pub provider_operations: super::coordinator::CliServeOperationCoordinator<UsageCommandOutput>,
    pub include_all_codex_accounts: bool,
    pub include_all_accounts: bool,
    pub persist_cli_sessions: bool,
}

impl ServeUsageContext {
    #[must_use]
    pub fn new(
        config: Config,
        config_fingerprint: impl Into<String>,
        refresh_interval: f64,
        provider_timeout: Option<f64>,
        provider_deadline: Option<std::time::Instant>,
        provider_operations: super::coordinator::CliServeOperationCoordinator<UsageCommandOutput>,
    ) -> Self {
        Self {
            config,
            config_fingerprint: config_fingerprint.into(),
            refresh_interval,
            provider_timeout,
            provider_deadline,
            provider_operations,
            include_all_codex_accounts: true,
            include_all_accounts: false,
            persist_cli_sessions: true,
        }
    }
}

/// Mirrors `CodexBarCLI.serveUsageOutput` (`DashboardAccountCollector.swift:5`).
pub async fn serve_usage_output<F, Fut>(
    providers: &[String],
    context: &ServeUsageContext,
    fetch_usage: F,
) -> UsageCommandOutput
where
    F: Fn(String, bool, Option<CliServeOperationProgressSink<UsageCommandOutput>>) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: Future<Output = UsageCommandOutput> + Send + 'static,
{
    let fp = super::serve_usage_operation_fingerprint(
        &context.config_fingerprint,
        context.include_all_codex_accounts,
        context.include_all_accounts,
    );
    let cfg = context.config.clone();
    let include_all_accounts = context.include_all_accounts;
    let fetch = Arc::new(fetch_usage);
    super::serve_collect_usage_outputs(
        providers,
        &fp,
        context.provider_deadline,
        &context.provider_operations,
        move |provider, publish| {
            let includes_all =
                serve_includes_configured_accounts(&provider, &cfg, include_all_accounts);
            let sink = include_all_accounts.then_some(publish);
            let fetch = Arc::clone(&fetch);
            fetch(provider, includes_all, sink)
        },
    )
    .await
}
