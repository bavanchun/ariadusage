// Ported from CodexBar Sources/CodexBarCore/OpenAIDashboardModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/OpenAISubscriptionMetadataModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/OpenAIWeb/OpenAISubscriptionMetadata.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ariadusage_core::model::snapshot::UsageSnapshot;
use ariadusage_core::model::window::{NamedRateWindow, RateWindow};
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::fs_private::write_private;

use super::credits::{CodexCreditLimitSnapshot, CreditEvent, CreditsSnapshot};
use super::reconciled::CodexReconciledState;

pub const DEFAULT_OPENAI_SUBSCRIPTIONS_URL: &str = "https://chatgpt.com/backend-api/subscriptions";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAISubscriptionMetadata {
    pub subscription_expires_at: Option<Timestamp>,
    #[serde(default)]
    pub subscription_expires_at_is_date_only: bool,
    pub subscription_renews_at: Option<Timestamp>,
    #[serde(default)]
    pub subscription_renews_at_is_date_only: bool,
}

impl OpenAISubscriptionMetadata {
    pub fn new(
        subscription_expires_at: Option<Timestamp>,
        subscription_renews_at: Option<Timestamp>,
    ) -> Self {
        Self {
            subscription_expires_at,
            subscription_expires_at_is_date_only: false,
            subscription_renews_at,
            subscription_renews_at_is_date_only: false,
        }
    }

    pub fn with_date_only_flags(
        subscription_expires_at: Option<Timestamp>,
        subscription_expires_at_is_date_only: bool,
        subscription_renews_at: Option<Timestamp>,
        subscription_renews_at_is_date_only: bool,
    ) -> Self {
        Self {
            subscription_expires_at,
            subscription_expires_at_is_date_only,
            subscription_renews_at,
            subscription_renews_at_is_date_only,
        }
    }

    pub fn has_any_date(&self) -> bool {
        self.subscription_expires_at.is_some() || self.subscription_renews_at.is_some()
    }

