// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::Arc;

use ariadusage_core::hosts::ProviderHosts;
use ariadusage_core::model::{
    NamedRateWindow, ProviderCostSnapshot, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_core::pipeline::{ClassifiedError, FetchError, TransportClass};
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue, USER_AGENT};
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{DeclaredOrigins, NetBroker, NetError, NetRequest};

use super::identity::ClaudeAccountIdentity;
use super::plan::ClaudePlan;
use super::rate_limit_gate::{ClaudeOAuthUsageRateLimitGate, parse_retry_after};
use super::reset_credits::{
    ClaudeCloudCreditsSnapshot, ClaudeRateLimitResetCredits, OAuthCedarEmberPayload,
    OAuthIguanaNecktiePayload,
};
use super::scoped_weekly::{
    ClaudeScopedWeeklyLimitMapper, OAuthScopedLimitEntry, parse_iso8601_timestamp,
};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const USAGE_PATH: &str = "/api/oauth/usage";
pub const PROFILE_PATH: &str = "/api/oauth/profile";
pub const BETA_HEADER_KEY: &str = "anthropic-beta";
pub const BETA_HEADER_VALUE: &str = "oauth-2025-04-20";
pub const DEFAULT_CLAUDE_CLI_VERSION: &str = "2.1.0";

const ROUTINE_KEYS: &[&str] = &[
    "seven_day_routines",
    "seven_day_claude_routines",
    "claude_routines",
    "routines",
    "routine",
    "seven_day_cowork",
    "cowork",
];

/// Decoded quota window from the Claude OAuth usage response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthUsageWindow {
    pub utilization: Option<f64>,
    #[serde(rename = "resets_at")]
    pub resets_at: Option<String>,
}

/// Decoded `extra_usage` spend tracking block from the Claude OAuth usage response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthExtraUsage {
    #[serde(rename = "is_enabled")]
    pub is_enabled: Option<bool>,
    #[serde(rename = "monthly_limit")]
    pub monthly_limit: Option<f64>,
    #[serde(rename = "used_credits")]
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
    pub currency: Option<String>,
}

/// Decoded payload from `/api/oauth/usage`.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthUsageResponse {
    pub five_hour: Option<OAuthUsageWindow>,
    pub seven_day: Option<OAuthUsageWindow>,
    pub seven_day_oauth_apps: Option<OAuthUsageWindow>,
    pub seven_day_sonnet: Option<OAuthUsageWindow>,
    pub seven_day_opus: Option<OAuthUsageWindow>,
    pub limits: Option<Vec<OAuthScopedLimitEntry>>,
    pub routines: Option<OAuthUsageWindow>,
    pub extra_usage: Option<OAuthExtraUsage>,
    pub cedar_ember: Option<OAuthCedarEmberPayload>,
    pub iguana_necktie: Option<OAuthIguanaNecktiePayload>,
}

/// Detailed error from a Claude OAuth usage fetch before conversion to `FetchError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeOAuthFetchError {
    #[error("request cancelled")]
    Cancelled,
    #[error("Claude OAuth token expired or invalid")]
    Unauthorized,
    #[error("Claude OAuth token does not have user:profile scope")]
    MissingUserProfileScope,
    #[error("Claude OAuth access forbidden")]
    Forbidden,
    #[error("Claude OAuth usage rate limited")]
    RateLimited {
        retry_after: Option<Timestamp>,
        cooldown_until: Timestamp,
        cached_gate: bool,
    },
    #[error("Claude OAuth server error (HTTP {status})")]
    ServerError { status: u16 },
    #[error("Claude OAuth HTTP error (HTTP {status})")]
    HttpError { status: u16 },
    #[error("Claude OAuth response could not be parsed")]
    InvalidResponse,
    #[error("Claude OAuth network failure")]
    Network(Option<TransportClass>),
}

