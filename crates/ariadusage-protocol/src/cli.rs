// Ported from CodexBar Sources/CodexBarCLI/CLIPayloads.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexExtraUsageCost.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! CLI JSON and TOON payload models for `ariadusage usage` and `ariadusage status`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::metric::MetricState;
use crate::snapshot::ProviderSnapshot;
use crate::usage::{
    BillingDate, CostProvenance, DetailSection, PaceStage, ProviderError, ProviderStatus,
    StatusIndicator,
};

const fn is_false(value: &bool) -> bool {
    !*value
}

const fn is_true(value: &bool) -> bool {
    *value
}

const fn default_true() -> bool {
    true
}

fn is_unknown_or_none(value: &Option<String>) -> bool {
    matches!(value.as_deref(), None | Some("unknown"))
}

/// Rate window payload matching upstream `CodexBarCore.RateWindow` JSON encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CliRateWindow {
    #[serde(rename = "usedPercent")]
    pub used_percent: f64,
    #[serde(rename = "windowMinutes", skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<i64>,
    #[serde(rename = "resetsAt", skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(rename = "resetDescription", skip_serializing_if = "Option::is_none")]
    pub reset_description: Option<String>,
    #[serde(rename = "nextRegenPercent", skip_serializing_if = "Option::is_none")]
    pub next_regen_percent: Option<f64>,
    #[serde(
        rename = "isSyntheticPlaceholder",
        default,
        skip_serializing_if = "is_false"
    )]
    pub is_synthetic_placeholder: bool,
}

impl CliRateWindow {
    /// Creates a standard rate window.
    pub fn new(
        used_percent: f64,
        window_minutes: Option<i64>,
        resets_at: Option<String>,
        reset_description: Option<String>,
    ) -> Self {
        Self {
            used_percent,
            window_minutes,
            resets_at,
            reset_description,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }
    }

    /// Alias for [`Self::new`] accepting `Into<String>` for optional string fields.
    pub fn of(
        used_percent: f64,
        window_minutes: Option<i64>,
        resets_at: Option<impl Into<String>>,
        reset_description: Option<impl Into<String>>,
    ) -> Self {
        Self::new(
            used_percent,
            window_minutes,
            resets_at.map(Into::into),
            reset_description.map(Into::into),
        )
    }

    /// Creates a rate window with optional regeneration percentage and synthetic placeholder flag.
    pub fn with_options(
        used_percent: f64,
        window_minutes: Option<i64>,
        resets_at: Option<String>,
        reset_description: Option<String>,
        next_regen_percent: Option<f64>,
        is_synthetic_placeholder: bool,
    ) -> Self {
        Self {
            used_percent,
            window_minutes,
            resets_at,
            reset_description,
            next_regen_percent,
            is_synthetic_placeholder,
        }
    }

    /// Remaining quota percentage (`max(0, 100 - usedPercent)`).
    pub fn remaining_percent(&self) -> f64 {
        (100.0 - self.used_percent).max(0.0)
    }

    /// Returns `None` when this window is a synthetic placeholder.
    pub fn measured(&self) -> Option<&Self> {
        if self.is_synthetic_placeholder {
            None
        } else {
            Some(self)
        }
    }
}

impl From<&crate::usage::RateWindow> for CliRateWindow {
    fn from(window: &crate::usage::RateWindow) -> Self {
        Self {
            used_percent: window.used_percent,
            window_minutes: window.window_minutes.map(i64::from),
            resets_at: window.resets_at.map(|ts| ts.to_string()),
            reset_description: window.reset_description.clone(),
            next_regen_percent: window.next_regen_percent,
            is_synthetic_placeholder: false,
        }
    }
}

/// Named extra rate window matching upstream `CodexBarCore.NamedRateWindow`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CliNamedRateWindow {
    pub id: String,
    pub title: String,
    pub window: CliRateWindow,
    #[serde(
        rename = "usageKnown",
        default = "default_true",
        skip_serializing_if = "is_true"
    )]
    pub usage_known: bool,
}

impl CliNamedRateWindow {
    pub fn new(id: impl Into<String>, title: impl Into<String>, window: CliRateWindow) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            window,
            usage_known: true,
        }
    }

    pub fn with_usage_known(
        id: impl Into<String>,
        title: impl Into<String>,
        window: CliRateWindow,
        usage_known: bool,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            window,
            usage_known,
        }
    }
}

/// Provider cost snapshot matching upstream `CodexBarCore.ProviderCostSnapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CliProviderCostSnapshot {
    pub used: f64,
    pub limit: f64,
    #[serde(rename = "currencyCode")]
    pub currency_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    #[serde(rename = "resetsAt", skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(rename = "nextRegenAmount", skip_serializing_if = "Option::is_none")]
    pub next_regen_amount: Option<f64>,
    #[serde(rename = "personalUsed", skip_serializing_if = "Option::is_none")]
    pub personal_used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<f64>,
    #[serde(rename = "balanceUpdatedAt", skip_serializing_if = "Option::is_none")]
    pub balance_updated_at: Option<String>,
    #[serde(rename = "balanceIsWorkspace", skip_serializing_if = "Option::is_none")]
    pub balance_is_workspace: Option<bool>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(
        rename = "balanceIsUnavailable",
        skip_serializing_if = "Option::is_none"
    )]
    pub balance_is_unavailable: Option<bool>,
}

