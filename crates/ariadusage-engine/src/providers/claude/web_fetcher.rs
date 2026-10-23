// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeWeb/ClaudeWebAPIFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeWeb/ClaudeWebExtraRateWindowParser.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use ariadusage_core::config::CookieSource;
use ariadusage_core::cookie as cookie_core;
use ariadusage_core::hosts::ProviderHosts;
use ariadusage_core::model::{
    NamedRateWindow, ProviderCostSnapshot, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_core::pipeline::{ClassifiedError, FetchError, TransportClass};
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{Confidence, ProviderId};
use cookie_core::{CookieRecord, CookieResolution, DeclaredDomains, pairs, resolve_cookie_source};
use jiff::Timestamp;
use reqwest::header::{ACCEPT, COOKIE, HeaderValue, SET_COOKIE};
use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::Value;

use crate::brokers::browser::{Browser, BrowserBroker, CookieQuery, DEFAULT_IMPORT_ORDER};
use crate::brokers::call::BrokerCall;
use crate::brokers::cookie_cache::{
    CacheScope, ConditionalMutationObservation, CookieCache, CookieCacheEntry,
};
use crate::brokers::net::{
    DeclaredOrigins, NetBroker, NetError, NetRequest, OriginError, RedactedHeaders,
};

use super::identity::ClaudeVerifiedAccountOwner;
use super::plan::ClaudePlan;
use super::reset_credits::{
    ClaudeCloudCreditsSnapshot, ClaudeRateLimitResetCredits, OAuthCedarEmberPayload,
    OAuthIguanaNecktiePayload,
};
use super::scoped_weekly::{
    ClaudeScopedWeeklyLimitMapper, OAuthScopedLimitEntry, parse_iso8601_timestamp,
};

pub const DEFAULT_WEB_BASE_URL: &str = "https://claude.ai/api";
pub const CLAUDE_COOKIE_DOMAIN: &str = "claude.ai";
pub const SESSION_KEY_COOKIE_NAME: &str = "sessionKey";
pub const SESSION_KEY_PREFIX: &str = "sk-ant-";
pub const DEFAULT_WEB_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const DEFAULT_PREPAID_CREDITS_TIMEOUT: Duration = Duration::from_secs(2);
pub const CLOUDFLARE_CHALLENGE_MESSAGE: &str = "claude.ai is behind a Cloudflare challenge, often caused by VPN or datacenter networks. Re-authenticating will not help. Switch Claude Usage source to OAuth in Settings (Usage credits balance will be unavailable), or try a different network.";

const ROUTINE_KEYS: &[&str] = &[
    "seven_day_routines",
    "seven_day_claude_routines",
    "claude_routines",
    "routines",
    "routine",
    "seven_day_cowork",
    "cowork",
];

static CLAUDE_DECLARED_DOMAINS: LazyLock<DeclaredDomains> = LazyLock::new(|| {
    DeclaredDomains::new([CLAUDE_COOKIE_DOMAIN]).expect("valid Claude cookie domain")
});

/// Returns the declared cookie domains for Claude (`claude.ai`).
pub fn claude_declared_domains() -> &'static DeclaredDomains {
    &CLAUDE_DECLARED_DOMAINS
}

/// Endpoint and timeout configuration for `claude.ai/api` requests.
#[derive(Debug, Clone)]
pub struct ClaudeWebEndpointConfig {
    pub base_url: Url,
    pub declared_origins: DeclaredOrigins,
    pub prepaid_credits_timeout: Duration,
}

impl Default for ClaudeWebEndpointConfig {
    fn default() -> Self {
        let base_url = Url::parse(DEFAULT_WEB_BASE_URL).expect("valid default Claude web base URL");
        let hosts = ProviderHosts::new([CLAUDE_COOKIE_DOMAIN]).expect("valid Claude web host");
        let declared_origins = DeclaredOrigins::from_provider_hosts(&hosts);
        Self {
            base_url,
            declared_origins,
            prepaid_credits_timeout: DEFAULT_PREPAID_CREDITS_TIMEOUT,
        }
    }
}

impl ClaudeWebEndpointConfig {
    /// Creates an endpoint config pointing at a custom base URL (for `httpmock` tests).
    pub fn with_base_url(base_url: Url) -> Result<Self, OriginError> {
        let declared_origins = DeclaredOrigins::from_urls([base_url.clone()])?;
        Ok(Self {
            base_url,
            declared_origins,
            prepaid_credits_timeout: DEFAULT_PREPAID_CREDITS_TIMEOUT,
        })
    }

    pub fn with_prepaid_credits_timeout(mut self, timeout: Duration) -> Self {
        self.prepaid_credits_timeout = timeout;
        self
    }

    /// Resolves a path relative to `base_url` while preserving `/api`.
    pub fn api_url(&self, path: &str) -> Result<Url, ClaudeWebFetchError> {
        let base = self.base_url.as_str().trim_end_matches('/');
        let rel = path.trim_start_matches('/');
        let prefix = if self.base_url.path().trim_matches('/').is_empty() {
            format!("{base}/api")
        } else {
            base.to_owned()
        };
        Url::parse(&format!("{prefix}/{rel}")).map_err(|_| ClaudeWebFetchError::NetworkError(None))
    }
}

/// Selected organization from `GET /api/organizations`.
#[derive(Clone, PartialEq, Eq)]
pub struct OrganizationInfo {
    pub id: String,
    pub name: Option<String>,
}

impl fmt::Debug for OrganizationInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrganizationInfo")
            .field("has_id", &(!self.id.is_empty()))
            .field("has_name", &self.name.is_some())
            .finish()
    }
}

/// Extracted `sessionKey` cookie and provenance metadata.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionKeyInfo {
    pub key: SecretString,
    pub source_label: String,
    pub cookie_count: usize,
}

impl SessionKeyInfo {
    pub fn new(
        key: impl Into<SecretString>,
        source_label: impl Into<String>,
        cookie_count: usize,
    ) -> Self {
        Self {
            key: key.into(),
            source_label: source_label.into(),
            cookie_count,
        }
    }
}

impl fmt::Debug for SessionKeyInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionKeyInfo")
            .field("key", &"[redacted]")
            .field("source_label", &self.source_label)
            .field("cookie_count", &self.cookie_count)
            .finish()
    }
}

/// Account metadata resolved from `GET /api/account`.
#[derive(Clone, PartialEq, Eq)]
pub struct WebAccountInfo {
    pub email: Option<String>,
    pub login_method: Option<String>,
}

impl fmt::Debug for WebAccountInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebAccountInfo")
            .field("has_email", &self.email.is_some())
            .field("login_method", &self.login_method)
            .finish()
    }
}

