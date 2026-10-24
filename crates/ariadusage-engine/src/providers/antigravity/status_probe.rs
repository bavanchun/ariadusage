// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+Deadline.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+LocalEndpoints.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+PortDetection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+ResponseModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityLocalhostSession.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ariadusage_core::model::{
    NamedRateWindow, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_core::pipeline::{BoxFuture, ClassifiedError, TransportClass};
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use reqwest::Method;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use serde_json::json;

use crate::brokers::call::BrokerCall;
use crate::brokers::local_probe::{LocalProbe, ProbeError};
use crate::brokers::net::{NetBody, NetBroker, NetError, NetRequest};
use crate::brokers::process::ProcessIdentity;

use super::family_visibility::{KnownFamily, contains_token, known_family};
use super::quota_summary::{
    AntigravityQuotaSummary, AntigravityQuotaSummaryBucket, parse_cli_usage_report,
    parse_quota_summary_response,
};

pub const GET_USER_STATUS_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/GetUserStatus";
pub const COMMAND_MODEL_CONFIG_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/GetCommandModelConfigs";
pub const UNLEASH_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUnleashData";
pub const QUOTA_SUMMARY_WINDOW_ID_PREFIX: &str = "antigravity-quota-summary-";
pub const COMPACT_FALLBACK_WINDOW_ID_PREFIX: &str = "antigravity-compact-fallback-";
pub const CURRENT_FLASH_CANONICAL_ID: &str = "gemini-3.7-flash";

const RETIRED_FLASH_PREFIXES: &[&str] = &["gemini-3.6-flash", "gemini-3.5-flash", "gemini-3-flash"];

#[derive(Debug, Clone, PartialEq)]
pub struct AntigravityModelQuota {
    pub label: String,
    pub model_id: String,
    pub remaining_fraction: Option<f64>,
    pub reset_time: Option<Timestamp>,
    pub reset_description: Option<String>,
}

impl AntigravityModelQuota {
    pub fn new(
        label: impl Into<String>,
        model_id: impl Into<String>,
        remaining_fraction: Option<f64>,
        reset_time: Option<Timestamp>,
        reset_description: Option<String>,
    ) -> Self {
        Self {
            label: label.into(),
            model_id: model_id.into(),
            remaining_fraction,
            reset_time,
            reset_description,
        }
    }

    pub fn remaining_percent(&self) -> f64 {
        let fraction = self.remaining_fraction.unwrap_or(0.0);
        fraction.clamp(0.0, 1.0) * 100.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntigravitySnapshotSource {
    Local,
    Remote,
}

#[derive(Clone, PartialEq)]
pub struct AntigravityStatusSnapshot {
    pub model_quotas: Vec<AntigravityModelQuota>,
    pub account_email: Option<String>,
    pub account_plan: Option<String>,
    pub source: AntigravitySnapshotSource,
    pub quota_summary: Option<AntigravityQuotaSummary>,
}

impl fmt::Debug for AntigravityStatusSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AntigravityStatusSnapshot")
            .field("model_quotas_count", &self.model_quotas.len())
            .field("has_account_email", &self.account_email.is_some())
            .field("account_plan", &self.account_plan)
            .field("source", &self.source)
            .field("has_quota_summary", &self.quota_summary.is_some())
            .finish()
    }
}

impl AntigravityStatusSnapshot {
    pub fn new(
        model_quotas: Vec<AntigravityModelQuota>,
        account_email: Option<String>,
        account_plan: Option<String>,
    ) -> Self {
        Self {
            model_quotas,
            account_email,
            account_plan,
            source: AntigravitySnapshotSource::Local,
            quota_summary: None,
        }
    }

    pub fn with_source(mut self, source: AntigravitySnapshotSource) -> Self {
        self.source = source;
        self
    }

    pub fn with_quota_summary(mut self, quota_summary: Option<AntigravityQuotaSummary>) -> Self {
        self.quota_summary = quota_summary;
        self
    }

    pub fn has_known_quota_summary(&self) -> bool {
        let Some(quota_summary) = &self.quota_summary else {
            return false;
        };
        quota_summary
            .groups
            .iter()
            .flat_map(|g| &g.buckets)
            .any(|b| !b.disabled && b.remaining_fraction.is_some())
    }

    pub fn has_quota_summary_buckets(&self) -> bool {
        let Some(quota_summary) = &self.quota_summary else {
            return false;
        };
        quota_summary.groups.iter().any(|g| !g.buckets.is_empty())
    }

    pub fn canonical_model_id(raw: &str) -> String {
        let trimmed = raw.trim();
        let lower = trimmed.to_lowercase();
        let Some(prefix) = Self::retired_flash_prefix(&lower) else {
            return trimmed.to_owned();
        };
        let suffix = lower[prefix.len()..].trim_start_matches('-');
        if suffix == "lite" || suffix.starts_with("lite-") {
            return trimmed.to_owned();
        }
        if contains_token("image", suffix) {
            return trimmed.to_owned();
        }
        CURRENT_FLASH_CANONICAL_ID.to_owned()
    }