impl CliProviderCostSnapshot {
    pub const CODEX_CREDITS_CURRENCY_CODE: &'static str = "Credits";

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        used: f64,
        limit: f64,
        currency_code: impl Into<String>,
        period: Option<String>,
        resets_at: Option<String>,
        updated_at: impl Into<String>,
    ) -> Self {
        Self {
            used,
            limit,
            currency_code: currency_code.into(),
            period,
            resets_at,
            next_regen_amount: None,
            personal_used: None,
            balance: None,
            balance_updated_at: None,
            balance_is_workspace: None,
            updated_at: updated_at.into(),
            balance_is_unavailable: None,
        }
    }

    /// False for credit-denominated amounts (`Credits`), which have no currency symbol.
    pub fn is_monetary(&self) -> bool {
        !self
            .currency_code
            .eq_ignore_ascii_case(Self::CODEX_CREDITS_CURRENCY_CODE)
    }

    /// Purchased extra credits that are distinct from the monthly included/assigned cap
    /// (`CodexExtraUsageCost.purchasedExtraCreditsBalance`).
    pub fn purchased_extra_credits_balance(credits: &CliCreditsSnapshot) -> Option<f64> {
        if !credits.balance_read_succeeded {
            return None;
        }
        if credits.has_workspace_balance() {
            return (credits.remaining > 0.0).then_some(credits.remaining);
        }
        if let Some(monthly) = credits.codex_credit_limit.as_ref() {
            if (credits.remaining - monthly.remaining).abs() > 0.000_1 && credits.remaining > 0.0 {
                return Some(credits.remaining);
            }
            return None;
        }
        (credits.remaining > 0.0).then_some(credits.remaining)
    }

    /// Projects Codex credits into a [`CliProviderCostSnapshot`] (`CodexExtraUsageCost.providerCost`).
    pub fn from_codex_credits(credits: &CliCreditsSnapshot) -> Option<Self> {
        let extra_balance = Self::purchased_extra_credits_balance(credits);
        let balance_is_unavailable =
            !credits.balance_read_succeeded && credits.credits_available == Some(true);
        let balance_updated_at = if credits.balance_read_succeeded || balance_is_unavailable {
            Some(credits.updated_at.clone())
        } else {
            None
        };
        let limit = credits
            .codex_credit_limit
            .as_ref()
            .filter(|c| c.limit > 0.0);
        if limit.is_none() && balance_updated_at.is_none() {
            return None;
        }
        Some(Self {
            used: limit.map_or(0.0, |c| c.used),
            limit: limit.map_or(0.0, |c| c.limit),
            currency_code: Self::CODEX_CREDITS_CURRENCY_CODE.to_string(),
            period: Some(
                limit
                    .map(|c| c.title.clone())
                    .unwrap_or_else(|| "Extra usage".to_string()),
            ),
            resets_at: limit.and_then(|c| c.resets_at.clone()),
            next_regen_amount: None,
            personal_used: None,
            balance: extra_balance,
            balance_updated_at,
            balance_is_workspace: credits.has_workspace_balance().then_some(true),
            updated_at: limit
                .map(|c| c.updated_at.clone())
                .unwrap_or_else(|| credits.updated_at.clone()),
            balance_is_unavailable: balance_is_unavailable.then_some(true),
        })
    }

    /// Projects a positive spend budget into a quota meter without assigning a time-window cadence.
    pub fn spend_limit_window(&self) -> Option<CliRateWindow> {
        if self.limit <= 0.0 {
            return None;
        }
        Some(CliRateWindow::new(
            ((self.used / self.limit) * 100.0).clamp(0.0, 100.0),
            None,
            self.resets_at.clone(),
            None,
        ))
    }

    pub fn from_protocol_cost(cost: &crate::usage::Cost, updated_at: &str) -> Self {
        Self {
            used: cost.used,
            limit: cost.limit.unwrap_or(0.0),
            currency_code: cost.currency_code.clone(),
            period: cost.period.clone(),
            resets_at: cost.resets_at.map(|ts| ts.to_string()),
            next_regen_amount: cost.next_regen_amount,
            personal_used: cost.personal_used,
            balance: cost.balance,
            balance_updated_at: cost.balance_updated_at.map(|ts| ts.to_string()),
            balance_is_workspace: cost.balance_is_workspace,
            updated_at: updated_at.to_string(),
            balance_is_unavailable: None,
        }
    }
}

/// Provider identity snapshot matching upstream `CodexBarCore.ProviderIdentitySnapshot`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CliProviderIdentitySnapshot {
    #[serde(rename = "providerID", skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(rename = "accountEmail", skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(
        rename = "accountOrganization",
        skip_serializing_if = "Option::is_none"
    )]
    pub account_organization: Option<String>,
    #[serde(rename = "loginMethod", skip_serializing_if = "Option::is_none")]
    pub login_method: Option<String>,
    #[serde(rename = "accountID", skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

