// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexOAuth/CodexOAuthUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexSpendControlLimitMapping.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexSpendControlNumber.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexSpendControlsMonthlyUsage.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use ariadusage_core::pipeline::{ClassifiedError, FetchError, FetchInteraction};
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use jiff::Timestamp;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use reqwest::{Method, Url};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{
    DeclaredOrigins, NetBroker, NetError, NetRequest, OverrideError, OverridePolicy,
    is_loopback_host, validate_override,
};

use super::credentials::CodexHomeScope;
use super::credits::{
    CodexCreditLimitSnapshot, CodexRateLimitResetCredit, CodexRateLimitResetCreditStatus,
    CodexRateLimitResetCreditsSnapshot,
};
use super::normalizer::{AdditionalRateLimit, WindowSnapshot};

pub const DEFAULT_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/";
pub const CHATGPT_USAGE_PATH: &str = "/wham/usage";
pub const CODEX_USAGE_PATH: &str = "/api/codex/usage";
pub const RATE_LIMIT_RESET_CREDITS_PATH: &str = "/wham/rate-limit-reset-credits";
pub const SPEND_CONTROLS_MONTHLY_USAGE_PATH_SUFFIX: &str =
    "/spend-controls/current-user/monthly-usage";
pub const WORKSPACE_REMAINING_BALANCE_PATH_SUFFIX: &str = "/remaining_balance";

/// Permissive numeric representation shared by Codex RPC and OAuth spend-limit payloads.
pub struct CodexSpendControlNumber;

impl CodexSpendControlNumber {
    pub fn double_from_value(value: &Value) -> Option<f64> {
        if let Some(num) = value.as_f64() {
            return Some(num);
        }
        if let Some(int_val) = value.as_i64() {
            return Some(int_val as f64);
        }
        if let Some(s) = value.as_str() {
            return s.trim().parse::<f64>().ok();
        }
        None
    }

    pub fn integer_from_value(value: &Value) -> Option<i64> {
        if let Some(int_val) = value.as_i64() {
            return Some(int_val);
        }
        if let Some(num) = value.as_f64() {
            if !num.is_finite() {
                return None;
            }
            let truncated = num.trunc();
            if truncated >= (i64::MIN as f64) && truncated < (i64::MAX as f64) {
                return Some(truncated as i64);
            }
            return None;
        }
        if let Some(s) = value.as_str() {
            return s.trim().parse::<i64>().ok();
        }
        None
    }

    pub fn double(map: &Map<String, Value>, key: &str) -> Option<f64> {
        map.get(key).and_then(Self::double_from_value)
    }