impl ClaudeOAuthFetchError {
    /// Converts a Claude OAuth fetch error into the core pipeline `FetchError`.
    pub fn into_fetch_error(self) -> FetchError {
        match self {
            Self::Cancelled => FetchError::Cancelled,
            Self::Unauthorized => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::AuthenticationExpired,
                "Claude OAuth token expired or invalid",
            )),
            Self::MissingUserProfileScope => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::PermissionDenied,
                "Claude OAuth token does not have user:profile scope",
            )),
            Self::Forbidden => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::PermissionDenied,
                "Claude OAuth access forbidden",
            )),
            Self::RateLimited { .. } => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::RateLimited,
                "Claude OAuth usage rate limited",
            )),
            Self::ServerError { .. } => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::ProviderUnavailable,
                "Claude OAuth server error",
            )),
            Self::HttpError { .. } => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::ApiFailure,
                "Claude OAuth request failed",
            )),
            Self::InvalidResponse => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::ParseFailure,
                "Claude OAuth response could not be parsed",
            )),
            Self::Network(transport) => {
                let mut err = ClassifiedError::new(
                    ProviderErrorKind::NetworkFailure,
                    "Claude OAuth network request failed",
                );
                if let Some(t) = transport {
                    err = err.with_transport(t);
                }
                FetchError::Classified(err)
            }
        }
    }
}

/// Endpoint and client configuration for Claude OAuth HTTP requests.
#[derive(Debug, Clone)]
pub struct ClaudeOAuthEndpointConfig {
    pub base_url: Url,
    pub declared_origins: DeclaredOrigins,
    pub cli_version: String,
}

impl Default for ClaudeOAuthEndpointConfig {
    fn default() -> Self {
        let base_url = Url::parse(DEFAULT_BASE_URL).expect("valid default Claude base URL");
        let hosts = ProviderHosts::new(["api.anthropic.com"]).expect("valid host");
        let declared_origins = DeclaredOrigins::from_provider_hosts(&hosts);
        Self {
            base_url,
            declared_origins,
            cli_version: DEFAULT_CLAUDE_CLI_VERSION.to_owned(),
        }
    }
}

impl ClaudeOAuthEndpointConfig {
    /// Creates an endpoint config pointing at a custom base URL (for `httpmock` tests).
    pub fn with_base_url(base_url: Url) -> Result<Self, crate::brokers::net::OriginError> {
        let declared_origins = DeclaredOrigins::from_urls([base_url.clone()])?;
        Ok(Self {
            base_url,
            declared_origins,
            cli_version: DEFAULT_CLAUDE_CLI_VERSION.to_owned(),
        })
    }

    pub fn with_cli_version(mut self, cli_version: impl Into<String>) -> Self {
        self.cli_version = cli_version.into();
        self
    }

    fn usage_url(&self, include_cedar_ember: bool) -> Result<Url, ClaudeOAuthFetchError> {
        let mut url = self
            .base_url
            .join(USAGE_PATH)
            .map_err(|_| ClaudeOAuthFetchError::Network(None))?;
        if include_cedar_ember {
            url.query_pairs_mut().append_pair("cedar_ember", "1");
        }
        Ok(url)
    }

    fn profile_url(&self) -> Result<Url, ClaudeOAuthFetchError> {
        self.base_url
            .join(PROFILE_PATH)
            .map_err(|_| ClaudeOAuthFetchError::Network(None))
    }

    pub fn primary_user_agent(&self) -> String {
        let v = normalized_claude_code_version(Some(&self.cli_version))
            .unwrap_or(DEFAULT_CLAUDE_CLI_VERSION);
        format!("claude-cli/{v} (external, cli)")
    }

    pub fn fallback_user_agent(&self) -> String {
        ClaudeOAuthUsageFetcher::claude_code_user_agent(Some(&self.cli_version))
    }
}

fn normalized_claude_code_version(version_string: Option<&str>) -> Option<&str> {
    let raw = version_string?.trim();
    if raw.is_empty() {
        return None;
    }
    let token = raw.split_whitespace().next().unwrap_or(raw).trim();
    if token.is_empty()
        || token
            .bytes()
            .any(|b| b < 0x20 || b == 0x7f || b == b'\r' || b == b'\n')
    {
        None
    } else {
        Some(token)
    }
}