impl CliProviderIdentitySnapshot {
    pub fn new(
        provider_id: Option<String>,
        account_email: Option<String>,
        account_organization: Option<String>,
        login_method: Option<String>,
    ) -> Self {
        Self {
            provider_id,
            account_email,
            account_organization,
            login_method,
            account_id: None,
        }
    }

    pub fn from_protocol(identity: &crate::usage::Identity) -> Self {
        Self {
            provider_id: Some(identity.provider_id.as_str().to_string()),
            account_email: identity.account_email.clone(),
            account_organization: identity.account_organization.clone(),
            login_method: identity.login_method.clone(),
            account_id: identity.account_id.clone(),
        }
    }
}

/// Single Codex rate-limit reset credit entry (`CodexRateLimitResetCredit` in `CreditsModels.swift`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CliCodexRateLimitResetCredit {
    pub id: String,
    #[serde(rename = "reset_type")]
    pub reset_type: String,
    pub status: String,
    #[serde(rename = "granted_at")]
    pub granted_at: String,
    #[serde(rename = "expires_at", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(rename = "redeem_started_at", skip_serializing_if = "Option::is_none")]
    pub redeem_started_at: Option<String>,
    #[serde(rename = "redeemed_at", skip_serializing_if = "Option::is_none")]
    pub redeemed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl CliCodexRateLimitResetCredit {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        reset_type: impl Into<String>,
        status: impl Into<String>,
        granted_at: impl Into<String>,
        expires_at: Option<String>,
        redeem_started_at: Option<String>,
        redeemed_at: Option<String>,
        title: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            reset_type: reset_type.into(),
            status: status.into(),
            granted_at: granted_at.into(),
            expires_at,
            redeem_started_at,
            redeemed_at,
            title,
            description,
        }
    }

    pub fn is_available(&self, now: jiff::Timestamp) -> bool {
        if self.status != "available" {
            return false;
        }
        if let Some(exp_str) = &self.expires_at
            && let Ok(exp) = exp_str.parse::<jiff::Timestamp>()
        {
            return exp > now;
        }
        true
    }
}

/// Snapshot of Codex rate-limit reset credits (`CodexRateLimitResetCreditsSnapshot` in `CreditsModels.swift`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CliCodexRateLimitResetCreditsSnapshot {
    pub credits: Vec<CliCodexRateLimitResetCredit>,
    #[serde(rename = "availableCount")]
    pub available_count: usize,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

impl CliCodexRateLimitResetCreditsSnapshot {
    pub fn new(
        credits: Vec<CliCodexRateLimitResetCredit>,
        available_count: usize,
        updated_at: impl Into<String>,
    ) -> Self {
        Self {
            credits,
            available_count,
            updated_at: updated_at.into(),
        }
    }

    /// Filtered and sorted available credits at `now` (`availableInventory(at:)` in `CreditsModels.swift`).
    pub fn available_credits(&self, now: jiff::Timestamp) -> Vec<&CliCodexRateLimitResetCredit> {
        let mut avail: Vec<&CliCodexRateLimitResetCredit> = self
            .credits
            .iter()
            .filter(|credit| credit.is_available(now))
            .collect();
        avail.sort_by(|lhs, rhs| {
            let lhs_ts = lhs
                .expires_at
                .as_deref()
                .and_then(|s| s.parse::<jiff::Timestamp>().ok());
            let rhs_ts = rhs
                .expires_at
                .as_deref()
                .and_then(|s| s.parse::<jiff::Timestamp>().ok());
            match (lhs_ts, rhs_ts) {
                (Some(l), Some(r)) if l != r => l.cmp(&r),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                _ => lhs.id.cmp(&rhs.id),
            }
        });
        avail
    }

    pub fn next_expiring_credit(
        &self,
        now: jiff::Timestamp,
    ) -> Option<&CliCodexRateLimitResetCredit> {
        self.available_credits(now)
            .into_iter()
            .find(|c| c.expires_at.is_some())
    }
}

/// Live-only Claude rate-limit reset credits snapshot (never serialized inside `usage`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliClaudeRateLimitResetCreditsSnapshot {
    pub remaining: usize,
    pub total: Option<usize>,
    pub expires_at: Option<String>,
    pub updated_at: String,
}

impl CliClaudeRateLimitResetCreditsSnapshot {
    pub fn new(
        remaining: usize,
        total: Option<usize>,
        expires_at: Option<String>,
        updated_at: impl Into<String>,
    ) -> Self {
        Self {
            remaining,
            total,
            expires_at,
            updated_at: updated_at.into(),
        }
    }

    pub fn current_available_count(&self, now: jiff::Timestamp) -> usize {
        if let Some(exp_str) = &self.expires_at
            && let Ok(exp) = exp_str.parse::<jiff::Timestamp>()
            && exp <= now
        {
            return 0;
        }
        self.remaining
    }