    pub fn integer(map: &Map<String, Value>, key: &str) -> Option<i64> {
        map.get(key).and_then(Self::integer_from_value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpendControlLimitSnapshot {
    pub limit: Option<f64>,
    pub used: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<i64>,
}

impl SpendControlLimitSnapshot {
    pub fn codex_credit_limit_snapshot(
        &self,
        updated_at: Timestamp,
    ) -> Option<CodexCreditLimitSnapshot> {
        self.codex_credit_limit_snapshot_with_title("Monthly credit limit", updated_at)
    }

    pub fn codex_credit_limit_snapshot_with_title(
        &self,
        title: impl Into<String>,
        updated_at: Timestamp,
    ) -> Option<CodexCreditLimitSnapshot> {
        let limit = self.limit?;
        if !limit.is_finite() || limit <= 0.0 {
            return None;
        }
        let used = if let Some(used) = self.used {
            used
        } else if let Some(remaining_pct) = self.remaining_percent {
            limit * (100.0 - remaining_pct).clamp(0.0, 100.0) / 100.0
        } else {
            0.0
        };
        let remaining_percent = self
            .remaining_percent
            .unwrap_or_else(|| (100.0 - (used / limit * 100.0)).clamp(0.0, 100.0));
        let resets_at = self
            .resets_at
            .filter(|&v| v > 0)
            .and_then(|v| Timestamp::from_second(v).ok());

        Some(CodexCreditLimitSnapshot::new(
            title,
            used,
            limit,
            remaining_percent,
            resets_at,
            updated_at,
        ))
    }
}

impl<'de> Deserialize<'de> for SpendControlLimitSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let limit = CodexSpendControlNumber::double(&map, "limit");
        let used = CodexSpendControlNumber::double(&map, "used");
        let remaining_percent = CodexSpendControlNumber::double(&map, "remainingPercent")
            .or_else(|| CodexSpendControlNumber::double(&map, "remaining_percent"));
        let resets_at = CodexSpendControlNumber::integer(&map, "resetsAt")
            .or_else(|| CodexSpendControlNumber::integer(&map, "resets_at"))
            .or_else(|| CodexSpendControlNumber::integer(&map, "reset_at"));
        Ok(Self {
            limit,
            used,
            remaining_percent,
            resets_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PlanType {
    Guest,
    Free,
    Go,
    Plus,
    Pro,
    FreeWorkspace,
    Team,
    Business,
    Education,
    Quorum,
    K12,
    Enterprise,
    Edu,
    Unknown(String),
}

impl PlanType {
    pub fn from_raw(value: &str) -> Self {
        match value {
            "guest" => Self::Guest,
            "free" => Self::Free,
            "go" => Self::Go,
            "plus" => Self::Plus,
            "pro" => Self::Pro,
            "free_workspace" => Self::FreeWorkspace,
            "team" => Self::Team,
            "business" => Self::Business,
            "education" => Self::Education,
            "quorum" => Self::Quorum,
            "k12" => Self::K12,
            "enterprise" => Self::Enterprise,
            "edu" => Self::Edu,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn raw_value(&self) -> &str {
        match self {
            Self::Guest => "guest",
            Self::Free => "free",
            Self::Go => "go",
            Self::Plus => "plus",
            Self::Pro => "pro",
            Self::FreeWorkspace => "free_workspace",
            Self::Team => "team",
            Self::Business => "business",
            Self::Education => "education",
            Self::Quorum => "quorum",
            Self::K12 => "k12",
            Self::Enterprise => "enterprise",
            Self::Edu => "edu",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

impl<'de> Deserialize<'de> for PlanType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_raw(&raw))
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct RateLimitDetails {
    pub primary_window: Option<WindowSnapshot>,
    pub secondary_window: Option<WindowSnapshot>,
    pub individual_limit: Option<SpendControlLimitSnapshot>,
    #[serde(skip)]
    pub primary_window_decode_failed: bool,
    #[serde(skip)]
    pub secondary_window_decode_failed: bool,
}

impl RateLimitDetails {
    pub fn has_window_decode_failure(&self) -> bool {
        self.primary_window_decode_failed || self.secondary_window_decode_failed
    }
}

impl<'de> Deserialize<'de> for RateLimitDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let (primary_window, primary_window_decode_failed) =
            decode_window_from_map(&map, "primary_window");
        let (secondary_window, secondary_window_decode_failed) =
            decode_window_from_map(&map, "secondary_window");
        let individual_limit =
            decode_optional_from_map::<SpendControlLimitSnapshot>(&map, "individual_limit")
                .or_else(|| {
                    decode_optional_from_map::<SpendControlLimitSnapshot>(&map, "individualLimit")
                });

        Ok(Self {
            primary_window,
            secondary_window,
            individual_limit,
            primary_window_decode_failed,
            secondary_window_decode_failed,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreditDetails {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<f64>,
}

impl<'de> Deserialize<'de> for CreditDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let has_credits = map
            .get("has_credits")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let unlimited = map
            .get("unlimited")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let balance = map.get("balance").and_then(|val| {
            if let Some(num) = val.as_f64() {
                Some(num)
            } else if let Some(int_val) = val.as_i64() {
                Some(int_val as f64)
            } else if let Some(s) = val.as_str() {
                s.parse::<f64>().ok()
            } else {
                None
            }
        });
        Ok(Self {
            has_credits,
            unlimited,
            balance,
        })
    }
}

#[derive(Clone, PartialEq, Serialize)]
pub struct CodexUsageResponse {
    pub account_id: Option<String>,
    pub plan_type: Option<PlanType>,
    pub rate_limit: Option<RateLimitDetails>,
    pub credits: Option<CreditDetails>,
    pub individual_limit: Option<SpendControlLimitSnapshot>,
    pub spend_control_individual_limit: Option<SpendControlLimitSnapshot>,
    pub spend_control_present: bool,
    pub additional_rate_limits: Option<Vec<AdditionalRateLimit>>,
    #[serde(skip)]
    pub additional_rate_limits_decode_failed: bool,
}

impl fmt::Debug for CodexUsageResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexUsageResponse")
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "[redacted]"),
            )
            .field("plan_type", &self.plan_type)
            .field("rate_limit", &self.rate_limit)
            .field("credits", &self.credits)
            .field("individual_limit", &self.individual_limit)
            .field(
                "spend_control_individual_limit",
                &self.spend_control_individual_limit,
            )
            .field("spend_control_present", &self.spend_control_present)
            .field("additional_rate_limits", &self.additional_rate_limits)
            .field(
                "additional_rate_limits_decode_failed",
                &self.additional_rate_limits_decode_failed,
            )
            .finish()
    }
}

impl CodexUsageResponse {
    /// Credit-limit source precedence: response root, then `rate_limit`, then `spend_control`.
    pub fn resolved_individual_limit(&self) -> Option<&SpendControlLimitSnapshot> {
        self.individual_limit
            .as_ref()
            .or_else(|| {
                self.rate_limit
                    .as_ref()
                    .and_then(|rl| rl.individual_limit.as_ref())
            })
            .or(self.spend_control_individual_limit.as_ref())
    }
}

impl<'de> Deserialize<'de> for CodexUsageResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let account_id = decode_optional_from_map::<String>(&map, "account_id")
            .or_else(|| decode_optional_from_map::<String>(&map, "accountId"));
        let plan_type = decode_optional_from_map::<PlanType>(&map, "plan_type");
        let rate_limit = decode_optional_from_map::<RateLimitDetails>(&map, "rate_limit");
        let credits = decode_optional_from_map::<CreditDetails>(&map, "credits");
        let individual_limit =
            decode_optional_from_map::<SpendControlLimitSnapshot>(&map, "individual_limit")
                .or_else(|| {
                    decode_optional_from_map::<SpendControlLimitSnapshot>(&map, "individualLimit")
                });
        let spend_control_present =
            map.contains_key("spend_control") || map.contains_key("spendControl");
        let spend_control_individual_limit = map
            .get("spend_control")
            .or_else(|| map.get("spendControl"))
            .and_then(Value::as_object)
            .and_then(|sc| {
                decode_optional_from_map::<SpendControlLimitSnapshot>(sc, "individual_limit")
                    .or_else(|| {
                        decode_optional_from_map::<SpendControlLimitSnapshot>(sc, "individualLimit")
                    })
            });

        let (additional_rate_limits, additional_rate_limits_decode_failed) =
            decode_lossy_additional_rate_limits(&map);

        Ok(Self {
            account_id,
            plan_type,
            rate_limit,
            credits,
            individual_limit,
            spend_control_individual_limit,
            spend_control_present,
            additional_rate_limits,
            additional_rate_limits_decode_failed,
        })
    }
}

fn decode_optional_from_map<T: for<'de> Deserialize<'de>>(
    map: &Map<String, Value>,
    key: &str,
) -> Option<T> {
    let val = map.get(key)?;
    if val.is_null() {
        return None;
    }
    serde_json::from_value(val.clone()).ok()
}

fn decode_window_from_map(map: &Map<String, Value>, key: &str) -> (Option<WindowSnapshot>, bool) {
    let Some(val) = map.get(key) else {
        return (None, false);
    };
    if val.is_null() {
        return (None, false);
    }
    match serde_json::from_value::<WindowSnapshot>(val.clone()) {
        Ok(window) => (Some(window), false),
        Err(_) => (None, true),
    }
}

fn decode_lossy_additional_rate_limits(
    map: &Map<String, Value>,
) -> (Option<Vec<AdditionalRateLimit>>, bool) {
    let Some(val) = map.get("additional_rate_limits") else {
        return (None, false);
    };
    if val.is_null() {
        return (None, false);
    }
    let Some(arr) = val.as_array() else {
        return (None, true);
    };
    let mut entries = Vec::new();
    let mut any_failed = false;
    for item in arr {
        match serde_json::from_value::<AdditionalRateLimit>(item.clone()) {
            Ok(entry) => {
                if entry.has_window_decode_failure() {
                    any_failed = true;
                }
                entries.push(entry);
            }
            Err(_) => {
                any_failed = true;
            }
        }
    }
    (Some(entries), any_failed)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodexWorkspaceRemainingBalanceResponse {
    pub balance: Option<f64>,
}

impl<'de> Deserialize<'de> for CodexWorkspaceRemainingBalanceResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let decoded = map.get("balance").and_then(|val| {
            if let Some(num) = val.as_f64() {
                Some(num)
            } else if let Some(int_val) = val.as_i64() {
                Some(int_val as f64)
            } else if let Some(s) = val.as_str() {
                s.trim().parse::<f64>().ok()
            } else {
                None
            }
        });
        let balance = decoded.filter(|v| v.is_finite()).map(|v| v.max(0.0));
        Ok(Self { balance })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectiveMonthlyLimit {
    pub limit: Option<f64>,
    pub limit_was_unmappable: bool,
    pub enforcement_mode: Option<String>,
    pub enforcement_mode_was_unmappable: bool,
    pub limit_mode: Option<String>,
}

impl<'de> Deserialize<'de> for EffectiveMonthlyLimit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let (limit, limit_was_unmappable) = decode_flexible_double_result(&map, "limit");
        let (enforcement_mode, enforcement_mode_was_unmappable) =
            decode_optional_string_result(&map, "enforcement_mode");
        let limit_mode = map
            .get("limit_mode")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Ok(Self {
            limit,
            limit_was_unmappable,
            enforcement_mode,
            enforcement_mode_was_unmappable,
            limit_mode,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodexSpendControlsMonthlyUsageResponse {
    pub current_month_usage: Option<f64>,
    pub effective_monthly_limit: Option<EffectiveMonthlyLimit>,
    current_month_usage_was_unmappable: bool,
}

impl CodexSpendControlsMonthlyUsageResponse {
    const INACTIVE_ENFORCEMENT_MODES: &'static [&'static str] =
        &["none", "off", "disabled", "no_limit"];

    pub fn monthly_limit_mapping_failed(&self) -> bool {
        let mapped_limit = self.effective_monthly_limit.as_ref().and_then(|e| e.limit);
        let limit_was_unmappable = self
            .effective_monthly_limit
            .as_ref()
            .is_some_and(|e| e.limit_was_unmappable);
        if !limit_was_unmappable && mapped_limit.unwrap_or(0.0) <= 0.0 {
            return false;
        }
        if self
            .effective_monthly_limit
            .as_ref()
            .is_some_and(|e| e.enforcement_mode_was_unmappable)
        {
            return true;
        }
        if self.has_inactive_enforcement() {
            return false;
        }
        if limit_was_unmappable {
            return true;
        }
        if mapped_limit.unwrap_or(0.0) <= 0.0 {
            return false;
        }
        self.current_month_usage_was_unmappable
    }

    fn has_inactive_enforcement(&self) -> bool {
        let Some(mode) = self
            .effective_monthly_limit
            .as_ref()
            .and_then(|e| e.enforcement_mode.as_deref())
        else {
            return false;
        };
        let lower = mode.to_lowercase();
        Self::INACTIVE_ENFORCEMENT_MODES.contains(&lower.as_str())
    }

    pub fn codex_credit_limit_snapshot(
        &self,
        updated_at: Timestamp,
    ) -> Option<CodexCreditLimitSnapshot> {
        let limit = self.effective_monthly_limit.as_ref()?.limit?;
        if !limit.is_finite() || limit <= 0.0 || self.has_inactive_enforcement() {
            return None;
        }
        let used = self.current_month_usage.unwrap_or(0.0).max(0.0);
        let remaining_percent = (100.0 - (used / limit * 100.0)).clamp(0.0, 100.0);
        Some(CodexCreditLimitSnapshot::new(
            "Monthly credit limit",
            used,
            limit,
            remaining_percent,
            None,
            updated_at,
        ))
    }
}

impl<'de> Deserialize<'de> for CodexSpendControlsMonthlyUsageResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let (current_month_usage, current_month_usage_was_unmappable) =
            decode_flexible_double_result(&map, "current_month_usage");
        let effective_monthly_limit = match map.get("effective_monthly_limit") {
            None | Some(Value::Null) => None,
            Some(val) => Some(
                serde_json::from_value::<EffectiveMonthlyLimit>(val.clone())
                    .map_err(serde::de::Error::custom)?,
            ),
        };
        Ok(Self {
            current_month_usage,
            effective_monthly_limit,
            current_month_usage_was_unmappable,
        })
    }
}

fn decode_flexible_double_result(map: &Map<String, Value>, key: &str) -> (Option<f64>, bool) {
    let Some(val) = map.get(key) else {
        return (None, false);
    };
    if val.is_null() {
        return (None, false);
    }
    if let Some(num) = val.as_f64() {
        return if num.is_finite() {
            (Some(num), false)
        } else {
            (None, true)
        };
    }
    if let Some(int_val) = val.as_i64() {
        return (Some(int_val as f64), false);
    }
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if let Ok(parsed) = trimmed.parse::<f64>()
            && parsed.is_finite()
        {
            return (Some(parsed), false);
        }
        return (None, true);
    }
    (None, true)
}

fn decode_optional_string_result(map: &Map<String, Value>, key: &str) -> (Option<String>, bool) {
    let Some(val) = map.get(key) else {
        return (None, false);
    };
    if val.is_null() {
        return (None, false);
    }
    if let Some(s) = val.as_str() {
        return (Some(s.to_owned()), false);
    }
    (None, true)
}

pub struct CodexSpendControlsMonthlyUsageGate;

impl CodexSpendControlsMonthlyUsageGate {
    pub fn should_fetch(response: &CodexUsageResponse) -> bool {
        if response
            .resolved_individual_limit()
            .and_then(|l| l.codex_credit_limit_snapshot(Timestamp::UNIX_EPOCH))
            .is_some()
        {
            return false;
        }
        if !response.spend_control_present {
            return false;
        }
        match &response.plan_type {
            Some(
                PlanType::Guest | PlanType::Free | PlanType::Go | PlanType::Plus | PlanType::Pro,
            ) => false,
            Some(
                PlanType::Team
                | PlanType::Business
                | PlanType::Education
                | PlanType::Quorum
                | PlanType::K12
                | PlanType::Enterprise
                | PlanType::Edu
                | PlanType::FreeWorkspace
                | PlanType::Unknown(_),
            )
            | None => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexOAuthFetchError {
    #[error("Codex OAuth token expired or invalid. Run `codex login` to re-authenticate.")]
    Unauthorized,
    #[error("Invalid response from Codex usage API.")]
    InvalidResponse,
    #[error("{}", format_server_error(*.0, .1.as_deref()))]
    ServerError(u16, Option<String>),
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Codex OAuth request was cancelled.")]
    Cancelled,
}

fn format_server_error(code: u16, message: Option<&str>) -> String {
    if let Some(msg) = message
        && !msg.is_empty()
    {
        format!("Codex API error {code}: {msg}")
    } else {
        format!("Codex API error {code}.")
    }
}

impl CodexOAuthFetchError {
    pub fn into_fetch_error(self) -> FetchError {
        match &self {
            Self::Unauthorized => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::AuthenticationExpired,
                self.to_string(),
            )),
            Self::InvalidResponse => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::ParseFailure,
                self.to_string(),
            )),
            Self::ServerError(status, _) => {
                let kind = if *status == 429 {
                    ProviderErrorKind::RateLimited
                } else if *status >= 500 {
                    ProviderErrorKind::ProviderUnavailable
                } else {
                    ProviderErrorKind::ApiFailure
                };
                FetchError::Classified(ClassifiedError::new(kind, self.to_string()))
            }
            Self::NetworkError(_) => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::NetworkFailure,
                self.to_string(),
            )),
            Self::Cancelled => FetchError::Cancelled,
        }
    }
}