/// Remaining prepaid Extra usage balance in major currency units.
#[derive(Debug, Clone, PartialEq)]
pub struct PrepaidBalance {
    pub amount: f64,
    pub currency_code: String,
}

/// Parsed usage data from the `claude.ai` web API.
#[derive(Clone, Default, PartialEq)]
pub struct WebUsageData {
    pub session_percent_used: Option<f64>,
    pub session_resets_at: Option<Timestamp>,
    pub weekly_percent_used: Option<f64>,
    pub weekly_resets_at: Option<Timestamp>,
    pub opus_percent_used: Option<f64>,
    pub extra_rate_windows: Vec<NamedRateWindow>,
    pub extra_usage_cost: Option<ProviderCostSnapshot>,
    pub reset_credits: Option<ClaudeRateLimitResetCredits>,
    pub cloud_credits: Option<ClaudeCloudCreditsSnapshot>,
    pub account_organization: Option<String>,
    pub account_organization_id: Option<String>,
    pub account_email: Option<String>,
    pub login_method: Option<String>,
}

impl fmt::Debug for WebUsageData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebUsageData")
            .field("session_percent_used", &self.session_percent_used)
            .field("session_resets_at", &self.session_resets_at)
            .field("weekly_percent_used", &self.weekly_percent_used)
            .field("weekly_resets_at", &self.weekly_resets_at)
            .field("opus_percent_used", &self.opus_percent_used)
            .field("extra_rate_windows", &self.extra_rate_windows)
            .field("extra_usage_cost", &self.extra_usage_cost)
            .field("reset_credits", &self.reset_credits)
            .field("cloud_credits", &self.cloud_credits)
            .field(
                "has_account_organization",
                &self.account_organization.is_some(),
            )
            .field(
                "has_account_organization_id",
                &self.account_organization_id.is_some(),
            )
            .field("has_account_email", &self.account_email.is_some())
            .field("login_method", &self.login_method)
            .finish()
    }
}

impl WebUsageData {
    /// Converts `WebUsageData` into a normalized `UsageSnapshot`.
    ///
    /// A missing web session window stays `None` rather than becoming a synthetic 0% window.
    pub fn to_usage_snapshot(
        &self,
        include_account_identity: bool,
        include_optional_usage: bool,
        updated_at: Timestamp,
    ) -> Result<UsageSnapshot, ClassifiedError> {
        let primary = self.session_percent_used.and_then(|pct| {
            RateWindow::new(pct, Some(300), self.session_resets_at, None, None, false).ok()
        });
        let secondary = self.weekly_percent_used.and_then(|pct| {
            RateWindow::new(pct, Some(10080), self.weekly_resets_at, None, None, false).ok()
        });
        let tertiary = self.opus_percent_used.and_then(|pct| {
            RateWindow::new(pct, Some(10080), self.weekly_resets_at, None, None, false).ok()
        });

        let extra_rate_windows = if self.extra_rate_windows.is_empty() {
            None
        } else {
            Some(self.extra_rate_windows.clone())
        };

        let mut details = self
            .reset_credits
            .as_ref()
            .map_or_else(Vec::new, ClaudeRateLimitResetCredits::detail_sections);
        if include_optional_usage && let Some(cc) = &self.cloud_credits {
            details.extend(cc.detail_sections(updated_at));
        }

        let account_id = if include_account_identity {
            ClaudeVerifiedAccountOwner::owner_id(
                None,
                self.account_email.as_deref(),
                self.account_organization_id.as_deref(),
            )
        } else {
            None
        };

        let identity = Some(ProviderIdentitySnapshot {
            provider_id: ProviderId::new("claude").ok(),
            account_email: self.account_email.clone(),
            account_organization: self.account_organization.clone(),
            login_method: self.login_method.clone(),
            account_id,
        });

        UsageSnapshot::new(
            primary,
            secondary,
            tertiary,
            extra_rate_windows,
            self.extra_usage_cost.clone(),
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
                "Claude web usage snapshot validation failed",
            )
        })
    }
}

/// Errors produced by `ClaudeWebAPIFetcher`.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeWebFetchError {
    #[error("request cancelled")]
    Cancelled,
    #[error("No Claude session key found in browser cookies.")]
    NoSessionKeyFound,
    #[error(
        "No readable Claude desktop session found. Sign in to Claude.app and check cookie access."
    )]
    NoDesktopSessionKeyFound,
    #[error("Invalid Claude session key format.")]
    InvalidSessionKey,
    #[error("Network error")]
    NetworkError(Option<TransportClass>),
    #[error("Invalid response from Claude API.")]
    InvalidResponse,
    #[error("Sign in to claude.ai (or refresh Claude cookies) to load usage data.")]
    Unauthorized,
    #[error("{CLOUDFLARE_CHALLENGE_MESSAGE}")]
    CloudflareChallenge,
    #[error("Claude API error: HTTP {status}")]
    ServerError { status: u16 },
    #[error("No Claude organization found for this account.")]
    NoOrganization,
    #[error("The configured Claude organization was not found for this session.")]
    OrganizationNotFound(String),
}

impl fmt::Debug for ClaudeWebFetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Cancelled"),
            Self::NoSessionKeyFound => f.write_str("NoSessionKeyFound"),
            Self::NoDesktopSessionKeyFound => f.write_str("NoDesktopSessionKeyFound"),
            Self::InvalidSessionKey => f.write_str("InvalidSessionKey"),
            Self::NetworkError(transport) => {
                f.debug_tuple("NetworkError").field(transport).finish()
            }
            Self::InvalidResponse => f.write_str("InvalidResponse"),
            Self::Unauthorized => f.write_str("Unauthorized"),
            Self::CloudflareChallenge => f.write_str("CloudflareChallenge"),
            Self::ServerError { status } => f
                .debug_struct("ServerError")
                .field("status", status)
                .finish(),
            Self::NoOrganization => f.write_str("NoOrganization"),
            Self::OrganizationNotFound(_) => f
                .debug_tuple("OrganizationNotFound")
                .field(&"[redacted]")
                .finish(),
        }
    }
}