    pub fn current_expiry(&self, now: jiff::Timestamp) -> Option<String> {
        if self.current_available_count(now) == 0 {
            return None;
        }
        self.expires_at.clone()
    }

    pub fn balance_summary(&self, now: jiff::Timestamp) -> Option<String> {
        let count = self.current_available_count(now);
        if count == 0 {
            return None;
        }
        let noun = if count == 1 {
            "reset available"
        } else {
            "resets available"
        };
        let mut parts = vec![format!("{count} {noun}")];
        if let Some(exp_str) = self.current_expiry(now)
            && let Ok(exp_ts) = exp_str.parse::<jiff::Timestamp>()
        {
            parts.push(format!("expires {}", format_reset_expiry(exp_ts)));
        }
        Some(parts.join(" · "))
    }
}

fn format_reset_expiry(ts: jiff::Timestamp) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let zdt = ts.to_zoned(jiff::tz::TimeZone::system());
    let month = MONTHS
        .get((zdt.month() as usize).saturating_sub(1))
        .copied()
        .unwrap_or("Jan");
    format!("{month} {}", zdt.day())
}

/// Single daily entry inside [`CliCostUsageTokenSnapshot`] (`CostUsageDailyReport.Entry` in `CostUsageModels.swift`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CliCostUsageDailyEntry {
    pub date: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub models_used: Option<Vec<String>>,
    pub model_breakdowns: Option<Vec<String>>,
}

/// Live history token/cost snapshot attached in-memory to [`CliUsageSnapshot`] (`CostUsageTokenSnapshot` in `CostUsageModels.swift`).
#[derive(Debug, Clone, PartialEq)]
pub struct CliCostUsageTokenSnapshot {
    pub session_tokens: Option<i64>,
    pub session_cost_usd: Option<f64>,
    pub last_30_days_tokens: Option<i64>,
    pub last_30_days_cost_usd: Option<f64>,
    pub currency_code: String,
    pub cost_provenance: CostProvenance,
    pub history_days: usize,
    pub history_label: Option<String>,
    pub daily: Vec<CliCostUsageDailyEntry>,
    pub updated_at: Option<String>,
}

impl Default for CliCostUsageTokenSnapshot {
    fn default() -> Self {
        Self {
            session_tokens: None,
            session_cost_usd: None,
            last_30_days_tokens: None,
            last_30_days_cost_usd: None,
            currency_code: "USD".to_string(),
            cost_provenance: CostProvenance::Unknown,
            history_days: 30,
            history_label: None,
            daily: Vec::new(),
            updated_at: None,
        }
    }
}

impl Default for CliProviderCostSnapshot {
    fn default() -> Self {
        Self {
            used: 0.0,
            limit: 0.0,
            currency_code: "USD".to_string(),
            period: None,
            resets_at: None,
            next_regen_amount: None,
            personal_used: None,
            balance: None,
            balance_updated_at: None,
            balance_is_workspace: None,
            updated_at: "1970-01-01T00:00:00Z".to_string(),
            balance_is_unavailable: None,
        }
    }
}

impl Default for CliUsageSnapshot {
    fn default() -> Self {
        Self::new(None, None, None, "1970-01-01T00:00:00Z", None)
    }
}

/// Usage snapshot embedded in [`ProviderPayload`], preserving exact upstream `CodingKeys` order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CliUsageSnapshot {
    pub primary: Option<CliRateWindow>,
    pub secondary: Option<CliRateWindow>,
    pub tertiary: Option<CliRateWindow>,
    #[serde(rename = "extraRateWindows", skip_serializing_if = "Option::is_none")]
    pub extra_rate_windows: Option<Vec<CliNamedRateWindow>>,
    #[serde(rename = "providerCost", skip_serializing_if = "Option::is_none")]
    pub provider_cost: Option<CliProviderCostSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<DetailSection>,
    #[serde(rename = "codexResetCredits", skip_serializing_if = "Option::is_none")]
    pub codex_reset_credits: Option<CliCodexRateLimitResetCreditsSnapshot>,
    #[serde(skip)]
    pub claude_reset_credits: Option<CliClaudeRateLimitResetCreditsSnapshot>,
    #[serde(skip)]
    pub cost_usage: Option<CliCostUsageTokenSnapshot>,
    #[serde(
        rename = "copilotMeteredZeroCredits",
        default,
        skip_serializing_if = "is_false"
    )]
    pub copilot_metered_zero_credits: bool,
    #[serde(
        rename = "subscriptionExpiresAt",
        skip_serializing_if = "Option::is_none"
    )]
    pub subscription_expires_at: Option<String>,
    #[serde(
        rename = "subscriptionRenewsAt",
        skip_serializing_if = "Option::is_none"
    )]
    pub subscription_renews_at: Option<String>,
    #[serde(
        rename = "subscriptionRenewsAtIsDateOnly",
        default,
        skip_serializing_if = "is_false"
    )]
    pub subscription_renews_at_is_date_only: bool,
    #[serde(
        rename = "subscriptionExpiresAtIsDateOnly",
        default,
        skip_serializing_if = "is_false"
    )]
    pub subscription_expires_at_is_date_only: bool,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<CliProviderIdentitySnapshot>,
    #[serde(rename = "dataConfidence", skip_serializing_if = "is_unknown_or_none")]
    pub data_confidence: Option<String>,
    #[serde(rename = "accountEmail", skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(
        rename = "accountOrganization",
        skip_serializing_if = "Option::is_none"
    )]
    pub account_organization: Option<String>,
    #[serde(rename = "loginMethod", skip_serializing_if = "Option::is_none")]
    pub login_method: Option<String>,
}