#[derive(Deserialize)]
struct RateLimitResetCreditsWireResponse {
    credits: Vec<RateLimitResetCreditWireItem>,
    available_count: i64,
}

#[derive(Deserialize)]
struct RateLimitResetCreditWireItem {
    id: String,
    reset_type: String,
    status: CodexRateLimitResetCreditStatus,
    granted_at: String,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    redeem_started_at: Option<String>,
    #[serde(default)]
    redeemed_at: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

impl RateLimitResetCreditWireItem {
    fn into_model(self) -> Result<CodexRateLimitResetCredit, CodexOAuthFetchError> {
        let granted_at = parse_iso8601(&self.granted_at)?;
        let expires_at = match self.expires_at {
            Some(s) => Some(parse_iso8601(&s)?),
            None => None,
        };
        let redeem_started_at = match self.redeem_started_at {
            Some(s) => Some(parse_iso8601(&s)?),
            None => None,
        };
        let redeemed_at = match self.redeemed_at {
            Some(s) => Some(parse_iso8601(&s)?),
            None => None,
        };
        Ok(CodexRateLimitResetCredit::new(
            &self.id,
            self.reset_type,
            self.status,
            granted_at,
            expires_at,
            redeem_started_at,
            redeemed_at,
            self.title,
            self.description,
        ))
    }
}

fn parse_iso8601(raw: &str) -> Result<Timestamp, CodexOAuthFetchError> {
    raw.trim()
        .parse::<Timestamp>()
        .map_err(|_| CodexOAuthFetchError::InvalidResponse)
}

pub struct CodexOAuthUsageFetcher;

impl CodexOAuthUsageFetcher {
    pub fn resolve_usage_url(
        env: &BTreeMap<String, String>,
        config_contents: Option<&str>,
        home_dir: Option<&Path>,
    ) -> Url {
        let base_url = Self::resolve_chatgpt_base_url(env, config_contents, home_dir);
        let normalized = Self::normalize_chatgpt_base_url(&base_url);
        let path = if normalized.contains("/backend-api") {
            CHATGPT_USAGE_PATH
        } else {
            CODEX_USAGE_PATH
        };
        let full = format!("{normalized}{path}");
        Url::parse(&full).unwrap_or_else(|_| {
            Url::parse(&format!("{DEFAULT_CHATGPT_BASE_URL}{CHATGPT_USAGE_PATH}"))
                .expect("valid default usage URL")
        })
    }