impl ClaudeWebFetchError {
    pub fn is_cancellation(&self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::NetworkError(Some(TransportClass::Cancelled))
        )
    }

    pub fn into_classified_error(self) -> ClassifiedError {
        let msg = self.to_string();
        match self {
            Self::Cancelled => ClassifiedError::new(ProviderErrorKind::NetworkFailure, msg)
                .with_transport(TransportClass::Cancelled),
            Self::NoSessionKeyFound | Self::NoDesktopSessionKeyFound => {
                ClassifiedError::new(ProviderErrorKind::MissingCredential, msg)
            }
            Self::InvalidSessionKey | Self::Unauthorized => {
                ClassifiedError::new(ProviderErrorKind::AuthenticationExpired, msg)
            }
            Self::CloudflareChallenge => {
                ClassifiedError::new(ProviderErrorKind::PermissionDenied, msg)
            }
            Self::ServerError { status: 429 } => {
                ClassifiedError::new(ProviderErrorKind::RateLimited, msg)
            }
            Self::ServerError { status: 500..=599 } => {
                ClassifiedError::new(ProviderErrorKind::ProviderUnavailable, msg)
            }
            Self::ServerError { .. } => ClassifiedError::new(ProviderErrorKind::ApiFailure, msg),
            Self::InvalidResponse | Self::NoOrganization | Self::OrganizationNotFound(_) => {
                ClassifiedError::new(ProviderErrorKind::ParseFailure, msg)
            }
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
        if self.is_cancellation() {
            FetchError::Cancelled
        } else {
            FetchError::Classified(self.into_classified_error())
        }
    }
}

/// Options controlling a `ClaudeWebAPIFetcher` fetch run.
#[derive(Debug, Clone)]
pub struct ClaudeWebFetchOptions {
    pub target_organization_id: Option<String>,
    pub include_usage_details: bool,
    pub include_prepaid_balance: bool,
    pub use_desktop_cookies: bool,
    pub cache_scope: Option<CacheScope>,
    pub now: Timestamp,
}

impl Default for ClaudeWebFetchOptions {
    fn default() -> Self {
        Self {
            target_organization_id: None,
            include_usage_details: true,
            include_prepaid_balance: true,
            use_desktop_cookies: false,
            cache_scope: None,
            now: Timestamp::now(),
        }
    }
}

pub type CustomSessionImporterFn =
    dyn Fn(&BrokerCall) -> Result<SessionKeyInfo, ClaudeWebFetchError> + Send + Sync;

pub type PerBrowserSessionImporterFn = dyn Fn(Browser, &BrokerCall) -> Result<Option<SessionKeyInfo>, ClaudeWebFetchError>
    + Send
    + Sync;

/// Source for browser session-key import (production `BrowserBroker` or deterministic test seams).
#[derive(Clone, Default)]
pub enum ClaudeWebSessionImporter {
    #[default]
    None,
    BrowserBroker(Arc<BrowserBroker>),
    Custom(Arc<CustomSessionImporterFn>),
    PerBrowser(Arc<PerBrowserSessionImporterFn>),
}

impl ClaudeWebSessionImporter {
    pub async fn import_session_key(
        &self,
        call: &BrokerCall,
        use_desktop_cookies: bool,
    ) -> Result<SessionKeyInfo, ClaudeWebFetchError> {
        if call.cancel.is_cancelled() {
            return Err(ClaudeWebFetchError::Cancelled);
        }
        let not_found_err = if use_desktop_cookies {
            ClaudeWebFetchError::NoDesktopSessionKeyFound
        } else {
            ClaudeWebFetchError::NoSessionKeyFound
        };

        match self {
            Self::None => Err(not_found_err),
            Self::Custom(f) => f(call),
            Self::PerBrowser(f) => {
                if use_desktop_cookies {
                    return Err(ClaudeWebFetchError::NoDesktopSessionKeyFound);
                }
                for browser in DEFAULT_IMPORT_ORDER {
                    if call.cancel.is_cancelled() {
                        return Err(ClaudeWebFetchError::Cancelled);
                    }
                    match f(browser, call) {
                        Ok(Some(info)) => return Ok(info),
                        Ok(None) | Err(_) => continue,
                    }
                }
                Err(not_found_err)
            }
            Self::BrowserBroker(broker) => {
                if use_desktop_cookies {
                    return Err(ClaudeWebFetchError::NoDesktopSessionKeyFound);
                }
                let CookieResolution::Import(auth) =
                    resolve_cookie_source(Some(CookieSource::Auto), false, None, false)
                else {
                    return Err(not_found_err);
                };
                let provider_id = ProviderId::new("claude").expect("valid provider id");
                for browser in DEFAULT_IMPORT_ORDER {
                    if call.cancel.is_cancelled() {
                        return Err(ClaudeWebFetchError::Cancelled);
                    }
                    let browsers = [browser];
                    let names = [SESSION_KEY_COOKIE_NAME];
                    let query = CookieQuery {
                        provider: provider_id.clone(),
                        domains: claude_declared_domains(),
                        names: Some(&names),
                        browsers: &browsers,
                        acknowledged_retry: false,
                    };
                    let Ok(candidates) = broker.candidates(call, auth, query).await else {
                        continue;
                    };
                    for candidate in candidates {
                        if let Some(key) = find_session_key_in_records(&candidate.records) {
                            return Ok(SessionKeyInfo {
                                key,
                                source_label: candidate.label,
                                cookie_count: candidate.records.len(),
                            });
                        }
                    }
                }
                Err(not_found_err)
            }
        }
    }
}

/// Serializes concurrent browser/cache web fetches so `Set-Cookie` session key rotations
/// are observed in order, while allowing cancelled waiters to relinquish the gate immediately.
#[derive(Clone, Default)]
pub struct ClaudeWebBrowserFetchGate {
    mutex: Arc<tokio::sync::Mutex<()>>,
}

impl ClaudeWebBrowserFetchGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn acquire(
        &self,
        call: &BrokerCall,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, ClaudeWebFetchError> {
        let lock_fut = self.mutex.clone().lock_owned();
        let Some(guard) = call.cancel.run_until_cancelled(lock_fut).await else {
            return Err(ClaudeWebFetchError::Cancelled);
        };
        if call.cancel.is_cancelled() {
            return Err(ClaudeWebFetchError::Cancelled);
        }
        Ok(guard)
    }
}

/// Tracks `Set-Cookie` session-key renewals across sequential requests in one web fetch.
pub struct ClaudeWebSessionKeyRenewalTracker {
    initial_session_key: SecretString,
    current_session_key: Mutex<SecretString>,
}

impl ClaudeWebSessionKeyRenewalTracker {
    pub fn new(initial_session_key: SecretString) -> Self {
        Self {
            initial_session_key: initial_session_key.clone(),
            current_session_key: Mutex::new(initial_session_key),
        }
    }

    pub fn session_key(&self) -> SecretString {
        self.current_session_key
            .lock()
            .expect("renewal tracker lock not poisoned")
            .clone()
    }

