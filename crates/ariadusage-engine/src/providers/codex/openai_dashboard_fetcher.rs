// Ported from CodexBar Sources/CodexBarCore/OpenAIWeb/OpenAIDashboardFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/OpenAIWeb/OpenAIDashboardFetcher+SessionAuthorization.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/OpenAIWeb/OpenAIDashboardFetcher+ReturnableData.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::fmt;
use std::time::{Duration, Instant};

use ariadusage_core::cookie::normalize;
use ariadusage_core::hosts::ProviderHosts;
use ariadusage_core::model::window::{NamedRateWindow, RateWindow};
use ariadusage_core::pipeline::{ClassifiedError, FetchError, TransportClass};
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use jiff::Timestamp;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, AUTHORIZATION, COOKIE, HeaderValue, USER_AGENT};
use reqwest::{Method, Url};
use serde::Deserialize;

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{
    DeclaredOrigins, NetBroker, NetError, NetRequest, OriginError, RedactedHeaders,
};

use super::credits::{CodexCreditLimitSnapshot, CreditEvent};
use super::dashboard_authority::normalize_workspace_account_id;
use super::identity::CodexIdentityResolver;
use super::normalizer::{CodexAdditionalRateLimitMapper, WindowSnapshot};
use super::openai_dashboard_cookie_importer::OpenAIDashboardBrowserCookieImporter;
use super::openai_dashboard_models::{
    OpenAIDashboardDailyBreakdown, OpenAIDashboardSnapshot, OpenAISubscriptionFetchResult,
    OpenAISubscriptionMetadataParser,
};
use super::openai_dashboard_parser::OpenAIDashboardParser;
use super::usage_fetcher::{
    CodexOAuthUsageFetcher, CodexSpendControlsMonthlyUsageGate,
    CodexSpendControlsMonthlyUsageResponse, CodexUsageResponse,
    CodexWorkspaceRemainingBalanceResponse,
};

pub const DEFAULT_CHATGPT_BASE_URL: &str = "https://chatgpt.com";
pub const CHATGPT_HOST: &str = "chatgpt.com";
pub const DEFAULT_CREDITS_PURCHASE_URL: &str = "https://chatgpt.com/codex/settings/usage";
pub const DEFAULT_DASHBOARD_API_TIMEOUT: Duration = Duration::from_secs(4);
pub const DEFAULT_SESSION_AUTH_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_SUBSCRIPTIONS_API_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone)]
pub struct OpenAIDashboardEndpointConfig {
    pub base_url: Url,
    pub declared_origins: DeclaredOrigins,
}

impl Default for OpenAIDashboardEndpointConfig {
    fn default() -> Self {
        let base_url = Url::parse(DEFAULT_CHATGPT_BASE_URL).expect("valid default ChatGPT URL");
        let hosts = ProviderHosts::new([CHATGPT_HOST]).expect("valid ChatGPT host");
        let declared_origins = DeclaredOrigins::from_provider_hosts(&hosts);
        Self {
            base_url,
            declared_origins,
        }
    }
}

impl OpenAIDashboardEndpointConfig {
    pub fn with_base_url(base_url: Url) -> Result<Self, OriginError> {
        let declared_origins = DeclaredOrigins::from_urls([base_url.clone()])?;
        Ok(Self {
            base_url,
            declared_origins,
        })
    }

    pub fn usage_url(&self) -> Result<Url, OpenAIDashboardFetchError> {
        self.join_path("/backend-api/wham/usage")
    }

    pub fn auth_session_url(&self) -> Result<Url, OpenAIDashboardFetchError> {
        self.join_path("/api/auth/session")
    }

    pub fn me_url(&self) -> Result<Url, OpenAIDashboardFetchError> {
        self.join_path("/backend-api/me")
    }

    pub fn spend_controls_monthly_usage_url(
        &self,
        account_id: &str,
    ) -> Result<Url, OpenAIDashboardFetchError> {
        let encoded = Self::encode_path_segment(account_id);
        if encoded.is_empty() {
            return Err(OpenAIDashboardFetchError::NoDashboardData);
        }
        self.join_path(&format!(
            "/backend-api/accounts/{encoded}/spend-controls/current-user/monthly-usage"
        ))
    }

    pub fn workspace_remaining_balance_url(
        &self,
        account_id: &str,
    ) -> Result<Url, OpenAIDashboardFetchError> {
        let encoded = Self::encode_path_segment(account_id);
        if encoded.is_empty() {
            return Err(OpenAIDashboardFetchError::NoDashboardData);
        }
        self.join_path(&format!(
            "/backend-api/accounts/{encoded}/remaining_balance"
        ))
    }

    pub fn subscriptions_url(
        &self,
        account_id: Option<&str>,
    ) -> Result<Url, OpenAIDashboardFetchError> {
        let mut url = self.join_path("/backend-api/subscriptions")?;
        if let Some(account_id) = account_id.map(str::trim).filter(|s| !s.is_empty()) {
            url.query_pairs_mut().append_pair("account_id", account_id);
        }
        Ok(url)
    }