    pub fn resolve_rate_limit_reset_credits_url(
        env: &BTreeMap<String, String>,
        config_contents: Option<&str>,
        home_dir: Option<&Path>,
    ) -> Url {
        let base_url = Self::resolve_chatgpt_base_url(env, config_contents, home_dir);
        let normalized = Self::normalize_chatgpt_base_url(&base_url);
        let full = format!("{normalized}{RATE_LIMIT_RESET_CREDITS_PATH}");
        Url::parse(&full).unwrap_or_else(|_| {
            Url::parse(&format!(
                "{DEFAULT_CHATGPT_BASE_URL}{RATE_LIMIT_RESET_CREDITS_PATH}"
            ))
            .expect("valid default reset credits URL")
        })
    }

    pub fn resolve_workspace_remaining_balance_url(
        env: &BTreeMap<String, String>,
        config_contents: Option<&str>,
        home_dir: Option<&Path>,
        account_id: &str,
    ) -> Option<Url> {
        let base_url = Self::resolve_chatgpt_base_url(env, config_contents, home_dir);
        let normalized = Self::normalize_chatgpt_base_url(&base_url);
        if !normalized.contains("/backend-api") {
            return None;
        }
        let encoded = Self::encode_path_segment(account_id)?;
        Url::parse(&format!(
            "{normalized}/accounts/{encoded}{WORKSPACE_REMAINING_BALANCE_PATH_SUFFIX}"
        ))
        .ok()
    }

