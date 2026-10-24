// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+Deadline.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+LocalEndpoints.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+PortDetection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+ResponseModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityLocalSnapshotSelection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityLocalhostSession.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
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

use super::family_visibility::{KnownFamily, known_family_for_window};
use super::quota_summary::{
    AntigravityQuotaSummary, AntigravityQuotaSummaryBucket, AntigravityQuotaSummaryGroup,
    parse_cli_usage_report, parse_quota_summary_response,
};

pub const QUOTA_SUMMARY_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
pub const GET_USER_STATUS_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/GetUserStatus";
pub const COMMAND_MODEL_CONFIG_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/GetCommandModelConfigs";
pub const UNLEASH_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUnleashData";
pub const QUOTA_SUMMARY_WINDOW_ID_PREFIX: &str = "antigravity-quota-summary-";
pub const COMPACT_FALLBACK_WINDOW_ID_PREFIX: &str = "antigravity-compact-fallback-";
pub const CURRENT_FLASH_CANONICAL_ID: &str = "gemini-3.7-flash";

const RETIRED_FLASH_TIERS: &[(&str, &str)] = &[
    ("gemini-3.6-flash", CURRENT_FLASH_CANONICAL_ID),
    ("gemini-3.6-flash-low", CURRENT_FLASH_CANONICAL_ID),
    ("gemini-3.6-flash-medium", CURRENT_FLASH_CANONICAL_ID),
    ("gemini-3.6-flash-high", CURRENT_FLASH_CANONICAL_ID),
    ("gemini-3.5-flash-extra-low", CURRENT_FLASH_CANONICAL_ID),
    ("gemini-3.5-flash-low", CURRENT_FLASH_CANONICAL_ID),
    ("gemini-3.5-flash-mid", CURRENT_FLASH_CANONICAL_ID),
    ("gemini-3.5-flash-high", CURRENT_FLASH_CANONICAL_ID),
    ("gemini-3-flash-agent", CURRENT_FLASH_CANONICAL_ID),
];