/// Parsed and mapped Claude OAuth usage snapshot before wrapping into `FetchResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeOAuthMappedUsage {
    pub snapshot: UsageSnapshot,
    pub reset_credits: Option<ClaudeRateLimitResetCredits>,
    pub cloud_credits: Option<ClaudeCloudCreditsSnapshot>,
    pub history_owner_id: Option<String>,
}

/// Stateless parser and HTTP fetcher for Claude's OAuth usage and profile endpoints.
pub struct ClaudeOAuthUsageFetcher;

impl ClaudeOAuthUsageFetcher {
    /// Formats the `claude-code/<version>` fallback `User-Agent`, extracting the first
    /// whitespace-delimited version token or falling back to `2.1.0`.
    pub fn claude_code_user_agent(version_string: Option<&str>) -> String {
        let version =
            normalized_claude_code_version(version_string).unwrap_or(DEFAULT_CLAUDE_CLI_VERSION);
        format!("claude-code/{version}")
    }

    /// Formats the `claude-code/<version>` fallback `User-Agent`, invoking `version_detector`
    /// only when `detect_claude_version` is `true`.
    pub fn claude_code_user_agent_with_detector<F>(
        detect_claude_version: bool,
        version_detector: F,
    ) -> String
    where
        F: FnOnce() -> Option<String>,
    {
        let detected = if detect_claude_version {
            version_detector()
        } else {
            None
        };
        Self::claude_code_user_agent(detected.as_deref())
    }

    /// Decodes a raw JSON payload from `/api/oauth/usage`.
    pub fn decode_usage_response(data: &[u8]) -> Result<OAuthUsageResponse, ClaudeOAuthFetchError> {
        let value: Value =
            serde_json::from_slice(data).map_err(|_| ClaudeOAuthFetchError::InvalidResponse)?;
        let obj = value
            .as_object()
            .ok_or(ClaudeOAuthFetchError::InvalidResponse)?;

        let five_hour = decode_window_field(obj, "five_hour");
        let seven_day = decode_window_field(obj, "seven_day");
        let seven_day_oauth_apps = decode_window_field(obj, "seven_day_oauth_apps");
        let seven_day_sonnet = decode_window_field(obj, "seven_day_sonnet");
        let seven_day_opus = decode_window_field(obj, "seven_day_opus");

        let limits = match obj.get("limits") {
            Some(Value::Null) | None => None,
            Some(val) => Some(OAuthScopedLimitEntry::parse_lossy_array(val)),
        };

        let mut routines = None;
        for key in ROUTINE_KEYS {
            if let Some(val) = obj.get(*key)
                && !val.is_null()
                && let Ok(parsed) = serde_json::from_value::<OAuthUsageWindow>(val.clone())
            {
                routines = Some(parsed);
                break;
            }
        }

        let extra_usage = obj
            .get("extra_usage")
            .filter(|v| !v.is_null())
            .and_then(|val| serde_json::from_value::<OAuthExtraUsage>(val.clone()).ok());

        // Optional feature blocks degrade gracefully if malformed or oversized.
        let cedar_ember = obj
            .get("cedar_ember")
            .and_then(OAuthCedarEmberPayload::from_json_value);
        let iguana_necktie = obj
            .get("iguana_necktie")
            .and_then(|v| serde_json::from_value::<OAuthIguanaNecktiePayload>(v.clone()).ok());

        Ok(OAuthUsageResponse {
            five_hour,
            seven_day,
            seven_day_oauth_apps,
            seven_day_sonnet,
            seven_day_opus,
            limits,
            routines,
            extra_usage,
            cedar_ember,
            iguana_necktie,
        })
    }