    pub fn renewed_cookie_header(&self) -> Option<SecretString> {
        let current = self
            .current_session_key
            .lock()
            .expect("renewal tracker lock not poisoned");
        if current.expose_secret() == self.initial_session_key.expose_secret() {
            None
        } else {
            Some(SecretString::new(format!(
                "{}={}",
                SESSION_KEY_COOKIE_NAME,
                current.expose_secret()
            )))
        }
    }

    pub fn observe(&self, status: u16, headers: &RedactedHeaders) {
        if status != 200 {
            return;
        }
        let mut latest: Option<SecretString> = None;
        for (name, val) in headers.iter() {
            if name != SET_COOKIE {
                continue;
            }
            let Ok(header_str) = val.to_str() else {
                continue;
            };
            if let Some(extracted) = Self::extract_from_set_cookie_header(header_str) {
                latest = Some(extracted);
            }
        }
        if let Some(new_key) = latest {
            let mut guard = self
                .current_session_key
                .lock()
                .expect("renewal tracker lock not poisoned");
            *guard = new_key;
        }
    }

    pub fn extract_from_set_cookie_header(header: &str) -> Option<SecretString> {
        let mut latest = None;
        let prefix_len = SESSION_KEY_COOKIE_NAME.len();
        for part in header.split([',', '\r', '\n']) {
            let trimmed = part.trim_start();
            if trimmed.len() > prefix_len
                && trimmed.is_char_boundary(prefix_len)
                && trimmed[..prefix_len].eq_ignore_ascii_case(SESSION_KEY_COOKIE_NAME)
                && trimmed.as_bytes()[prefix_len] == b'='
            {
                let rest = &trimmed[prefix_len + 1..];
                let raw_val = rest.split(';').next().unwrap_or(rest).trim();
                if raw_val.starts_with(SESSION_KEY_PREFIX) {
                    latest = Some(SecretString::new(raw_val.to_owned()));
                }
            }
        }
        latest
    }
}

/// Parses extra rate windows (`limits` scoped weekly windows + `claude-routines`) from web usage JSON.
pub struct ClaudeWebExtraRateWindowParser;

impl ClaudeWebExtraRateWindowParser {
    pub fn parse(
        obj: &serde_json::Map<String, Value>,
    ) -> (Vec<NamedRateWindow>, BTreeMap<String, String>) {
        let mut windows =
            ClaudeScopedWeeklyLimitMapper::named_windows(&Self::scoped_weekly_limits(obj));
        let mut source_keys = BTreeMap::new();

        for key in ROUTINE_KEYS {
            if let Some(raw_window) = obj.get(*key).and_then(Value::as_object) {
                if let Some(utilization) = strict_number_percent(raw_window.get("utilization")) {
                    let resets_at = raw_window
                        .get("resets_at")
                        .and_then(Value::as_str)
                        .and_then(parse_iso8601_timestamp);
                    if let Ok(rate_window) =
                        RateWindow::new(utilization, Some(10080), resets_at, None, None, false)
                    {
                        windows.push(NamedRateWindow::new(
                            "claude-routines",
                            "Daily Routines",
                            rate_window,
                        ));
                        source_keys.insert("claude-routines".to_owned(), (*key).to_owned());
                    }
                }
                break;
            }
        }

        (windows, source_keys)
    }

    pub fn scoped_weekly_limits(
        obj: &serde_json::Map<String, Value>,
    ) -> Vec<OAuthScopedLimitEntry> {
        match obj.get("limits") {
            Some(Value::Null) | None => Vec::new(),
            Some(val) => OAuthScopedLimitEntry::parse_lossy_array(val),
        }
    }
}

struct CachePersistence {
    source_label: String,
    expected_observation: ConditionalMutationObservation,
    persist_initial_session_key: bool,
    scope: Option<CacheScope>,
}

/// Fetches Claude usage data from the `claude.ai/api` endpoints using session cookies.
pub struct ClaudeWebAPIFetcher;

impl ClaudeWebAPIFetcher {
    /// Returns `true` if `cookie_header` contains a valid `sessionKey=sk-ant-...` cookie.
    pub fn has_session_key_in_header(cookie_header: Option<&str>) -> bool {
        let Some(raw) = cookie_header else {
            return false;
        };
        Self::session_key_info(raw).is_ok()
    }

    /// Checks whether a session key is available in `cookie_cache` or via `importer`.
    pub async fn has_session_key(
        cookie_cache: &CookieCache,
        scope: Option<&CacheScope>,
        importer: &ClaudeWebSessionImporter,
        use_desktop_cookies: bool,
        call: &BrokerCall,
    ) -> bool {
        let provider_id = ProviderId::new("claude").expect("valid provider id");
        if !use_desktop_cookies
            && let Some(cached) = cookie_cache.load(&provider_id, scope)
            && Self::has_session_key_in_header(Some(cached.header.expose_secret()))
        {
            return true;
        }
        importer
            .import_session_key(call, use_desktop_cookies)
            .await
            .is_ok()
    }

    /// Extracts `SessionKeyInfo` from a raw manual/cached `Cookie` header string.
    pub fn session_key_info(cookie_header: &str) -> Result<SessionKeyInfo, ClaudeWebFetchError> {
        let cookie_pairs = pairs(cookie_header);
        if let Some(key) = find_session_key_in_pairs(&cookie_pairs) {
            return Ok(SessionKeyInfo {
                key,
                source_label: "Manual".to_owned(),
                cookie_count: cookie_pairs.len(),
            });
        }
        Err(ClaudeWebFetchError::NoSessionKeyFound)
    }

    /// Fetches usage using an explicit manual `Cookie` header (does not mutate `CookieCache`).
    pub async fn fetch_usage_with_cookie_header(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        cookie_header: &str,
        options: &ClaudeWebFetchOptions,
        call: &BrokerCall,
    ) -> Result<WebUsageData, ClaudeWebFetchError> {
        let session_info = Self::session_key_info(cookie_header)?;
        Self::fetch_usage_with_session(net, endpoint, &session_info, options, None, None, call)
            .await
    }

    /// Fetches usage serialized across concurrent callers, trying `CookieCache` first
    /// (unless `use_desktop_cookies` is set) and falling back to `importer`.
    pub async fn fetch_usage_serialized(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        cookie_cache: &CookieCache,
        importer: &ClaudeWebSessionImporter,
        gate: &ClaudeWebBrowserFetchGate,
        options: &ClaudeWebFetchOptions,
        call: &BrokerCall,
    ) -> Result<WebUsageData, ClaudeWebFetchError> {
        let _guard = gate.acquire(call).await?;
        Self::fetch_usage_inner(net, endpoint, cookie_cache, importer, options, call).await
    }