    fn retired_flash_prefix(lower: &str) -> Option<&'static str> {
        RETIRED_FLASH_PREFIXES
            .iter()
            .copied()
            .find(|&prefix| lower == prefix || lower.starts_with(&format!("{prefix}-")))
    }

    pub fn humanized_model_id(raw: &str) -> String {
        let canonical = Self::canonical_model_id(raw);
        let trimmed = canonical.trim();
        if trimmed.is_empty() {
            return String::new();
        }

        let raw_tokens: Vec<&str> = trimmed
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .collect();
        if raw_tokens.is_empty() {
            return trimmed.to_owned();
        }

        let mut tokens: Vec<String> = Vec::with_capacity(raw_tokens.len());
        let mut index = 0;
        while index < raw_tokens.len() {
            if index + 1 < raw_tokens.len()
                && Self::is_numeric_version_token(raw_tokens[index])
                && Self::is_numeric_version_token(raw_tokens[index + 1])
            {
                tokens.push(format!("{}.{}", raw_tokens[index], raw_tokens[index + 1]));
                index += 2;
                continue;
            }
            tokens.push(raw_tokens[index].to_owned());
            index += 1;
        }

        tokens
            .into_iter()
            .map(|token| {
                let lower = token.to_lowercase();
                match lower.as_str() {
                    "gpt" => "GPT".to_owned(),
                    "oss" => "OSS".to_owned(),
                    "api" => "API".to_owned(),
                    _ => {
                        if token.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                            token
                        } else {
                            let mut chars = lower.chars();
                            match chars.next() {
                                Some(first) => {
                                    let mut s = first.to_uppercase().to_string();
                                    s.push_str(chars.as_str());
                                    s
                                }
                                None => String::new(),
                            }
                        }
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn is_numeric_version_token(token: &str) -> bool {
        !token.is_empty() && token.chars().all(|c| c.is_ascii_digit())
    }

    pub fn quota_display_label(quota: &AntigravityModelQuota) -> String {
        let trimmed_label = quota.label.trim();
        let trimmed_model_id = quota.model_id.trim();
        if !trimmed_label.is_empty() && !trimmed_label.eq_ignore_ascii_case(trimmed_model_id) {
            return trimmed_label.to_owned();
        }
        if !trimmed_model_id.is_empty() {
            return Self::humanized_model_id(trimmed_model_id);
        }
        trimmed_label.to_owned()
    }

    pub fn to_usage_snapshot(&self) -> Result<UsageSnapshot, AntigravityStatusProbeError> {
        self.to_usage_snapshot_at(Timestamp::now())
    }

    pub fn to_usage_snapshot_at(
        &self,
        now: Timestamp,
    ) -> Result<UsageSnapshot, AntigravityStatusProbeError> {
        if (self.has_known_quota_summary()
            || (self.has_quota_summary_buckets() && self.model_quotas.is_empty()))
            && let Some(quota_summary) = &self.quota_summary
        {
            return Self::quota_summary_to_usage_snapshot(
                quota_summary,
                self.account_email.as_deref(),
                self.account_plan.as_deref(),
                now,
            );
        }

        let selected = Self::select_models(&self.model_quotas);
        if selected.is_empty() && self.model_quotas.is_empty() {
            return Err(AntigravityStatusProbeError::ParseFailed(
                "No quota models available".to_owned(),
            ));
        }

        let primary = selected
            .first()
            .and_then(|q| q.as_ref())
            .and_then(Self::rate_window_for_quota);
        let secondary = selected
            .get(1)
            .and_then(|q| q.as_ref())
            .and_then(Self::rate_window_for_quota);
        let tertiary = selected
            .get(2)
            .and_then(|q| q.as_ref())
            .and_then(Self::rate_window_for_quota);
        let extra_rate_windows = Self::build_extra_rate_windows(&self.model_quotas, self.source);

        let identity = ProviderIdentitySnapshot::new(
            Some(ProviderId::new("antigravity").expect("valid provider id")),
            self.account_email.clone(),
            None,
            self.account_plan.clone(),
            None,
        );

        UsageSnapshot::new(
            primary,
            secondary,
            tertiary,
            extra_rate_windows,
            None,
            Vec::new(),
            None,
            None,
            now,
            Some(identity),
            Confidence::Exact,
        )
        .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))
    }

    pub(crate) fn used_percent(remaining_fraction: f64) -> f64 {
        let clamped_remaining = remaining_fraction.clamp(0.0, 1.0);
        ((1.0 - clamped_remaining) * 100.0).clamp(0.0, 100.0)
    }

    fn rate_window_for_quota(quota: &AntigravityModelQuota) -> Option<RateWindow> {
        let remaining_fraction = quota.remaining_fraction?;
        RateWindow::new(
            Self::used_percent(remaining_fraction),
            None,
            quota.reset_time,
            quota.reset_description.clone(),
            None,
            false,
        )
        .ok()
    }

    fn select_models(models: &[AntigravityModelQuota]) -> Vec<Option<AntigravityModelQuota>> {
        let ordered = vec![
            Self::most_constrained_quota(models, KnownFamily::Gemini),
            Self::most_constrained_quota(models, KnownFamily::ClaudeGpt),
        ];
        if ordered.iter().all(Option::is_none) {
            return Vec::new();
        }
        ordered
    }

    fn most_constrained_quota(
        models: &[AntigravityModelQuota],
        family: KnownFamily,
    ) -> Option<AntigravityModelQuota> {
        models
            .iter()
            .filter(|q| known_family(q) == Some(family) && q.remaining_fraction.is_some())
            .min_by(|lhs, rhs| lhs.remaining_percent().total_cmp(&rhs.remaining_percent()))
            .cloned()
    }

    fn build_extra_rate_windows(
        quotas: &[AntigravityModelQuota],
        source: AntigravitySnapshotSource,
    ) -> Option<Vec<NamedRateWindow>> {
        let mut grouped_windows = Vec::new();
        for group in MODEL_GROUPS {
            let members: Vec<&AntigravityModelQuota> = quotas
                .iter()
                .filter(|q| known_family(q) == Some(group.family))
                .collect();
            if let Some(window) = Self::rate_window_for_group(&group, &members) {
                grouped_windows.push(window);
            }
        }

        let ungrouped: Vec<&AntigravityModelQuota> = quotas
            .iter()
            .filter(|q| known_family(q).is_none())
            .collect();
        let visible = match source {
            AntigravitySnapshotSource::Local => Self::filter_local(&ungrouped),
            AntigravitySnapshotSource::Remote => Self::filter_remote(&ungrouped),
        };
        let sorted = Self::sort_quotas(&visible);

        let mut seen_ids: BTreeSet<String> = grouped_windows.iter().map(|w| w.id.clone()).collect();
        let mut result = grouped_windows;
        for quota in sorted {
            let base_id = if quota.model_id.trim().is_empty() {
                quota.label.trim()
            } else {
                quota.model_id.trim()
            };
            let window_id_base =
                if source == AntigravitySnapshotSource::Local && known_family(quota).is_none() {
                    format!("{COMPACT_FALLBACK_WINDOW_ID_PREFIX}{base_id}")
                } else {
                    base_id.to_owned()
                };
            let mut id = window_id_base.clone();
            if seen_ids.contains(&id) {
                let mut suffix = 2usize;
                while seen_ids.contains(&format!("{window_id_base}#{suffix}")) {
                    suffix += 1;
                }
                id = format!("{window_id_base}#{suffix}");
            }
            seen_ids.insert(id.clone());

            let Some(window) = Self::rate_window_for_quota(quota) else {
                continue;
            };
            let title = Self::quota_display_label(quota);
            result.push(NamedRateWindow::new(id, title, window));
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    fn rate_window_for_group(
        group: &ModelGroupSpec,
        quotas: &[&AntigravityModelQuota],
    ) -> Option<NamedRateWindow> {
        if quotas.is_empty() {
            return None;
        }
        let has_measured = quotas.iter().any(|q| q.remaining_fraction.is_some());
        if has_measured {
            return None;
        }

        let reset_source = quotas
            .iter()
            .copied()
            .find(|q| q.reset_time.is_some() || q.reset_description.is_some())
            .or_else(|| quotas.first().copied())?;
        let window = RateWindow::new(
            0.0,
            None,
            reset_source.reset_time,
            reset_source.reset_description.clone(),
            None,
            false,
        )
        .ok()?;
        Some(NamedRateWindow::new(group.id, group.title, window).with_usage_known(false))
    }

    fn filter_local<'a>(quotas: &[&'a AntigravityModelQuota]) -> Vec<&'a AntigravityModelQuota> {
        quotas.to_vec()
    }

    fn filter_remote<'a>(quotas: &[&'a AntigravityModelQuota]) -> Vec<&'a AntigravityModelQuota> {
        quotas
            .iter()
            .copied()
            .filter(|q| {
                let is_consumed = q.remaining_fraction.is_none_or(|f| f < 0.999);
                known_family(q).is_some() || is_consumed
            })
            .collect()
    }

    fn sort_quotas<'a>(quotas: &[&'a AntigravityModelQuota]) -> Vec<&'a AntigravityModelQuota> {
        let mut sorted = quotas.to_vec();
        sorted.sort_by(|a, b| {
            let fam_a = Self::classify_for_sort(a);
            let fam_b = Self::classify_for_sort(b);
            if fam_a != fam_b {
                return fam_a.cmp(&fam_b);
            }

            let ver_a = Self::extract_version(a);
            let ver_b = Self::extract_version(b);
            match (ver_a, ver_b) {
                (Some(va), Some(vb)) if (va - vb).abs() > 1e-9 => {
                    return vb.total_cmp(&va);
                }
                (Some(_), None) => return std::cmp::Ordering::Less,
                (None, Some(_)) => return std::cmp::Ordering::Greater,
                _ => {}
            }

            let tier_a = Self::extract_tier(a);
            let tier_b = Self::extract_tier(b);
            if tier_a != tier_b {
                return tier_a.cmp(&tier_b);
            }

            let label_cmp = a.label.cmp(&b.label);
            if label_cmp != std::cmp::Ordering::Equal {
                return label_cmp;
            }
            a.model_id.cmp(&b.model_id)
        });
        sorted
    }

    fn classify_for_sort(q: &AntigravityModelQuota) -> SortFamily {
        for raw_candidate in [&q.model_id, &q.label] {
            let candidate = Self::canonical_model_id(raw_candidate);
            let lower = candidate.to_lowercase();
            if lower.contains("claude") {
                return SortFamily::Claude;
            }
            if lower.contains("gpt") {
                return SortFamily::Gpt;
            }
            if lower.contains("gemini") {
                if lower.contains("flash") {
                    return SortFamily::GeminiFlash;
                }
                if lower.contains("pro") {
                    return SortFamily::GeminiPro;
                }
            }
        }
        SortFamily::Unknown
    }

    fn extract_version(q: &AntigravityModelQuota) -> Option<f64> {
        for raw_candidate in [&q.label, &q.model_id] {
            let candidate = Self::canonical_model_id(raw_candidate);
            if let Some(ver) = first_number(&candidate) {
                return Some(ver);
            }
        }
        None
    }

    fn extract_tier(q: &AntigravityModelQuota) -> i32 {
        for raw_candidate in [&q.label, &q.model_id] {
            let candidate = Self::canonical_model_id(raw_candidate);
            let lower = candidate.to_lowercase();
            let tokens: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).collect();
            if tokens.contains(&"high") {
                return 0;
            }
            if tokens.contains(&"medium") || tokens.contains(&"mid") {
                return 1;
            }
            if tokens.contains(&"low") {
                return 2;
            }
        }
        1
    }

    fn quota_summary_to_usage_snapshot(
        summary: &AntigravityQuotaSummary,
        account_email: Option<&str>,
        account_plan: Option<&str>,
        now: Timestamp,
    ) -> Result<UsageSnapshot, AntigravityStatusProbeError> {
        let buckets: Vec<(&str, &AntigravityQuotaSummaryBucket)> = summary
            .groups
            .iter()
            .flat_map(|g| g.buckets.iter().map(move |b| (g.name.as_str(), b)))
            .collect();
        let measured_buckets: Vec<(&str, &AntigravityQuotaSummaryBucket)> = buckets
            .iter()
            .copied()
            .filter(|(_, b)| !b.disabled && b.remaining_fraction.is_some())
            .collect();

        let gemini_buckets: Vec<&AntigravityQuotaSummaryBucket> = measured_buckets
            .iter()
            .filter(|(g, b)| Self::is_gemini_summary_bucket(g, b))
            .map(|(_, b)| *b)
            .collect();
        let third_party_buckets: Vec<&AntigravityQuotaSummaryBucket> = measured_buckets
            .iter()
            .filter(|(g, b)| Self::is_third_party_summary_bucket(g, b))
            .map(|(_, b)| *b)
            .collect();

        let mut primary_bucket = Self::preferred_gemini_summary_bucket(&gemini_buckets);
        let mut secondary_bucket = Self::most_constrained_summary_bucket(&third_party_buckets);

        if primary_bucket.is_none() && secondary_bucket.is_none() {
            let remaining: Vec<&AntigravityQuotaSummaryBucket> =
                measured_buckets.iter().map(|(_, b)| *b).collect();
            primary_bucket = Self::most_constrained_summary_bucket(&remaining);
            if let Some(pb) = primary_bucket {
                let rest: Vec<&AntigravityQuotaSummaryBucket> =
                    remaining.into_iter().filter(|b| b.id != pb.id).collect();
                secondary_bucket = Self::most_constrained_summary_bucket(&rest);
            }
        }

        let extra_rate_windows = Self::build_quota_summary_extra_windows(summary);
        let identity = ProviderIdentitySnapshot::new(
            Some(ProviderId::new("antigravity").expect("valid provider id")),
            account_email.map(str::to_owned),
            None,
            account_plan.map(str::to_owned),
            None,
        );

        UsageSnapshot::new(
            primary_bucket.and_then(Self::rate_window_for_bucket),
            secondary_bucket.and_then(Self::rate_window_for_bucket),
            None,
            extra_rate_windows,
            None,
            Vec::new(),
            None,
            None,
            now,
            Some(identity),
            Confidence::Exact,
        )
        .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))
    }

    fn build_quota_summary_extra_windows(
        summary: &AntigravityQuotaSummary,
    ) -> Option<Vec<NamedRateWindow>> {
        let mut seen_ids = BTreeSet::new();
        let mut windows = Vec::new();
        let multi_group = summary.groups.len() > 1;
        for group in &summary.groups {
            for bucket in &group.buckets {
                let base_id = format!(
                    "{QUOTA_SUMMARY_WINDOW_ID_PREFIX}{}",
                    Self::sanitized_quota_summary_id(&bucket.id)
                );
                let mut id = base_id.clone();
                if seen_ids.contains(&id) {
                    let mut suffix = 2usize;
                    while seen_ids.contains(&format!("{base_id}#{suffix}")) {
                        suffix += 1;
                    }
                    id = format!("{base_id}#{suffix}");
                }
                seen_ids.insert(id.clone());

                let title = Self::quota_summary_bucket_title(&group.name, bucket, multi_group);
                let usage_known = !bucket.disabled && bucket.remaining_fraction.is_some();
                if let Some(window) = Self::rate_window_for_bucket(bucket) {
                    windows.push(
                        NamedRateWindow::new(id, title, window).with_usage_known(usage_known),
                    );
                }
            }
        }

        if windows.is_empty() {
            None
        } else {
            Some(windows)
        }
    }

    fn rate_window_for_bucket(bucket: &AntigravityQuotaSummaryBucket) -> Option<RateWindow> {
        let used_percent = match bucket.remaining_fraction {
            Some(rem) if !bucket.disabled => Self::used_percent(rem),
            _ => 0.0,
        };
        RateWindow::new(
            used_percent,
            bucket.window_minutes.map(i64::from),
            bucket.reset_time,
            None,
            None,
            false,
        )
        .ok()
    }

    fn is_gemini_summary_bucket(group_name: &str, bucket: &AntigravityQuotaSummaryBucket) -> bool {
        if Self::is_ordinary_gemini_summary_bucket_id(&bucket.id) {
            return true;
        }
        let text = format!("{} {} {}", group_name, bucket.id, bucket.name).to_lowercase();
        if text.contains("tab") || contains_token("image", &text) || text.contains("lite") {
            return false;
        }
        text.contains("gemini")
    }

    fn is_third_party_summary_bucket(
        group_name: &str,
        bucket: &AntigravityQuotaSummaryBucket,
    ) -> bool {
        let text = format!("{} {} {}", group_name, bucket.id, bucket.name).to_lowercase();
        let tokens = Self::quota_summary_tokens(&text);
        text.contains("claude")
            || text.contains("gpt")
            || text.contains("third-party")
            || text.contains("third party")
            || tokens.contains(&"3p")
    }

    fn preferred_gemini_summary_bucket<'a>(
        buckets: &[&'a AntigravityQuotaSummaryBucket],
    ) -> Option<&'a AntigravityQuotaSummaryBucket> {
        let ordinary: Vec<&'a AntigravityQuotaSummaryBucket> = buckets
            .iter()
            .copied()
            .filter(|b| Self::is_ordinary_gemini_summary_bucket_id(&b.id))
            .collect();
        let candidates = if ordinary.is_empty() {
            buckets
        } else {
            &ordinary
        };
        Self::most_constrained_summary_bucket(candidates)
    }

    pub(crate) fn is_ordinary_gemini_summary_bucket_id(id: &str) -> bool {
        let lower = id.to_lowercase();
        let tokens = Self::quota_summary_tokens(&lower);
        if tokens.first().copied() != Some("gemini") || tokens.len() < 2 {
            return false;
        }
        let qualifiers = &tokens[1..];
        const ALLOWED: &[&str] = &[
            "5h", "5hr", "5hrs", "hour", "hours", "session", "weekly", "week", "1w", "7d", "day",
            "daily", "1d", "24h", "quota", "limit",
        ];
        qualifiers.iter().all(|q| ALLOWED.contains(q))
    }

    fn most_constrained_summary_bucket<'a>(
        buckets: &[&'a AntigravityQuotaSummaryBucket],
    ) -> Option<&'a AntigravityQuotaSummaryBucket> {
        buckets.iter().copied().min_by(|lhs, rhs| {
            let l_rem = lhs.remaining_fraction.unwrap_or(1.0);
            let r_rem = rhs.remaining_fraction.unwrap_or(1.0);
            if (l_rem - r_rem).abs() > 0.0001 {
                return l_rem.total_cmp(&r_rem);
            }
            Self::summary_bucket_precedence(lhs).cmp(&Self::summary_bucket_precedence(rhs))
        })
    }

    fn summary_bucket_precedence(bucket: &AntigravityQuotaSummaryBucket) -> i32 {
        match bucket.window_minutes {
            Some(300) => 0,
            Some(10_080) => 1,
            Some(minutes) if minutes > 0 => 2,
            _ => 3,
        }
    }

    fn quota_summary_bucket_title(
        group_name: &str,
        bucket: &AntigravityQuotaSummaryBucket,
        include_group: bool,
    ) -> String {
        let mut title = bucket.name.trim().to_owned();
        if let Some(cadence) = Self::formatted_quota_summary_cadence(bucket)
            && !title.to_lowercase().contains(&cadence.to_lowercase())
        {
            title = format!("{title} ({cadence})");
        }
        let trimmed_group = group_name.trim();
        if include_group
            && !trimmed_group.is_empty()
            && !title.to_lowercase().contains(&trimmed_group.to_lowercase())
        {
            return format!("{trimmed_group} - {title}");
        }
        title
    }

    fn formatted_quota_summary_cadence(bucket: &AntigravityQuotaSummaryBucket) -> Option<String> {
        match bucket.window_minutes {
            Some(300) => Some("5h".to_owned()),
            Some(10_080) => Some("weekly".to_owned()),
            Some(1440) => Some("daily".to_owned()),
            _ => {
                let label = bucket.window_label.as_deref()?.trim();
                if label.is_empty() {
                    None
                } else {
                    Some(label.to_owned())
                }
            }
        }
    }

    fn sanitized_quota_summary_id(raw: &str) -> String {
        let lower = raw.to_lowercase();
        let tokens = Self::quota_summary_tokens(&lower);
        if tokens.is_empty() {
            "bucket".to_owned()
        } else {
            tokens.join("-")
        }
    }

    pub(crate) fn quota_summary_tokens(text: &str) -> Vec<&str> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|part| !part.is_empty())
            .collect()
    }
}