impl CliUsageSnapshot {
    /// Creates a new [`CliUsageSnapshot`] and automatically mirrors `account_email`,
    /// `account_organization`, and `login_method` from `identity`.
    pub fn new(
        primary: Option<CliRateWindow>,
        secondary: Option<CliRateWindow>,
        tertiary: Option<CliRateWindow>,
        updated_at: impl Into<String>,
        identity: Option<CliProviderIdentitySnapshot>,
    ) -> Self {
        let account_email = identity.as_ref().and_then(|i| i.account_email.clone());
        let account_organization = identity
            .as_ref()
            .and_then(|i| i.account_organization.clone());
        let login_method = identity.as_ref().and_then(|i| i.login_method.clone());
        Self {
            primary,
            secondary,
            tertiary,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            codex_reset_credits: None,
            claude_reset_credits: None,
            cost_usage: None,
            copilot_metered_zero_credits: false,
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: updated_at.into(),
            identity,
            data_confidence: None,
            account_email,
            account_organization,
            login_method,
        }
    }

    /// Synchronizes top-level identity fields from `self.identity`.
    pub fn sync_identity_fields(&mut self) {
        self.account_email = self.identity.as_ref().and_then(|i| i.account_email.clone());
        self.account_organization = self
            .identity
            .as_ref()
            .and_then(|i| i.account_organization.clone());
        self.login_method = self.identity.as_ref().and_then(|i| i.login_method.clone());
    }

    /// Returns identity if it belongs to `provider_id`.
    pub fn identity_for(&self, provider_id: &str) -> Option<&CliProviderIdentitySnapshot> {
        let id = self.identity.as_ref()?;
        if id.provider_id.as_deref() == Some(provider_id) {
            Some(id)
        } else {
            None
        }
    }

    pub fn account_email_for(&self, provider_id: &str) -> Option<&str> {
        self.identity_for(provider_id)
            .and_then(|i| i.account_email.as_deref())
    }

    pub fn account_organization_for(&self, provider_id: &str) -> Option<&str> {
        self.identity_for(provider_id)
            .and_then(|i| i.account_organization.as_deref())
    }

    pub fn login_method_for(&self, provider_id: &str) -> Option<&str> {
        self.identity_for(provider_id)
            .and_then(|i| i.login_method.as_deref())
    }

    pub fn has_rate_limit_windows(&self) -> bool {
        self.primary.is_some()
            || self.secondary.is_some()
            || self.tertiary.is_some()
            || self
                .extra_rate_windows
                .as_ref()
                .is_some_and(|w| !w.is_empty())
    }

    pub fn measured_rate_windows(&self) -> Vec<&CliRateWindow> {
        let mut out = Vec::new();
        for w in [&self.primary, &self.secondary, &self.tertiary]
            .into_iter()
            .flatten()
        {
            if let Some(m) = w.measured() {
                out.push(m);
            }
        }
        if let Some(extras) = &self.extra_rate_windows {
            for named in extras {
                if named.usage_known
                    && let Some(m) = named.window.measured()
                {
                    out.push(m);
                }
            }
        }
        out
    }

    pub fn rate_limits_unavailable(&self, provider_id: &str) -> bool {
        if provider_id == "claude" {
            return self
                .primary
                .as_ref()
                .is_some_and(|p| p.is_synthetic_placeholder)
                && self.measured_rate_windows().is_empty();
        }
        if !matches!(provider_id, "codex" | "antigravity") {
            return false;
        }
        if self.identity_for(provider_id).is_none() {
            return false;
        }
        !self.has_rate_limit_windows()
    }

    /// Attaches Codex extra-usage credit cost to this snapshot (`CodexExtraUsageCost.attaching`).
    pub fn attaching_codex_credits(mut self, credits: Option<&CliCreditsSnapshot>) -> Self {
        if let Some(credits) = credits
            && let Some(cost) = CliProviderCostSnapshot::from_codex_credits(credits)
        {
            self.provider_cost = Some(cost);
            return self;
        }
        if self.provider_cost.as_ref().is_some_and(|c| {
            c.currency_code
                .eq_ignore_ascii_case(CliProviderCostSnapshot::CODEX_CREDITS_CURRENCY_CODE)
        }) {
            self.provider_cost = None;
        }
        self
    }