    async fn fetch_usage_inner(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        cookie_cache: &CookieCache,
        importer: &ClaudeWebSessionImporter,
        options: &ClaudeWebFetchOptions,
        call: &BrokerCall,
    ) -> Result<WebUsageData, ClaudeWebFetchError> {
        if options.use_desktop_cookies {
            // Reimport on each refresh so a desktop account switch cannot reuse a cached browser session.
            let session = importer.import_session_key(call, true).await?;
            return Self::fetch_usage_with_session(
                net, endpoint, &session, options, None, None, call,
            )
            .await;
        }

        let provider_id = ProviderId::new("claude").expect("valid provider id");
        let scope = options.cache_scope.as_ref();
        let mut cache_observation =
            cookie_cache.observe_for_conditional_mutation(&provider_id, scope);
        let mut invalidated_cache_error: Option<ClaudeWebFetchError> = None;

        if let Some(cached) = cache_observation.entry.clone()
            && !cached.header.expose_secret().trim().is_empty()
        {
            match Self::fetch_usage_and_renew_cache(
                net,
                endpoint,
                cookie_cache,
                &cached,
                &cache_observation,
                options,
                call,
            )
            .await
            {
                Ok(usage) => return Ok(usage),
                Err(
                    err @ (ClaudeWebFetchError::Unauthorized
                    | ClaudeWebFetchError::NoSessionKeyFound
                    | ClaudeWebFetchError::InvalidSessionKey),
                ) => {
                    let cleared = cookie_cache.clear_if_current(&provider_id, scope, Some(&cached));
                    if cleared {
                        cache_observation = cache_observation.after_owned_clear();
                    }
                    invalidated_cache_error = Some(err);
                }
                Err(err) => return Err(err),
            }
        }

        let session_info = match importer.import_session_key(call, false).await {
            Ok(info) => info,
            Err(import_err) => {
                if let Some(cached_err) = invalidated_cache_error {
                    return Err(cached_err);
                }
                return Err(import_err);
            }
        };

        Self::fetch_usage_with_session(
            net,
            endpoint,
            &session_info,
            options,
            Some(cookie_cache),
            Some(CachePersistence {
                source_label: session_info.source_label.clone(),
                expected_observation: cache_observation,
                persist_initial_session_key: true,
                scope: options.cache_scope.clone(),
            }),
            call,
        )
        .await
    }

    async fn fetch_usage_and_renew_cache(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        cookie_cache: &CookieCache,
        cached_entry: &CookieCacheEntry,
        expected_observation: &ConditionalMutationObservation,
        options: &ClaudeWebFetchOptions,
        call: &BrokerCall,
    ) -> Result<WebUsageData, ClaudeWebFetchError> {
        let session_info = Self::session_key_info(cached_entry.header.expose_secret())?;
        Self::fetch_usage_with_session(
            net,
            endpoint,
            &session_info,
            options,
            Some(cookie_cache),
            Some(CachePersistence {
                source_label: cached_entry.source_label.clone(),
                expected_observation: expected_observation.clone(),
                persist_initial_session_key: false,
                scope: options.cache_scope.clone(),
            }),
            call,
        )
        .await
    }

    async fn fetch_usage_with_session(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        session_key_info: &SessionKeyInfo,
        options: &ClaudeWebFetchOptions,
        cookie_cache: Option<&CookieCache>,
        cache_persistence: Option<CachePersistence>,
        call: &BrokerCall,
    ) -> Result<WebUsageData, ClaudeWebFetchError> {
        if call.cancel.is_cancelled() {
            return Err(ClaudeWebFetchError::Cancelled);
        }

        let renewal_tracker = ClaudeWebSessionKeyRenewalTracker::new(session_key_info.key.clone());

        let organization = Self::fetch_organization_info(
            net,
            endpoint,
            &renewal_tracker.session_key(),
            options.target_organization_id.as_deref(),
            &renewal_tracker,
            call,
        )
        .await?;

        let mut usage = Self::fetch_usage_data(
            net,
            endpoint,
            &organization.id,
            &renewal_tracker.session_key(),
            options.now,
            &renewal_tracker,
            call,
        )
        .await?;
        usage.account_organization_id = Some(organization.id.clone());

        if options.include_usage_details
            && usage.extra_usage_cost.is_none()
            && let Some(extra) = Self::fetch_overage_spend_limit(
                net,
                endpoint,
                &organization.id,
                &renewal_tracker.session_key(),
                options.now,
                &renewal_tracker,
                call,
            )
            .await?
        {
            usage.extra_usage_cost = Some(extra);
        }

        if options.include_prepaid_balance
            && let Some(balance) = Self::fetch_prepaid_balance(
                net,
                endpoint,
                &organization.id,
                &renewal_tracker.session_key(),
                &renewal_tracker,
                call,
            )
            .await?
        {
            usage.extra_usage_cost = Some(Self::applying_prepaid_balance(
                &balance,
                usage.extra_usage_cost.take(),
                options.now,
            ));
        }

        if let Some(account) = Self::fetch_account_info(
            net,
            endpoint,
            &renewal_tracker.session_key(),
            Some(&organization.id),
            &renewal_tracker,
            call,
        )
        .await?
        {
            usage.account_email = account.email;
            usage.login_method = account.login_method;
        }

        if usage.account_organization.is_none() {
            usage.account_organization = organization.name;
        }

        if let (Some(cache), Some(persistence)) = (cookie_cache, cache_persistence) {
            Self::persist_session_key_if_needed(
                cache,
                &session_key_info.key,
                &persistence,
                renewal_tracker.renewed_cookie_header(),
                options.now,
            );
        }

        if call.cancel.is_cancelled() {
            return Err(ClaudeWebFetchError::Cancelled);
        }

        Ok(usage)
    }

    fn persist_session_key_if_needed(
        cookie_cache: &CookieCache,
        initial_session_key: &SecretString,
        persistence: &CachePersistence,
        renewed_cookie_header: Option<SecretString>,
        now: Timestamp,
    ) {
        let imported_cookie_header = persistence.persist_initial_session_key.then(|| {
            SecretString::new(format!(
                "{}={}",
                SESSION_KEY_COOKIE_NAME,
                initial_session_key.expose_secret()
            ))
        });
        let Some(header_to_store) = renewed_cookie_header.or(imported_cookie_header) else {
            return;
        };
        let provider_id = ProviderId::new("claude").expect("valid provider id");
        let _ = cookie_cache.store_if_observation_current(
            &provider_id,
            persistence.scope.as_ref(),
            &persistence.expected_observation,
            header_to_store.expose_secret(),
            &persistence.source_label,
            now,
        );
    }