    fn join_path(&self, path: &str) -> Result<Url, OpenAIDashboardFetchError> {
        let base = self.base_url.as_str().trim_end_matches('/');
        let rel = path.trim_start_matches('/');
        Url::parse(&format!("{base}/{rel}"))
            .map_err(|_| OpenAIDashboardFetchError::NetworkError(None))
    }

    fn encode_path_segment(raw: &str) -> String {
        let trimmed = raw.trim();
        let mut encoded = String::with_capacity(trimmed.len());
        for byte in trimmed.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                encoded.push(char::from(byte));
            } else {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
        encoded
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenAIDashboardFetchError {
    #[error("Codex web dashboard requires a signed-in ChatGPT session.")]
    LoginRequired,
    #[error("Codex web dashboard returned no usage data.")]
    NoDashboardData,
    #[error("OpenAI cookie header is invalid.")]
    InvalidCookieHeader,
    #[error("Codex web dashboard failed with HTTP {status}.")]
    ServerError { status: u16 },
    #[error("Codex web dashboard request timed out.")]
    Timeout,
    #[error("Codex web dashboard network error.")]
    NetworkError(Option<TransportClass>),
    #[error("request cancelled")]
    Cancelled,
}

impl OpenAIDashboardFetchError {
    pub fn into_classified_error(self) -> ClassifiedError {
        let msg = self.to_string();
        match self {
            Self::Cancelled => ClassifiedError::new(ProviderErrorKind::NetworkFailure, msg)
                .with_transport(TransportClass::Cancelled),
            Self::LoginRequired | Self::InvalidCookieHeader => {
                ClassifiedError::new(ProviderErrorKind::AuthenticationExpired, msg)
            }
            Self::NoDashboardData => ClassifiedError::new(ProviderErrorKind::ParseFailure, msg),
            Self::ServerError { status: 429 } => {
                ClassifiedError::new(ProviderErrorKind::RateLimited, msg)
            }
            Self::ServerError { status: 500..=599 } => {
                ClassifiedError::new(ProviderErrorKind::ProviderUnavailable, msg)
            }
            Self::ServerError { .. } => ClassifiedError::new(ProviderErrorKind::ApiFailure, msg),
            Self::Timeout => ClassifiedError::new(ProviderErrorKind::NetworkFailure, msg)
                .with_transport(TransportClass::Timeout),
            Self::NetworkError(transport) => {
                let mut err = ClassifiedError::new(ProviderErrorKind::NetworkFailure, msg);
                if let Some(t) = transport {
                    err = err.with_transport(t);
                }
                err
            }
        }
    }

    pub fn into_fetch_error(self) -> FetchError {
        if matches!(
            self,
            Self::Cancelled | Self::NetworkError(Some(TransportClass::Cancelled))
        ) {
            FetchError::Cancelled
        } else {
            FetchError::Classified(self.into_classified_error())
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SessionAuthorization {
    pub bearer_token: SecretString,
    pub verified_email: String,
}

impl fmt::Debug for SessionAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionAuthorization")
            .field("bearer_token", &"[redacted]")
            .field("verified_email", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DashboardAPIData {
    pub account_id: Option<String>,
    pub primary_limit: Option<RateWindow>,
    pub secondary_limit: Option<RateWindow>,
    pub extra_rate_windows: Option<Vec<NamedRateWindow>>,
    pub credits_remaining: Option<f64>,
    pub credits_available: Option<bool>,
    pub balance_is_workspace: Option<bool>,
    pub codex_credit_limit: Option<CodexCreditLimitSnapshot>,
    pub account_plan: Option<String>,
}

impl DashboardAPIData {
    pub fn has_usage_data(&self) -> bool {
        self.primary_limit.is_some()
            || self.secondary_limit.is_some()
            || self
                .extra_rate_windows
                .as_ref()
                .is_some_and(|w| !w.is_empty())
            || self.credits_remaining.is_some()
            || self.credits_available == Some(true)
            || self.codex_credit_limit.is_some()
            || self.account_plan.as_ref().is_some_and(|p| !p.is_empty())
    }

    /// Converts direct HTTPS JSON API data into `OpenAIDashboardSnapshot`.
    ///
    /// The four page-JS-only fields (`code_review_remaining_percent` with `code_review_limit`,
    /// `credit_events` with `daily_breakdown`, `usage_breakdown`, and `credits_purchase_url`)
    /// remain absent (`None` / `Vec::new()`) because they require a macOS-app WebView (P8).
    pub fn to_snapshot(
        &self,
        signed_in_email: Option<String>,
        updated_at: Timestamp,
    ) -> OpenAIDashboardSnapshot {
        OpenAIDashboardSnapshot::new(
            signed_in_email,
            self.account_id.clone(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            self.primary_limit.clone(),
            self.secondary_limit.clone(),
            self.extra_rate_windows.clone(),
            self.credits_remaining,
            self.credits_available,
            self.balance_is_workspace,
            self.codex_credit_limit.clone(),
            self.account_plan.clone(),
            None,
            None,
            updated_at,
        )
    }
}

#[derive(Clone, PartialEq)]
pub struct DashboardAPIResponse {
    pub api_data: DashboardAPIData,
    pub verified_signed_in_email: Option<String>,
}

impl fmt::Debug for DashboardAPIResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DashboardAPIResponse")
            .field("api_data", &self.api_data)
            .field(
                "has_verified_signed_in_email",
                &self.verified_signed_in_email.is_some(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DashboardScrapePayload {
    pub login_required: bool,
    pub did_timeout: bool,
    pub body_text: String,
    pub html: String,
    pub signed_in_email: Option<String>,
    pub auth_status: Option<String>,
    pub rows: Vec<[String; 3]>,
    pub usage_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
    pub credits_purchase_url: Option<String>,
    pub has_usage_limits: bool,
    pub has_credits_remaining: bool,
    pub has_credits_table: bool,
}

#[derive(Deserialize)]
struct AuthSessionPayload {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    user: Option<AuthSessionUser>,
}

#[derive(Deserialize)]
struct AuthSessionUser {
    email: Option<String>,
}

pub struct OpenAIDashboardFetcher;

impl OpenAIDashboardFetcher {
    pub fn dashboard_api_data(response: &CodexUsageResponse, now: Timestamp) -> DashboardAPIData {
        let primary = response
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.primary_window.as_ref())
            .and_then(WindowSnapshot::to_rate_window_unchecked);
        let secondary = response
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.secondary_window.as_ref())
            .and_then(WindowSnapshot::to_rate_window_unchecked);
        let extra = CodexAdditionalRateLimitMapper::extra_rate_windows(
            response.additional_rate_limits.as_deref(),
            now,
        );
        let extra_rate_windows = (!extra.is_empty()).then_some(extra);
        let credits_remaining = response.credits.as_ref().and_then(|c| c.balance);
        let credits_available = response
            .credits
            .as_ref()
            .map(|c| c.has_credits && !c.unlimited);
        let codex_credit_limit = response
            .resolved_individual_limit()
            .and_then(|l| l.codex_credit_limit_snapshot(now));
        let account_plan = response
            .plan_type
            .as_ref()
            .map(|p| p.raw_value().to_owned())
            .filter(|p| !p.is_empty());

        DashboardAPIData {
            account_id: response.account_id.clone(),
            primary_limit: primary,
            secondary_limit: secondary,
            extra_rate_windows,
            credits_remaining,
            credits_available,
            balance_is_workspace: None,
            codex_credit_limit,
            account_plan,
        }
    }

    pub async fn fetch_dashboard_session_authorization(
        net: &NetBroker,
        endpoint: &OpenAIDashboardEndpointConfig,
        cookie_header: &str,
        deadline: Option<Instant>,
        now_fn: &(dyn Fn() -> Instant + Send + Sync),
        call: Option<&BrokerCall>,
    ) -> Option<SessionAuthorization> {
        let timeout = Self::remaining_timeout(deadline, DEFAULT_SESSION_AUTH_TIMEOUT, now_fn())?;
        let url = endpoint.auth_session_url().ok()?;
        let (data, status) =
            Self::execute_get(net, endpoint, url, cookie_header, None, None, timeout, call).await?;
        if !(200..=299).contains(&status) {
            return None;
        }
        let decoded: AuthSessionPayload = serde_json::from_slice(&data).ok()?;
        let raw_email = decoded.user.and_then(|u| u.email)?;
        let email = raw_email.trim().to_owned();
        if email.is_empty() {
            return None;
        }
        let raw_token = decoded.access_token?;
        let token = raw_token.trim().to_owned();
        if token.is_empty() || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return None;
        }
        Some(SessionAuthorization {
            bearer_token: SecretString::new(token),
            verified_email: email,
        })
    }

    pub async fn fetch_dashboard_usage_response(
        net: &NetBroker,
        endpoint: &OpenAIDashboardEndpointConfig,
        cookie_header: &str,
        deadline: Option<Instant>,
        now_fn: &(dyn Fn() -> Instant + Send + Sync),
        call: Option<&BrokerCall>,
    ) -> Option<(Vec<u8>, u16, Option<SessionAuthorization>)> {
        let timeout = Self::remaining_timeout(deadline, DEFAULT_DASHBOARD_API_TIMEOUT, now_fn())?;
        let url = endpoint.usage_url().ok()?;
        let (data, status) =
            Self::execute_get(net, endpoint, url, cookie_header, None, None, timeout, call).await?;

        if status == 401
            && let Some(authorization) = Self::fetch_dashboard_session_authorization(
                net,
                endpoint,
                cookie_header,
                deadline,
                now_fn,
                call,
            )
            .await
        {
            let retry_timeout =
                Self::remaining_timeout(deadline, DEFAULT_DASHBOARD_API_TIMEOUT, now_fn())?;
            let retry_url = endpoint.usage_url().ok()?;
            let (retry_data, retry_status) = Self::execute_get(
                net,
                endpoint,
                retry_url,
                cookie_header,
                Some(authorization.bearer_token.expose_secret()),
                None,
                retry_timeout,
                call,
            )
            .await?;
            return Some((retry_data, retry_status, Some(authorization)));
        }

        Some((data, status, None))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_dashboard_api_response(
        net: &NetBroker,
        endpoint: &OpenAIDashboardEndpointConfig,
        cookie_header: &str,
        deadline: Option<Instant>,
        now: Timestamp,
        now_fn: &(dyn Fn() -> Instant + Send + Sync),
        call: Option<&BrokerCall>,
    ) -> Option<DashboardAPIResponse> {
        let (data, status, authorization) = Self::fetch_dashboard_usage_response(
            net,
            endpoint,
            cookie_header,
            deadline,
            now_fn,
            call,
        )
        .await?;
        if !(200..=299).contains(&status) {
            return None;
        }
        let decoded = CodexOAuthUsageFetcher::decode_usage_response(&data).ok()?;
        let mut result = Self::dashboard_api_data(&decoded, now);
        let bearer = authorization
            .as_ref()
            .map(|a| a.bearer_token.expose_secret());

        if let Some(monthly_limit) = Self::fetch_spend_controls_monthly_usage_if_needed(
            net,
            endpoint,
            &decoded,
            cookie_header,
            bearer,
            deadline,
            now,
            now_fn,
            call,
        )
        .await
        {
            result.codex_credit_limit = Some(monthly_limit);
        }

        if result.credits_remaining.is_none()
            && let Some(workspace_balance) = Self::fetch_workspace_remaining_balance_if_needed(
                net,
                endpoint,
                &decoded,
                cookie_header,
                bearer,
                deadline,
                now_fn,
                call,
            )
            .await
        {
            result.credits_remaining = Some(workspace_balance);
            result.balance_is_workspace = Some(true);
        }

        Some(DashboardAPIResponse {
            api_data: result,
            verified_signed_in_email: authorization.map(|a| a.verified_email),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_spend_controls_monthly_usage_if_needed(
        net: &NetBroker,
        endpoint: &OpenAIDashboardEndpointConfig,
        response: &CodexUsageResponse,
        cookie_header: &str,
        bearer_token: Option<&str>,
        deadline: Option<Instant>,
        now: Timestamp,
        now_fn: &(dyn Fn() -> Instant + Send + Sync),
        call: Option<&BrokerCall>,
    ) -> Option<CodexCreditLimitSnapshot> {
        if !CodexSpendControlsMonthlyUsageGate::should_fetch(response) {
            return None;
        }
        let account_id = response.account_id.as_deref()?.trim();
        if account_id.is_empty() {
            return None;
        }
        let timeout = Self::remaining_timeout(deadline, DEFAULT_DASHBOARD_API_TIMEOUT, now_fn())?;
        let url = endpoint
            .spend_controls_monthly_usage_url(account_id)
            .ok()?;
        let (data, status) = Self::execute_get(
            net,
            endpoint,
            url,
            cookie_header,
            bearer_token,
            None,
            timeout,
            call,
        )
        .await?;
        if !(200..=299).contains(&status) {
            return None;
        }
        let decoded: CodexSpendControlsMonthlyUsageResponse =
            serde_json::from_slice(&data).ok()?;
        decoded.codex_credit_limit_snapshot(now)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_workspace_remaining_balance_if_needed(
        net: &NetBroker,
        endpoint: &OpenAIDashboardEndpointConfig,
        response: &CodexUsageResponse,
        cookie_header: &str,
        bearer_token: Option<&str>,
        deadline: Option<Instant>,
        now_fn: &(dyn Fn() -> Instant + Send + Sync),
        call: Option<&BrokerCall>,
    ) -> Option<f64> {
        let credits = response.credits.as_ref()?;
        if !credits.has_credits || credits.unlimited || credits.balance.is_some() {
            return None;
        }
        let account_id = response.account_id.as_deref()?.trim();
        if account_id.is_empty() {
            return None;
        }
        let timeout = Self::remaining_timeout(deadline, DEFAULT_DASHBOARD_API_TIMEOUT, now_fn())?;
        let url = endpoint.workspace_remaining_balance_url(account_id).ok()?;
        let (data, status) = Self::execute_get(
            net,
            endpoint,
            url,
            cookie_header,
            bearer_token,
            Some(account_id),
            timeout,
            call,
        )
        .await?;
        if !(200..=299).contains(&status) {
            return None;
        }
        CodexWorkspaceRemainingBalanceResponse::parse(&data)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_dashboard_api_preflight(
        net: &NetBroker,
        endpoint: &OpenAIDashboardEndpointConfig,
        cookie_header: &str,
        deadline: Option<Instant>,
        now: Timestamp,
        now_fn: &(dyn Fn() -> Instant + Send + Sync),
        call: Option<&BrokerCall>,
    ) -> Option<(DashboardAPIData, Option<String>)> {
        let response = Self::fetch_dashboard_api_response(
            net,
            endpoint,
            cookie_header,
            deadline,
            now,
            now_fn,
            call,
        )
        .await?;
        let signed_in_email = if let Some(verified) = response.verified_signed_in_email {
            Some(verified)
        } else if response.api_data.has_usage_data() {
            OpenAIDashboardBrowserCookieImporter::fetch_signed_in_email_from_api(
                net,
                endpoint,
                cookie_header,
                deadline,
                now_fn,
                call,
            )
            .await
        } else {
            None
        };
        Some((response.api_data, signed_in_email))
    }

    pub async fn fetch_subscription_from_api(
        net: &NetBroker,
        endpoint: &OpenAIDashboardEndpointConfig,
        cookie_header: &str,
        account_id: Option<&str>,
        deadline: Option<Instant>,
        now_fn: &(dyn Fn() -> Instant + Send + Sync),
        call: Option<&BrokerCall>,
    ) -> OpenAISubscriptionFetchResult {
        let Some(timeout) =
            Self::remaining_timeout(deadline, DEFAULT_SUBSCRIPTIONS_API_TIMEOUT, now_fn())
        else {
            return OpenAISubscriptionFetchResult::Unavailable;
        };
        let Ok(url) = endpoint.subscriptions_url(account_id) else {
            return OpenAISubscriptionFetchResult::Unavailable;
        };
        let Some((data, status)) =
            Self::execute_get(net, endpoint, url, cookie_header, None, None, timeout, call).await
        else {
            return OpenAISubscriptionFetchResult::Unavailable;
        };
        if !(200..=299).contains(&status) {
            return OpenAISubscriptionFetchResult::Unavailable;
        }
        OpenAISubscriptionMetadataParser::parse_result(&data)
    }

    pub fn subscription_metadata_result(
        api_result: OpenAISubscriptionFetchResult,
        fallback_result: Option<OpenAISubscriptionFetchResult>,
    ) -> OpenAISubscriptionFetchResult {
        match api_result {
            OpenAISubscriptionFetchResult::Resolved(_)
            | OpenAISubscriptionFetchResult::ConfirmedAbsent => api_result,
            OpenAISubscriptionFetchResult::Unavailable => {
                fallback_result.unwrap_or(OpenAISubscriptionFetchResult::Unavailable)
            }
        }
    }

    pub fn snapshot_by_merging_api(
        api: &DashboardAPIData,
        previous: Option<&OpenAIDashboardSnapshot>,
        signed_in_email: Option<String>,
        subscription_result: OpenAISubscriptionFetchResult,
        updated_at: Timestamp,
    ) -> OpenAIDashboardSnapshot {
        let snapshot = api.to_snapshot(signed_in_email, updated_at);
        let matching_previous = previous.and_then(|prev| {
            let prev_email = CodexIdentityResolver::normalize_email(prev.signed_in_email.as_deref())?;
            let curr_email =
                CodexIdentityResolver::normalize_email(snapshot.signed_in_email.as_deref())?;
            if prev_email == curr_email && Self::dashboard_can_reuse_snapshot(&snapshot, prev) {
                Some(prev)
            } else {
                None
            }
        });

        let (
            subscription_expires_at,
            subscription_expires_at_is_date_only,
            subscription_renews_at,
            subscription_renews_at_is_date_only,
        ) = match subscription_result {
            OpenAISubscriptionFetchResult::Resolved(metadata) => (
                metadata.subscription_expires_at,
                metadata.subscription_expires_at_is_date_only,
                metadata.subscription_renews_at,
                metadata.subscription_renews_at_is_date_only,
            ),
            OpenAISubscriptionFetchResult::ConfirmedAbsent => (None, false, None, false),
            OpenAISubscriptionFetchResult::Unavailable => matching_previous
                .map(|prev| {
                    (
                        prev.subscription_expires_at,
                        prev.subscription_expires_at_is_date_only,
                        prev.subscription_renews_at,
                        prev.subscription_renews_at_is_date_only,
                    )
                })
                .unwrap_or((None, false, None, false)),
        };

        let Some(prev) = matching_previous else {
            let mut out = snapshot;
            out.subscription_expires_at = subscription_expires_at;
            out.subscription_expires_at_is_date_only = subscription_expires_at_is_date_only;
            out.subscription_renews_at = subscription_renews_at;
            out.subscription_renews_at_is_date_only = subscription_renews_at_is_date_only;
            return out;
        };

        let mut merged = OpenAIDashboardSnapshot::new(
            snapshot
                .signed_in_email
                .or_else(|| prev.signed_in_email.clone()),
            snapshot.account_id.or_else(|| prev.account_id.clone()),
            snapshot
                .code_review_remaining_percent
                .or(prev.code_review_remaining_percent),
            snapshot
                .code_review_limit
                .or_else(|| prev.code_review_limit.clone()),
            if snapshot.credit_events.is_empty() {
                prev.credit_events.clone()
            } else {
                snapshot.credit_events
            },
            if snapshot.daily_breakdown.is_empty() {
                prev.daily_breakdown.clone()
            } else {
                snapshot.daily_breakdown
            },
            if snapshot.usage_breakdown.is_empty() {
                prev.usage_breakdown.clone()
            } else {
                snapshot.usage_breakdown
            },
            snapshot
                .credits_purchase_url
                .or_else(|| prev.credits_purchase_url.clone()),
            snapshot
                .primary_limit
                .or_else(|| prev.primary_limit.clone()),
            snapshot
                .secondary_limit
                .or_else(|| prev.secondary_limit.clone()),
            snapshot
                .extra_rate_windows
                .or_else(|| prev.extra_rate_windows.clone()),
            if snapshot.credits_remaining.is_some() {
                snapshot.credits_remaining
            } else if api.credits_available == Some(false) {
                None
            } else {
                prev.credits_remaining
            },
            snapshot.credits_available.or(prev.credits_available),
            if snapshot.balance_is_workspace.is_some() {
                snapshot.balance_is_workspace
            } else if snapshot.credits_remaining.is_some() || api.credits_available == Some(false) {
                None
            } else {
                prev.balance_is_workspace
            },
            snapshot
                .codex_credit_limit
                .or_else(|| prev.codex_credit_limit.clone()),
            snapshot.account_plan.or_else(|| prev.account_plan.clone()),
            subscription_expires_at,
            subscription_renews_at,
            snapshot.updated_at,
        );
        merged.subscription_expires_at_is_date_only = subscription_expires_at_is_date_only;
        merged.subscription_renews_at_is_date_only = subscription_renews_at_is_date_only;
        merged
    }

    pub fn filling_missing_page_fields(
        snapshot: OpenAIDashboardSnapshot,
        previous: Option<&OpenAIDashboardSnapshot>,
    ) -> OpenAIDashboardSnapshot {
        let Some(prev) = previous else {
            return snapshot;
        };
        let Some(prev_email) =
            CodexIdentityResolver::normalize_email(prev.signed_in_email.as_deref())
        else {
            return snapshot;
        };
        let Some(curr_email) =
            CodexIdentityResolver::normalize_email(snapshot.signed_in_email.as_deref())
        else {
            return snapshot;
        };
        if prev_email != curr_email || !Self::dashboard_can_reuse_snapshot(&snapshot, prev) {
            return snapshot;
        }

        let subscription_expires_at = snapshot
            .subscription_expires_at
            .or(prev.subscription_expires_at);
        let subscription_expires_at_is_date_only = if snapshot.subscription_expires_at.is_some() {
            snapshot.subscription_expires_at_is_date_only
        } else {
            prev.subscription_expires_at_is_date_only
        };
        let subscription_renews_at = snapshot
            .subscription_renews_at
            .or(prev.subscription_renews_at);
        let subscription_renews_at_is_date_only = if snapshot.subscription_renews_at.is_some() {
            snapshot.subscription_renews_at_is_date_only
        } else {
            prev.subscription_renews_at_is_date_only
        };

        let mut merged = OpenAIDashboardSnapshot::new(
            snapshot
                .signed_in_email
                .or_else(|| prev.signed_in_email.clone()),
            snapshot.account_id.or_else(|| prev.account_id.clone()),
            snapshot
                .code_review_remaining_percent
                .or(prev.code_review_remaining_percent),
            snapshot
                .code_review_limit
                .or_else(|| prev.code_review_limit.clone()),
            if snapshot.credit_events.is_empty() {
                prev.credit_events.clone()
            } else {
                snapshot.credit_events
            },
            if snapshot.daily_breakdown.is_empty() {
                prev.daily_breakdown.clone()
            } else {
                snapshot.daily_breakdown
            },
            if snapshot.usage_breakdown.is_empty() {
                prev.usage_breakdown.clone()
            } else {
                snapshot.usage_breakdown
            },
            snapshot
                .credits_purchase_url
                .or_else(|| prev.credits_purchase_url.clone()),
            snapshot
                .primary_limit
                .or_else(|| prev.primary_limit.clone()),
            snapshot
                .secondary_limit
                .or_else(|| prev.secondary_limit.clone()),
            snapshot
                .extra_rate_windows
                .or_else(|| prev.extra_rate_windows.clone()),
            snapshot.credits_remaining.or(prev.credits_remaining),
            snapshot.credits_available.or(prev.credits_available),
            if snapshot.balance_is_workspace.is_some() {
                snapshot.balance_is_workspace
            } else if snapshot.credits_remaining.is_some() {
                None
            } else {
                prev.balance_is_workspace
            },
            snapshot
                .codex_credit_limit
                .or_else(|| prev.codex_credit_limit.clone()),
            snapshot.account_plan.or_else(|| prev.account_plan.clone()),
            subscription_expires_at,
            subscription_renews_at,
            snapshot.updated_at,
        );
        merged.subscription_expires_at_is_date_only = subscription_expires_at_is_date_only;
        merged.subscription_renews_at_is_date_only = subscription_renews_at_is_date_only;
        merged
    }

    pub fn snapshot_for_unpaired_page(
        page: OpenAIDashboardSnapshot,
        previous: Option<&OpenAIDashboardSnapshot>,
        expected_email: Option<&str>,
    ) -> OpenAIDashboardSnapshot {
        if page
            .signed_in_email
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Self::filling_missing_page_fields(page, previous);
        }
        let Some(prev) = previous else {
            return page;
        };
        let Some(expected) = CodexIdentityResolver::normalize_email(expected_email) else {
            return page;
        };
        let Some(prev_email) =
            CodexIdentityResolver::normalize_email(prev.signed_in_email.as_deref())
        else {
            return page;
        };
        if expected != prev_email {
            return page;
        }
        let mut with_email = page;
        with_email.signed_in_email = Some(prev_email);
        Self::filling_missing_page_fields(with_email, Some(prev))
    }

    pub fn should_wait_for_page_identity(
        signed_in_email: Option<&str>,
        expected_email: Option<&str>,
        attempt: usize,
        max_attempts: usize,
    ) -> bool {
        if attempt >= max_attempts {
            return false;
        }
        if signed_in_email.is_some_and(|s| !s.trim().is_empty()) {
            return false;
        }
        CodexIdentityResolver::normalize_email(expected_email).is_some()
    }

    pub fn has_returnable_dashboard_data(snapshot: &OpenAIDashboardSnapshot) -> bool {
        snapshot.primary_limit.is_some()
            || snapshot.secondary_limit.is_some()
            || snapshot
                .extra_rate_windows
                .as_ref()
                .is_some_and(|w| !w.is_empty())
            || snapshot.code_review_remaining_percent.is_some()
            || snapshot.code_review_limit.is_some()
            || snapshot.credits_remaining.is_some()
            || snapshot.credits_available == Some(true)
            || snapshot.codex_credit_limit.is_some()
            || !snapshot.credit_events.is_empty()
            || !snapshot.daily_breakdown.is_empty()
            || !snapshot.usage_breakdown.is_empty()
            || snapshot
                .account_plan
                .as_ref()
                .is_some_and(|p| !p.trim().is_empty())
    }

    pub fn parse_dashboard_scrape(
        scrape: &DashboardScrapePayload,
        previous: Option<&OpenAIDashboardSnapshot>,
        expected_email: Option<&str>,
        now: Timestamp,
    ) -> Result<OpenAIDashboardSnapshot, OpenAIDashboardFetchError> {
        let bootstrap_email =
            OpenAIDashboardParser::parse_signed_in_email_from_client_bootstrap(&scrape.html);
        let auth_status = scrape
            .auth_status
            .clone()
            .or_else(|| OpenAIDashboardParser::parse_auth_status_from_client_bootstrap(&scrape.html));
        if scrape.login_required
            || auth_status
                .as_deref()
                .is_some_and(|s| s.trim().eq_ignore_ascii_case("logged_out"))
        {
            return Err(OpenAIDashboardFetchError::LoginRequired);
        }

        let signed_in_email = scrape
            .signed_in_email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or(bootstrap_email);
        let code_review =
            OpenAIDashboardParser::parse_code_review_remaining_percent(&scrape.body_text);
        let code_review_limit =
            OpenAIDashboardParser::parse_code_review_limit(&scrape.body_text, now);
        let (primary_limit, secondary_limit) =
            OpenAIDashboardParser::parse_rate_limits(&scrape.body_text, now);
        let credits_remaining = OpenAIDashboardParser::parse_credits_remaining(&scrape.body_text);
        let account_plan = OpenAIDashboardParser::parse_plan_from_html(&scrape.html);
        let credit_events: Vec<CreditEvent> =
            OpenAIDashboardParser::parse_credit_events(&scrape.rows);
        let daily_breakdown = OpenAIDashboardSnapshot::make_daily_breakdown(&credit_events, 30);

        let page_snapshot = OpenAIDashboardSnapshot::new(
            signed_in_email,
            None,
            code_review,
            code_review_limit,
            credit_events,
            daily_breakdown,
            scrape.usage_breakdown.clone(),
            scrape
                .credits_purchase_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            primary_limit,
            secondary_limit,
            None,
            credits_remaining,
            None,
            None,
            None,
            account_plan,
            None,
            None,
            now,
        );

        let snapshot = Self::snapshot_for_unpaired_page(page_snapshot, previous, expected_email);
        if Self::has_returnable_dashboard_data(&snapshot) {
            Ok(snapshot)
        } else {
            Err(OpenAIDashboardFetchError::NoDashboardData)
        }
    }

    /// Loads the latest dashboard snapshot using direct HTTPS JSON mode against the 6 pinned
    /// `chatgpt.com` endpoints.
    #[allow(clippy::too_many_arguments)]
    pub async fn load_latest_dashboard(
        net: &NetBroker,
        endpoint: &OpenAIDashboardEndpointConfig,
        raw_cookie_header: &str,
        previous: Option<&OpenAIDashboardSnapshot>,
        timeout: Duration,
        now: Timestamp,
        now_fn: &(dyn Fn() -> Instant + Send + Sync),
        call: Option<&BrokerCall>,
    ) -> Result<OpenAIDashboardSnapshot, OpenAIDashboardFetchError> {
        if call.is_some_and(|c| c.cancel.is_cancelled()) {
            return Err(OpenAIDashboardFetchError::Cancelled);
        }
        let Some(cookie_header) = normalize(raw_cookie_header) else {
            return Err(OpenAIDashboardFetchError::InvalidCookieHeader);
        };
        let effective_timeout = timeout.max(Duration::from_secs(1));
        let deadline = Some(now_fn() + effective_timeout);

        let Some((usage_bytes, status, authorization)) = Self::fetch_dashboard_usage_response(
            net,
            endpoint,
            &cookie_header,
            deadline,
            now_fn,
            call,
        )
        .await
        else {
            if call.is_some_and(|c| c.cancel.is_cancelled()) {
                return Err(OpenAIDashboardFetchError::Cancelled);
            }
            if deadline.is_some_and(|d| now_fn() >= d) {
                return Err(OpenAIDashboardFetchError::Timeout);
            }
            return Err(OpenAIDashboardFetchError::NetworkError(None));
        };

        if status == 401 || status == 403 {
            return Err(OpenAIDashboardFetchError::LoginRequired);
        }
        if !(200..=299).contains(&status) {
            return Err(OpenAIDashboardFetchError::ServerError { status });
        }

        let decoded = CodexOAuthUsageFetcher::decode_usage_response(&usage_bytes)
            .map_err(|_| OpenAIDashboardFetchError::NoDashboardData)?;
        let mut api_data = Self::dashboard_api_data(&decoded, now);
        let bearer = authorization
            .as_ref()
            .map(|a| a.bearer_token.expose_secret());

        if let Some(monthly_limit) = Self::fetch_spend_controls_monthly_usage_if_needed(
            net,
            endpoint,
            &decoded,
            &cookie_header,
            bearer,
            deadline,
            now,
            now_fn,
            call,
        )
        .await
        {
            api_data.codex_credit_limit = Some(monthly_limit);
        }

        if api_data.credits_remaining.is_none()
            && let Some(workspace_balance) = Self::fetch_workspace_remaining_balance_if_needed(
                net,
                endpoint,
                &decoded,
                &cookie_header,
                bearer,
                deadline,
                now_fn,
                call,
            )
            .await
        {
            api_data.credits_remaining = Some(workspace_balance);
            api_data.balance_is_workspace = Some(true);
        }

        let signed_in_email = if let Some(verified) = authorization.map(|a| a.verified_email) {
            Some(verified)
        } else {
            OpenAIDashboardBrowserCookieImporter::fetch_signed_in_email_from_api(
                net,
                endpoint,
                &cookie_header,
                deadline,
                now_fn,
                call,
            )
            .await
        };

        let subscription_result = Self::fetch_subscription_from_api(
            net,
            endpoint,
            &cookie_header,
            api_data.account_id.as_deref(),
            deadline,
            now_fn,
            call,
        )
        .await;

        let snapshot = Self::snapshot_by_merging_api(
            &api_data,
            previous,
            signed_in_email,
            subscription_result,
            now,
        );
        if Self::has_returnable_dashboard_data(&snapshot) {
            Ok(snapshot)
        } else {
            Err(OpenAIDashboardFetchError::NoDashboardData)
        }
    }

    pub fn dashboard_can_reuse_snapshot(
        current: &OpenAIDashboardSnapshot,
        previous: &OpenAIDashboardSnapshot,
    ) -> bool {
        let prev_id = normalize_workspace_account_id(previous.account_id.as_deref());
        let curr_id = normalize_workspace_account_id(current.account_id.as_deref());
        let previous_requires_workspace_scope = previous.balance_is_workspace == Some(true)
            || (previous.credits_available == Some(true) && previous.credits_remaining.is_none());
        if previous_requires_workspace_scope {
            return curr_id.is_some() && curr_id == prev_id;
        }
        if let (Some(curr_id), Some(prev_id)) = (curr_id, prev_id) {
            return curr_id == prev_id;
        }
        true
    }

    fn remaining_timeout(
        deadline: Option<Instant>,
        cap: Duration,
        now: Instant,
    ) -> Option<Duration> {
        let Some(deadline) = deadline else {
            return Some(cap);
        };
        let remaining = deadline.checked_duration_since(now)?;
        if remaining.is_zero() {
            return None;
        }
        Some(remaining.min(cap))
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_get(
        net: &NetBroker,
        endpoint: &OpenAIDashboardEndpointConfig,
        url: Url,
        cookie_header: &str,
        bearer_token: Option<&str>,
        chatgpt_account_id: Option<&str>,
        timeout: Duration,
        call: Option<&BrokerCall>,
    ) -> Option<(Vec<u8>, u16)> {
        if call.is_some_and(|c| c.cancel.is_cancelled()) {
            return None;
        }
        let mut headers = RedactedHeaders::new();
        headers.insert_secret(COOKIE, HeaderValue::from_str(cookie_header).ok()?);
        headers.insert_public(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert_public(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9"),
        );
        headers.insert_public(USER_AGENT, HeaderValue::from_static("CodexBar"));
        if let Some(token) = bearer_token {
            let auth_val = format!("Bearer {}", token.trim());
            headers.insert_secret(AUTHORIZATION, HeaderValue::from_str(&auth_val).ok()?);
        }
        if let Some(account_id) = chatgpt_account_id.map(str::trim).filter(|s| !s.is_empty()) {
            headers.insert_secret(
                "ChatGPT-Account-Id",
                HeaderValue::from_str(account_id).ok()?,
            );
        }

        let request = NetRequest {
            url,
            method: Method::GET,
            headers,
            body: None,
            timeout,
        };

        let result = if let Some(call) = call {
            net.execute(call, &endpoint.declared_origins, request).await
        } else {
            let default_call = BrokerCall::default();
            net.execute(&default_call, &endpoint.declared_origins, request)
                .await
        };
        match result {
            Ok(resp) => Some((resp.body.to_vec(), resp.status.as_u16())),
            Err(NetError::Cancelled) => None,
            Err(_) => None,
        }
    }
}