    /// Converts a [`ProviderSnapshot`] from the IPC v1 daemon into a [`CliUsageSnapshot`].
    pub fn from_provider_snapshot(snap: &ProviderSnapshot, fallback_updated_at: &str) -> Self {
        let updated_at = snap
            .updated_at
            .map(|ts| ts.to_string())
            .unwrap_or_else(|| fallback_updated_at.to_string());
        let primary = snap
            .windows
            .primary
            .as_ref()
            .and_then(|m| m.value.as_ref())
            .map(CliRateWindow::from);
        let secondary = snap
            .windows
            .secondary
            .as_ref()
            .and_then(|m| m.value.as_ref())
            .map(CliRateWindow::from);
        let tertiary = snap
            .windows
            .tertiary
            .as_ref()
            .and_then(|m| m.value.as_ref())
            .map(CliRateWindow::from);
        let extra: Vec<CliNamedRateWindow> = snap
            .windows
            .extra
            .iter()
            .filter_map(|nw| {
                let win = nw.window.value.as_ref()?;
                let usage_known = nw.window.state != MetricState::Unknown;
                Some(CliNamedRateWindow::with_usage_known(
                    nw.id.clone(),
                    nw.title.clone(),
                    CliRateWindow::from(win),
                    usage_known,
                ))
            })
            .collect();
        let extra_rate_windows = if extra.is_empty() { None } else { Some(extra) };
        let provider_cost = snap
            .cost
            .as_ref()
            .and_then(|m| m.value.as_ref())
            .map(|c| CliProviderCostSnapshot::from_protocol_cost(c, &updated_at));
        let identity = snap
            .identity
            .as_ref()
            .and_then(|m| m.value.as_ref())
            .map(CliProviderIdentitySnapshot::from_protocol);

        let mut usage = Self::new(primary, secondary, tertiary, updated_at, identity);
        usage.extra_rate_windows = extra_rate_windows;
        usage.provider_cost = provider_cost;
        usage.details = snap.details.clone();
        if let Some(sub) = snap.subscription.as_ref().and_then(|m| m.value.as_ref()) {
            if let Some(exp) = &sub.expires {
                let (iso, date_only) = billing_date_to_iso(exp);
                usage.subscription_expires_at = Some(iso);
                usage.subscription_expires_at_is_date_only = date_only;
            }
            if let Some(ren) = &sub.renews {
                let (iso, date_only) = billing_date_to_iso(ren);
                usage.subscription_renews_at = Some(iso);
                usage.subscription_renews_at_is_date_only = date_only;
            }
        }
        usage
    }
}

fn billing_date_to_iso(date: &BillingDate) -> (String, bool) {
    (date.at.to_string(), date.date_only)
}

/// Top-level reset credits summary payload in [`ProviderPayload`] (`ProviderResetCreditsPayload` in `CLIPayloads.swift`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderResetCreditsPayload {
    pub available: usize,
    #[serde(rename = "nextExpiresAt", skip_serializing_if = "Option::is_none")]
    pub next_expires_at: Option<String>,
}

impl ProviderResetCreditsPayload {
    /// Creates a [`ProviderResetCreditsPayload`] from [`CliCodexRateLimitResetCreditsSnapshot`] at `now`.
    pub fn from_codex(
        snapshot: &CliCodexRateLimitResetCreditsSnapshot,
        now: jiff::Timestamp,
    ) -> Self {
        let available = snapshot.available_credits(now).len();
        let next_expires_at = snapshot
            .next_expiring_credit(now)
            .and_then(|c| c.expires_at.clone());
        Self {
            available,
            next_expires_at,
        }
    }

    /// Resolves the top-level `resetCredits` summary from a [`CliUsageSnapshot`] at `now`.
    pub fn make(usage: &CliUsageSnapshot, now: jiff::Timestamp) -> Option<Self> {
        usage
            .codex_reset_credits
            .as_ref()
            .map(|c| Self::from_codex(c, now))
    }
}

/// Rate window label metadata in [`ProviderPayload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderWindowLabelsPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tertiary: Option<String>,
}

/// Pace details for a single rate window in [`ProviderPacePayload`] (`PacePayload` in `CLIPayloads.swift`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PacePayload {
    pub stage: PaceStage,
    /// Rounded (`used - expected`); positive = deficit, negative = reserve.
    #[serde(rename = "deltaPercent")]
    pub delta_percent: f64,
    #[serde(rename = "expectedUsedPercent")]
    pub expected_used_percent: f64,
    #[serde(rename = "willLastToReset")]
    pub will_last_to_reset: bool,
    #[serde(rename = "etaSeconds", skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<f64>,
    /// Always absent in CLI output; kept for schema parity.
    #[serde(rename = "runOutProbability", skip_serializing_if = "Option::is_none")]
    pub run_out_probability: Option<f64>,
    pub summary: String,
}

/// Monthly credit limit snapshot embedded in [`CliCreditsSnapshot`] (`CodexCreditLimitSnapshot` in `CreditsModels.swift`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CliCodexCreditLimitSnapshot {
    pub title: String,
    pub used: f64,
    pub limit: f64,
    pub remaining: f64,
    #[serde(rename = "remainingPercent")]
    pub remaining_percent: f64,
    #[serde(rename = "resetsAt", skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