    async fn fetch_organization_info(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        session_key: &SecretString,
        target_organization_id: Option<&str>,
        renewal_tracker: &ClaudeWebSessionKeyRenewalTracker,
        call: &BrokerCall,
    ) -> Result<OrganizationInfo, ClaudeWebFetchError> {
        let url = endpoint.api_url("/organizations")?;
        let req = build_web_get_request(url, session_key);
        let resp = net
            .send_response(call, &endpoint.declared_origins, req)
            .await
            .map_err(map_web_net_error)?;
        let status = resp.status.as_u16();
        renewal_tracker.observe(status, &resp.headers);

        if status == 200 {
            Self::parse_organizations_response(resp.body(), target_organization_id)
        } else {
            Err(Self::classify_http_error(
                status,
                &resp.headers,
                resp.body(),
            ))
        }
    }

    async fn fetch_usage_data(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        org_id: &str,
        session_key: &SecretString,
        now: Timestamp,
        renewal_tracker: &ClaudeWebSessionKeyRenewalTracker,
        call: &BrokerCall,
    ) -> Result<WebUsageData, ClaudeWebFetchError> {
        let mut resp = Self::request_usage(net, endpoint, org_id, session_key, true, call).await?;
        let mut status = resp.status.as_u16();
        renewal_tracker.observe(status, &resp.headers);

        if !matches!(status, 200 | 401 | 429)
            && !(status == 403 && Self::is_cloudflare_challenge(&resp.headers, resp.body()))
        {
            if call.cancel.is_cancelled() {
                return Err(ClaudeWebFetchError::Cancelled);
            }
            resp = Self::request_usage(net, endpoint, org_id, session_key, false, call).await?;
            status = resp.status.as_u16();
            renewal_tracker.observe(status, &resp.headers);
        }

        if status == 200 {
            Self::parse_usage_response(resp.body(), now)
        } else {
            Err(Self::classify_http_error(
                status,
                &resp.headers,
                resp.body(),
            ))
        }
    }

    async fn request_usage(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        org_id: &str,
        session_key: &SecretString,
        include_resets: bool,
        call: &BrokerCall,
    ) -> Result<crate::brokers::net::NetResponse, ClaudeWebFetchError> {
        let mut url = endpoint.api_url(&format!("/organizations/{org_id}/usage"))?;
        if include_resets {
            url.query_pairs_mut().append_pair("cedar_ember", "1");
        }
        let req = build_web_get_request(url, session_key);
        net.send_response(call, &endpoint.declared_origins, req)
            .await
            .map_err(map_web_net_error)
    }

    async fn fetch_overage_spend_limit(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        org_id: &str,
        session_key: &SecretString,
        now: Timestamp,
        renewal_tracker: &ClaudeWebSessionKeyRenewalTracker,
        call: &BrokerCall,
    ) -> Result<Option<ProviderCostSnapshot>, ClaudeWebFetchError> {
        let path = format!("/organizations/{org_id}/overage_spend_limit");
        let data = Self::fetch_best_effort_json(
            net,
            endpoint,
            &path,
            session_key,
            Duration::from_secs(15),
            renewal_tracker,
            call,
        )
        .await?;
        Ok(data.and_then(|bytes| Self::parse_overage_spend_limit(&bytes, now)))
    }

    async fn fetch_prepaid_balance(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        org_id: &str,
        session_key: &SecretString,
        renewal_tracker: &ClaudeWebSessionKeyRenewalTracker,
        call: &BrokerCall,
    ) -> Result<Option<PrepaidBalance>, ClaudeWebFetchError> {
        let path = format!("/organizations/{org_id}/prepaid/credits");
        let data = Self::fetch_best_effort_json(
            net,
            endpoint,
            &path,
            session_key,
            endpoint.prepaid_credits_timeout,
            renewal_tracker,
            call,
        )
        .await?;
        Ok(data.and_then(|bytes| Self::parse_prepaid_balance(&bytes)))
    }

    async fn fetch_account_info(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        session_key: &SecretString,
        org_id: Option<&str>,
        renewal_tracker: &ClaudeWebSessionKeyRenewalTracker,
        call: &BrokerCall,
    ) -> Result<Option<WebAccountInfo>, ClaudeWebFetchError> {
        let data = Self::fetch_best_effort_json(
            net,
            endpoint,
            "/account",
            session_key,
            Duration::from_secs(15),
            renewal_tracker,
            call,
        )
        .await?;
        Ok(data.and_then(|bytes| Self::parse_account_info(&bytes, org_id)))
    }