struct ModelGroupSpec {
    id: &'static str,
    title: &'static str,
    family: KnownFamily,
}

const MODEL_GROUPS: [ModelGroupSpec; 2] = [
    ModelGroupSpec {
        id: KnownFamily::Gemini.window_id(),
        title: "Gemini Models",
        family: KnownFamily::Gemini,
    },
    ModelGroupSpec {
        id: KnownFamily::ClaudeGpt.window_id(),
        title: "Claude and GPT",
        family: KnownFamily::ClaudeGpt,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SortFamily {
    GeminiPro = 0,
    GeminiFlash = 1,
    Claude = 2,
    Gpt = 3,
    Unknown = 4,
}

fn first_number(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            return text[start..i].parse::<f64>().ok();
        }
        i += 1;
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityPlanInfoSummary {
    pub plan_name: Option<String>,
    pub plan_display_name: Option<String>,
    pub display_name: Option<String>,
    pub product_name: Option<String>,
    pub plan_short_name: Option<String>,
}

impl AntigravityPlanInfoSummary {
    pub fn preferred_name(&self) -> Option<String> {
        let candidates = [
            self.plan_display_name.as_deref(),
            self.display_name.as_deref(),
            self.product_name.as_deref(),
            self.plan_name.as_deref(),
            self.plan_short_name.as_deref(),
        ];
        for candidate in candidates.into_iter().flatten() {
            let trimmed = candidate.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AntigravityStatusProbeError {
    #[error("Antigravity language server not detected. Launch Antigravity and retry.")]
    NotRunning,
    #[error("Antigravity CSRF token not found. Restart Antigravity and retry.")]
    MissingCSRFToken,
    #[error("Antigravity port detection failed: {0}")]
    PortDetectionFailed(String),
    #[error("Antigravity API error: {0}")]
    ApiError(String),
    #[error("Could not parse Antigravity quota: {0}")]
    ParseFailed(String),
    #[error("Antigravity status probe timed out")]
    TimedOut,
}

impl AntigravityStatusProbeError {
    pub fn into_classified_error(self) -> ClassifiedError {
        match &self {
            Self::NotRunning | Self::MissingCSRFToken | Self::PortDetectionFailed(_) => {
                ClassifiedError::new(ProviderErrorKind::ProviderUnavailable, self.to_string())
            }
            Self::ApiError(_) => {
                ClassifiedError::new(ProviderErrorKind::ApiFailure, self.to_string())
            }
            Self::ParseFailed(_) => {
                ClassifiedError::new(ProviderErrorKind::ParseFailure, self.to_string())
            }
            Self::TimedOut => {
                ClassifiedError::new(ProviderErrorKind::NetworkFailure, self.to_string())
                    .with_transport(TransportClass::Timeout)
            }
        }
    }
}

// MARK: - Deadlines (AntigravityStatusProbe+Deadline.swift)

#[derive(Clone)]
pub struct OperationDeadline {
    pub deadline: Instant,
    now_provider: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl OperationDeadline {
    pub fn new(timeout: Duration) -> Self {
        let now = Instant::now();
        Self {
            deadline: now + timeout.max(Duration::ZERO),
            now_provider: Arc::new(Instant::now),
        }
    }

    pub fn with_now_provider<F>(timeout: Duration, now_provider: F) -> Self
    where
        F: Fn() -> Instant + Send + Sync + 'static,
    {
        let now = now_provider();
        Self {
            deadline: now + timeout.max(Duration::ZERO),
            now_provider: Arc::new(now_provider),
        }
    }

    pub fn timeout_for_next_step(
        &self,
        cap: Option<Duration>,
    ) -> Result<Duration, AntigravityStatusProbeError> {
        let now = (self.now_provider)();
        let Some(remaining) = AntigravityStatusProbe::remaining_time(now, self.deadline) else {
            return Err(AntigravityStatusProbeError::TimedOut);
        };
        if let Some(cap) = cap {
            AntigravityStatusProbe::bounded_step_timeout(remaining, cap)
                .ok_or(AntigravityStatusProbeError::TimedOut)
        } else {
            Ok(remaining)
        }
    }

    pub fn has_expired(&self) -> bool {
        let now = (self.now_provider)();
        AntigravityStatusProbe::should_abort_endpoint_scan(now, self.deadline)
    }
}

// MARK: - Localhost Session & Endpoints (AntigravityLocalhostSession.swift & +LocalEndpoints.swift)

pub struct LocalhostTrustPolicy;

impl LocalhostTrustPolicy {
    pub fn should_accept_server_trust(host: &str) -> bool {
        matches!(host, "127.0.0.1" | "localhost" | "::1")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntigravityProcessKind {
    App,
    Ide,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessScope {
    IdeAndCli,
    AppOnly,
    IdeOnly,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: i32,
    pub command: String,
    pub identity: Option<ProcessIdentity>,
}

impl fmt::Debug for ProcessEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessEntry")
            .field("pid", &self.pid)
            .field("has_command", &!self.command.is_empty())
            .finish()
    }
}

impl ProcessEntry {
    pub fn new(pid: i32, command: impl Into<String>) -> Self {
        Self {
            pid,
            command: command.into(),
            identity: None,
        }
    }

    pub fn with_identity(mut self, identity: ProcessIdentity) -> Self {
        self.identity = Some(identity);
        self
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProcessInfoResult {
    pub pid: i32,
    pub csrf_token: Option<String>,
    pub extension_port: Option<u16>,
    pub extension_csrf_token: Option<String>,
    pub command: String,
    pub identity: Option<ProcessIdentity>,
}

impl fmt::Debug for ProcessInfoResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessInfoResult")
            .field("pid", &self.pid)
            .field("has_csrf_token", &self.csrf_token.is_some())
            .field("extension_port", &self.extension_port)
            .field(
                "has_extension_csrf_token",
                &self.extension_csrf_token.is_some(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointSource {
    ExtensionServer,
    LanguageServer,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AntigravityConnectionEndpoint {
    pub scheme: String,
    pub port: u16,
    pub csrf_token: String,
    pub source: EndpointSource,
}

impl fmt::Debug for AntigravityConnectionEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AntigravityConnectionEndpoint")
            .field("scheme", &self.scheme)
            .field("port", &self.port)
            .field("has_csrf_token", &!self.csrf_token.is_empty())
            .field("source", &self.source)
            .finish()
    }
}

impl AntigravityConnectionEndpoint {
    pub fn new(
        scheme: impl Into<String>,
        port: u16,
        csrf_token: impl Into<String>,
        source: EndpointSource,
    ) -> Self {
        Self {
            scheme: scheme.into(),
            port,
            csrf_token: csrf_token.into(),
            source,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestPayload {
    pub path: String,
    pub body: serde_json::Value,
}

impl RequestPayload {
    pub fn new(path: impl Into<String>, body: serde_json::Value) -> Self {
        Self {
            path: path.into(),
            body,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub endpoints: Vec<AntigravityConnectionEndpoint>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseSummary {
    pub reachability_known: bool,
    pub user_status_keys: Vec<String>,
    pub token_usage_keys: Vec<String>,
    pub models_count: Option<usize>,
    pub command_configs_count: Option<usize>,
    pub plan_info: Option<AntigravityPlanInfoSummary>,
}

pub type EndpointRequestSender = Arc<
    dyn Fn(
            RequestPayload,
            AntigravityConnectionEndpoint,
            Duration,
        ) -> BoxFuture<'static, Result<Vec<u8>, AntigravityStatusProbeError>>
        + Send
        + Sync,
>;

pub struct AntigravityStatusProbe;

impl AntigravityStatusProbe {
    pub const STEP_TIMEOUT_CAP: Duration = Duration::from_secs(4);
    pub const MIN_REMAINING_TIME: Duration = Duration::from_millis(50);

    pub fn local_probe_schemes() -> &'static [&'static str] {
        #[cfg(target_os = "linux")]
        {
            &["https", "http"]
        }
        #[cfg(not(target_os = "linux"))]
        {
            &["https"]
        }
    }

    pub fn remaining_time(now: Instant, deadline: Instant) -> Option<Duration> {
        let remaining = deadline.checked_duration_since(now)?;
        if remaining.is_zero() {
            None
        } else {
            Some(remaining)
        }
    }

    pub fn bounded_step_timeout(remaining: Duration, cap: Duration) -> Option<Duration> {
        if remaining.is_zero() {
            return None;
        }
        let effective_cap = if cap.is_zero() {
            Self::STEP_TIMEOUT_CAP
        } else {
            cap
        };
        let bounded = remaining.min(effective_cap);
        if bounded.is_zero() {
            None
        } else {
            Some(bounded)
        }
    }

    pub fn should_abort_endpoint_scan(now: Instant, deadline: Instant) -> bool {
        let Some(remaining) = Self::remaining_time(now, deadline) else {
            return true;
        };
        remaining < Self::MIN_REMAINING_TIME
    }

    // MARK: - Process Classification & Flag Parsing

    pub fn antigravity_process_kind(command: &str) -> Option<AntigravityProcessKind> {
        let lower = command.to_lowercase();
        if Self::is_language_server_command_line(&lower)
            && Self::is_antigravity_command_line(&lower)
        {
            if Self::is_antigravity_ide_command_line(&lower) {
                return Some(AntigravityProcessKind::Ide);
            }
            return Some(AntigravityProcessKind::App);
        }
        if Self::is_antigravity_cli_command_line(&lower) {
            return Some(AntigravityProcessKind::Cli);
        }
        None
    }

    pub fn is_antigravity_language_server_command_line(command: &str) -> bool {
        Self::antigravity_process_kind(command).is_some()
    }

    pub fn matches_scope(kind: AntigravityProcessKind, scope: ProcessScope) -> bool {
        match scope {
            ProcessScope::IdeAndCli => true,
            ProcessScope::AppOnly => kind == AntigravityProcessKind::App,
            ProcessScope::IdeOnly => kind == AntigravityProcessKind::Ide,
        }
    }

    fn is_language_server_command_line(lower: &str) -> bool {
        // Matches `(^|[/\\])language(?:_|-)server(?:[_-][a-z0-9]+)*(?:\.exe)?(\s|$)`
        for token in lower.split_whitespace() {
            let basename = token.rsplit(['/', '\\']).next().unwrap_or(token);
            let stem = basename.strip_suffix(".exe").unwrap_or(basename);
            let Some(rest) = stem
                .strip_prefix("language_server")
                .or_else(|| stem.strip_prefix("language-server"))
            else {
                continue;
            };
            if rest.is_empty() {
                return true;
            }
            if (rest.starts_with('_') || rest.starts_with('-'))
                && rest
                    .split(['_', '-'])
                    .skip(1)
                    .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric()))
            {
                return true;
            }
        }
        false
    }

    fn is_antigravity_command_line(lower: &str) -> bool {
        if lower.contains("--app_data_dir") && lower.contains("antigravity") {
            return true;
        }
        if lower.contains("antigravity.app/") || lower.contains("antigravity.app\\") {
            return true;
        }
        if Self::contains_renamed_gemini_app_path(lower) {
            return true;
        }
        if lower.contains("antigravity ide.app/") || lower.contains("antigravity ide.app\\") {
            return true;
        }
        if lower.contains("/antigravity/") || lower.contains("\\antigravity\\") {
            return true;
        }
        false
    }

    fn contains_renamed_gemini_app_path(lower: &str) -> bool {
        for separator in ['/', '\\'] {
            let needle = format!("{separator}gemini.app{separator}");
            if lower.contains(&needle) {
                return true;
            }
        }
        false
    }

    fn is_antigravity_ide_command_line(lower: &str) -> bool {
        if lower.contains("antigravity ide.app/") || lower.contains("antigravity ide.app\\") {
            return true;
        }
        if lower.contains("--app_data_dir antigravity-ide")
            || lower.contains("--app_data_dir=antigravity-ide")
        {
            return true;
        }
        if lower.contains("/extensions/antigravity/bin/language_server")
            || lower.contains("\\extensions\\antigravity\\bin\\language_server")
        {
            return true;
        }
        false
    }

    fn is_antigravity_cli_command_line(lower: &str) -> bool {
        // Matches `(^|[/\\])(antigravity-cli|antigravity_cli)([\s/\\]|$)` or `(^|[/\\])agy(\s|$)`
        for token in lower.split_whitespace() {
            for component in token.split(['/', '\\']) {
                if component == "antigravity-cli"
                    || component == "antigravity_cli"
                    || component == ".antigravity-cli"
                    || component == ".antigravity_cli"
                {
                    return true;
                }
            }
            let basename = token.rsplit(['/', '\\']).next().unwrap_or(token);
            if basename == "agy" || basename == "agy.exe" {
                return true;
            }
        }
        false
    }

    pub fn process_infos_from_process_list_output(
        output: &str,
        scope: ProcessScope,
    ) -> Result<Vec<ProcessInfoResult>, AntigravityStatusProbeError> {
        let mut entries = Vec::new();
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut parts = trimmed.splitn(2, char::is_whitespace);
            let Some(pid_str) = parts.next() else {
                continue;
            };
            let Some(rest) = parts.next() else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<i32>() else {
                continue;
            };
            entries.push(ProcessEntry::new(pid, rest.trim_start()));
        }
        Self::process_infos_from_entries(&entries, scope)
    }

    pub fn process_infos_from_entries(
        entries: &[ProcessEntry],
        scope: ProcessScope,
    ) -> Result<Vec<ProcessInfoResult>, AntigravityStatusProbeError> {
        let mut matches = Vec::new();
        let mut saw_antigravity = false;

        for entry in entries {
            let Some(kind) = Self::antigravity_process_kind(&entry.command) else {
                continue;
            };
            if !Self::matches_scope(kind, scope) {
                continue;
            }
            saw_antigravity = true;
            let csrf = Self::extract_flag("--csrf_token", &entry.command);
            let ext_csrf = Self::extract_flag("--extension_server_csrf_token", &entry.command);
            let port = Self::extract_port("--extension_server_port", &entry.command);
            if csrf.is_none() && ext_csrf.is_none() && port.is_none() {
                continue;
            }
            matches.push(ProcessInfoResult {
                pid: entry.pid,
                csrf_token: csrf,
                extension_port: port,
                extension_csrf_token: ext_csrf,
                command: entry.command.clone(),
                identity: entry.identity,
            });
        }

        if matches.is_empty() {
            if saw_antigravity {
                return Err(AntigravityStatusProbeError::MissingCSRFToken);
            }
            return Err(AntigravityStatusProbeError::NotRunning);
        }

        Ok(matches)
    }

    pub fn resolved_csrf_token(
        csrf_token: Option<&str>,
        extension_csrf_token: Option<&str>,
        command: &str,
    ) -> Option<String> {
        for val in [csrf_token, extension_csrf_token].into_iter().flatten() {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
        let lower = command.to_lowercase();
        if lower.contains("/.antigravity-cli/")
            || lower.contains("\\.antigravity-cli\\")
            || lower.contains("--server_port")
        {
            return Some(String::new());
        }
        None
    }

    pub fn extract_flag(flag: &str, command: &str) -> Option<String> {
        let flag_lower = flag.to_lowercase();
        let tokens: Vec<&str> = command.split_whitespace().collect();
        for (idx, token) in tokens.iter().enumerate() {
            let token_lower = token.to_lowercase();
            if token_lower == flag_lower {
                if let Some(next) = tokens.get(idx + 1)
                    && !next.starts_with('-')
                    && !next.is_empty()
                {
                    return Some((*next).to_owned());
                }
            } else if token_lower.starts_with(&format!("{flag_lower}=")) {
                let value = &token[flag.len() + 1..];
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
        None
    }

    pub fn extract_port(flag: &str, command: &str) -> Option<u16> {
        let raw = Self::extract_flag(flag, command)?;
        raw.parse::<u16>().ok()
    }

    pub fn parse_listening_ports(output: &str) -> Vec<u16> {
        let mut ports = BTreeSet::new();
        for line in output.lines() {
            if !line.contains("(LISTEN)") {
                continue;
            }
            // Match `:<digits> (LISTEN)`
            if let Some(listen_idx) = line.find("(LISTEN)") {
                let before = line[..listen_idx].trim_end();
                if let Some(colon_idx) = before.rfind(':') {
                    let digits = &before[colon_idx + 1..];
                    if !digits.is_empty()
                        && digits.chars().all(|c| c.is_ascii_digit())
                        && let Ok(port) = digits.parse::<u16>()
                    {
                        ports.insert(port);
                    }
                }
            }
        }
        ports.into_iter().collect()
    }

    pub fn fallback_probe_port(ports: &[u16], extension_port: Option<u16>) -> Option<u16> {
        if let Some(ext) = extension_port
            && let Some(&non_ext) = ports.iter().find(|&&p| p != ext)
        {
            return Some(non_ext);
        }
        ports.first().copied()
    }

    pub fn is_reachable_probe_error(error: &AntigravityStatusProbeError) -> bool {
        matches!(error, AntigravityStatusProbeError::ApiError(msg) if msg.starts_with("HTTP "))
    }

    // MARK: - Connection Candidates & Endpoint Resolution

    pub fn connection_candidates(
        ports: &[u16],
        csrf_token: Option<&str>,
        extension_port: Option<u16>,
        extension_csrf_token: Option<&str>,
    ) -> Vec<AntigravityConnectionEndpoint> {
        Self::connection_candidates_with_schemes(
            ports,
            csrf_token,
            extension_port,
            extension_csrf_token,
            &["https"],
        )
    }

    pub fn connection_candidates_for_current_platform(
        ports: &[u16],
        csrf_token: Option<&str>,
        extension_port: Option<u16>,
        extension_csrf_token: Option<&str>,
    ) -> Vec<AntigravityConnectionEndpoint> {
        Self::connection_candidates_with_schemes(
            ports,
            csrf_token,
            extension_port,
            extension_csrf_token,
            Self::local_probe_schemes(),
        )
    }

    pub fn connection_candidates_with_schemes(
        ports: &[u16],
        csrf_token: Option<&str>,
        extension_port: Option<u16>,
        extension_csrf_token: Option<&str>,
        schemes: &[&str],
    ) -> Vec<AntigravityConnectionEndpoint> {
        let mut candidates = Vec::new();
        let trimmed_csrf = csrf_token.map(str::trim);
        let trimmed_ext_csrf = extension_csrf_token
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if let Some(csrf) = trimmed_csrf {
            let mut language_ports = Vec::new();
            for &port in ports {
                if Some(port) != extension_port && !language_ports.contains(&port) {
                    language_ports.push(port);
                }
            }
            if language_ports.is_empty() {
                for &port in ports {
                    if !language_ports.contains(&port) {
                        language_ports.push(port);
                    }
                }
            }
            for port in language_ports {
                for &scheme in schemes {
                    candidates.push(AntigravityConnectionEndpoint::new(
                        scheme,
                        port,
                        csrf,
                        EndpointSource::LanguageServer,
                    ));
                }
            }
        }

        let resolved_ext_csrf = trimmed_ext_csrf.or_else(|| trimmed_csrf.filter(|s| !s.is_empty()));
        if let (Some(ext_port), Some(ext_csrf)) = (extension_port, resolved_ext_csrf) {
            for &scheme in schemes {
                let candidate = AntigravityConnectionEndpoint::new(
                    scheme,
                    ext_port,
                    ext_csrf,
                    EndpointSource::ExtensionServer,
                );
                if !candidates
                    .iter()
                    .any(|e| e.scheme == candidate.scheme && e.port == candidate.port)
                {
                    candidates.push(candidate);
                }
            }
        }

        candidates
    }

    pub fn request_endpoints(
        primary: &AntigravityConnectionEndpoint,
        candidates: &[AntigravityConnectionEndpoint],
    ) -> Vec<AntigravityConnectionEndpoint> {
        let mut ordered = vec![primary.clone()];
        for candidate in candidates {
            let already_included = ordered.iter().any(|existing| {
                existing.scheme == candidate.scheme
                    && existing.port == candidate.port
                    && existing.csrf_token == candidate.csrf_token
                    && existing.source == candidate.source
            });
            if !already_included {
                ordered.push(candidate.clone());
            }
        }
        ordered
    }

    pub async fn resolve_working_endpoint<F, Fut>(
        candidates: &[AntigravityConnectionEndpoint],
        deadline: &OperationDeadline,
        test_send: F,
    ) -> Result<AntigravityConnectionEndpoint, AntigravityStatusProbeError>
    where
        F: Fn(AntigravityConnectionEndpoint, Duration) -> Fut,
        Fut: std::future::Future<Output = Result<(), AntigravityStatusProbeError>>,
    {
        if candidates.is_empty() {
            return Err(AntigravityStatusProbeError::PortDetectionFailed(
                "no working API port found".to_owned(),
            ));
        }

        let mut saw_timeout = false;
        let mut reachable_fallback: Option<AntigravityConnectionEndpoint> = None;

        for candidate in candidates {
            if deadline.has_expired() {
                return Err(AntigravityStatusProbeError::TimedOut);
            }
            let step_timeout = match deadline.timeout_for_next_step(Some(Self::STEP_TIMEOUT_CAP)) {
                Ok(t) => t,
                Err(_) => return Err(AntigravityStatusProbeError::TimedOut),
            };
            match test_send(candidate.clone(), step_timeout).await {
                Ok(()) => return Ok(candidate.clone()),
                Err(AntigravityStatusProbeError::TimedOut) => {
                    saw_timeout = true;
                }
                Err(err) => {
                    if reachable_fallback.is_none() && Self::is_reachable_probe_error(&err) {
                        reachable_fallback = Some(candidate.clone());
                    }
                }
            }
        }

        if let Some(fallback) = reachable_fallback {
            return Ok(fallback);
        }
        if saw_timeout {
            return Err(AntigravityStatusProbeError::TimedOut);
        }

        let language_ports: Vec<u16> = candidates
            .iter()
            .filter(|c| c.source == EndpointSource::LanguageServer)
            .map(|c| c.port)
            .collect();
        let ext_port = candidates
            .iter()
            .find(|c| c.source == EndpointSource::ExtensionServer)
            .map(|c| c.port);
        if let Some(port) = Self::fallback_probe_port(&language_ports, ext_port)
            && let Some(endpoint) = candidates.iter().find(|c| c.port == port)
        {
            return Ok(endpoint.clone());
        }
        if let Some(first) = candidates.first() {
            return Ok(first.clone());
        }
        Err(AntigravityStatusProbeError::PortDetectionFailed(
            "no working API port found".to_owned(),
        ))
    }

    pub async fn fetch_process_snapshots<F, Fut>(
        processes: &[ProcessInfoResult],
        deadline: &OperationDeadline,
        fetch_process: F,
    ) -> Result<Vec<AntigravityStatusSnapshot>, AntigravityStatusProbeError>
    where
        F: Fn(ProcessInfoResult) -> Fut,
        Fut: std::future::Future<
                Output = Result<AntigravityStatusSnapshot, AntigravityStatusProbeError>,
            >,
    {
        let mut snapshots = Vec::new();
        let mut last_error: Option<AntigravityStatusProbeError> = None;

        for process in processes {
            if deadline.has_expired() {
                if !snapshots.is_empty() {
                    break;
                }
                return Err(AntigravityStatusProbeError::TimedOut);
            }
            match fetch_process(process.clone()).await {
                Ok(snapshot) => {
                    snapshots.push(snapshot);
                }
                Err(AntigravityStatusProbeError::TimedOut) => {
                    last_error = Some(AntigravityStatusProbeError::TimedOut);
                    if deadline.has_expired() {
                        break;
                    }
                }
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }

        if !snapshots.is_empty() {
            return Ok(snapshots);
        }
        Err(last_error.unwrap_or(AntigravityStatusProbeError::NotRunning))
    }

    pub async fn fetch_snapshot(
        context: &RequestContext,
        deadline: &OperationDeadline,
        send: EndpointRequestSender,
    ) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
        match Self::make_parsed_request(
            RequestPayload::new(GET_USER_STATUS_PATH, Self::default_request_body()),
            context,
            deadline,
            Arc::clone(&send),
            Self::parse_user_status_response,
        )
        .await
        {
            Ok(snapshot) => Ok(snapshot),
            Err(AntigravityStatusProbeError::TimedOut) => {
                Err(AntigravityStatusProbeError::TimedOut)
            }
            Err(user_status_error) => {
                match Self::make_parsed_request(
                    RequestPayload::new(COMMAND_MODEL_CONFIG_PATH, Self::default_request_body()),
                    context,
                    deadline,
                    send,
                    Self::parse_command_model_response,
                )
                .await
                {
                    Ok(snapshot) => Ok(snapshot),
                    Err(AntigravityStatusProbeError::TimedOut) => {
                        Err(AntigravityStatusProbeError::TimedOut)
                    }
                    Err(fallback_error) => {
                        if Self::should_prefer_user_status_error(
                            &user_status_error,
                            &fallback_error,
                        ) {
                            Err(user_status_error)
                        } else {
                            Err(fallback_error)
                        }
                    }
                }
            }
        }
    }

    fn should_prefer_user_status_error(
        user_status_error: &AntigravityStatusProbeError,
        fallback_error: &AntigravityStatusProbeError,
    ) -> bool {
        matches!(
            fallback_error,
            AntigravityStatusProbeError::ApiError(msg)
                if msg.contains("HTTP 404") || msg.contains("HTTP 405") || msg.contains("HTTP 501")
        ) && !matches!(user_status_error, AntigravityStatusProbeError::TimedOut)
    }

    pub async fn make_parsed_request<T, P>(
        payload: RequestPayload,
        context: &RequestContext,
        deadline: &OperationDeadline,
        send: EndpointRequestSender,
        parse: P,
    ) -> Result<T, AntigravityStatusProbeError>
    where
        P: Fn(&[u8]) -> Result<T, AntigravityStatusProbeError>,
    {
        let mut last_error: Option<AntigravityStatusProbeError> = None;

        for endpoint in &context.endpoints {
            if deadline.has_expired() {
                return Err(AntigravityStatusProbeError::TimedOut);
            }
            let step_timeout = match deadline.timeout_for_next_step(Some(Self::STEP_TIMEOUT_CAP)) {
                Ok(t) => t.min(context.timeout),
                Err(_) => return Err(AntigravityStatusProbeError::TimedOut),
            };
            match send(payload.clone(), endpoint.clone(), step_timeout).await {
                Ok(data) => match parse(&data) {
                    Ok(parsed) => return Ok(parsed),
                    Err(err) => {
                        last_error = Some(err);
                    }
                },
                Err(AntigravityStatusProbeError::TimedOut) => {
                    last_error = Some(AntigravityStatusProbeError::TimedOut);
                    if deadline.has_expired() {
                        return Err(AntigravityStatusProbeError::TimedOut);
                    }
                }
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            AntigravityStatusProbeError::ApiError("No Antigravity endpoints available".to_owned())
        }))
    }

    pub fn local_snapshot_score(snapshot: &AntigravityStatusSnapshot) -> (i32, i32, i32, i32, i32) {
        let has_email = i32::from(
            snapshot
                .account_email
                .as_deref()
                .is_some_and(|e| !e.trim().is_empty()),
        );
        let has_plan = i32::from(
            snapshot
                .account_plan
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty()),
        );
        let has_gemini = i32::from(snapshot.model_quotas.iter().any(|q| {
            known_family(q) == Some(KnownFamily::Gemini) && q.remaining_fraction.is_some()
        }));
        let has_claude_gpt = i32::from(snapshot.model_quotas.iter().any(|q| {
            known_family(q) == Some(KnownFamily::ClaudeGpt) && q.remaining_fraction.is_some()
        }));
        let family_coverage = has_gemini + has_claude_gpt;
        let measured_count = snapshot
            .model_quotas
            .iter()
            .filter(|q| q.remaining_fraction.is_some())
            .count() as i32;
        (
            has_email,
            has_plan,
            family_coverage,
            measured_count,
            snapshot.model_quotas.len() as i32,
        )
    }

    pub fn preferred_local_snapshot(
        snapshots: &[AntigravityStatusSnapshot],
    ) -> Option<AntigravityStatusSnapshot> {
        snapshots
            .iter()
            .max_by_key(|s| Self::local_snapshot_score(s))
            .cloned()
    }

    // MARK: - Live LocalProbe + NetBroker Execution

    pub async fn fetch(
        local_probe: &LocalProbe,
        net: &Arc<NetBroker>,
        call: &BrokerCall,
        timeout: Duration,
        scope: ProcessScope,
    ) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
        let deadline = OperationDeadline::new(timeout);
        if deadline.has_expired() {
            return Err(AntigravityStatusProbeError::TimedOut);
        }

        let probe_processes = local_probe
            .same_user_processes(call, |view| {
                Self::antigravity_process_kind(&view.command_line()).is_some()
            })
            .await
            .map_err(Self::map_probe_error)?;

        let entries: Vec<ProcessEntry> = probe_processes
            .into_iter()
            .map(|p| {
                ProcessEntry::new(p.identity.pid, p.argv.command_line()).with_identity(p.identity)
            })
            .collect();
        let processes = Self::process_infos_from_entries(&entries, scope)?;

        let deadline_for_closure = deadline.clone();
        let snapshots = Self::fetch_process_snapshots(&processes, &deadline, move |process| {
            let net = Arc::clone(net);
            let call = call.clone();
            let deadline = deadline_for_closure.clone();
            async move {
                Self::fetch_snapshot_for_process(local_probe, &net, &call, &process, &deadline)
                    .await
            }
        })
        .await?;

        Self::preferred_local_snapshot(&snapshots).ok_or(AntigravityStatusProbeError::NotRunning)
    }

    async fn fetch_snapshot_for_process(
        local_probe: &LocalProbe,
        net: &Arc<NetBroker>,
        call: &BrokerCall,
        process: &ProcessInfoResult,
        deadline: &OperationDeadline,
    ) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
        let csrf_token = Self::resolved_csrf_token(
            process.csrf_token.as_deref(),
            process.extension_csrf_token.as_deref(),
            &process.command,
        );
        if csrf_token.is_none()
            && process.extension_port.is_none()
            && process.extension_csrf_token.is_none()
        {
            return Err(AntigravityStatusProbeError::MissingCSRFToken);
        }

        let port_timeout = deadline.timeout_for_next_step(Some(Self::STEP_TIMEOUT_CAP))?;
        let ports = local_probe
            .listening_tcp_ports(call, process.pid, port_timeout)
            .await
            .map_err(Self::map_probe_error)?;
        let candidates = Self::connection_candidates_for_current_platform(
            &ports,
            csrf_token.as_deref(),
            process.extension_port,
            process.extension_csrf_token.as_deref(),
        );
        if candidates.is_empty() {
            return Err(AntigravityStatusProbeError::MissingCSRFToken);
        }

        let identity = process.identity;
        let send: EndpointRequestSender = {
            let net = Arc::clone(net);
            let call = call.clone();
            let local_probe = local_probe.clone();
            Arc::new(move |payload, endpoint, step_timeout| {
                let net = Arc::clone(&net);
                let call = call.clone();
                let local_probe = local_probe.clone();
                Box::pin(async move {
                    if let Some(id) = identity {
                        let still_owned = local_probe
                            .listener_still_owned(&call, id, endpoint.port)
                            .await
                            .map_err(Self::map_probe_error)?;
                        if !still_owned {
                            return Err(AntigravityStatusProbeError::PortDetectionFailed(
                                "listener no longer owned by process".to_owned(),
                            ));
                        }
                    }
                    Self::send_loopback_request(&net, &call, &payload, &endpoint, step_timeout)
                        .await
                })
            })
        };

        let working_endpoint =
            Self::resolve_working_endpoint(&candidates, deadline, |endpoint, step_timeout| {
                let send = Arc::clone(&send);
                async move {
                    let payload = RequestPayload::new(UNLEASH_PATH, Self::unleash_request_body());
                    send(payload, endpoint, step_timeout).await.map(|_| ())
                }
            })
            .await?;

        let context = RequestContext {
            endpoints: Self::request_endpoints(&working_endpoint, &candidates),
            timeout: Self::STEP_TIMEOUT_CAP,
        };
        Self::fetch_snapshot(&context, deadline, send).await
    }

    pub async fn send_loopback_request(
        net: &NetBroker,
        call: &BrokerCall,
        payload: &RequestPayload,
        endpoint: &AntigravityConnectionEndpoint,
        timeout: Duration,
    ) -> Result<Vec<u8>, AntigravityStatusProbeError> {
        let url_str = format!(
            "{}://127.0.0.1:{}{}",
            endpoint.scheme, endpoint.port, payload.path
        );
        let url = reqwest::Url::parse(&url_str)
            .map_err(|e| AntigravityStatusProbeError::ApiError(format!("Invalid URL: {e}")))?;
        let body_bytes = serde_json::to_vec(&payload.body)
            .map_err(|e| AntigravityStatusProbeError::ApiError(e.to_string()))?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("connect-protocol-version"),
            HeaderValue::from_static("1"),
        );

        let req = NetRequest {
            method: Method::POST,
            url,
            headers,
            credentials: vec![(
                HeaderName::from_static("x-codeium-csrf-token"),
                SecretString::new(endpoint.csrf_token.clone()),
            )],
            body: NetBody::Plain(body_bytes),
            timeout: Some(timeout),
            body_cap: None,
        };

        let response = net
            .send_loopback(call, req)
            .await
            .map_err(Self::map_net_error)?;
        if response.status != reqwest::StatusCode::OK {
            let body_text = String::from_utf8_lossy(&response.body);
            return Err(AntigravityStatusProbeError::ApiError(format!(
                "HTTP {}: {}",
                response.status.as_u16(),
                body_text
            )));
        }
        Ok(response.body.to_vec())
    }

    fn map_probe_error(err: ProbeError) -> AntigravityStatusProbeError {
        match err {
            ProbeError::NoListeningPorts => AntigravityStatusProbeError::PortDetectionFailed(
                "no listening ports found".to_owned(),
            ),
            ProbeError::TimedOut | ProbeError::Cancelled => AntigravityStatusProbeError::TimedOut,
            ProbeError::Unsupported | ProbeError::ProcUnavailable => {
                AntigravityStatusProbeError::PortDetectionFailed(err.to_string())
            }
        }
    }

    fn map_net_error(err: NetError) -> AntigravityStatusProbeError {
        match err {
            NetError::Transport(TransportClass::Timeout) | NetError::Cancelled => {
                AntigravityStatusProbeError::TimedOut
            }
            NetError::Http { status } => {
                AntigravityStatusProbeError::ApiError(format!("HTTP {status}"))
            }
            other => AntigravityStatusProbeError::ApiError(other.to_string()),
        }
    }

    pub fn default_request_body() -> serde_json::Value {
        json!({
            "metadata": {
                "ideName": "antigravity",
                "extensionName": "antigravity",
                "ideVersion": "unknown",
                "locale": "en",
            }
        })
    }

    pub fn unleash_request_body() -> serde_json::Value {
        json!({
            "context": {
                "properties": {
                    "devMode": "false",
                    "extensionVersion": "unknown",
                    "hasAnthropicModelAccess": "true",
                    "ide": "antigravity",
                    "ideVersion": "unknown",
                    "installationId": "ariadusage",
                    "language": "UNSPECIFIED",
                    "os": "macos",
                    "requestedModelId": "MODEL_UNSPECIFIED",
                }
            }
        })
    }

    // MARK: - Response Parsing (+ResponseModels.swift)

    pub fn parse_user_status_response(
        data: &[u8],
    ) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
        let response: UserStatusResponse = serde_json::from_slice(data)
            .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))?;
        if let Some(invalid) = Self::invalid_code(response.code.as_ref()) {
            return Err(AntigravityStatusProbeError::ApiError(format!(
                "Code {invalid}"
            )));
        }
        let Some(user_status) = response.user_status else {
            return Err(AntigravityStatusProbeError::ParseFailed(
                "Missing userStatus".to_owned(),
            ));
        };
        let model_configs = user_status
            .cascade_model_config_data
            .and_then(|c| c.client_model_configs)
            .unwrap_or_default();
        let models: Vec<AntigravityModelQuota> = model_configs
            .iter()
            .filter_map(Self::quota_from_config)
            .collect();
        let plan_info = user_status
            .plan_status
            .and_then(|p| p.plan_info)
            .and_then(|info| info.into_summary().preferred_name());
        Ok(AntigravityStatusSnapshot {
            model_quotas: models,
            account_email: user_status.email,
            account_plan: plan_info,
            source: AntigravitySnapshotSource::Local,
            quota_summary: None,
        })
    }

    pub fn parse_command_model_response(
        data: &[u8],
    ) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
        let response: CommandModelConfigResponse = serde_json::from_slice(data)
            .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))?;
        if let Some(invalid) = Self::invalid_code(response.code.as_ref()) {
            return Err(AntigravityStatusProbeError::ApiError(format!(
                "Code {invalid}"
            )));
        }
        let model_configs = response.client_model_configs.unwrap_or_default();
        let models: Vec<AntigravityModelQuota> = model_configs
            .iter()
            .filter_map(Self::quota_from_config)
            .collect();
        Ok(AntigravityStatusSnapshot {
            model_quotas: models,
            account_email: None,
            account_plan: None,
            source: AntigravitySnapshotSource::Local,
            quota_summary: None,
        })
    }

    pub fn parse_quota_summary_response(
        data: &[u8],
    ) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
        parse_quota_summary_response(data)
    }

    pub fn parse_cli_usage_report(
        data: &[u8],
    ) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
        parse_cli_usage_report(data)
    }

    fn quota_from_config(config: &ModelConfigWire) -> Option<AntigravityModelQuota> {
        let quota = config.quota_info.as_ref()?;
        let reset_time = quota.reset_time.as_deref().and_then(Self::parse_date);
        if quota.remaining_fraction.is_none() && reset_time.is_none() {
            return None;
        }
        let model_id = config
            .model_or_alias
            .as_ref()
            .and_then(|m| m.model.clone())
            .unwrap_or_else(|| config.label.clone());
        Some(AntigravityModelQuota {
            label: config.label.clone(),
            model_id,
            remaining_fraction: quota.remaining_fraction,
            reset_time,
            reset_description: None,
        })
    }

    pub fn parse_date(raw: &str) -> Option<Timestamp> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Ok(ts) = trimmed.parse::<Timestamp>() {
            return Some(ts);
        }
        if let Ok(seconds) = trimmed.parse::<f64>()
            && seconds.is_finite()
        {
            let whole_secs = seconds.trunc() as i64;
            let nanos = ((seconds - seconds.trunc()) * 1_000_000_000.0).round() as i32;
            return Timestamp::new(whole_secs, nanos).ok();
        }
        None
    }

    fn invalid_code(code: Option<&CodeValue>) -> Option<String> {
        let code = code?;
        if code.is_ok() {
            None
        } else {
            Some(code.raw_value())
        }
    }
}

#[derive(Clone, Deserialize)]
struct UserStatusResponse {
    code: Option<CodeValue>,
    #[serde(rename = "userStatus")]
    user_status: Option<UserStatusWire>,
}

#[derive(Clone, Deserialize)]
struct CommandModelConfigResponse {
    code: Option<CodeValue>,
    #[serde(rename = "clientModelConfigs")]
    client_model_configs: Option<Vec<ModelConfigWire>>,
}

#[derive(Clone, Deserialize)]
struct UserStatusWire {
    email: Option<String>,
    #[serde(rename = "planStatus")]
    plan_status: Option<PlanStatusWire>,
    #[serde(rename = "cascadeModelConfigData")]
    cascade_model_config_data: Option<ModelConfigDataWire>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanStatusWire {
    #[serde(rename = "planInfo")]
    plan_info: Option<PlanInfoWire>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanInfoWire {
    #[serde(rename = "planName")]
    plan_name: Option<String>,
    #[serde(rename = "planDisplayName")]
    plan_display_name: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "productName")]
    product_name: Option<String>,
    #[serde(rename = "planShortName")]
    plan_short_name: Option<String>,
}

impl PlanInfoWire {
    fn into_summary(self) -> AntigravityPlanInfoSummary {
        AntigravityPlanInfoSummary {
            plan_name: self.plan_name,
            plan_display_name: self.plan_display_name,
            display_name: self.display_name,
            product_name: self.product_name,
            plan_short_name: self.plan_short_name,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ModelConfigDataWire {
    #[serde(rename = "clientModelConfigs")]
    client_model_configs: Option<Vec<ModelConfigWire>>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelConfigWire {
    label: String,
    #[serde(rename = "modelOrAlias")]
    model_or_alias: Option<ModelAliasWire>,
    #[serde(rename = "quotaInfo")]
    quota_info: Option<QuotaInfoWire>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelAliasWire {
    model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuotaInfoWire {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum CodeValue {
    Int(i64),
    String(String),
}

impl CodeValue {
    fn is_ok(&self) -> bool {
        match self {
            Self::Int(v) => *v == 0,
            Self::String(v) => {
                let lower = v.to_lowercase();
                lower == "ok" || lower == "success" || v == "0"
            }
        }
    }

    fn raw_value(&self) -> String {
        match self {
            Self::Int(v) => v.to_string(),
            Self::String(v) => v.clone(),
        }
    }
}