    pub fn applying(&self, snapshot: &UsageSnapshot) -> UsageSnapshot {
        if !self.has_any_date() {
            return snapshot.clone();
        }
        snapshot.clone().with_subscription_metadata(
            self.subscription_expires_at,
            self.subscription_renews_at,
            self.subscription_expires_at_is_date_only,
            self.subscription_renews_at_is_date_only,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAISubscriptionFetchResult {
    Resolved(OpenAISubscriptionMetadata),
    ConfirmedAbsent,
    Unavailable,
}

impl OpenAISubscriptionFetchResult {
    pub fn metadata(&self) -> Option<&OpenAISubscriptionMetadata> {
        match self {
            Self::Resolved(metadata) => Some(metadata),
            Self::ConfirmedAbsent | Self::Unavailable => None,
        }
    }

    pub fn applying(&self, snapshot: &UsageSnapshot) -> UsageSnapshot {
        match self {
            Self::Resolved(metadata) => metadata.applying(snapshot),
            Self::ConfirmedAbsent => {
                if snapshot.subscription_expires_at.is_none()
                    && snapshot.subscription_renews_at.is_none()
                {
                    return snapshot.clone();
                }
                snapshot
                    .clone()
                    .with_subscription_metadata(None, None, false, false)
            }
            Self::Unavailable => snapshot.clone(),
        }
    }
}

pub struct OpenAISubscriptionMetadataParser;

#[derive(Debug, Clone, PartialEq, Eq)]
enum SubscriptionItemParseResult {
    Resolved(OpenAISubscriptionMetadata),
    ConfirmedAbsent,
    Missing,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActiveUntilParseResult {
    Resolved(Timestamp, bool),
    ExplicitlyAbsent,
    Missing,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WillRenewParseResult {
    Value(bool),
    Absent,
    Invalid,
}

impl OpenAISubscriptionMetadataParser {
    pub fn parse(data: &[u8]) -> Option<OpenAISubscriptionMetadata> {
        Self::parse_result(data).metadata().cloned()
    }

    pub fn parse_result(data: &[u8]) -> OpenAISubscriptionFetchResult {
        let Ok(value) = serde_json::from_slice::<Value>(data) else {
            return OpenAISubscriptionFetchResult::Unavailable;
        };
        match value {
            Value::Object(obj) => Self::parse_collection(&[obj]),
            Value::Array(arr) => {
                let mut objects = Vec::with_capacity(arr.len());
                for item in arr {
                    let Value::Object(obj) = item else {
                        return OpenAISubscriptionFetchResult::Unavailable;
                    };
                    objects.push(obj);
                }
                Self::parse_collection(&objects)
            }
            _ => OpenAISubscriptionFetchResult::Unavailable,
        }
    }

    fn parse_collection(
        items: &[serde_json::Map<String, Value>],
    ) -> OpenAISubscriptionFetchResult {
        if items.is_empty() {
            return OpenAISubscriptionFetchResult::ConfirmedAbsent;
        }
        let mut saw_confirmed_absent = false;
        for item in items {
            for key in ["subscriptions", "data"] {
                if let Some(raw_nested) = item.get(key) {
                    match raw_nested {
                        Value::Null => continue,
                        Value::Array(arr) => {
                            let mut nested = Vec::with_capacity(arr.len());
                            for child in arr {
                                let Value::Object(obj) = child else {
                                    return OpenAISubscriptionFetchResult::Unavailable;
                                };
                                nested.push(obj.clone());
                            }
                            match Self::parse_collection(&nested) {
                                OpenAISubscriptionFetchResult::Resolved(metadata) => {
                                    return OpenAISubscriptionFetchResult::Resolved(metadata);
                                }
                                OpenAISubscriptionFetchResult::ConfirmedAbsent => {
                                    saw_confirmed_absent = true;
                                }
                                OpenAISubscriptionFetchResult::Unavailable => {
                                    return OpenAISubscriptionFetchResult::Unavailable;
                                }
                            }
                        }
                        _ => return OpenAISubscriptionFetchResult::Unavailable,
                    }
                }
            }

            match Self::parse_item(item) {
                SubscriptionItemParseResult::Resolved(metadata) => {
                    return OpenAISubscriptionFetchResult::Resolved(metadata);
                }
                SubscriptionItemParseResult::ConfirmedAbsent => {
                    saw_confirmed_absent = true;
                }
                SubscriptionItemParseResult::Missing => {}
                SubscriptionItemParseResult::Invalid => {
                    return OpenAISubscriptionFetchResult::Unavailable;
                }
            }
        }

        if saw_confirmed_absent {
            OpenAISubscriptionFetchResult::ConfirmedAbsent
        } else {
            OpenAISubscriptionFetchResult::Unavailable
        }
    }

    fn parse_item(item: &serde_json::Map<String, Value>) -> SubscriptionItemParseResult {
        let active_until = Self::parse_active_until(item);
        let will_renew = Self::parse_will_renew(item);

        match active_until {
            ActiveUntilParseResult::Missing => {
                if will_renew == WillRenewParseResult::Invalid {
                    SubscriptionItemParseResult::Invalid
                } else {
                    SubscriptionItemParseResult::Missing
                }
            }
            ActiveUntilParseResult::Invalid => SubscriptionItemParseResult::Invalid,
            ActiveUntilParseResult::ExplicitlyAbsent => {
                if will_renew == WillRenewParseResult::Invalid {
                    SubscriptionItemParseResult::Invalid
                } else {
                    SubscriptionItemParseResult::ConfirmedAbsent
                }
            }
            ActiveUntilParseResult::Resolved(ts, is_date_only) => match will_renew {
                WillRenewParseResult::Invalid => SubscriptionItemParseResult::Invalid,
                WillRenewParseResult::Value(true) => SubscriptionItemParseResult::Resolved(
                    OpenAISubscriptionMetadata::with_date_only_flags(
                        None,
                        false,
                        Some(ts),
                        is_date_only,
                    ),
                ),
                WillRenewParseResult::Value(false) | WillRenewParseResult::Absent => {
                    SubscriptionItemParseResult::Resolved(
                        OpenAISubscriptionMetadata::with_date_only_flags(
                            Some(ts),
                            is_date_only,
                            None,
                            false,
                        ),
                    )
                }
            },
        }
    }

    fn parse_active_until(item: &serde_json::Map<String, Value>) -> ActiveUntilParseResult {
        let raw_value = item.get("active_until").or_else(|| item.get("activeUntil"));
        let Some(raw_value) = raw_value else {
            return ActiveUntilParseResult::Missing;
        };
        match raw_value {
            Value::Null => ActiveUntilParseResult::ExplicitlyAbsent,
            Value::String(raw) => match Self::parse_date(raw) {
                Some((ts, is_date_only)) => ActiveUntilParseResult::Resolved(ts, is_date_only),
                None => ActiveUntilParseResult::Invalid,
            },
            _ => ActiveUntilParseResult::Invalid,
        }
    }

    fn parse_will_renew(item: &serde_json::Map<String, Value>) -> WillRenewParseResult {
        let raw_value = item.get("will_renew").or_else(|| item.get("willRenew"));
        let Some(raw_value) = raw_value else {
            return WillRenewParseResult::Absent;
        };
        match raw_value {
            Value::Null => WillRenewParseResult::Absent,
            Value::Bool(b) => WillRenewParseResult::Value(*b),
            _ => WillRenewParseResult::Invalid,
        }
    }

    fn parse_date(raw: &str) -> Option<(Timestamp, bool)> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.len() == 10 && trimmed.as_bytes()[4] == b'-' && trimmed.as_bytes()[7] == b'-' {
            let date = Date::strptime("%Y-%m-%d", trimmed).ok()?;
            let ts = date.at(0, 0, 0, 0).to_zoned(TimeZone::UTC).ok()?.timestamp();
            return Some((ts, true));
        }
        let ts = trimmed.parse::<Timestamp>().ok()?;
        Some((ts, false))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIDashboardServiceUsage {
    pub service: String,
    pub credits_used: f64,
}

impl OpenAIDashboardServiceUsage {
    pub fn new(service: impl Into<String>, credits_used: f64) -> Self {
        Self {
            service: service.into(),
            credits_used,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIDashboardUsageBreakdownSummary {
    pub daily: Vec<OpenAIDashboardDailyBreakdown>,
    pub today_credits: Option<f64>,
    pub total_credits: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIDashboardDailyBreakdown {
    pub day: String,
    pub services: Vec<OpenAIDashboardServiceUsage>,
    pub total_credits_used: f64,
}

impl OpenAIDashboardDailyBreakdown {
    pub fn new(
        day: impl Into<String>,
        services: Vec<OpenAIDashboardServiceUsage>,
        total_credits_used: f64,
    ) -> Self {
        Self {
            day: day.into(),
            services,
            total_credits_used,
        }
    }

    pub fn recent_usage_summary(
        breakdown: &[Self],
        history_days: i32,
        now: Timestamp,
        time_zone: &TimeZone,
    ) -> OpenAIDashboardUsageBreakdownSummary {
        let days = history_days.clamp(1, 365);
        let today_zoned = now.to_zoned(time_zone.clone());
        let today_date = today_zoned.date();
        let span_days = i64::from(days - 1);
        let start_date = today_date
            .checked_sub(jiff::Span::new().days(span_days))
            .unwrap_or(today_date);
        let today_key = format!(
            "{:04}-{:02}-{:02}",
            today_date.year(),
            today_date.month(),
            today_date.day()
        );
        let start_key = format!(
            "{:04}-{:02}-{:02}",
            start_date.year(),
            start_date.month(),
            start_date.day()
        );

        let mut sanitized_by_day: BTreeMap<String, OpenAIDashboardDailyBreakdown> = BTreeMap::new();
        for item in breakdown {
            let day = item.day.trim();
            if !Self::is_valid_day_key(day) || day < start_key.as_str() || day > today_key.as_str()
            {
                continue;
            }
            let mut merged_services: BTreeMap<String, f64> = BTreeMap::new();
            for service in &item.services {
                let name = service.service.trim();
                let amount = service.credits_used;
                if name.is_empty() || !amount.is_finite() || amount <= 0.0 {
                    continue;
                }
                let next = merged_services.get(name).copied().unwrap_or(0.0) + amount;
                if !next.is_finite() {
                    return OpenAIDashboardUsageBreakdownSummary {
                        daily: Vec::new(),
                        today_credits: None,
                        total_credits: None,
                    };
                }
                merged_services.insert(name.to_owned(), next);
            }

            let mut services: Vec<OpenAIDashboardServiceUsage> = merged_services
                .into_iter()
                .map(|(service, credits_used)| OpenAIDashboardServiceUsage {
                    service,
                    credits_used,
                })
                .collect();
            services.sort_by(|lhs, rhs| {
                if (lhs.credits_used - rhs.credits_used).abs() < f64::EPSILON {
                    lhs.service.cmp(&rhs.service)
                } else {
                    rhs.credits_used
                        .partial_cmp(&lhs.credits_used)
                        .unwrap_or(Ordering::Equal)
                }
            });

            let services_total: f64 = services.iter().map(|s| s.credits_used).sum();
            let total_credits_used = if item.total_credits_used.is_finite()
                && item.total_credits_used > 0.0
            {
                if services_total.is_finite() && services_total > item.total_credits_used {
                    services_total
                } else {
                    item.total_credits_used
                }
            } else {
                services_total
            };
            if !total_credits_used.is_finite() || total_credits_used <= 0.0 {
                continue;
            }

            if let Some(existing) = sanitized_by_day.get(day).cloned() {
                let mut combined_services: BTreeMap<String, f64> = BTreeMap::new();
                for service in existing.services.iter().chain(services.iter()) {
                    let next = combined_services
                        .get(&service.service)
                        .copied()
                        .unwrap_or(0.0)
                        + service.credits_used;
                    if !next.is_finite() {
                        return OpenAIDashboardUsageBreakdownSummary {
                            daily: Vec::new(),
                            today_credits: None,
                            total_credits: None,
                        };
                    }
                    combined_services.insert(service.service.clone(), next);
                }
                let mut merged_list: Vec<OpenAIDashboardServiceUsage> = combined_services
                    .into_iter()
                    .map(|(service, credits_used)| OpenAIDashboardServiceUsage {
                        service,
                        credits_used,
                    })
                    .collect();
                merged_list.sort_by(|lhs, rhs| {
                    if (lhs.credits_used - rhs.credits_used).abs() < f64::EPSILON {
                        lhs.service.cmp(&rhs.service)
                    } else {
                        rhs.credits_used
                            .partial_cmp(&lhs.credits_used)
                            .unwrap_or(Ordering::Equal)
                    }
                });
                let combined_total = existing.total_credits_used + total_credits_used;
                if !combined_total.is_finite() {
                    return OpenAIDashboardUsageBreakdownSummary {
                        daily: Vec::new(),
                        today_credits: None,
                        total_credits: None,
                    };
                }
                sanitized_by_day.insert(
                    day.to_owned(),
                    OpenAIDashboardDailyBreakdown {
                        day: day.to_owned(),
                        services: merged_list,
                        total_credits_used: combined_total,
                    },
                );
            } else {
                sanitized_by_day.insert(
                    day.to_owned(),
                    OpenAIDashboardDailyBreakdown {
                        day: day.to_owned(),
                        services,
                        total_credits_used,
                    },
                );
            }
        }

        let mut daily: Vec<OpenAIDashboardDailyBreakdown> =
            sanitized_by_day.into_values().collect();
        daily.sort_by(|a, b| b.day.cmp(&a.day));
        if daily.is_empty() {
            return OpenAIDashboardUsageBreakdownSummary {
                daily: Vec::new(),
                today_credits: None,
                total_credits: None,
            };
        }

        let total_credits: f64 = daily.iter().map(|d| d.total_credits_used).sum();
        if !total_credits.is_finite() {
            return OpenAIDashboardUsageBreakdownSummary {
                daily: Vec::new(),
                today_credits: None,
                total_credits: None,
            };
        }
        let today_credits = daily
            .iter()
            .find(|d| d.day == today_key)
            .map(|d| d.total_credits_used)
            .filter(|v| v.is_finite());

        OpenAIDashboardUsageBreakdownSummary {
            daily,
            today_credits,
            total_credits: Some(total_credits),
        }
    }

    fn is_valid_day_key(day: &str) -> bool {
        if day.len() != 10 || day.as_bytes()[4] != b'-' || day.as_bytes()[7] != b'-' {
            return false;
        }
        Date::strptime("%Y-%m-%d", day).is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIDashboardSnapshot {
    pub signed_in_email: Option<String>,
    pub account_id: Option<String>,
    pub code_review_remaining_percent: Option<f64>,
    pub code_review_limit: Option<RateWindow>,
    pub credit_events: Vec<CreditEvent>,
    pub daily_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
    pub usage_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
    pub credits_purchase_url: Option<String>,
    pub primary_limit: Option<RateWindow>,
    pub secondary_limit: Option<RateWindow>,
    pub extra_rate_windows: Option<Vec<NamedRateWindow>>,
    pub credits_remaining: Option<f64>,
    pub credits_available: Option<bool>,
    pub balance_is_workspace: Option<bool>,
    pub codex_credit_limit: Option<CodexCreditLimitSnapshot>,
    pub account_plan: Option<String>,
    pub subscription_expires_at: Option<Timestamp>,
    #[serde(skip)]
    pub subscription_expires_at_is_date_only: bool,
    pub subscription_renews_at: Option<Timestamp>,
    #[serde(skip)]
    pub subscription_renews_at_is_date_only: bool,
    pub updated_at: Timestamp,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOpenAIDashboardSnapshot {
    signed_in_email: Option<String>,
    account_id: Option<String>,
    code_review_remaining_percent: Option<f64>,
    code_review_limit: Option<RateWindow>,
    #[serde(default)]
    credit_events: Vec<CreditEvent>,
    daily_breakdown: Option<Vec<OpenAIDashboardDailyBreakdown>>,
    #[serde(default)]
    usage_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
    credits_purchase_url: Option<String>,
    primary_limit: Option<RateWindow>,
    secondary_limit: Option<RateWindow>,
    extra_rate_windows: Option<Vec<NamedRateWindow>>,
    credits_remaining: Option<f64>,
    credits_available: Option<bool>,
    balance_is_workspace: Option<bool>,
    codex_credit_limit: Option<CodexCreditLimitSnapshot>,
    account_plan: Option<String>,
    subscription_expires_at: Option<Timestamp>,
    subscription_renews_at: Option<Timestamp>,
    updated_at: Option<Timestamp>,
}

impl<'de> Deserialize<'de> for OpenAIDashboardSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawOpenAIDashboardSnapshot::deserialize(deserializer)?;
        let daily_breakdown = raw
            .daily_breakdown
            .unwrap_or_else(|| Self::make_daily_breakdown(&raw.credit_events, 30));
        let usage_breakdown = Self::removing_skill_usage_services(&raw.usage_breakdown);
        Ok(Self {
            signed_in_email: raw.signed_in_email,
            account_id: raw.account_id,
            code_review_remaining_percent: raw.code_review_remaining_percent,
            code_review_limit: raw.code_review_limit,
            credit_events: raw.credit_events,
            daily_breakdown,
            usage_breakdown,
            credits_purchase_url: raw.credits_purchase_url,
            primary_limit: raw.primary_limit,
            secondary_limit: raw.secondary_limit,
            extra_rate_windows: raw.extra_rate_windows,
            credits_remaining: raw.credits_remaining,
            credits_available: raw.credits_available,
            balance_is_workspace: raw.balance_is_workspace,
            codex_credit_limit: raw.codex_credit_limit,
            account_plan: raw.account_plan,
            subscription_expires_at: raw.subscription_expires_at,
            subscription_expires_at_is_date_only: false,
            subscription_renews_at: raw.subscription_renews_at,
            subscription_renews_at_is_date_only: false,
            updated_at: raw.updated_at.unwrap_or_else(Timestamp::now),
        })
    }
}

impl Default for OpenAIDashboardSnapshot {
    fn default() -> Self {
        Self {
            signed_in_email: None,
            account_id: None,
            code_review_remaining_percent: None,
            code_review_limit: None,
            credit_events: Vec::new(),
            daily_breakdown: Vec::new(),
            usage_breakdown: Vec::new(),
            credits_purchase_url: None,
            primary_limit: None,
            secondary_limit: None,
            extra_rate_windows: None,
            credits_remaining: None,
            credits_available: None,
            balance_is_workspace: None,
            codex_credit_limit: None,
            account_plan: None,
            subscription_expires_at: None,
            subscription_expires_at_is_date_only: false,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            updated_at: Timestamp::UNIX_EPOCH,
        }
    }
}

impl OpenAIDashboardSnapshot {
    const EXCLUDED_USAGE_BREAKDOWN_SERVICE_PREFIXES: &'static [&'static str] = &["skillusage:"];

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        signed_in_email: Option<String>,
        account_id: Option<String>,
        code_review_remaining_percent: Option<f64>,
        code_review_limit: Option<RateWindow>,
        credit_events: Vec<CreditEvent>,
        daily_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
        usage_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
        credits_purchase_url: Option<String>,
        primary_limit: Option<RateWindow>,
        secondary_limit: Option<RateWindow>,
        extra_rate_windows: Option<Vec<NamedRateWindow>>,
        credits_remaining: Option<f64>,
        credits_available: Option<bool>,
        balance_is_workspace: Option<bool>,
        codex_credit_limit: Option<CodexCreditLimitSnapshot>,
        account_plan: Option<String>,
        subscription_expires_at: Option<Timestamp>,
        subscription_renews_at: Option<Timestamp>,
        updated_at: Timestamp,
    ) -> Self {
        Self {
            signed_in_email,
            account_id,
            code_review_remaining_percent,
            code_review_limit,
            credit_events,
            daily_breakdown,
            usage_breakdown: Self::removing_skill_usage_services(&usage_breakdown),
            credits_purchase_url,
            primary_limit,
            secondary_limit,
            extra_rate_windows,
            credits_remaining,
            credits_available,
            balance_is_workspace,
            codex_credit_limit,
            account_plan,
            subscription_expires_at,
            subscription_expires_at_is_date_only: false,
            subscription_renews_at,
            subscription_renews_at_is_date_only: false,
            updated_at,
        }
    }

    pub fn removing_skill_usage_services(
        breakdown: &[OpenAIDashboardDailyBreakdown],
    ) -> Vec<OpenAIDashboardDailyBreakdown> {
        let mut out = Vec::new();
        for item in breakdown {
            let mut filtered_services = Vec::new();
            let mut removed_any = false;
            for service in &item.services {
                let normalized = service.service.trim().to_ascii_lowercase();
                if Self::EXCLUDED_USAGE_BREAKDOWN_SERVICE_PREFIXES
                    .iter()
                    .any(|prefix| normalized.starts_with(prefix))
                {
                    removed_any = true;
                } else {
                    filtered_services.push(service.clone());
                }
            }
            if !removed_any {
                out.push(item.clone());
                continue;
            }
            if !filtered_services.is_empty() {
                let filtered_total: f64 = filtered_services.iter().map(|s| s.credits_used).sum();
                out.push(OpenAIDashboardDailyBreakdown {
                    day: item.day.clone(),
                    services: filtered_services,
                    total_credits_used: filtered_total,
                });
            } else if item.services.is_empty()
                && item.total_credits_used.is_finite()
                && item.total_credits_used > 0.0
            {
                out.push(item.clone());
            }
        }
        out
    }

    pub fn make_daily_breakdown(
        events: &[CreditEvent],
        max_days: usize,
    ) -> Vec<OpenAIDashboardDailyBreakdown> {
        if events.is_empty() || max_days == 0 {
            return Vec::new();
        }
        let mut totals_by_day: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
        for event in events {
            if !event.credits_used.is_finite() || event.credits_used <= 0.0 {
                continue;
            }
            let utc = event.date.to_zoned(TimeZone::UTC);
            let day = format!("{:04}-{:02}-{:02}", utc.year(), utc.month(), utc.day());
            let day_map = totals_by_day.entry(day).or_default();
            *day_map.entry(event.service.clone()).or_insert(0.0) += event.credits_used;
        }

        let mut sorted_days: Vec<String> = totals_by_day.keys().cloned().collect();
        sorted_days.sort_by(|a, b| b.cmp(a));
        sorted_days.truncate(max_days);

        let mut result = Vec::with_capacity(sorted_days.len());
        for day in sorted_days {
            let Some(service_map) = totals_by_day.remove(&day) else {
                continue;
            };
            let mut services: Vec<OpenAIDashboardServiceUsage> = service_map
                .into_iter()
                .map(|(service, credits_used)| OpenAIDashboardServiceUsage {
                    service,
                    credits_used,
                })
                .collect();
            services.sort_by(|lhs, rhs| {
                if (lhs.credits_used - rhs.credits_used).abs() < f64::EPSILON {
                    lhs.service.cmp(&rhs.service)
                } else {
                    rhs.credits_used
                        .partial_cmp(&lhs.credits_used)
                        .unwrap_or(Ordering::Equal)
                }
            });
            let total: f64 = services.iter().map(|s| s.credits_used).sum();
            result.push(OpenAIDashboardDailyBreakdown {
                day,
                services,
                total_credits_used: total,
            });
        }
        result
    }

    pub fn to_usage_snapshot(
        &self,
        provider: ProviderId,
        account_email: Option<&str>,
        account_plan: Option<&str>,
    ) -> Option<UsageSnapshot> {
        CodexReconciledState::from_attached_dashboard(self, provider, account_email, account_plan)
            .map(|r| r.to_usage_snapshot(Confidence::Exact))
    }

    pub fn to_credits_snapshot(&self) -> Option<CreditsSnapshot> {
        if self.credits_remaining.is_none()
            && self.codex_credit_limit.is_none()
            && self.credits_available != Some(true)
        {
            return None;
        }
        Some(CreditsSnapshot::with_details(
            self.credits_remaining.unwrap_or(0.0),
            self.credit_events.clone(),
            self.updated_at,
            self.codex_credit_limit.clone(),
            self.credits_remaining.is_some(),
            self.credits_available,
            self.balance_is_workspace == Some(true),
        ))
    }

    pub fn to_token_cost_snapshot(
        &self,
        now: Timestamp,
        time_zone: &TimeZone,
    ) -> Option<OpenAIDashboardUsageBreakdownSummary> {
        let source = if !self.usage_breakdown.is_empty() {
            &self.usage_breakdown
        } else {
            &self.daily_breakdown
        };
        let summary = OpenAIDashboardDailyBreakdown::recent_usage_summary(source, 30, now, time_zone);
        if summary.daily.is_empty() {
            None
        } else {
            Some(summary)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIDashboardCache {
    pub account_email: String,
    pub snapshot: OpenAIDashboardSnapshot,
}

impl OpenAIDashboardCache {
    pub fn new(account_email: impl Into<String>, snapshot: OpenAIDashboardSnapshot) -> Self {
        Self {
            account_email: account_email.into(),
            snapshot,
        }
    }
}

#[derive(Clone, Default)]
pub struct OpenAIDashboardCacheStore {
    memory: Arc<Mutex<Option<OpenAIDashboardCache>>>,
    file_path: Option<PathBuf>,
}

impl std::fmt::Debug for OpenAIDashboardCacheStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAIDashboardCacheStore")
            .field("has_file_path", &self.file_path.is_some())
            .finish()
    }
}

impl OpenAIDashboardCacheStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_file_path(file_path: impl Into<PathBuf>) -> Self {
        Self {
            memory: Arc::new(Mutex::new(None)),
            file_path: Some(file_path.into()),
        }
    }

    pub fn load(&self) -> Option<OpenAIDashboardCache> {
        if let Some(path) = &self.file_path {
            return Self::load_from_path(path);
        }
        self.memory
            .lock()
            .expect("dashboard cache lock not poisoned")
            .clone()
    }

    pub fn save(&self, cache: &OpenAIDashboardCache) {
        if let Some(path) = &self.file_path {
            let _ = Self::save_to_path(cache, path);
            return;
        }
        let mut guard = self
            .memory
            .lock()
            .expect("dashboard cache lock not poisoned");
        *guard = Some(cache.clone());
    }

    pub fn clear(&self) {
        if let Some(path) = &self.file_path {
            Self::clear_path(path);
            return;
        }
        let mut guard = self
            .memory
            .lock()
            .expect("dashboard cache lock not poisoned");
        *guard = None;
    }

    pub fn load_from_path(path: &Path) -> Option<OpenAIDashboardCache> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save_to_path(cache: &OpenAIDashboardCache, path: &Path) -> std::io::Result<()> {
        let mut bytes = serde_json::to_vec(cache).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        write_private(path, &bytes)
    }

    pub fn clear_path(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}