    async fn fetch_best_effort_json(
        net: &Arc<NetBroker>,
        endpoint: &ClaudeWebEndpointConfig,
        path: &str,
        session_key: &SecretString,
        timeout: Duration,
        renewal_tracker: &ClaudeWebSessionKeyRenewalTracker,
        call: &BrokerCall,
    ) -> Result<Option<Vec<u8>>, ClaudeWebFetchError> {
        if call.cancel.is_cancelled() {
            return Err(ClaudeWebFetchError::Cancelled);
        }
        let Ok(url) = endpoint.api_url(path) else {
            return Ok(None);
        };
        let req = build_web_get_request(url, session_key);
        let child_cancel = call.cancel.child_token();
        let sub_call = BrokerCall {
            interaction: call.interaction,
            cancel: child_cancel.clone(),
            request_id: call.request_id.clone(),
        };

        let send_fut = net.send_response(&sub_call, &endpoint.declared_origins, req);
        match tokio::time::timeout(timeout, send_fut).await {
            Ok(Ok(resp)) => {
                if call.cancel.is_cancelled() {
                    return Err(ClaudeWebFetchError::Cancelled);
                }
                let status = resp.status.as_u16();
                renewal_tracker.observe(status, &resp.headers);
                if status == 200 {
                    Ok(Some(resp.body().to_vec()))
                } else {
                    Ok(None)
                }
            }
            Ok(Err(NetError::Cancelled)) => {
                if call.cancel.is_cancelled() {
                    Err(ClaudeWebFetchError::Cancelled)
                } else {
                    Ok(None)
                }
            }
            Ok(Err(_)) => {
                if call.cancel.is_cancelled() {
                    Err(ClaudeWebFetchError::Cancelled)
                } else {
                    Ok(None)
                }
            }
            Err(_) => {
                child_cancel.cancel();
                if call.cancel.is_cancelled() {
                    Err(ClaudeWebFetchError::Cancelled)
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Classifies a non-200 HTTP response from `claude.ai/api`.
    pub fn classify_http_error(
        status: u16,
        headers: &RedactedHeaders,
        body: &[u8],
    ) -> ClaudeWebFetchError {
        match status {
            401 => ClaudeWebFetchError::Unauthorized,
            403 if Self::is_cloudflare_challenge(headers, body) => {
                ClaudeWebFetchError::CloudflareChallenge
            }
            403 => ClaudeWebFetchError::Unauthorized,
            code => ClaudeWebFetchError::ServerError { status: code },
        }
    }

    /// Detects whether an HTTP response is a Cloudflare challenge interstitial.
    pub fn is_cloudflare_challenge(headers: &RedactedHeaders, body: &[u8]) -> bool {
        if let Some(cf) = headers.get("cf-mitigated").and_then(|v| v.to_str().ok())
            && cf.trim().eq_ignore_ascii_case("challenge")
        {
            return true;
        }
        let prefix_len = body.len().min(64 * 1024);
        let Ok(body_prefix) = std::str::from_utf8(&body[..prefix_len]) else {
            return false;
        };
        body_prefix.to_ascii_lowercase().contains("just a moment")
    }

    /// Parses `/api/organizations/{org_id}/usage` JSON into `WebUsageData`.
    pub fn parse_usage_response(
        data: &[u8],
        now: Timestamp,
    ) -> Result<WebUsageData, ClaudeWebFetchError> {
        let value: Value =
            serde_json::from_slice(data).map_err(|_| ClaudeWebFetchError::InvalidResponse)?;
        let obj = value
            .as_object()
            .ok_or(ClaudeWebFetchError::InvalidResponse)?;

        let mut session_percent = None;
        let mut session_resets = None;
        if let Some(five_hour) = obj.get("five_hour").and_then(Value::as_object) {
            session_percent = strict_number_percent(five_hour.get("utilization"));
            session_resets = five_hour
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(parse_iso8601_timestamp);
        }

        let mut weekly_percent = None;
        let mut weekly_resets = None;
        if let Some(seven_day) = obj.get("seven_day").and_then(Value::as_object) {
            weekly_percent = strict_number_percent(seven_day.get("utilization"));
            weekly_resets = seven_day
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(parse_iso8601_timestamp);
        }

        let opus_percent =
            if let Some(sonnet) = obj.get("seven_day_sonnet").and_then(Value::as_object) {
                strict_number_percent(sonnet.get("utilization"))
            } else if let Some(opus) = obj.get("seven_day_opus").and_then(Value::as_object) {
                strict_number_percent(opus.get("utilization"))
            } else {
                None
            };

        let (extra_rate_windows, _source_keys) = ClaudeWebExtraRateWindowParser::parse(obj);
        let extra_usage_cost = parse_inline_extra_usage_cost(obj.get("extra_usage"), now);

        let reset_credits = obj
            .get("cedar_ember")
            .and_then(OAuthCedarEmberPayload::from_json_value)
            .and_then(|p| ClaudeRateLimitResetCredits::from_payload(Some(&p), now));

        let cloud_credits = obj
            .get("iguana_necktie")
            .and_then(|v| serde_json::from_value::<OAuthIguanaNecktiePayload>(v.clone()).ok())
            .and_then(|p| ClaudeCloudCreditsSnapshot::from_payload(Some(&p)));

        Ok(WebUsageData {
            session_percent_used: session_percent,
            session_resets_at: session_resets,
            weekly_percent_used: weekly_percent,
            weekly_resets_at: weekly_resets,
            opus_percent_used: opus_percent,
            extra_rate_windows,
            extra_usage_cost,
            reset_credits,
            cloud_credits,
            account_organization: None,
            account_organization_id: None,
            account_email: None,
            login_method: None,
        })
    }

    /// Parses `/api/organizations` JSON and selects the target or preferred chat organization.
    pub fn parse_organizations_response(
        data: &[u8],
        target_organization_id: Option<&str>,
    ) -> Result<OrganizationInfo, ClaudeWebFetchError> {
        let organizations: Vec<ClaudeWebOrganizationResponse> =
            serde_json::from_slice(data).map_err(|_| ClaudeWebFetchError::InvalidResponse)?;

        if let Some(target) = target_organization_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let selected = organizations
                .into_iter()
                .find(|org| org.uuid == target)
                .ok_or_else(|| ClaudeWebFetchError::OrganizationNotFound(target.to_owned()))?;
            return Ok(selected.into_organization_info());
        }

        let selected = organizations
            .iter()
            .find(|org| org.has_chat_capability())
            .or_else(|| organizations.iter().find(|org| !org.is_api_only()))
            .or_else(|| organizations.first())
            .ok_or(ClaudeWebFetchError::NoOrganization)?;

        Ok(selected.clone().into_organization_info())
    }

    /// Parses `/api/organizations/{org_id}/overage_spend_limit` JSON into `ProviderCostSnapshot`.
    pub fn parse_overage_spend_limit(data: &[u8], now: Timestamp) -> Option<ProviderCostSnapshot> {
        let decoded: OverageSpendLimitResponse = serde_json::from_slice(data).ok()?;
        if decoded.is_enabled != Some(true) {
            return None;
        }
        let used = decoded.used_credits.filter(|v| v.is_finite())?;
        let limit = decoded.monthly_credit_limit.filter(|v| v.is_finite())?;
        let currency = decoded
            .currency
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        Some(make_web_extra_usage_cost(used, limit, currency, now))
    }

    /// Parses `/api/organizations/{org_id}/prepaid/credits` JSON into `PrepaidBalance`.
    pub fn parse_prepaid_balance(data: &[u8]) -> Option<PrepaidBalance> {
        let decoded: PrepaidCreditsResponse = serde_json::from_slice(data).ok()?;
        if !decoded.amount.is_finite() || decoded.amount < 0.0 {
            return None;
        }
        let currency = decoded.currency.trim().to_ascii_uppercase();
        if currency.is_empty() {
            return None;
        }
        Some(PrepaidBalance {
            amount: decoded.amount / 100.0,
            currency_code: currency,
        })
    }

    /// Parses `/api/organizations/{org_id}/prepaid/credits` directly into a `ProviderCostSnapshot`.
    pub fn parse_prepaid_credits(data: &[u8], now: Timestamp) -> Option<ProviderCostSnapshot> {
        let balance = Self::parse_prepaid_balance(data)?;
        Some(Self::applying_prepaid_balance(&balance, None, now))
    }

    /// Merges a `PrepaidBalance` into an existing `ProviderCostSnapshot` (or creates a balance-only snapshot).
    pub fn applying_prepaid_balance(
        balance: &PrepaidBalance,
        cost: Option<ProviderCostSnapshot>,
        now: Timestamp,
    ) -> ProviderCostSnapshot {
        if let Some(mut existing) = cost {
            if existing
                .currency_code
                .eq_ignore_ascii_case(&balance.currency_code)
            {
                existing.balance = Some(balance.amount);
            }
            return existing;
        }
        let mut snapshot = ProviderCostSnapshot::new(0.0, 0.0, balance.currency_code.clone(), now);
        snapshot.period = Some("Extra usage".to_owned());
        snapshot.balance = Some(balance.amount);
        snapshot
    }

    /// Parses `/api/account` JSON into `WebAccountInfo`.
    pub fn parse_account_info(data: &[u8], org_id: Option<&str>) -> Option<WebAccountInfo> {
        let response: AccountResponse = serde_json::from_slice(data).ok()?;
        let email = response
            .email_address
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let membership = select_membership(response.memberships.as_deref(), org_id);
        let login_method = ClaudePlan::web_login_method(
            membership.and_then(|m| m.organization.rate_limit_tier.as_deref()),
            membership.and_then(|m| m.organization.billing_type.as_deref()),
            membership.and_then(|m| m.seat_tier.as_deref()),
        );
        Some(WebAccountInfo {
            email,
            login_method,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ClaudeWebOrganizationResponse {
    uuid: String,
    name: Option<String>,
    capabilities: Option<Vec<String>>,
}

impl ClaudeWebOrganizationResponse {
    fn normalized_capabilities(&self) -> BTreeSet<String> {
        self.capabilities
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|c| c.to_ascii_lowercase())
            .collect()
    }

    fn has_chat_capability(&self) -> bool {
        self.normalized_capabilities().contains("chat")
    }

    fn is_api_only(&self) -> bool {
        let caps = self.normalized_capabilities();
        caps.len() == 1 && caps.contains("api")
    }

    fn into_organization_info(self) -> OrganizationInfo {
        let name = self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        OrganizationInfo {
            id: self.uuid,
            name,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OverageSpendLimitResponse {
    #[serde(rename = "monthly_credit_limit")]
    monthly_credit_limit: Option<f64>,
    currency: Option<String>,
    #[serde(rename = "used_credits")]
    used_credits: Option<f64>,
    #[serde(rename = "is_enabled")]
    is_enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct PrepaidCreditsResponse {
    amount: f64,
    currency: String,
}

#[derive(Debug, Deserialize)]
struct AccountResponse {
    #[serde(rename = "email_address")]
    email_address: Option<String>,
    memberships: Option<Vec<AccountMembership>>,
}

#[derive(Debug, Deserialize)]
struct AccountMembership {
    organization: AccountOrganization,
    #[serde(rename = "seat_tier")]
    seat_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccountOrganization {
    uuid: Option<String>,
    #[serde(rename = "rate_limit_tier")]
    rate_limit_tier: Option<String>,
    #[serde(rename = "billing_type")]
    billing_type: Option<String>,
}

fn select_membership<'a>(
    memberships: Option<&'a [AccountMembership]>,
    org_id: Option<&str>,
) -> Option<&'a AccountMembership> {
    let memberships = memberships?;
    if memberships.is_empty() {
        return None;
    }
    if let Some(target_org) = org_id
        && let Some(matched) = memberships
            .iter()
            .find(|m| m.organization.uuid.as_deref() == Some(target_org))
    {
        return Some(matched);
    }
    memberships.first()
}

fn parse_inline_extra_usage_cost(
    extra_usage: Option<&Value>,
    now: Timestamp,
) -> Option<ProviderCostSnapshot> {
    let obj = extra_usage?.as_object()?;
    let used = flexible_double_value(obj.get("used_credits"))?;
    let limit = flexible_double_value(
        obj.get("monthly_limit")
            .or_else(|| obj.get("monthly_credit_limit")),
    )?;
    if limit <= 0.0 {
        return None;
    }
    let currency = obj
        .get("currency")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("USD");
    Some(make_web_extra_usage_cost(used, limit, currency, now))
}

fn make_web_extra_usage_cost(
    used_credits: f64,
    monthly_credit_limit: f64,
    currency_code: &str,
    now: Timestamp,
) -> ProviderCostSnapshot {
    let mut snapshot = ProviderCostSnapshot::new(
        used_credits / 100.0,
        monthly_credit_limit / 100.0,
        currency_code,
        now,
    );
    snapshot.period = Some("Monthly cap".to_owned());
    snapshot
}

fn strict_number_percent(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64().filter(|v| v.is_finite()),
        _ => None,
    }
}

fn flexible_double_value(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64().filter(|v| v.is_finite()),
        Value::String(s) => s.parse::<f64>().ok().filter(|v| v.is_finite()),
        _ => None,
    }
}

fn find_session_key_in_pairs(pairs: &[(String, String)]) -> Option<SecretString> {
    for (name, val) in pairs {
        if name == SESSION_KEY_COOKIE_NAME {
            let trimmed = val.trim();
            if trimmed.starts_with(SESSION_KEY_PREFIX) {
                return Some(SecretString::new(trimmed.to_owned()));
            }
        }
    }
    None
}

fn find_session_key_in_records(records: &[CookieRecord]) -> Option<SecretString> {
    for record in records {
        if record.name == SESSION_KEY_COOKIE_NAME {
            let trimmed = record.value.expose_secret().trim();
            if trimmed.starts_with(SESSION_KEY_PREFIX) {
                return Some(SecretString::new(trimmed.to_owned()));
            }
        }
    }
    None
}

fn build_web_get_request(url: Url, session_key: &SecretString) -> NetRequest {
    let mut req = NetRequest::new(Method::GET, url);
    req.timeout = Some(DEFAULT_WEB_REQUEST_TIMEOUT);
    req.headers
        .insert(ACCEPT, HeaderValue::from_static("application/json"));
    let hdr_val = format!(
        "{}={}",
        SESSION_KEY_COOKIE_NAME,
        session_key.expose_secret()
    );
    req.credentials.push((COOKIE, SecretString::new(hdr_val)));
    req
}

fn map_web_net_error(err: NetError) -> ClaudeWebFetchError {
    match err {
        NetError::Cancelled => ClaudeWebFetchError::Cancelled,
        NetError::Transport(t) => ClaudeWebFetchError::NetworkError(Some(t)),
        NetError::Http { status: 401 } => ClaudeWebFetchError::Unauthorized,
        NetError::Http { status: 403 } => ClaudeWebFetchError::Unauthorized,
        NetError::Http { status } => ClaudeWebFetchError::ServerError { status },
        _ => ClaudeWebFetchError::NetworkError(None),
    }
}