const MODEL_LABEL_ACRONYMS: &[&str] = &["ai", "api", "gpt", "oss"];
const SESSION_CADENCE_ALIASES: &[&str] = &["session", "5h", "5-hour", "five hour", "five-hour"];

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
        let Some(fraction) = self.remaining_fraction else {
            return 0.0;
        };
        (fraction * 100.0).clamp(0.0, 100.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AntigravityModelFamily {
    ClaudeModels = 0,
    Gpt = 1,
    GeminiPro = 2,
    GeminiFlash = 3,
    Unknown = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AntigravityUsagePool {
    GeminiAi,
    ClaudeGpt,
}

impl AntigravityUsagePool {
    pub const fn id(self) -> &'static str {
        match self {
            Self::GeminiAi => "antigravity-gemini",
            Self::ClaudeGpt => "antigravity-claude-gpt",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::GeminiAi => "Gemini Models",
            Self::ClaudeGpt => "Claude and GPT models",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AntigravityModelVersion {
    major: i32,
    minor: i32,
}

#[derive(Debug, Clone)]
struct AntigravityNormalizedModel {
    quota: AntigravityModelQuota,
    family: AntigravityModelFamily,
    is_image: bool,
    is_lite: bool,
    is_autocomplete: bool,
    version: Option<AntigravityModelVersion>,
    tier: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuotaSummaryBucketKind {
    Session,
    Weekly,
    Other,
}

impl QuotaSummaryBucketKind {
    const fn window_minutes(self) -> Option<i64> {
        match self {
            Self::Session => Some(300),
            Self::Weekly => Some(10_080),
            Self::Other => None,
        }
    }

    const fn sort_rank(self) -> i32 {
        match self {
            Self::Session => 0,
            Self::Weekly => 1,
            Self::Other => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AntigravitySnapshotSource {
    Local,
    Remote,
    Cli,
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
            source: AntigravitySnapshotSource::Remote,
            quota_summary: None,
        }
    }

    pub fn from_quota_summary(
        quota_summary: AntigravityQuotaSummary,
        account_email: Option<String>,
        account_plan: Option<String>,
        source: AntigravitySnapshotSource,
    ) -> Self {
        Self {
            model_quotas: Vec::new(),
            account_email,
            account_plan,
            source,
            quota_summary: Some(quota_summary),
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

    pub fn with_identity(&self, snapshot: Option<&AntigravityStatusSnapshot>) -> Self {
        let Some(snapshot) = snapshot else {
            return self.clone();
        };
        let account_email = snapshot
            .account_email
            .clone()
            .or_else(|| self.account_email.clone());
        let account_plan = snapshot
            .account_plan
            .clone()
            .or_else(|| self.account_plan.clone());
        Self {
            model_quotas: self.model_quotas.clone(),
            account_email,
            account_plan,
            source: self.source,
            quota_summary: self.quota_summary.clone(),
        }
    }

    pub fn has_known_quota_summary(&self) -> bool {
        let Some(quota_summary) = &self.quota_summary else {
            return false;
        };
        quota_summary.groups.iter().any(|g| {
            g.buckets
                .iter()
                .any(|b| !b.disabled && b.remaining_fraction.is_some())
        })
    }

    pub fn has_quota_summary_buckets(&self) -> bool {
        let Some(quota_summary) = &self.quota_summary else {
            return false;
        };
        quota_summary.groups.iter().any(|g| !g.buckets.is_empty())
    }

    pub fn canonical_model_id(model_id: &str) -> String {
        let key = model_id.trim().to_lowercase();
        for &(retired, replacement) in RETIRED_FLASH_TIERS {
            if key == retired {
                return replacement.to_owned();
            }
        }
        model_id.to_owned()
    }

    pub fn humanized_model_id(model_id: &str) -> String {
        let canonical = Self::canonical_model_id(model_id);
        let parts: Vec<&str> = canonical.split('-').collect();
        let mut words: Vec<String> = Vec::new();
        let mut index = 0;

        while index < parts.len() {
            let part = parts[index];
            if Self::is_single_digit(part)
                && index + 1 < parts.len()
                && Self::is_single_digit(parts[index + 1])
            {
                let mut version_parts = vec![part];
                index += 1;
                while index < parts.len() && Self::is_single_digit(parts[index]) {
                    version_parts.push(parts[index]);
                    index += 1;
                }
                words.push(version_parts.join("."));
                continue;
            }

            if !part.is_empty() && part.chars().all(|c| c.is_numeric() || c == '.') {
                words.push(part.to_owned());
            } else if MODEL_LABEL_ACRONYMS.contains(&part.to_lowercase().as_str()) {
                words.push(part.to_uppercase());
            } else {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => {
                        let mut s = first.to_uppercase().to_string();
                        s.push_str(chars.as_str());
                        words.push(s);
                    }
                    None => words.push(String::new()),
                }
            }
            index += 1;
        }

        words.join(" ")
    }

    fn is_single_digit(value: &str) -> bool {
        value.len() == 1 && value.chars().all(|c| c.is_numeric())
    }

    pub fn quota_display_label(quota: &AntigravityModelQuota) -> String {
        let trimmed = quota.label.trim();
        if !trimmed.is_empty() && trimmed != quota.model_id {
            return quota.label.clone();
        }
        Self::humanized_model_id(&Self::canonical_model_id(&quota.model_id))
    }

    pub fn to_usage_snapshot(&self) -> Result<UsageSnapshot, AntigravityStatusProbeError> {
        self.to_usage_snapshot_at(Timestamp::now())
    }

    pub fn to_usage_snapshot_at(
        &self,
        now: Timestamp,
    ) -> Result<UsageSnapshot, AntigravityStatusProbeError> {
        if let Some(quota_summary) = &self.quota_summary {
            return Self::usage_snapshot_from_quota_summary(
                quota_summary,
                self.account_email.as_deref(),
                self.account_plan.as_deref(),
                now,
            );
        }

        if self.model_quotas.is_empty() {
            return Err(AntigravityStatusProbeError::ParseFailed(
                "No quota models available".to_owned(),
            ));
        }

        let normalized: Vec<AntigravityNormalizedModel> = self
            .model_quotas
            .iter()
            .map(Self::normalize_model)
            .collect();
        let summary_candidates: Vec<AntigravityNormalizedModel> = normalized
            .iter()
            .filter(|m| Self::is_summary_candidate(m))
            .cloned()
            .collect();

        let primary_quota =
            Self::representative(AntigravityUsagePool::GeminiAi, &summary_candidates);
        let secondary_quota =
            Self::representative(AntigravityUsagePool::ClaudeGpt, &summary_candidates);

        let fallback_quota = if primary_quota.is_none() && secondary_quota.is_none() {
            match self.source {
                AntigravitySnapshotSource::Local | AntigravitySnapshotSource::Cli => {
                    let unknown_candidates: Vec<AntigravityNormalizedModel> = normalized
                        .iter()
                        .filter(|m| {
                            m.family == AntigravityModelFamily::Unknown
                                && Self::is_selectable_text_model(m)
                                && m.quota.remaining_fraction.is_some()
                        })
                        .cloned()
                        .collect();
                    Self::fallback_representative(&unknown_candidates)
                }
                AntigravitySnapshotSource::Remote => None,
            }
        } else {
            None
        };

        let primary = primary_quota.as_ref().and_then(Self::rate_window_for_quota);
        let secondary = secondary_quota
            .as_ref()
            .and_then(Self::rate_window_for_quota);

        let mut represented_quotas = BTreeMap::new();
        if let Some(pq) = primary_quota {
            represented_quotas.insert(AntigravityUsagePool::GeminiAi, pq);
        }
        if let Some(sq) = secondary_quota {
            represented_quotas.insert(AntigravityUsagePool::ClaudeGpt, sq);
        }

        let extra_windows = Self::extra_rate_windows(
            &normalized,
            &summary_candidates,
            fallback_quota.as_ref().map(|q| q.model_id.as_str()),
            &represented_quotas,
            self.source,
        );

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
            None,
            if extra_windows.is_empty() {
                None
            } else {
                Some(extra_windows)
            },
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

    fn rate_window_for_quota(quota: &AntigravityModelQuota) -> Option<RateWindow> {
        let used = (100.0 - quota.remaining_percent()).clamp(0.0, 100.0);
        RateWindow::new(
            used,
            None,
            quota.reset_time,
            quota.reset_description.clone(),
            None,
            false,
        )
        .ok()
    }

    pub fn models_for_pool(&self, pool: AntigravityUsagePool) -> Vec<AntigravityModelQuota> {
        self.model_quotas
            .iter()
            .map(Self::normalize_model)
            .filter(|m| Self::is_summary_candidate(m) && Self::usage_pool(m) == Some(pool))
            .map(|m| m.quota)
            .collect()
    }

    pub fn representative_for_pool(
        &self,
        pool: AntigravityUsagePool,
    ) -> Option<AntigravityModelQuota> {
        let normalized: Vec<AntigravityNormalizedModel> = self
            .model_quotas
            .iter()
            .map(Self::normalize_model)
            .collect();
        let summary_candidates: Vec<AntigravityNormalizedModel> = normalized
            .iter()
            .filter(|m| Self::is_summary_candidate(m))
            .cloned()
            .collect();
        Self::representative(pool, &summary_candidates)
    }

    fn normalize_model(quota: &AntigravityModelQuota) -> AntigravityNormalizedModel {
        let canonical_id = Self::canonical_model_id(&quota.model_id);
        let canonical_quota = if canonical_id != quota.model_id {
            AntigravityModelQuota {
                label: quota.label.clone(),
                model_id: canonical_id,
                remaining_fraction: quota.remaining_fraction,
                reset_time: quota.reset_time,
                reset_description: quota.reset_description.clone(),
            }
        } else {
            quota.clone()
        };

        let model_id = canonical_quota.model_id.to_lowercase();
        let label = canonical_quota.label.to_lowercase();
        let family = Self::family_for_model_id_and_label(&model_id, &label);

        let is_lite = model_id.contains("lite") || label.contains("lite");
        let is_autocomplete = model_id.contains("autocomplete")
            || label.contains("autocomplete")
            || model_id.starts_with("tab_");
        let is_image = model_id.contains("image") || label.contains("image");

        let version = Self::parse_version(&label);
        let tier = Self::parse_tier(&label, &model_id);

        AntigravityNormalizedModel {
            quota: canonical_quota,
            family,
            is_image,
            is_lite,
            is_autocomplete,
            version,
            tier,
        }
    }

    fn is_summary_candidate(model: &AntigravityNormalizedModel) -> bool {
        Self::usage_pool(model).is_some() && Self::is_selectable_text_model(model)
    }

    fn is_selectable_text_model(model: &AntigravityNormalizedModel) -> bool {
        !model.is_lite && !model.is_autocomplete && !model.is_image
    }

    fn usage_pool(model: &AntigravityNormalizedModel) -> Option<AntigravityUsagePool> {
        match model.family {
            AntigravityModelFamily::GeminiPro | AntigravityModelFamily::GeminiFlash => {
                Some(AntigravityUsagePool::GeminiAi)
            }
            AntigravityModelFamily::ClaudeModels | AntigravityModelFamily::Gpt => {
                Some(AntigravityUsagePool::ClaudeGpt)
            }
            AntigravityModelFamily::Unknown => None,
        }
    }

    fn family_for_model_id_and_label(model_id: &str, label: &str) -> AntigravityModelFamily {
        let model_id_family = Self::family_from_text(model_id);
        if model_id_family != AntigravityModelFamily::Unknown {
            return model_id_family;
        }
        Self::family_from_text(label)
    }

    fn family_from_text(text: &str) -> AntigravityModelFamily {
        if text.contains("claude") {
            return AntigravityModelFamily::ClaudeModels;
        }
        if text.contains("gpt") || text.contains("openai") {
            return AntigravityModelFamily::Gpt;
        }
        if text.contains("gemini") && text.contains("pro") {
            return AntigravityModelFamily::GeminiPro;
        }
        if text.contains("gemini") && text.contains("flash") {
            return AntigravityModelFamily::GeminiFlash;
        }
        AntigravityModelFamily::Unknown
    }

    fn parse_version(label: &str) -> Option<AntigravityModelVersion> {
        // Matches `(\d+)(?:[.\-](\d+))?`
        let bytes = label.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let start_major = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let major = label[start_major..i].parse::<i32>().ok()?;
                let minor = if i + 1 < bytes.len()
                    && (bytes[i] == b'.' || bytes[i] == b'-')
                    && bytes[i + 1].is_ascii_digit()
                {
                    i += 1;
                    let start_minor = i;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    label[start_minor..i].parse::<i32>().unwrap_or(0)
                } else {
                    0
                };
                return Some(AntigravityModelVersion { major, minor });
            }
            i += 1;
        }
        None
    }

    fn parse_tier(label: &str, model_id: &str) -> i32 {
        let combined = format!("{label} {model_id}");
        if combined.contains("high") {
            return 0;
        }
        if combined.contains("medium") {
            return 1;
        }
        if combined.contains("low") {
            return 2;
        }
        1
    }

    fn representative(
        pool: AntigravityUsagePool,
        models: &[AntigravityNormalizedModel],
    ) -> Option<AntigravityModelQuota> {
        let candidates: Vec<&AntigravityNormalizedModel> = models
            .iter()
            .filter(|m| Self::usage_pool(m) == Some(pool) && m.quota.remaining_fraction.is_some())
            .collect();
        candidates
            .into_iter()
            .min_by(|lhs, rhs| {
                let l_rem = lhs.quota.remaining_percent();
                let r_rem = rhs.quota.remaining_percent();
                if l_rem != r_rem {
                    return l_rem.total_cmp(&r_rem);
                }
                match (lhs.quota.reset_time, rhs.quota.reset_time) {
                    (Some(left), Some(right)) if left != right => left.cmp(&right),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    _ => lhs
                        .quota
                        .label
                        .to_lowercase()
                        .cmp(&rhs.quota.label.to_lowercase()),
                }
            })
            .map(|m| m.quota.clone())
    }

    fn fallback_representative(
        models: &[AntigravityNormalizedModel],
    ) -> Option<AntigravityModelQuota> {
        models
            .iter()
            .min_by(|lhs, rhs| {
                let l_rem = lhs.quota.remaining_percent();
                let r_rem = rhs.quota.remaining_percent();
                if l_rem != r_rem {
                    return l_rem.total_cmp(&r_rem);
                }
                lhs.quota
                    .label
                    .to_lowercase()
                    .cmp(&rhs.quota.label.to_lowercase())
            })
            .map(|m| m.quota.clone())
    }

    fn extra_rate_windows(
        models: &[AntigravityNormalizedModel],
        summary_candidates: &[AntigravityNormalizedModel],
        compact_fallback_model_id: Option<&str>,
        represented_quotas: &BTreeMap<AntigravityUsagePool, AntigravityModelQuota>,
        source: AntigravitySnapshotSource,
    ) -> Vec<NamedRateWindow> {
        let mut reset_only_pool_windows = Vec::new();
        for pool in [
            AntigravityUsagePool::GeminiAi,
            AntigravityUsagePool::ClaudeGpt,
        ] {
            if represented_quotas.contains_key(&pool) {
                continue;
            }
            let reset_only = summary_candidates.iter().find(|model| {
                Self::usage_pool(model) == Some(pool)
                    && model.quota.remaining_fraction.is_none()
                    && (model.quota.reset_time.is_some() || model.quota.reset_description.is_some())
            });
            if let Some(reset_only) = reset_only
                && let Some(window) = Self::rate_window_for_quota(&reset_only.quota)
            {
                reset_only_pool_windows.push(
                    NamedRateWindow::new(pool.id(), pool.title(), window).with_usage_known(false),
                );
            }
        }

        let mut grouped: BTreeMap<String, Vec<&AntigravityNormalizedModel>> = BTreeMap::new();
        for model in models {
            grouped
                .entry(model.quota.model_id.to_lowercase())
                .or_default()
                .push(model);
        }

        let mut distinct_models: Vec<AntigravityNormalizedModel> = grouped
            .into_values()
            .filter_map(|group| {
                group
                    .into_iter()
                    .min_by(|lhs, rhs| {
                        let lhs_has = lhs.quota.remaining_fraction.is_some();
                        let rhs_has = rhs.quota.remaining_fraction.is_some();
                        if lhs_has != rhs_has {
                            return if lhs_has {
                                std::cmp::Ordering::Less
                            } else {
                                std::cmp::Ordering::Greater
                            };
                        }
                        let l_rem = lhs.quota.remaining_percent();
                        let r_rem = rhs.quota.remaining_percent();
                        if l_rem != r_rem {
                            return l_rem.total_cmp(&r_rem);
                        }
                        lhs.quota.label.cmp(&rhs.quota.label)
                    })
                    .cloned()
            })
            .filter(|model| {
                let pool = Self::usage_pool(model).or(if model.is_autocomplete {
                    Some(AntigravityUsagePool::GeminiAi)
                } else {
                    None
                });
                let pool_quota = if source == AntigravitySnapshotSource::Remote {
                    pool.and_then(|p| represented_quotas.get(&p))
                } else {
                    None
                };
                compact_fallback_model_id == Some(model.quota.model_id.as_str())
                    || Self::should_show_distinct_extra_window(model, pool_quota)
            })
            .collect();

        distinct_models.sort_by(Self::model_order_cmp);

        let distinct_windows: Vec<NamedRateWindow> = distinct_models
            .into_iter()
            .filter_map(|m| {
                let window = Self::rate_window_for_quota(&m.quota)?;
                let id = if compact_fallback_model_id == Some(m.quota.model_id.as_str()) {
                    Self::compact_fallback_window_id(&m.quota.model_id)
                } else {
                    m.quota.model_id.clone()
                };
                let title = Self::quota_display_label(&m.quota);
                let usage_known = m.quota.remaining_fraction.is_some();
                Some(NamedRateWindow::new(id, title, window).with_usage_known(usage_known))
            })
            .collect();

        reset_only_pool_windows.extend(distinct_windows);
        reset_only_pool_windows
    }

    fn compact_fallback_window_id(model_id: &str) -> String {
        format!("{COMPACT_FALLBACK_WINDOW_ID_PREFIX}{model_id}")
    }

    fn should_show_distinct_extra_window(
        model: &AntigravityNormalizedModel,
        pool_quota: Option<&AntigravityModelQuota>,
    ) -> bool {
        if Self::is_summary_candidate(model) {
            return false;
        }
        if let (Some(pool_quota), Some(reset)) = (pool_quota, model.quota.reset_time)
            && Some(reset) == pool_quota.reset_time
            && model.quota.remaining_fraction == pool_quota.remaining_fraction
        {
            return false;
        }
        if model.quota.remaining_fraction.is_none() {
            return model.quota.reset_time.is_some() || model.quota.reset_description.is_some();
        }
        model.quota.remaining_percent() < 99.9
    }

    fn model_order_cmp(
        lhs: &AntigravityNormalizedModel,
        rhs: &AntigravityNormalizedModel,
    ) -> std::cmp::Ordering {
        if lhs.family != rhs.family {
            return lhs.family.cmp(&rhs.family);
        }
        match (lhs.version, rhs.version) {
            (Some(lv), Some(rv)) if lv != rv => return rv.cmp(&lv),
            (Some(_), None) => return std::cmp::Ordering::Less,
            (None, Some(_)) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        if lhs.tier != rhs.tier {
            return lhs.tier.cmp(&rhs.tier);
        }
        lhs.quota
            .label
            .to_lowercase()
            .cmp(&rhs.quota.label.to_lowercase())
    }

    fn usage_snapshot_from_quota_summary(
        quota_summary: &AntigravityQuotaSummary,
        account_email: Option<&str>,
        account_plan: Option<&str>,
        now: Timestamp,
    ) -> Result<UsageSnapshot, AntigravityStatusProbeError> {
        let named_windows = Self::quota_summary_windows(quota_summary);
        if named_windows.is_empty() {
            return Err(AntigravityStatusProbeError::ParseFailed(
                "No quota buckets available".to_owned(),
            ));
        }

        let primary = Self::quota_summary_representative(KnownFamily::Gemini, &named_windows);
        let secondary = Self::quota_summary_representative(KnownFamily::ClaudeGpt, &named_windows);

        let identity = ProviderIdentitySnapshot::new(
            Some(ProviderId::new("antigravity").expect("valid provider id")),
            account_email.map(str::to_owned),
            None,
            account_plan.map(str::to_owned),
            None,
        );

        UsageSnapshot::new(
            primary,
            secondary,
            None,
            Some(named_windows),
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

    fn quota_summary_representative(
        family: KnownFamily,
        windows: &[NamedRateWindow],
    ) -> Option<RateWindow> {
        let family_windows: Vec<&NamedRateWindow> = windows
            .iter()
            .filter(|w| w.usage_known && known_family_for_window(w) == Some(family))
            .collect();
        if family_windows.is_empty() {
            return None;
        }

        let preferred_windows: Vec<&NamedRateWindow> = if family == KnownFamily::Gemini {
            let ordinary: Vec<&NamedRateWindow> = family_windows
                .iter()
                .copied()
                .filter(|w| Self::is_ordinary_gemini_quota_summary_window_id(&w.id))
                .collect();
            if ordinary.is_empty() {
                family_windows
            } else {
                ordinary
            }
        } else {
            family_windows
        };

        preferred_windows
            .into_iter()
            .max_by(|lhs, rhs| {
                if lhs.window.used_percent != rhs.window.used_percent {
                    return lhs.window.used_percent.total_cmp(&rhs.window.used_percent);
                }
                rhs.title.to_lowercase().cmp(&lhs.title.to_lowercase())
            })
            .map(|w| w.window.clone())
    }

    fn quota_summary_windows(quota_summary: &AntigravityQuotaSummary) -> Vec<NamedRateWindow> {
        let mut sorted_groups = quota_summary.groups.clone();
        sorted_groups.sort_by_key(Self::quota_summary_group_sort_rank);

        let mut windows = Vec::new();
        for group in sorted_groups {
            let group_title = Self::quota_summary_group_display_title(&group.label);
            let mut sorted_buckets = group.buckets;
            sorted_buckets.sort_by(Self::quota_summary_bucket_cmp);
            for bucket in sorted_buckets {
                let remaining_percent = bucket
                    .remaining_fraction
                    .map(|f| (f * 100.0).clamp(0.0, 100.0))
                    .unwrap_or(0.0);
                let kind = Self::quota_summary_bucket_kind(&bucket);
                let bucket_title = Self::quota_summary_bucket_display_title(&bucket);
                let title = format!("{group_title} {bucket_title}").trim().to_owned();
                let usage_known = !bucket.disabled && bucket.remaining_fraction.is_some();
                if let Ok(window) = RateWindow::new(
                    (100.0 - remaining_percent).clamp(0.0, 100.0),
                    kind.window_minutes(),
                    bucket.reset_time,
                    bucket.reset_description.clone(),
                    None,
                    false,
                ) {
                    windows.push(
                        NamedRateWindow::new(
                            format!("{QUOTA_SUMMARY_WINDOW_ID_PREFIX}{}", bucket.bucket_id),
                            title,
                            window,
                        )
                        .with_usage_known(usage_known),
                    );
                }
            }
        }
        windows
    }

    fn quota_summary_group_sort_rank(group: &AntigravityQuotaSummaryGroup) -> i32 {
        match Self::quota_summary_group_display_title(&group.label).as_str() {
            "Gemini" => 0,
            "Claude/GPT" => 1,
            _ => 2,
        }
    }

    fn quota_summary_group_display_title(raw: &str) -> String {
        let trimmed = raw.trim();
        let lower = trimmed.to_lowercase();
        if lower.contains("gemini") {
            return "Gemini".to_owned();
        }
        if lower.contains("claude") || lower.contains("gpt") {
            return "Claude/GPT".to_owned();
        }
        if trimmed.is_empty() {
            "Quota".to_owned()
        } else {
            trimmed.to_owned()
        }
    }

    fn quota_summary_bucket_cmp(
        lhs: &AntigravityQuotaSummaryBucket,
        rhs: &AntigravityQuotaSummaryBucket,
    ) -> std::cmp::Ordering {
        let lhs_rank = Self::quota_summary_bucket_kind(lhs).sort_rank();
        let rhs_rank = Self::quota_summary_bucket_kind(rhs).sort_rank();
        lhs_rank.cmp(&rhs_rank)
    }

    fn is_ordinary_gemini_quota_summary_window_id(window_id: &str) -> bool {
        let normalized = window_id.trim().to_lowercase().replace('_', "-");
        let normalized = normalized
            .strip_suffix(" limit")
            .unwrap_or(&normalized)
            .trim();
        for alias in SESSION_CADENCE_ALIASES {
            if normalized == format!("antigravity-quota-summary-gemini-{alias}") {
                return true;
            }
        }
        normalized == "antigravity-quota-summary-gemini-weekly"
    }

    fn quota_summary_bucket_display_title(bucket: &AntigravityQuotaSummaryBucket) -> String {
        match Self::quota_summary_bucket_kind(bucket) {
            QuotaSummaryBucketKind::Session => "5-hour".to_owned(),
            QuotaSummaryBucketKind::Weekly => "weekly".to_owned(),
            QuotaSummaryBucketKind::Other => {
                if let Some(display_name) = bucket
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    return display_name.to_owned();
                }
                if let Some(window) = bucket
                    .window
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    return window.to_owned();
                }
                bucket.bucket_id.clone()
            }
        }
    }

    fn quota_summary_bucket_kind(bucket: &AntigravityQuotaSummaryBucket) -> QuotaSummaryBucketKind {
        let candidates = Self::quota_cadence_candidates(bucket);
        if SESSION_CADENCE_ALIASES
            .iter()
            .any(|alias| candidates.contains(*alias))
        {
            return QuotaSummaryBucketKind::Session;
        }
        if candidates.contains("weekly") {
            return QuotaSummaryBucketKind::Weekly;
        }
        QuotaSummaryBucketKind::Other
    }

    fn quota_cadence_candidates(bucket: &AntigravityQuotaSummaryBucket) -> BTreeSet<String> {
        let explicit = bucket
            .window
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let values: Vec<&str> = if let Some(explicit) = explicit {
            vec![explicit]
        } else {
            let mut v = vec![bucket.bucket_id.as_str()];
            if let Some(dn) = bucket.display_name.as_deref() {
                v.push(dn);
            }
            v
        };
        let mut candidates = BTreeSet::new();
        for raw_value in values {
            let normalized = raw_value.trim().to_lowercase().replace('_', "-");
            if normalized.is_empty() {
                continue;
            }
            let mut normalized_candidates = vec![normalized.clone()];
            if let Some(stripped) = normalized.strip_suffix(" limit") {
                normalized_candidates.push(stripped.to_owned());
            }
            for candidate in normalized_candidates {
                for alias in SESSION_CADENCE_ALIASES
                    .iter()
                    .copied()
                    .chain(std::iter::once("weekly"))
                {
                    if candidate.ends_with(&format!("-{alias}")) {
                        candidates.insert(alias.to_owned());
                    }
                }
                candidates.insert(candidate);
            }
        }
        candidates
    }
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
    #[error("{}", Self::port_detection_description(.0))]
    PortDetectionFailed(String),
    #[error("{}", Self::api_error_description(.0))]
    ApiError(String),
    #[error("Could not parse Antigravity quota: {0}")]
    ParseFailed(String),
    #[error("Antigravity quota request timed out.")]
    TimedOut,
    #[error("Antigravity quota request cancelled.")]
    Cancelled,
}

impl AntigravityStatusProbeError {
    fn port_detection_description(message: &str) -> String {
        match message {
            "lsof not available" => {
                "Antigravity port detection needs lsof. Install it, then retry.".to_owned()
            }
            "no listening ports found" => {
                "Antigravity is running but not exposing ports yet. Try again in a few seconds."
                    .to_owned()
            }
            _ => format!("Antigravity port detection failed: {message}"),
        }
    }

    fn api_error_description(message: &str) -> String {
        if message.contains("HTTP 401") || message.contains("HTTP 403") {
            "Antigravity session expired. Restart Antigravity and retry.".to_owned()
        } else {
            format!("Antigravity API error: {message}")
        }
    }

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
            Self::TimedOut | Self::Cancelled => {
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

    pub fn now(&self) -> Instant {
        (self.now_provider)()
    }

    pub fn remaining(&self) -> Option<Duration> {
        AntigravityStatusProbe::remaining_time(self.now(), self.deadline)
    }

    pub fn fallback_reserving(&self) -> Self {
        let now = self.now();
        let remaining = self.deadline.saturating_duration_since(now);
        let attempt_budget = remaining / 2;
        Self {
            deadline: now + attempt_budget,
            now_provider: Arc::clone(&self.now_provider),
        }
    }

    pub fn timeout_for_next_step(
        &self,
        cap: Option<Duration>,
    ) -> Result<Duration, AntigravityStatusProbeError> {
        let now = self.now();
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

    pub fn timeout_for_endpoint_attempt(
        &self,
        timeout: Duration,
        remaining_attempt_count: usize,
    ) -> Result<Duration, AntigravityStatusProbeError> {
        let Some(remaining) = self.remaining() else {
            return Err(AntigravityStatusProbeError::TimedOut);
        };
        let count = remaining_attempt_count.max(1) as u32;
        let per_attempt = remaining / count;
        if per_attempt.is_zero() {
            return Err(AntigravityStatusProbeError::TimedOut);
        }
        Ok(timeout.min(per_attempt))
    }

    pub fn has_expired(&self) -> bool {
        AntigravityStatusProbe::should_abort_endpoint_scan(self.now(), self.deadline)
    }
}

// MARK: - Localhost Session & Endpoints (AntigravityLocalhostSession.swift & +LocalEndpoints.swift)

pub struct LocalhostTrustPolicy;

impl LocalhostTrustPolicy {
    pub const SERVER_TRUST_AUTH_METHOD: &str = "NSURLAuthenticationMethodServerTrust";

    pub fn should_accept_server_trust(host: &str) -> bool {
        matches!(
            host.trim().to_ascii_lowercase().as_str(),
            "127.0.0.1" | "localhost"
        )
    }

    pub fn should_accept_server_trust_challenge(
        authentication_method: &str,
        host: &str,
        has_server_trust: bool,
    ) -> bool {
        authentication_method == Self::SERVER_TRUST_AUTH_METHOD
            && has_server_trust
            && Self::should_accept_server_trust(host)
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
    CliHttps,
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

    pub fn requires_csrf_token(&self) -> bool {
        match self.source {
            EndpointSource::LanguageServer | EndpointSource::ExtensionServer => true,
            EndpointSource::CliHttps => false,
        }
    }

    pub fn matches_request_target(&self, other: &Self) -> bool {
        self.scheme == other.scheme
            && self.port == other.port
            && self.csrf_token == other.csrf_token
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
        let bounded = remaining.min(cap);
        if bounded.is_zero() {
            None
        } else {
            Some(bounded)
        }
    }

    pub fn should_abort_endpoint_scan(now: Instant, deadline: Instant) -> bool {
        Self::remaining_time(now, deadline).is_none()
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
        for prefix in ["language_server", "language-server"] {
            for (idx, _) in lower.match_indices(prefix) {
                if idx > 0 {
                    let prev = lower.as_bytes()[idx - 1];
                    if prev != b'/' && prev != b'\\' {
                        continue;
                    }
                }
                let after_prefix = &lower[idx + prefix.len()..];
                let token_len = after_prefix
                    .find(char::is_whitespace)
                    .unwrap_or(after_prefix.len());
                let suffix = &after_prefix[..token_len];
                let stem = suffix.strip_suffix(".exe").unwrap_or(suffix);
                if stem.is_empty() {
                    return true;
                }
                if (stem.starts_with('_') || stem.starts_with('-'))
                    && stem.split(['_', '-']).skip(1).all(|seg| {
                        !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric())
                    })
                {
                    return true;
                }
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
        for needle in ["antigravity-cli", "antigravity_cli"] {
            for (idx, _) in lower.match_indices(needle) {
                if idx > 0 {
                    let prev = lower.as_bytes()[idx - 1];
                    if prev != b'/' && prev != b'\\' {
                        continue;
                    }
                }
                let end = idx + needle.len();
                if end == lower.len() {
                    return true;
                }
                let next_char = lower[end..].chars().next().unwrap_or(' ');
                if next_char.is_whitespace() || next_char == '/' || next_char == '\\' {
                    return true;
                }
            }
        }
        for (idx, _) in lower.match_indices("agy") {
            if idx > 0 {
                let prev = lower.as_bytes()[idx - 1];
                if prev != b'/' && prev != b'\\' {
                    continue;
                }
            }
            let end = idx + "agy".len();
            if end == lower.len() {
                return true;
            }
            let next_char = lower[end..].chars().next().unwrap_or(' ');
            if next_char.is_whitespace() {
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
        let mut saw_tokenless_ide = false;

        for entry in entries {
            let Some(kind) = Self::antigravity_process_kind(&entry.command) else {
                continue;
            };
            if !Self::matches_scope(kind, scope) {
                continue;
            }
            let Some(token) = Self::resolved_csrf_token_for_kind(kind, &entry.command) else {
                saw_tokenless_ide = true;
                continue;
            };
            let ext_csrf = Self::extract_flag("--extension_server_csrf_token", &entry.command);
            let port = Self::extract_port("--extension_server_port", &entry.command);
            matches.push(ProcessInfoResult {
                pid: entry.pid,
                csrf_token: Some(token),
                extension_port: port,
                extension_csrf_token: ext_csrf,
                command: entry.command.clone(),
                identity: entry.identity,
            });
        }

        if !matches.is_empty() {
            return Ok(matches);
        }
        if saw_tokenless_ide {
            return Err(AntigravityStatusProbeError::MissingCSRFToken);
        }
        Err(AntigravityStatusProbeError::NotRunning)
    }

    pub fn resolved_csrf_token_for_kind(
        kind: AntigravityProcessKind,
        command: &str,
    ) -> Option<String> {
        if let Some(token) = Self::extract_flag("--csrf_token", command) {
            return Some(token);
        }
        match kind {
            AntigravityProcessKind::App | AntigravityProcessKind::Ide => None,
            AntigravityProcessKind::Cli => Some(String::new()),
        }
    }

    pub fn resolved_csrf_token(
        csrf_token: Option<&str>,
        extension_csrf_token: Option<&str>,
        command: &str,
    ) -> Option<String> {
        if let Some(token) = csrf_token {
            return Some(token.trim().to_owned());
        }
        if let Some(kind) = Self::antigravity_process_kind(command)
            && let Some(token) = Self::resolved_csrf_token_for_kind(kind, command)
        {
            return Some(token);
        }
        if let Some(ext) = extension_csrf_token
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(ext.to_owned());
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
        if let Some(&non_ext) = ports.iter().find(|&&p| Some(p) != extension_port) {
            return Some(non_ext);
        }
        if let Some(ext) = extension_port {
            return Some(ext);
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
        let language_csrf = csrf_token.map(str::trim).unwrap_or_default();
        let ext_csrf = extension_csrf_token
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let mut endpoints = if csrf_token.is_some() {
            Self::language_server_endpoints(ports, language_csrf, schemes)
        } else {
            Vec::new()
        };

        for endpoint in
            Self::extension_server_endpoints(extension_port, csrf_token.map(str::trim), ext_csrf)
        {
            if !endpoints
                .iter()
                .any(|existing| existing.matches_request_target(&endpoint))
            {
                endpoints.push(endpoint);
            }
        }

        endpoints
    }

    fn language_server_endpoints(
        ports: &[u16],
        language_csrf: &str,
        schemes: &[&str],
    ) -> Vec<AntigravityConnectionEndpoint> {
        let mut endpoints = Vec::new();
        for &port in ports {
            for &scheme in schemes {
                endpoints.push(AntigravityConnectionEndpoint::new(
                    scheme,
                    port,
                    language_csrf,
                    EndpointSource::LanguageServer,
                ));
            }
        }
        endpoints
    }

    fn extension_server_endpoints(
        extension_port: Option<u16>,
        language_csrf: Option<&str>,
        extension_csrf: Option<&str>,
    ) -> Vec<AntigravityConnectionEndpoint> {
        let Some(ext_port) = extension_port else {
            return Vec::new();
        };
        let mut endpoints = Vec::new();
        if let Some(ext_csrf) = extension_csrf {
            endpoints.push(AntigravityConnectionEndpoint::new(
                "http",
                ext_port,
                ext_csrf,
                EndpointSource::ExtensionServer,
            ));
        }
        if let Some(lang_csrf) = language_csrf
            && !lang_csrf.is_empty()
            && extension_csrf != Some(lang_csrf)
        {
            endpoints.push(AntigravityConnectionEndpoint::new(
                "http",
                ext_port,
                lang_csrf,
                EndpointSource::ExtensionServer,
            ));
        }
        endpoints
    }

    pub fn request_endpoints(
        primary: &AntigravityConnectionEndpoint,
        candidates: &[AntigravityConnectionEndpoint],
    ) -> Vec<AntigravityConnectionEndpoint> {
        let mut ordered = vec![primary.clone()];
        let (first_source, second_source) = if primary.source == EndpointSource::ExtensionServer {
            (
                EndpointSource::ExtensionServer,
                EndpointSource::LanguageServer,
            )
        } else {
            (
                EndpointSource::LanguageServer,
                EndpointSource::ExtensionServer,
            )
        };

        for source in [first_source, second_source] {
            for candidate in candidates.iter().filter(|c| c.source == source) {
                if !ordered
                    .iter()
                    .any(|existing| existing.matches_request_target(candidate))
                {
                    ordered.push(candidate.clone());
                }
            }
        }
        for candidate in candidates {
            if !ordered
                .iter()
                .any(|existing| existing.matches_request_target(candidate))
            {
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

        for (index, candidate) in candidates.iter().enumerate() {
            let remaining_attempts = candidates.len() - index;
            let step_timeout = deadline
                .timeout_for_endpoint_attempt(Self::STEP_TIMEOUT_CAP, remaining_attempts)?;
            match test_send(candidate.clone(), step_timeout).await {
                Ok(()) => return Ok(candidate.clone()),
                Err(err) if Self::is_reachable_probe_error(&err) => {
                    return Ok(candidate.clone());
                }
                Err(_) => {}
            }
        }

        if let Some(fallback) = Self::fallback_probe_endpoint(candidates) {
            return Ok(fallback);
        }
        Err(AntigravityStatusProbeError::PortDetectionFailed(
            "no working API port found".to_owned(),
        ))
    }

    fn fallback_probe_endpoint(
        endpoints: &[AntigravityConnectionEndpoint],
    ) -> Option<AntigravityConnectionEndpoint> {
        if let Some(language_server) = endpoints
            .iter()
            .find(|e| e.source == EndpointSource::LanguageServer)
        {
            return Some(language_server.clone());
        }
        endpoints.first().cloned()
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
        if deadline.has_expired() {
            return Err(AntigravityStatusProbeError::TimedOut);
        }

        let mut pending: Vec<Option<std::pin::Pin<Box<Fut>>>> = processes
            .iter()
            .cloned()
            .map(|p| Some(Box::pin(fetch_process(p))))
            .collect();
        let mut results: Vec<
            Option<Result<AntigravityStatusSnapshot, AntigravityStatusProbeError>>,
        > = vec![None; processes.len()];

        std::future::poll_fn(|cx| {
            let mut all_done = true;
            for (index, slot) in pending.iter_mut().enumerate() {
                if let Some(fut) = slot {
                    match fut.as_mut().poll(cx) {
                        std::task::Poll::Ready(res) => {
                            results[index] = Some(res);
                            *slot = None;
                        }
                        std::task::Poll::Pending => {
                            all_done = false;
                        }
                    }
                }
            }
            if all_done {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;

        let mut snapshots = Vec::new();
        let mut last_error: Option<AntigravityStatusProbeError> = None;

        for outcome in results.into_iter().flatten() {
            match outcome {
                Ok(snapshot) => {
                    snapshots.push(snapshot);
                }
                Err(AntigravityStatusProbeError::Cancelled) => {
                    return Err(AntigravityStatusProbeError::Cancelled);
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
        let (quota_summary_ctx, quota_summary_deadline) =
            Self::fallback_reserving_request_context(context, deadline);
        if let Ok(quota_summary) = Self::make_parsed_request(
            RequestPayload::new(QUOTA_SUMMARY_PATH, Self::quota_summary_request_body()),
            &quota_summary_ctx,
            &quota_summary_deadline,
            Arc::clone(&send),
            Self::parse_quota_summary_response,
        )
        .await
            && quota_summary.has_known_quota_summary()
        {
            let identity_ctx = Self::identity_request_context(context);
            let identity = Self::make_parsed_request(
                RequestPayload::new(GET_USER_STATUS_PATH, Self::default_request_body()),
                &identity_ctx,
                deadline,
                Arc::clone(&send),
                Self::parse_user_status_response,
            )
            .await
            .ok();
            return Ok(quota_summary.with_identity(identity.as_ref()));
        }

        let (user_status_ctx, user_status_deadline) =
            Self::fallback_reserving_request_context(context, deadline);
        if let Ok(snapshot) = Self::make_parsed_request(
            RequestPayload::new(GET_USER_STATUS_PATH, Self::default_request_body()),
            &user_status_ctx,
            &user_status_deadline,
            Arc::clone(&send),
            Self::parse_user_status_response,
        )
        .await
        {
            return Ok(snapshot);
        }

        Self::make_parsed_request(
            RequestPayload::new(COMMAND_MODEL_CONFIG_PATH, Self::default_request_body()),
            context,
            deadline,
            send,
            Self::parse_command_model_response,
        )
        .await
    }

    fn fallback_reserving_request_context(
        context: &RequestContext,
        deadline: &OperationDeadline,
    ) -> (RequestContext, OperationDeadline) {
        let sub_deadline = deadline.fallback_reserving();
        let remaining = deadline.remaining().unwrap_or(Duration::ZERO);
        let attempt_budget = remaining / 2;
        let sub_context = RequestContext {
            endpoints: context.endpoints.clone(),
            timeout: context.timeout.min(attempt_budget),
        };
        (sub_context, sub_deadline)
    }

    fn identity_request_context(context: &RequestContext) -> RequestContext {
        RequestContext {
            endpoints: context.endpoints.clone(),
            timeout: context.timeout.min(Duration::from_secs(1)),
        }
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
            let Ok(step_timeout) = deadline.timeout_for_next_step(Some(context.timeout)) else {
                if last_error.is_none() {
                    last_error = Some(AntigravityStatusProbeError::TimedOut);
                }
                break;
            };
            match send(payload.clone(), endpoint.clone(), step_timeout).await {
                Ok(data) => match parse(&data) {
                    Ok(parsed) => return Ok(parsed),
                    Err(err) => {
                        last_error = Some(err);
                    }
                },
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            AntigravityStatusProbeError::ApiError("Invalid response".to_owned())
        }))
    }

    pub fn local_snapshot_score(snapshot: &AntigravityStatusSnapshot) -> i32 {
        let mut score = 0;
        if let Some(quota_summary) = &snapshot.quota_summary {
            let buckets: Vec<&AntigravityQuotaSummaryBucket> = quota_summary
                .groups
                .iter()
                .flat_map(|g| g.buckets.iter())
                .collect();
            let known_buckets = buckets
                .iter()
                .filter(|b| !b.disabled && b.remaining_fraction.is_some())
                .count() as i32;
            score += 1000;
            score += (quota_summary.groups.len() as i32) * 10;
            score += buckets.len() as i32;
            score += known_buckets * 20;
        } else {
            let known_rows = snapshot
                .model_quotas
                .iter()
                .filter(|q| q.remaining_fraction.is_some())
                .count() as i32;
            score += snapshot.model_quotas.len() as i32;
            score += known_rows * 10;
        }
        if snapshot.account_email.is_some() {
            score += 2;
        }
        if snapshot.account_plan.is_some() {
            score += 1;
        }
        score
    }

    pub fn preferred_local_snapshot_matching(
        snapshots: &[AntigravityStatusSnapshot],
        expected_account_email: Option<&str>,
    ) -> Option<AntigravityStatusSnapshot> {
        let mut candidates: Vec<&AntigravityStatusSnapshot> = snapshots.iter().collect();
        if let Some(expected) = Self::normalized_account_email(expected_account_email) {
            let matches: Vec<&AntigravityStatusSnapshot> = snapshots
                .iter()
                .filter(|s| {
                    let Some(found) = Self::normalized_account_email(s.account_email.as_deref())
                    else {
                        return false;
                    };
                    found.eq_ignore_ascii_case(&expected)
                })
                .collect();
            if !matches.is_empty() {
                candidates = matches;
            }
        }

        let mut best_snapshot: Option<&AntigravityStatusSnapshot> = None;
        let mut best_score = i32::MIN;
        for snapshot in candidates {
            let score = Self::local_snapshot_score(snapshot);
            if score > best_score {
                best_snapshot = Some(snapshot);
                best_score = score;
            }
        }
        best_snapshot.cloned()
    }

    pub fn preferred_local_snapshot(
        snapshots: &[AntigravityStatusSnapshot],
    ) -> Option<AntigravityStatusSnapshot> {
        Self::preferred_local_snapshot_matching(snapshots, None)
    }

    fn normalized_account_email(email: Option<&str>) -> Option<String> {
        let trimmed = email?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }

    // MARK: - Live LocalProbe + NetBroker Execution

    pub async fn fetch(
        local_probe: &LocalProbe,
        net: &Arc<NetBroker>,
        call: &BrokerCall,
        timeout: Duration,
        scope: ProcessScope,
    ) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
        Self::fetch_with_expected_account(local_probe, net, call, timeout, scope, None).await
    }

    pub async fn fetch_with_expected_account(
        local_probe: &LocalProbe,
        net: &Arc<NetBroker>,
        call: &BrokerCall,
        timeout: Duration,
        scope: ProcessScope,
        expected_account_email: Option<&str>,
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

        Self::preferred_local_snapshot_matching(&snapshots, expected_account_email)
            .ok_or(AntigravityStatusProbeError::NotRunning)
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

        let mut credentials = Vec::new();
        if endpoint.requires_csrf_token() && !endpoint.csrf_token.is_empty() {
            credentials.push((
                HeaderName::from_static("x-codeium-csrf-token"),
                SecretString::new(endpoint.csrf_token.clone()),
            ));
        }

        let req = NetRequest {
            method: Method::POST,
            url,
            headers,
            credentials,
            body: NetBody::Plain(body_bytes),
            timeout: Some(timeout),
            body_cap: None,
        };

        let response = net
            .send_loopback(call, req)
            .await
            .map_err(Self::map_net_error)?;
        if response.status != reqwest::StatusCode::OK {
            return Err(AntigravityStatusProbeError::ApiError(format!(
                "HTTP {}",
                response.status.as_u16()
            )));
        }
        Ok(response.body.to_vec())
    }

    fn map_probe_error(err: ProbeError) -> AntigravityStatusProbeError {
        match err {
            ProbeError::NoListeningPorts => AntigravityStatusProbeError::PortDetectionFailed(
                "no listening ports found".to_owned(),
            ),
            ProbeError::TimedOut => AntigravityStatusProbeError::TimedOut,
            ProbeError::Cancelled => AntigravityStatusProbeError::Cancelled,
            ProbeError::Unsupported | ProbeError::ProcUnavailable => {
                AntigravityStatusProbeError::PortDetectionFailed(err.to_string())
            }
        }
    }

    fn map_net_error(err: NetError) -> AntigravityStatusProbeError {
        match err {
            NetError::Transport(TransportClass::Timeout) => AntigravityStatusProbeError::TimedOut,
            NetError::Cancelled => AntigravityStatusProbeError::Cancelled,
            NetError::Http { status } => {
                AntigravityStatusProbeError::ApiError(format!("HTTP {status}"))
            }
            other => AntigravityStatusProbeError::ApiError(other.to_string()),
        }
    }

    pub fn quota_summary_request_body() -> serde_json::Value {
        json!({
            "forceRefresh": true,
        })
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
            return Err(AntigravityStatusProbeError::ApiError(invalid));
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
        let plan_name = user_status
            .user_tier
            .and_then(|t| t.preferred_name())
            .or_else(|| {
                user_status
                    .plan_status
                    .and_then(|p| p.plan_info)
                    .and_then(|info| info.into_summary().preferred_name())
            });
        Ok(AntigravityStatusSnapshot {
            model_quotas: models,
            account_email: user_status.email,
            account_plan: plan_name,
            source: AntigravitySnapshotSource::Local,
            quota_summary: None,
        })
    }

    pub fn parse_plan_info_summary(
        data: &[u8],
    ) -> Result<Option<AntigravityPlanInfoSummary>, AntigravityStatusProbeError> {
        let response: UserStatusResponse = serde_json::from_slice(data)
            .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))?;
        if let Some(invalid) = Self::invalid_code(response.code.as_ref()) {
            return Err(AntigravityStatusProbeError::ApiError(invalid));
        }
        let Some(user_status) = response.user_status else {
            return Err(AntigravityStatusProbeError::ParseFailed(
                "Missing userStatus".to_owned(),
            ));
        };
        Ok(user_status
            .plan_status
            .and_then(|p| p.plan_info)
            .map(PlanInfoWire::into_summary))
    }

    pub fn parse_command_model_response(
        data: &[u8],
    ) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
        let response: CommandModelConfigResponse = serde_json::from_slice(data)
            .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))?;
        if let Some(invalid) = Self::invalid_code(response.code.as_ref()) {
            return Err(AntigravityStatusProbeError::ApiError(invalid));
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
        let model_id = config
            .model_or_alias
            .as_ref()
            .and_then(|m| m.model.clone())?;
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
    #[serde(rename = "userTier")]
    user_tier: Option<UserTierWire>,
}

#[derive(Debug, Clone, Deserialize)]
struct UserTierWire {
    #[allow(dead_code)]
    id: Option<String>,
    name: Option<String>,
    #[allow(dead_code)]
    description: Option<String>,
}

impl UserTierWire {
    fn preferred_name(&self) -> Option<String> {
        let name = self.name.as_deref()?.trim();
        if name.is_empty() {
            None
        } else {
            Some(name.to_owned())
        }
    }
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