    /// Maps a decoded `OAuthUsageResponse` into a `ClaudeOAuthMappedUsage` and `UsageSnapshot`.
    #[allow(clippy::too_many_arguments)]
    pub fn map_usage_response(
        response: &OAuthUsageResponse,
        subscription_type: Option<&str>,
        rate_limit_tier: Option<&str>,
        account_identity: Option<&ClaudeAccountIdentity>,
        history_owner_id: Option<String>,
        include_optional_usage: bool,
        updated_at: Timestamp,
    ) -> Result<ClaudeOAuthMappedUsage, ClassifiedError> {
        let primary = make_rate_window(response.five_hour.as_ref(), 300)
            .or_else(|| make_rate_window(response.seven_day.as_ref(), 10080))
            .or_else(|| make_rate_window(response.seven_day_oauth_apps.as_ref(), 10080))
            .or_else(|| make_rate_window(response.seven_day_sonnet.as_ref(), 10080))
            .or_else(|| make_rate_window(response.seven_day_opus.as_ref(), 10080));

        let secondary = make_rate_window(response.seven_day.as_ref(), 10080);
        let tertiary = make_rate_window(
            response
                .seven_day_sonnet
                .as_ref()
                .or(response.seven_day_opus.as_ref()),
            10080,
        );

        let mut extra_windows =
            ClaudeScopedWeeklyLimitMapper::named_windows(response.limits.as_deref().unwrap_or(&[]));
        if let Some(routines_window) = make_rate_window(response.routines.as_ref(), 10080) {
            extra_windows.push(NamedRateWindow::new(
                "claude-routines",
                "Daily Routines",
                routines_window,
            ));
        }
        let extra_rate_windows = if extra_windows.is_empty() {
            None
        } else {
            Some(extra_windows)
        };

        let plan = ClaudePlan::from_oauth_credentials(subscription_type, rate_limit_tier);
        let extra_usage_period = if primary.is_none() || plan == Some(ClaudePlan::Enterprise) {
            "Spend limit"
        } else {
            "Monthly cap"
        };
        let provider_cost = make_extra_usage_cost(
            response.extra_usage.as_ref(),
            updated_at,
            extra_usage_period,
        );

        if primary.is_none()
            && provider_cost
                .as_ref()
                .and_then(ProviderCostSnapshot::spend_limit_window)
                .is_none()
        {
            return Err(ClassifiedError::new(
                ProviderErrorKind::ParseFailure,
                "Claude OAuth response missing session data",
            ));
        }

        let reset_credits =
            ClaudeRateLimitResetCredits::from_payload(response.cedar_ember.as_ref(), updated_at);
        let cloud_credits =
            ClaudeCloudCreditsSnapshot::from_payload(response.iguana_necktie.as_ref());

        let mut details = reset_credits
            .as_ref()
            .map_or_else(Vec::new, ClaudeRateLimitResetCredits::detail_sections);
        if include_optional_usage && let Some(cc) = &cloud_credits {
            details.extend(cc.detail_sections(updated_at));
        }

        let login_method = ClaudePlan::oauth_login_method(subscription_type, rate_limit_tier);
        let account_email = account_identity.and_then(|i| i.email.clone());
        let account_id = account_identity.and_then(ClaudeAccountIdentity::verified_owner_id);

        let identity = if login_method.is_some() || account_email.is_some() || account_id.is_some()
        {
            Some(ProviderIdentitySnapshot {
                provider_id: ProviderId::new("claude").ok(),
                account_email,
                account_organization: None,
                login_method,
                account_id,
            })
        } else {
            None
        };

        let snapshot = UsageSnapshot::new(
            primary,
            secondary,
            tertiary,
            extra_rate_windows,
            provider_cost,
            details,
            None,
            None,
            updated_at,
            identity,
            Confidence::Exact,
        )
        .map_err(|_| {
            ClassifiedError::new(
                ProviderErrorKind::ParseFailure,
                "Claude OAuth snapshot validation failed",
            )
        })?;

        Ok(ClaudeOAuthMappedUsage {
            snapshot,
            reset_credits,
            cloud_credits,
            history_owner_id,
        })
    }