impl CliCodexCreditLimitSnapshot {
    pub fn new(
        title: impl Into<String>,
        used: f64,
        limit: f64,
        remaining_percent: f64,
        resets_at: Option<String>,
        updated_at: impl Into<String>,
    ) -> Self {
        let title_str = title.into();
        let trimmed = title_str.trim();
        let resolved_title = if trimmed.is_empty() {
            "Monthly credit limit".to_string()
        } else {
            trimmed.to_string()
        };
        let used = used.max(0.0);
        let limit = limit.max(0.0);
        let remaining = (limit - used).max(0.0);
        let remaining_percent = remaining_percent.clamp(0.0, 100.0);
        Self {
            title: resolved_title,
            used,
            limit,
            remaining,
            remaining_percent,
            resets_at,
            updated_at: updated_at.into(),
        }
    }

    pub fn used_percent(&self) -> f64 {
        (100.0 - self.remaining_percent).clamp(0.0, 100.0)
    }
}

/// Credits snapshot embedded in [`ProviderPayload`] (`CreditsSnapshot` in `CreditsModels.swift`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CliCreditsSnapshot {
    pub remaining: f64,
    pub events: Vec<CliCreditEvent>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "codexCreditLimit", skip_serializing_if = "Option::is_none")]
    pub codex_credit_limit: Option<CliCodexCreditLimitSnapshot>,
    #[serde(rename = "balanceReadSucceeded", default = "default_true")]
    pub balance_read_succeeded: bool,
    #[serde(rename = "creditsAvailable", skip_serializing_if = "Option::is_none")]
    pub credits_available: Option<bool>,
    #[serde(rename = "balanceIsWorkspace", default)]
    pub balance_is_workspace: bool,
}

impl CliCreditsSnapshot {
    pub fn new(remaining: f64, events: Vec<CliCreditEvent>, updated_at: impl Into<String>) -> Self {
        Self {
            remaining,
            events,
            updated_at: updated_at.into(),
            codex_credit_limit: None,
            balance_read_succeeded: true,
            credits_available: None,
            balance_is_workspace: false,
        }
    }

    pub fn has_workspace_balance(&self) -> bool {
        self.balance_read_succeeded && self.balance_is_workspace
    }

    pub fn display_remaining(&self) -> Option<f64> {
        if self.has_workspace_balance() {
            return Some(self.remaining);
        }
        self.codex_credit_limit
            .as_ref()
            .map(|c| c.remaining)
            .or_else(|| self.balance_read_succeeded.then_some(self.remaining))
    }
}

/// Per-lane pace payload in [`ProviderPayload`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderPacePayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<PacePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<PacePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tertiary: Option<PacePayload>,
}

/// Status payload in [`ProviderPayload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderStatusPayload {
    pub indicator: StatusIndicator,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub url: String,
}

impl ProviderStatusPayload {
    /// Formats ` – <description>` when a non-empty description is present.
    pub fn description_suffix(&self) -> String {
        match self.description.as_deref() {
            Some(text) if !text.is_empty() => format!(" – {text}"),
            _ => String::new(),
        }
    }

    /// CLI label for the status indicator (`StatusIndicator.label` in `CLIPayloads.swift`).
    pub const fn indicator_label(&self) -> &'static str {
        status_indicator_cli_label(self.indicator)
    }
}

/// Returns the upstream CLI status indicator label (`CLIPayloads.swift:222`).
pub const fn status_indicator_cli_label(indicator: StatusIndicator) -> &'static str {
    match indicator {
        StatusIndicator::None => "Operational",
        StatusIndicator::Minor => "Partial outage",
        StatusIndicator::Major => "Major outage",
        StatusIndicator::Critical => "Critical issue",
        StatusIndicator::Maintenance => "Maintenance",
        StatusIndicator::Unknown => "Status unknown",
    }
}

impl From<&ProviderStatus> for ProviderStatusPayload {
    fn from(status: &ProviderStatus) -> Self {
        Self {
            indicator: status.indicator,
            description: status.description.clone(),
            updated_at: status.updated_at.map(|ts| ts.to_string()),
            url: status.page_url.clone().unwrap_or_default(),
        }
    }
}

/// CLI error classification kind (`CLIErrorKind` in `CLIPayloads.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CliErrorKind {
    Args,
    Config,
    Provider,
    Runtime,
}

/// Structured error payload for JSON output (`ProviderErrorPayload` in `CLIErrorReporting.swift`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderErrorPayload {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<CliErrorKind>,
}

impl ProviderErrorPayload {
    pub fn new(code: i32, message: impl Into<String>, kind: Option<CliErrorKind>) -> Self {
        Self {
            code,
            message: message.into(),
            kind,
        }
    }

    pub fn from_protocol_error(err: &ProviderError, exit_code: i32) -> Self {
        Self {
            code: exit_code,
            message: err.message.clone(),
            kind: Some(CliErrorKind::Provider),
        }
    }
}