    pub fn resolve_spend_controls_monthly_usage_url(
        env: &BTreeMap<String, String>,
        config_contents: Option<&str>,
        home_dir: Option<&Path>,
        account_id: &str,
    ) -> Option<Url> {
        let base_url = Self::resolve_chatgpt_base_url(env, config_contents, home_dir);
        let normalized = Self::normalize_chatgpt_base_url(&base_url);
        if !normalized.contains("/backend-api") {
            return None;
        }
        let encoded = Self::encode_path_segment(account_id)?;
        Url::parse(&format!(
            "{normalized}/accounts/{encoded}{SPEND_CONTROLS_MONTHLY_USAGE_PATH_SUFFIX}"
        ))
        .ok()
    }

    fn encode_path_segment(segment: &str) -> Option<String> {
        if segment.is_empty() {
            return None;
        }
        let mut out = String::new();
        for b in segment.bytes() {
            let is_unreserved = b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'-' | b'.'
                        | b'_'
                        | b'~'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                        | b':'
                        | b'@'
                );
            if is_unreserved {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        if out.is_empty() { None } else { Some(out) }
    }

    fn resolve_chatgpt_base_url(
        env: &BTreeMap<String, String>,
        config_contents: Option<&str>,
        home_dir: Option<&Path>,
    ) -> String {
        if let Some(contents) = config_contents
            && let Some(parsed) = Self::parse_chatgpt_base_url(contents)
        {
            return parsed;
        }
        if let Some(contents) = Self::load_config_contents(env, home_dir)
            && let Some(parsed) = Self::parse_chatgpt_base_url(&contents)
            && (parsed.is_empty() || Self::validate_chatgpt_base_url(&parsed).is_ok())
        {
            return parsed;
        }
        DEFAULT_CHATGPT_BASE_URL.to_owned()
    }

    pub fn validate_chatgpt_base_url(value: &str) -> Result<Url, OverrideError> {
        let trimmed = value.trim();
        if !trimmed.starts_with("https://") && !trimmed.starts_with("http://") {
            return Err(OverrideError::UnsupportedScheme);
        }
        let normalized = Self::normalize_chatgpt_base_url(trimmed);
        let url = validate_override(&normalized, &OverridePolicy::AllowLoopbackHttp)?;
        if url.query().is_some() || url.fragment().is_some() {
            return Err(OverrideError::Malformed);
        }
        let host = url.host_str().ok_or(OverrideError::MissingHost)?;
        if !is_loopback_host(host) {
            validate_override(&normalized, &Self::provider_override_policy())?;
        }
        Ok(url)
    }

    fn provider_override_policy() -> OverridePolicy {
        OverridePolicy::ProviderOwnedOnly {
            allowed_hosts: vec![
                "chatgpt.com".to_owned(),
                "chat.openai.com".to_owned(),
                "api.openai.com".to_owned(),
            ],
            allowed_domain_suffixes: vec!["chatgpt.com".to_owned(), "openai.com".to_owned()],
        }
    }

    fn resolve_validated_chatgpt_base_url(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Result<(String, Option<Url>), CodexOAuthFetchError> {
        if let Some(contents) = Self::load_config_contents(env, home_dir)
            && let Some(raw) = Self::parse_chatgpt_base_url(&contents)
        {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                let validated = Self::validate_chatgpt_base_url(trimmed)
                    .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
                let normalized = Self::normalize_chatgpt_base_url(trimmed);
                return Ok((normalized, Some(validated)));
            }
        }
        Ok((
            Self::normalize_chatgpt_base_url(DEFAULT_CHATGPT_BASE_URL),
            None,
        ))
    }

    pub fn normalize_chatgpt_base_url(value: &str) -> String {
        let mut trimmed = value.trim().to_owned();
        if trimmed.is_empty() {
            trimmed = DEFAULT_CHATGPT_BASE_URL.to_owned();
        }
        while trimmed.ends_with('/') {
            trimmed.pop();
        }
        if (trimmed.starts_with("https://chatgpt.com")
            || trimmed.starts_with("https://chat.openai.com"))
            && !trimmed.contains("/backend-api")
        {
            trimmed.push_str("/backend-api");
        }
        trimmed
    }

    pub fn parse_chatgpt_base_url(contents: &str) -> Option<String> {
        for raw_line in contents.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key_raw, val_raw)) = line.split_once('=') else {
                continue;
            };
            if key_raw.trim() != "chatgpt_base_url" {
                continue;
            }
            let mut value = val_raw.trim();
            if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
                || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
            {
                value = &value[1..value.len() - 1];
            }
            return Some(value.trim().to_owned());
        }
        None
    }

    fn load_config_contents(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Option<String> {
        const MAX_CONFIG_FILE_BYTES: u64 = 1024 * 1024;

        let root = CodexHomeScope::ambient_home_url(env, home_dir)?;
        let config_path = root.join("config.toml");
        let meta = fs::symlink_metadata(&config_path).ok()?;
        if !meta.file_type().is_file() || meta.len() > MAX_CONFIG_FILE_BYTES {
            return None;
        }
        let file = fs::File::open(&config_path).ok()?;
        let file_meta = file.metadata().ok()?;
        if !file_meta.file_type().is_file() || file_meta.len() > MAX_CONFIG_FILE_BYTES {
            return None;
        }
        let mut buf = String::new();
        file.take(MAX_CONFIG_FILE_BYTES + 1)
            .read_to_string(&mut buf)
            .ok()?;
        if buf.len() as u64 > MAX_CONFIG_FILE_BYTES {
            return None;
        }
        Some(buf)
    }

    pub fn decode_usage_response(data: &[u8]) -> Result<CodexUsageResponse, CodexOAuthFetchError> {
        serde_json::from_slice(data).map_err(|_| CodexOAuthFetchError::InvalidResponse)
    }

    pub fn decode_spend_controls_monthly_usage_response(
        data: &[u8],
    ) -> Result<CodexSpendControlsMonthlyUsageResponse, CodexOAuthFetchError> {
        serde_json::from_slice(data).map_err(|_| CodexOAuthFetchError::InvalidResponse)
    }

    pub fn decode_workspace_remaining_balance_response(
        data: &[u8],
    ) -> Result<CodexWorkspaceRemainingBalanceResponse, CodexOAuthFetchError> {
        serde_json::from_slice(data).map_err(|_| CodexOAuthFetchError::InvalidResponse)
    }

    pub fn decode_rate_limit_reset_credits(
        data: &[u8],
        now: Timestamp,
    ) -> Result<CodexRateLimitResetCreditsSnapshot, CodexOAuthFetchError> {
        let payload: RateLimitResetCreditsWireResponse =
            serde_json::from_slice(data).map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        let available_count = usize::try_from(payload.available_count)
            .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        let mut credits = Vec::with_capacity(payload.credits.len());
        for item in payload.credits {
            credits.push(item.into_model()?);
        }
        Ok(CodexRateLimitResetCreditsSnapshot::new(
            credits,
            available_count,
            now,
        ))
    }

    pub fn build_usage_request(
        url: Url,
        access_token: &str,
        account_id: Option<&str>,
    ) -> Result<NetRequest, CodexOAuthFetchError> {
        let mut headers = Self::common_headers();
        if let Some(acct) = account_id.filter(|s| !s.is_empty()) {
            let val =
                HeaderValue::from_str(acct).map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
            headers.insert(HeaderName::from_static("chatgpt-account-id"), val);
        }
        Self::build_request_with_headers(url, access_token, headers, Duration::from_secs(30))
    }

    pub fn build_rate_limit_reset_credits_request(
        url: Url,
        access_token: &str,
        account_id: Option<&str>,
        timeout: Duration,
    ) -> Result<NetRequest, CodexOAuthFetchError> {
        let mut headers = Self::common_headers();
        headers.insert(
            HeaderName::from_static("openai-beta"),
            HeaderValue::from_static("codex-1"),
        );
        headers.insert(
            HeaderName::from_static("originator"),
            HeaderValue::from_static("Codex Desktop"),
        );
        if let Some(acct) = account_id.filter(|s| !s.is_empty()) {
            let val =
                HeaderValue::from_str(acct).map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
            headers.insert(HeaderName::from_static("chatgpt-account-id"), val);
        }
        Self::build_request_with_headers(url, access_token, headers, timeout)
    }

    pub fn build_account_scoped_get_request(
        url: Url,
        access_token: &str,
        account_id: &str,
        timeout: Duration,
    ) -> Result<NetRequest, CodexOAuthFetchError> {
        let mut headers = Self::common_headers();
        let val =
            HeaderValue::from_str(account_id).map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        headers.insert(HeaderName::from_static("chatgpt-account-id"), val);
        Self::build_request_with_headers(url, access_token, headers, timeout)
    }

    fn common_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("AriadUsage"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers
    }

    fn build_request_with_headers(
        url: Url,
        access_token: &str,
        headers: HeaderMap,
        timeout: Duration,
    ) -> Result<NetRequest, CodexOAuthFetchError> {
        HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        let mut req = NetRequest::new(Method::GET, url);
        req.headers = headers;
        req.credentials.push((
            AUTHORIZATION,
            SecretString::new(format!("Bearer {access_token}")),
        ));
        req.timeout = Some(timeout);
        Ok(req)
    }

    pub async fn fetch_usage(
        net: &NetBroker,
        access_token: &str,
        account_id: Option<&str>,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Result<CodexUsageResponse, CodexOAuthFetchError> {
        Self::fetch_usage_with_call(net, access_token, account_id, env, home_dir, None).await
    }

    pub async fn fetch_usage_with_call(
        net: &NetBroker,
        access_token: &str,
        account_id: Option<&str>,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        call: Option<&BrokerCall>,
    ) -> Result<CodexUsageResponse, CodexOAuthFetchError> {
        let (normalized, validated_override) =
            Self::resolve_validated_chatgpt_base_url(env, home_dir)?;
        let path = if normalized.contains("/backend-api") {
            CHATGPT_USAGE_PATH
        } else {
            CODEX_USAGE_PATH
        };
        let url = Url::parse(&format!("{normalized}{path}"))
            .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        let req = Self::build_usage_request(url, access_token, account_id)?;
        let body = Self::perform_request(net, validated_override.as_ref(), req, call).await?;
        Self::decode_usage_response(&body)
    }

    pub async fn fetch_rate_limit_reset_credits(
        net: &NetBroker,
        access_token: &str,
        account_id: Option<&str>,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        timeout: Duration,
        now: Timestamp,
    ) -> Result<CodexRateLimitResetCreditsSnapshot, CodexOAuthFetchError> {
        Self::fetch_rate_limit_reset_credits_with_call(
            net,
            access_token,
            account_id,
            env,
            home_dir,
            timeout,
            now,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_rate_limit_reset_credits_with_call(
        net: &NetBroker,
        access_token: &str,
        account_id: Option<&str>,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        timeout: Duration,
        now: Timestamp,
        call: Option<&BrokerCall>,
    ) -> Result<CodexRateLimitResetCreditsSnapshot, CodexOAuthFetchError> {
        let (normalized, validated_override) =
            Self::resolve_validated_chatgpt_base_url(env, home_dir)?;
        let url = Url::parse(&format!("{normalized}{RATE_LIMIT_RESET_CREDITS_PATH}"))
            .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        let req =
            Self::build_rate_limit_reset_credits_request(url, access_token, account_id, timeout)?;
        let body = Self::perform_request(net, validated_override.as_ref(), req, call).await?;
        Self::decode_rate_limit_reset_credits(&body, now)
    }

    pub async fn fetch_workspace_remaining_balance(
        net: &NetBroker,
        access_token: &str,
        account_id: &str,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        timeout: Duration,
    ) -> Result<CodexWorkspaceRemainingBalanceResponse, CodexOAuthFetchError> {
        Self::fetch_workspace_remaining_balance_with_call(
            net,
            access_token,
            account_id,
            env,
            home_dir,
            timeout,
            None,
        )
        .await
    }

    pub async fn fetch_workspace_remaining_balance_with_call(
        net: &NetBroker,
        access_token: &str,
        account_id: &str,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        timeout: Duration,
        call: Option<&BrokerCall>,
    ) -> Result<CodexWorkspaceRemainingBalanceResponse, CodexOAuthFetchError> {
        let (normalized, validated_override) =
            Self::resolve_validated_chatgpt_base_url(env, home_dir)?;
        if !normalized.contains("/backend-api") {
            return Err(CodexOAuthFetchError::InvalidResponse);
        }
        let encoded =
            Self::encode_path_segment(account_id).ok_or(CodexOAuthFetchError::InvalidResponse)?;
        let url = Url::parse(&format!(
            "{normalized}/accounts/{encoded}{WORKSPACE_REMAINING_BALANCE_PATH_SUFFIX}"
        ))
        .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        let req = Self::build_account_scoped_get_request(url, access_token, account_id, timeout)?;
        let body = Self::perform_request(net, validated_override.as_ref(), req, call).await?;
        Self::decode_workspace_remaining_balance_response(&body)
    }

    pub async fn fetch_spend_controls_monthly_usage(
        net: &NetBroker,
        access_token: &str,
        account_id: &str,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        timeout: Duration,
    ) -> Result<CodexSpendControlsMonthlyUsageResponse, CodexOAuthFetchError> {
        Self::fetch_spend_controls_monthly_usage_with_call(
            net,
            access_token,
            account_id,
            env,
            home_dir,
            timeout,
            None,
        )
        .await
    }

    pub async fn fetch_spend_controls_monthly_usage_with_call(
        net: &NetBroker,
        access_token: &str,
        account_id: &str,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        timeout: Duration,
        call: Option<&BrokerCall>,
    ) -> Result<CodexSpendControlsMonthlyUsageResponse, CodexOAuthFetchError> {
        let (normalized, validated_override) =
            Self::resolve_validated_chatgpt_base_url(env, home_dir)?;
        if !normalized.contains("/backend-api") {
            return Err(CodexOAuthFetchError::InvalidResponse);
        }
        let encoded =
            Self::encode_path_segment(account_id).ok_or(CodexOAuthFetchError::InvalidResponse)?;
        let url = Url::parse(&format!(
            "{normalized}/accounts/{encoded}{SPEND_CONTROLS_MONTHLY_USAGE_PATH_SUFFIX}"
        ))
        .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        let req = Self::build_account_scoped_get_request(url, access_token, account_id, timeout)?;
        let body = Self::perform_request(net, validated_override.as_ref(), req, call).await?;
        Self::decode_spend_controls_monthly_usage_response(&body)
    }

    fn declared_origins(
        validated_override: Option<&Url>,
    ) -> Result<DeclaredOrigins, CodexOAuthFetchError> {
        let mut urls = vec![
            Url::parse("https://chatgpt.com").expect("valid static origin"),
            Url::parse("https://chat.openai.com").expect("valid static origin"),
            Url::parse("https://api.openai.com").expect("valid static origin"),
        ];
        if let Some(override_url) = validated_override {
            urls.push(override_url.clone());
        }
        DeclaredOrigins::from_urls(urls).map_err(|_| CodexOAuthFetchError::InvalidResponse)
    }

    async fn perform_request(
        net: &NetBroker,
        validated_override: Option<&Url>,
        req: NetRequest,
        call: Option<&BrokerCall>,
    ) -> Result<Vec<u8>, CodexOAuthFetchError> {
        if let Some(call) = call
            && call.cancel.is_cancelled()
        {
            return Err(CodexOAuthFetchError::Cancelled);
        }
        let origins = Self::declared_origins(validated_override)?;
        let default_call;
        let effective_call = if let Some(call) = call {
            call
        } else {
            default_call = BrokerCall {
                interaction: FetchInteraction::UserInitiated,
                cancel: CancellationToken::new(),
                request_id: "codex.oauth".to_owned(),
            };
            &default_call
        };
        let resp = net
            .send_response(effective_call, &origins, req)
            .await
            .map_err(|err| match err {
                NetError::Cancelled => CodexOAuthFetchError::Cancelled,
                NetError::Transport(_) => {
                    CodexOAuthFetchError::NetworkError("transport".to_owned())
                }
                other => CodexOAuthFetchError::NetworkError(other.to_string()),
            })?;
        match resp.status.as_u16() {
            200..=299 => Ok(resp.body().to_vec()),
            401 | 403 => Err(CodexOAuthFetchError::Unauthorized),
            status => {
                let message = extract_structured_error_message(resp.body());
                Err(CodexOAuthFetchError::ServerError(status, message))
            }
        }
    }
}

fn extract_structured_error_message(body: &[u8]) -> Option<String> {
    let val: Value = serde_json::from_slice(body).ok()?;
    let obj = val.as_object()?;
    if let Some(err_obj) = obj.get("error").and_then(Value::as_object) {
        if let Some(msg) = err_obj
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(msg.to_owned());
        }
        if let Some(code) = err_obj
            .get("code")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(code.to_owned());
        }
    }
    if let Some(detail) = obj
        .get("detail")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(detail.to_owned());
    }
    None
}