    /// Executes the HTTP usage request against `/api/oauth/usage` through `NetBroker`,
    /// including rate-limit gate checks and the `claude-code/<version>` inventory fallback retry.
    pub async fn fetch_usage_response(
        net: &NetBroker,
        endpoint: &ClaudeOAuthEndpointConfig,
        rate_limit_gate: &ClaudeOAuthUsageRateLimitGate,
        access_token: &SecretString,
        call: &BrokerCall,
        now: Timestamp,
    ) -> Result<OAuthUsageResponse, ClaudeOAuthFetchError> {
        let token_str = access_token.expose_secret().trim();
        if token_str.is_empty() {
            return Err(ClaudeOAuthFetchError::Unauthorized);
        }

        if call.interaction != ariadusage_core::pipeline::FetchInteraction::UserInitiated
            && let Some(cooldown_until) = rate_limit_gate.active_cooldown_until(token_str, now)
        {
            return Err(ClaudeOAuthFetchError::RateLimited {
                retry_after: Some(cooldown_until),
                cooldown_until,
                cached_gate: true,
            });
        }

        let primary_url = endpoint.usage_url(true)?;
        let primary_ua = endpoint.primary_user_agent();
        let primary_req = build_oauth_get_request(primary_url, token_str, &primary_ua)?;
        let mut response = net
            .send_response(call, &endpoint.declared_origins, primary_req)
            .await
            .map_err(map_net_error)?;

        if should_retry_with_claude_code_user_agent(response.status.as_u16(), response.body()) {
            if call.cancel.is_cancelled() {
                return Err(ClaudeOAuthFetchError::Cancelled);
            }
            let fallback_url = endpoint.usage_url(false)?;
            let fallback_ua = endpoint.fallback_user_agent();
            let fallback_req = build_oauth_get_request(fallback_url, token_str, &fallback_ua)?;
            response = net
                .send_response(call, &endpoint.declared_origins, fallback_req)
                .await
                .map_err(map_net_error)?;
        }

        match response.status.as_u16() {
            200 => {
                rate_limit_gate.clear(token_str);
                Self::decode_usage_response(response.body())
            }
            401 => Err(ClaudeOAuthFetchError::Unauthorized),
            403 => {
                let body_text = String::from_utf8_lossy(response.body());
                if body_text.contains("user:profile") {
                    Err(ClaudeOAuthFetchError::MissingUserProfileScope)
                } else {
                    Err(ClaudeOAuthFetchError::Forbidden)
                }
            }
            429 => {
                let retry_after_raw = response
                    .headers
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok());
                let retry_after = parse_retry_after(retry_after_raw, now);
                let cooldown_until = rate_limit_gate.record_rate_limit(token_str, retry_after, now);
                Err(ClaudeOAuthFetchError::RateLimited {
                    retry_after,
                    cooldown_until,
                    cached_gate: false,
                })
            }
            status @ 500..=599 => Err(ClaudeOAuthFetchError::ServerError { status }),
            status => Err(ClaudeOAuthFetchError::HttpError { status }),
        }
    }

    /// Fetches optional account identity from `/api/oauth/profile`.
    ///
    /// Non-cancellation failures return `Ok(None)` so quota reporting never fails because of
    /// profile enrichment. Cancellation is propagated immediately.
    pub async fn fetch_profile_identity(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeOAuthEndpointConfig,
        access_token: &SecretString,
        call: &BrokerCall,
    ) -> Result<Option<ClaudeAccountIdentity>, ClaudeOAuthFetchError> {
        if call.cancel.is_cancelled() {
            return Err(ClaudeOAuthFetchError::Cancelled);
        }
        let token_str = access_token.expose_secret().trim();
        if token_str.is_empty() {
            return Ok(None);
        }
        let Ok(url) = endpoint.profile_url() else {
            return Ok(None);
        };
        let req = build_oauth_profile_request(url, token_str);

        match net
            .send_response(call, &endpoint.declared_origins, req)
            .await
        {
            Ok(resp) if resp.status.as_u16() == 200 => {
                Ok(ClaudeAccountIdentity::decode_profile(resp.body()))
            }
            Ok(_) => Ok(None),
            Err(NetError::Cancelled) => Err(ClaudeOAuthFetchError::Cancelled),
            Err(_) => {
                if call.cancel.is_cancelled() {
                    Err(ClaudeOAuthFetchError::Cancelled)
                } else {
                    Ok(None)
                }
            }
        }
    }
}