/// Antigravity plan metadata summary in [`ProviderPayload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AntigravityPlanInfoSummary {
    #[serde(rename = "planName", skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    #[serde(rename = "planDisplayName", skip_serializing_if = "Option::is_none")]
    pub plan_display_name: Option<String>,
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(rename = "productName", skip_serializing_if = "Option::is_none")]
    pub product_name: Option<String>,
    #[serde(rename = "planShortName", skip_serializing_if = "Option::is_none")]
    pub plan_short_name: Option<String>,
    #[serde(
        rename = "monthlyPromptCredits",
        skip_serializing_if = "Option::is_none"
    )]
    pub monthly_prompt_credits: Option<i64>,
    #[serde(
        rename = "availablePromptCredits",
        skip_serializing_if = "Option::is_none"
    )]
    pub available_prompt_credits: Option<i64>,
    #[serde(rename = "monthlyFlowCredits", skip_serializing_if = "Option::is_none")]
    pub monthly_flow_credits: Option<i64>,
    #[serde(
        rename = "availableFlowCredits",
        skip_serializing_if = "Option::is_none"
    )]
    pub available_flow_credits: Option<i64>,
}

/// Credit usage event in [`OpenAiDashboardSnapshot`] (`CreditEvent` in `CreditsModels.swift`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CliCreditEvent {
    pub id: String,
    pub date: String,
    pub service: String,
    #[serde(rename = "creditsUsed")]
    pub credits_used: f64,
}

impl CliCreditEvent {
    pub fn new(
        id: impl Into<String>,
        date: impl Into<String>,
        service: impl Into<String>,
        credits_used: f64,
    ) -> Self {
        Self {
            id: id.into(),
            date: date.into(),
            service: service.into(),
            credits_used,
        }
    }
}

/// Service usage entry inside [`OpenAiDashboardDailyBreakdown`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAiDashboardServiceUsage {
    pub service: String,
    #[serde(rename = "creditsUsed")]
    pub credits_used: f64,
}

/// Single day breakdown inside [`OpenAiDashboardSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAiDashboardDailyBreakdown {
    pub day: String,
    pub services: Vec<OpenAiDashboardServiceUsage>,
    #[serde(rename = "totalCreditsUsed")]
    pub total_credits_used: f64,
}

/// OpenAI web dashboard snapshot in [`ProviderPayload`] (`OpenAIDashboardSnapshot` in `OpenAIDashboardModels.swift`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAiDashboardSnapshot {
    #[serde(rename = "signedInEmail", skip_serializing_if = "Option::is_none")]
    pub signed_in_email: Option<String>,
    #[serde(
        rename = "codeReviewRemainingPercent",
        skip_serializing_if = "Option::is_none"
    )]
    pub code_review_remaining_percent: Option<f64>,
    #[serde(rename = "codeReviewLimit", skip_serializing_if = "Option::is_none")]
    pub code_review_limit: Option<CliRateWindow>,
    #[serde(rename = "creditEvents")]
    pub credit_events: Vec<CliCreditEvent>,
    #[serde(rename = "dailyBreakdown")]
    pub daily_breakdown: Vec<OpenAiDashboardDailyBreakdown>,
    #[serde(rename = "usageBreakdown")]
    pub usage_breakdown: Vec<OpenAiDashboardDailyBreakdown>,
    #[serde(rename = "creditsPurchaseURL", skip_serializing_if = "Option::is_none")]
    pub credits_purchase_url: Option<String>,
    #[serde(rename = "primaryLimit", skip_serializing_if = "Option::is_none")]
    pub primary_limit: Option<CliRateWindow>,
    #[serde(rename = "secondaryLimit", skip_serializing_if = "Option::is_none")]
    pub secondary_limit: Option<CliRateWindow>,
    #[serde(rename = "creditsRemaining", skip_serializing_if = "Option::is_none")]
    pub credits_remaining: Option<f64>,
    #[serde(rename = "accountPlan", skip_serializing_if = "Option::is_none")]
    pub account_plan: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "creditsUnlimited", skip_serializing_if = "Option::is_none")]
    pub credits_unlimited: Option<bool>,
}

/// Top-level per-provider payload for `ariadusage usage --format json` and `--format toon`
/// (`ProviderPayload` in `CLIPayloads.swift`), preserving exact `CodingKeys` field order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    pub usage: Option<CliUsageSnapshot>,
    #[serde(rename = "rateWindowLabels", skip_serializing_if = "Option::is_none")]
    pub rate_window_labels: Option<ProviderWindowLabelsPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<CliCreditsSnapshot>,
    #[serde(rename = "resetCredits", skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<ProviderResetCreditsPayload>,
    #[serde(
        rename = "antigravityPlanInfo",
        skip_serializing_if = "Option::is_none"
    )]
    pub antigravity_plan_info: Option<AntigravityPlanInfoSummary>,
    #[serde(rename = "openaiDashboard", skip_serializing_if = "Option::is_none")]
    pub openai_dashboard: Option<OpenAiDashboardSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderErrorPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<ProviderPacePayload>,
    #[serde(rename = "ageSeconds", skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<i64>,
}