fn should_retry_with_claude_code_user_agent(status: u16, body: &[u8]) -> bool {
    match status {
        400 => true,
        403 => !String::from_utf8_lossy(body).contains("user:profile"),
        _ => false,
    }
}

fn build_oauth_get_request(
    url: Url,
    access_token: &str,
    user_agent: &str,
) -> Result<NetRequest, ClaudeOAuthFetchError> {
    let mut req = NetRequest::new(Method::GET, url);
    req.headers
        .insert(ACCEPT, HeaderValue::from_static("application/json"));
    req.headers
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    req.headers.insert(
        HeaderName::from_static(BETA_HEADER_KEY),
        HeaderValue::from_static(BETA_HEADER_VALUE),
    );
    let ua_val =
        HeaderValue::from_str(user_agent).map_err(|_| ClaudeOAuthFetchError::Network(None))?;
    req.headers.insert(USER_AGENT, ua_val);
    req.credentials.push((
        AUTHORIZATION,
        SecretString::new(format!("Bearer {access_token}")),
    ));
    Ok(req)
}

fn build_oauth_profile_request(url: Url, access_token: &str) -> NetRequest {
    let mut req = NetRequest::new(Method::GET, url);
    req.headers
        .insert(ACCEPT, HeaderValue::from_static("application/json"));
    req.headers
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    req.credentials.push((
        AUTHORIZATION,
        SecretString::new(format!("Bearer {access_token}")),
    ));
    req
}

fn map_net_error(err: NetError) -> ClaudeOAuthFetchError {
    match err {
        NetError::Cancelled => ClaudeOAuthFetchError::Cancelled,
        NetError::Transport(t) => ClaudeOAuthFetchError::Network(Some(t)),
        NetError::Http { status: 401 } => ClaudeOAuthFetchError::Unauthorized,
        NetError::Http { status: 403 } => ClaudeOAuthFetchError::Forbidden,
        NetError::Http {
            status: status @ 500..=599,
        } => ClaudeOAuthFetchError::ServerError { status },
        NetError::Http { status } => ClaudeOAuthFetchError::HttpError { status },
        _ => ClaudeOAuthFetchError::Network(None),
    }
}

fn decode_window_field(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<OAuthUsageWindow> {
    obj.get(key)
        .filter(|v| !v.is_null())
        .and_then(|val| serde_json::from_value::<OAuthUsageWindow>(val.clone()).ok())
}

fn make_rate_window(window: Option<&OAuthUsageWindow>, window_minutes: i64) -> Option<RateWindow> {
    let window = window?;
    let utilization = window.utilization.filter(|v| v.is_finite())?;
    let resets_at = window
        .resets_at
        .as_deref()
        .and_then(parse_iso8601_timestamp);
    RateWindow::new(
        utilization,
        Some(window_minutes),
        resets_at,
        None,
        None,
        false,
    )
    .ok()
}

fn make_extra_usage_cost(
    extra: Option<&OAuthExtraUsage>,
    updated_at: Timestamp,
    period: &str,
) -> Option<ProviderCostSnapshot> {
    let extra = extra?;
    if extra.is_enabled != Some(true) {
        return None;
    }
    let used = extra.used_credits.filter(|v| v.is_finite())?;
    let limit = extra.monthly_limit.filter(|v| v.is_finite())?;
    let currency = extra
        .currency
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("USD");
    let mut snapshot = ProviderCostSnapshot::new(used / 100.0, limit / 100.0, currency, updated_at);
    snapshot.period = Some(period.to_owned());
    Some(snapshot)
}
