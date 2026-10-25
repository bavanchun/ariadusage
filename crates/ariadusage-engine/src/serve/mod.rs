// Ported from CodexBar Sources/CodexBarCLI/CLIServeCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIServeHeapTrimmer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIHelp.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
//
// `serve` runtime, router, response cache, operation fingerprinting, and CLI
// option decoders.

pub mod accounts;
pub mod auth;
pub mod coordinator;
pub mod dashboard;
pub mod http_server;
pub mod provider_icons;
pub mod web_ui;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::config::{Config, encode, normalize};
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use serde::Serialize;
use tokio::task::AbortHandle;

use crate::config_store::ConfigStore;
use accounts::{FileManagedCodexAccountStore, ManagedCodexAccount, serve_accounts_route};
use auth::{CliServeDashboardAuth, CliServeSecurity, serve_unauthorized_response};
use coordinator::{CliServeOperationCoordinator, CliServeOperationProgressSink};
use dashboard::{
    CostPayload, DashboardIdentityMode, DashboardSnapshotBuilder, DashboardSnapshotDetail,
    DashboardSnapshotPayload, ProviderErrorPayload, ProviderPayload, UsageCommandOutput,
    usage_cache_account_key,
};
use http_server::{CliLocalHttpRequest, CliLocalHttpResponse};

pub const DEFAULT_SERVE_PORT: u16 = 8080;
pub const DEFAULT_SERVE_REFRESH_INTERVAL: f64 = 60.0;
pub const DEFAULT_SERVE_REQUEST_TIMEOUT: f64 = 30.0;
pub const MAXIMUM_SERVE_REQUEST_TIMEOUT: f64 = 86_400.0;
pub const SERVE_COST_REFRESHES_PRICING_IN_BACKGROUND: bool = true;
pub const DASHBOARD_COST_REFRESHES_PRICING_IN_BACKGROUND: bool = false;
pub const DASHBOARD_TOKEN_ENV_VAR: &str = "ARIADUSAGE_DASHBOARD_TOKEN";

/// Signal monitor descriptor for interactive and hangup termination (`CLITerminationSignalMonitor`).
pub struct CliTerminationSignalMonitor {
    triggered: Arc<std::sync::atomic::AtomicBool>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
    on_signal: Arc<dyn Fn() + Send + Sync>,
}

impl CliTerminationSignalMonitor {
    /// `[SIGINT, SIGTERM, SIGHUP]` on POSIX platforms.
    pub const SIGNAL_NUMBERS: [i32; 3] = [2, 15, 1];

    pub fn new<F>(on_signal: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self {
            triggered: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stopped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            on_signal: Arc::new(on_signal),
        }
    }

    pub fn trigger_for_testing(&self) -> bool {
        if self.stopped.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        if self
            .triggered
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return false;
        }
        (self.on_signal)();
        true
    }

    pub fn stop(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// v1 provider slugs recognized by `ProviderSelection(argument:)` in CLI/serve routes.
pub const KNOWN_PROVIDER_SLUGS: &[&str] = &["claude", "codex", "antigravity"];

pub const COST_SUPPORTED_PROVIDERS: &[&str] = &["claude", "codex", "antigravity"];

/// Route matched by `CliServeRouter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliServeRoute {
    WebUi,
    ProviderIcon {
        name: String,
    },
    Health,
    Accounts {
        id: Option<String>,
    },
    Usage {
        provider: Option<String>,
    },
    Cost {
        provider: Option<String>,
    },
    DashboardSnapshot {
        provider: Option<String>,
        detail: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CliServeRouteError {
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("not found")]
    NotFound,
}

/// Request router for `ariadusage serve` (`CLIServeRouter`).
pub struct CliServeRouter;

impl CliServeRouter {
    pub fn route(
        method: &str,
        path: &str,
        query_items: &[(String, Option<String>)],
    ) -> Result<CliServeRoute, CliServeRouteError> {
        if !method.eq_ignore_ascii_case("GET") {
            return Err(CliServeRouteError::MethodNotAllowed);
        }
        let provider = query_items
            .iter()
            .rev()
            .find_map(|(k, v)| (k == "provider").then_some(v.as_deref()).flatten())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        if path == "/" {
            return Ok(CliServeRoute::WebUi);
        }
        if let Some(rest) = path.strip_prefix("/icons/")
            && let Some(name) = rest.strip_suffix(".svg")
        {
            return Ok(CliServeRoute::ProviderIcon {
                name: name.to_string(),
            });
        }
        if path == "/health" {
            return Ok(CliServeRoute::Health);
        }
        if path == "/accounts" {
            return Ok(CliServeRoute::Accounts { id: None });
        }
        if let Some(id) = path.strip_prefix("/accounts/") {
            if id.is_empty() || id.contains('/') || id == "." || id == ".." {
                return Err(CliServeRouteError::NotFound);
            }
            return Ok(CliServeRoute::Accounts {
                id: Some(id.to_string()),
            });
        }
        if path == "/usage" {
            return Ok(CliServeRoute::Usage { provider });
        }
        if path == "/cost" {
            return Ok(CliServeRoute::Cost { provider });
        }
        if path == "/dashboard/v1/snapshot" {
            let detail = query_items
                .iter()
                .rev()
                .find_map(|(k, v)| (k == "detail").then_some(v.as_deref()).flatten())
                .map(str::trim)
                .map(ToString::to_string);
            return Ok(CliServeRoute::DashboardSnapshot { provider, detail });
        }
        Err(CliServeRouteError::NotFound)
    }
}

/// Argument validation error for `serve` flags and query parameters.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliServeArgumentError {
    #[error("--host must be 'localhost' or an IPv4 address.")]
    InvalidHost,
    #[error("--port must be between 1 and 65535.")]
    InvalidPort,
    #[error("--refresh-interval must be zero or greater.")]
    InvalidRefreshInterval,
    #[error("--request-timeout must be zero or greater.")]
    InvalidRequestTimeout,
    #[error(
        "--dashboard-token is not supported because secrets must not be passed on argv; set serve.dashboardToken with `ariadusage secret set --id serve.dashboardToken` or set ARIADUSAGE_DASHBOARD_TOKEN."
    )]
    UnsupportedDashboardTokenFlag,
    #[error("{source_name} must not be empty or whitespace.")]
    EmptyDashboardToken { source_name: String },
    #[error("Unknown provider '{0}'.")]
    InvalidProvider(String),
    #[error("Unknown dashboard detail '{0}'.")]
    InvalidDashboardDetail(String),
    #[error("--identity must be redacted or full.")]
    InvalidIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliServeDashboardTokenResolution {
    Absent,
    Token(SecretString),
    Empty { source: String },
}

#[must_use]
pub fn resolve_dashboard_token(
    secret_store_token: Option<&SecretString>,
    environment: &BTreeMap<String, String>,
) -> CliServeDashboardTokenResolution {
    if let Some(secret) = secret_store_token {
        let trimmed = secret.expose_secret().trim();
        return if trimmed.is_empty() {
            CliServeDashboardTokenResolution::Empty {
                source: "serve.dashboardToken".to_string(),
            }
        } else {
            CliServeDashboardTokenResolution::Token(SecretString::new(trimmed.to_string()))
        };
    }
    if let Some(raw) = environment.get(DASHBOARD_TOKEN_ENV_VAR) {
        let trimmed = raw.trim();
        return if trimmed.is_empty() {
            CliServeDashboardTokenResolution::Empty {
                source: DASHBOARD_TOKEN_ENV_VAR.to_string(),
            }
        } else {
            CliServeDashboardTokenResolution::Token(SecretString::new(trimmed.to_string()))
        };
    }
    CliServeDashboardTokenResolution::Absent
}

#[must_use]
pub fn decode_serve_host(raw: Option<&str>) -> Option<String> {
    let raw = raw.unwrap_or("127.0.0.1");
    let host = raw.trim();
    if host.is_empty() {
        return None;
    }
    let bind_host = CliServeSecurity::bind_host(Some(host));
    CliServeSecurity::is_supported_ipv4_bind_host(&bind_host).then(|| host.to_string())
}

#[must_use]
pub fn decode_serve_port(raw: Option<&str>) -> Option<u16> {
    let Some(raw) = raw else {
        return Some(DEFAULT_SERVE_PORT);
    };
    let parsed: i64 = raw.trim().parse().ok()?;
    u16::try_from(parsed).ok().filter(|&p| p > 0)
}

#[must_use]
pub fn decode_serve_refresh_interval(raw: Option<&str>) -> Option<f64> {
    let Some(raw) = raw else {
        return Some(DEFAULT_SERVE_REFRESH_INTERVAL);
    };
    let parsed: f64 = raw.trim().parse().ok()?;
    (parsed.is_finite() && parsed >= 0.0).then_some(parsed)
}

#[must_use]
pub fn decode_serve_request_timeout(raw: Option<&str>) -> Option<f64> {
    let Some(raw) = raw else {
        return Some(DEFAULT_SERVE_REQUEST_TIMEOUT);
    };
    let parsed: f64 = raw.trim().parse().ok()?;
    (parsed.is_finite() && parsed >= 0.0).then_some(parsed)
}

#[must_use]
pub fn decode_dashboard_timeout(raw: Option<&str>) -> Option<f64> {
    let Some(raw) = raw else {
        return Some(DEFAULT_SERVE_REQUEST_TIMEOUT);
    };
    let parsed: f64 = raw.trim().parse().ok()?;
    (parsed.is_finite() && (0.0..=MAXIMUM_SERVE_REQUEST_TIMEOUT).contains(&parsed))
        .then_some(parsed)
}

#[must_use]
pub fn decode_dashboard_identity_mode(raw: Option<&str>) -> Option<DashboardIdentityMode> {
    let Some(raw) = raw else {
        return Some(DashboardIdentityMode::Full);
    };
    match DashboardIdentityMode::parse(raw) {
        Some(DashboardIdentityMode::Redacted) => Some(DashboardIdentityMode::Redacted),
        Some(DashboardIdentityMode::Full) => Some(DashboardIdentityMode::Full),
        _ => None,
    }
}

#[must_use]
pub fn decode_dashboard_output_path(raw: Option<&str>) -> Option<Option<std::path::PathBuf>> {
    let Some(raw) = raw else {
        return Some(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(Some(std::path::PathBuf::from(trimmed)))
}

#[must_use]
pub fn resolve_dashboard_identity_mode(
    configured: Option<DashboardIdentityMode>,
    hides_personal_info: bool,
    all_accounts: bool,
) -> DashboardIdentityMode {
    if let Some(configured) = configured {
        return configured;
    }
    if all_accounts {
        return DashboardIdentityMode::None;
    }
    if hides_personal_info {
        DashboardIdentityMode::Redacted
    } else {
        DashboardIdentityMode::Full
    }
}

#[must_use]
pub fn dashboard_identity_flag_present(raw: Option<&str>) -> bool {
    raw.is_some()
}

#[must_use]
pub fn clamped_serve_request_timeout(request_timeout: f64) -> f64 {
    if !request_timeout.is_finite() {
        return MAXIMUM_SERVE_REQUEST_TIMEOUT;
    }
    request_timeout.clamp(0.0, MAXIMUM_SERVE_REQUEST_TIMEOUT)
}

#[must_use]
pub fn serve_provider_timeout(request_timeout: f64) -> Option<f64> {
    if !request_timeout.is_finite() || request_timeout <= 0.0 {
        return None;
    }
    let clamped = request_timeout.min(MAXIMUM_SERVE_REQUEST_TIMEOUT);
    Some(clamped * 0.8)
}

#[must_use]
pub fn serve_request_deadline(started_at: Instant, request_timeout: f64) -> Option<Instant> {
    if !request_timeout.is_finite() || request_timeout <= 0.0 {
        return None;
    }
    let timeout = clamped_serve_request_timeout(request_timeout);
    if timeout <= 0.0 {
        return None;
    }
    Some(started_at + Duration::from_secs_f64(timeout))
}

#[must_use]
pub fn serve_provider_deadline(started_at: Instant, request_timeout: f64) -> Option<Instant> {
    let timeout = serve_provider_timeout(request_timeout)?;
    Some(started_at + Duration::from_secs_f64(timeout))
}

#[must_use]
pub fn serve_cost_provider_deadline(
    started_at: Instant,
    provider_timeout: Option<f64>,
    request_deadline: Option<Instant>,
) -> Option<Instant> {
    let Some(provider_timeout) = provider_timeout else {
        return request_deadline;
    };
    let provider_deadline = started_at + Duration::from_secs_f64(provider_timeout.max(0.0));
    match request_deadline {
        Some(req_deadline) => Some(provider_deadline.min(req_deadline)),
        None => Some(provider_deadline),
    }
}

#[must_use]
pub fn serve_stale_ttl(refresh_interval: f64) -> f64 {
    if refresh_interval <= 0.0 {
        return 0.0;
    }
    if !refresh_interval.is_finite() {
        return CliServeResponseCache::MAXIMUM_STALE_TTL;
    }
    (refresh_interval * 10.0).clamp(300.0, CliServeResponseCache::MAXIMUM_STALE_TTL)
}

#[must_use]
pub fn serve_cli_session_idle_window(refresh_interval: f64) -> f64 {
    180.0f64.max(refresh_interval + 60.0)
}

#[must_use]
pub fn serve_usage_operation_fingerprint(
    config_fingerprint: &str,
    include_all_codex_accounts: bool,
    include_all_accounts: bool,
) -> String {
    format!(
        "{config_fingerprint}|codexAccounts={}|allAccounts={}",
        if include_all_codex_accounts {
            "all"
        } else {
            "selected"
        },
        if include_all_accounts {
            "all"
        } else {
            "selected"
        }
    )
}

#[derive(Debug, Clone)]
pub struct CliServeConfigSnapshot {
    pub config: Config,
    pub cache_token: String,
}

pub fn load_serve_config_snapshot(
    config_store: &ConfigStore,
) -> Result<CliServeConfigSnapshot, String> {
    let config = config_store
        .load_effective()
        .map_err(|_| "could not load configuration".to_string())?;
    let cache_token = serve_config_cache_token(&config);
    Ok(CliServeConfigSnapshot {
        config,
        cache_token,
    })
}

#[must_use]
pub fn serve_config_cache_token(config: &Config) -> String {
    let normalized = normalize(config.clone());
    let data = encode(&normalized);
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    format!("{hash:x}")
}

pub fn serve_dashboard_operation_key(
    identity_mode: DashboardIdentityMode,
    usage_bars_show_used: bool,
    provider: Option<&str>,
    all_accounts: bool,
) -> Result<String, CliServeArgumentError> {
    let fill = if usage_bars_show_used {
        "used"
    } else {
        "remaining"
    };
    let suffix = if all_accounts { "-all-accounts" } else { "" };
    let kind = format!("dashboard-{}-{fill}{suffix}", identity_mode.as_str());
    serve_operation_key(&kind, provider)
}

pub fn serve_operation_key(
    kind: &str,
    provider: Option<&str>,
) -> Result<String, CliServeArgumentError> {
    let Some(provider) = provider else {
        return Ok(format!("{kind}:default"));
    };
    let resolved = parse_provider_selection(provider)?;
    Ok(format!("{kind}:{}", resolved.join(",")))
}

#[must_use]
pub fn serve_cache_key(operation_key: &str, config_token: &str) -> String {
    format!("{operation_key}:{config_token}")
}

pub fn parse_provider_selection(raw: &str) -> Result<Vec<String>, CliServeArgumentError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliServeArgumentError::InvalidProvider(raw.to_string()));
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "all" || lower == "both" {
        return Ok(vec![lower]);
    }
    let mut out = Vec::new();
    for part in trimmed.split(',') {
        let p = part.trim().to_ascii_lowercase();
        if p.is_empty() || !KNOWN_PROVIDER_SLUGS.contains(&p.as_str()) {
            return Err(CliServeArgumentError::InvalidProvider(trimmed.to_string()));
        }
        out.push(p);
    }
    Ok(out)
}

#[must_use]
pub fn should_cache_serve_response(response: &CliLocalHttpResponse) -> bool {
    if response.status_code != 200 {
        return false;
    }
    if let Ok(items) = serde_json::from_slice::<Vec<serde_json::Value>>(&response.body) {
        return !items.iter().any(value_has_error);
    }
    if let Ok(obj) = serde_json::from_slice::<serde_json::Value>(&response.body)
        && let Some(providers) = obj.get("providers").and_then(serde_json::Value::as_array)
    {
        return !providers.iter().any(value_has_error);
    }
    true
}

fn value_has_error(item: &serde_json::Value) -> bool {
    item.get("error").is_some_and(|e| !e.is_null())
}

#[derive(Debug, Clone)]
pub struct CliServeCoordinatedResponse {
    pub response: CliLocalHttpResponse,
    pub is_committed: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CachePolicy {
    pub ttl: f64,
    pub stale_ttl: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct UsageItemKey {
    provider: String,
    account_id: String,
}

#[derive(Debug, Clone)]
struct Entry {
    expires_at: Timestamp,
    response: CliLocalHttpResponse,
}

#[derive(Debug, Clone)]
struct LastGoodEntry {
    recorded_at: Timestamp,
    response: CliLocalHttpResponse,
}

#[derive(Debug, Clone)]
struct LastGoodItem {
    recorded_at: Timestamp,
    value: serde_json::Value,
}

#[derive(Default)]
struct ResponseCacheInner {
    entries: HashMap<String, Entry>,
    last_good: HashMap<String, LastGoodEntry>,
    last_good_usage_items: HashMap<String, HashMap<UsageItemKey, LastGoodItem>>,
    last_good_cost_items: HashMap<String, HashMap<String, LastGoodItem>>,
    last_good_cost_order: HashMap<String, Vec<String>>,
}

/// Response cache with stale-while-revalidate and per-account / per-provider
/// error-row replacement (`CLIServeResponseCache`).
#[derive(Clone)]
pub struct CliServeResponseCache {
    pub operations: CliServeOperationCoordinator<CliServeCoordinatedResponse>,
    pub wall_clock: Arc<dyn Fn() -> Timestamp + Send + Sync>,
    inner: Arc<Mutex<ResponseCacheInner>>,
}

impl Default for CliServeResponseCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CliServeResponseCache {
    pub const MAXIMUM_STALE_TTL: f64 = 3600.0;

    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(
            CliServeOperationCoordinator::new(),
            Arc::new(Timestamp::now),
        )
    }

    #[must_use]
    pub fn with_clock(
        operations: CliServeOperationCoordinator<CliServeCoordinatedResponse>,
        wall_clock: Arc<dyn Fn() -> Timestamp + Send + Sync>,
    ) -> Self {
        Self {
            operations,
            wall_clock,
            inner: Arc::new(Mutex::new(ResponseCacheInner::default())),
        }
    }

    #[must_use]
    pub fn cached_response(&self, key: &str, now: Timestamp) -> Option<CliLocalHttpResponse> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune_expired_entries(&mut guard, now);
        guard.entries.get(key).map(|e| e.response.clone())
    }

    #[must_use]
    pub fn stale_response_for_revalidation(
        &self,
        key: &str,
        stale_ttl: f64,
        now: Timestamp,
    ) -> Option<CliLocalHttpResponse> {
        if stale_ttl <= 0.0 {
            return None;
        }
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune_expired_entries(&mut guard, now);
        let entry = guard.last_good.get(key)?;
        if elapsed_seconds(now, entry.recorded_at) <= stale_ttl {
            Some(entry.response.clone())
        } else {
            None
        }
    }

    #[must_use]
    pub fn complete_fetch(
        &self,
        response: CliLocalHttpResponse,
        key: &str,
        policy: CachePolicy,
        now: Timestamp,
        should_cache: bool,
    ) -> CliLocalHttpResponse {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let stale_response = Self::stale_response(&mut guard, key, policy.stale_ttl, now);
        let usage_merge = Self::merge_last_good_usage_items(
            &mut guard,
            &response,
            key,
            policy.stale_ttl,
            now,
            should_cache,
        );
        let cost_merge = Self::merge_last_good_cost_items(
            &mut guard,
            &response,
            key,
            policy.stale_ttl,
            now,
            should_cache,
        );
        if should_cache {
            if policy.ttl > 0.0 && response.status_code == 200 {
                let ttl_ms = (policy.ttl * 1000.0) as i64;
                let expires_at = now
                    .checked_add(jiff::SignedDuration::from_millis(ttl_ms))
                    .unwrap_or(now);
                guard.entries.insert(
                    key.to_string(),
                    Entry {
                        expires_at,
                        response: response.clone(),
                    },
                );
            }
            guard.last_good.insert(
                key.to_string(),
                LastGoodEntry {
                    recorded_at: now,
                    response: response.clone(),
                },
            );
            return response;
        }
        usage_merge
            .or(cost_merge)
            .or(stale_response)
            .unwrap_or(response)
    }

    #[must_use]
    pub fn cached_entry_count(&self) -> usize {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.entries.len()
    }

    #[must_use]
    pub fn cached_stale_variant_count(&self) -> usize {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut keys: HashSet<&String> = guard.last_good.keys().collect();
        keys.extend(guard.last_good_usage_items.keys());
        keys.extend(guard.last_good_cost_items.keys());
        keys.len()
    }

    fn prune_expired_entries(inner: &mut ResponseCacheInner, now: Timestamp) {
        inner.entries.retain(|_, v| v.expires_at > now);
        inner
            .last_good
            .retain(|_, v| elapsed_seconds(now, v.recorded_at) <= Self::MAXIMUM_STALE_TTL);
        inner.last_good_usage_items.retain(|_, items| {
            items.retain(|_, item| {
                elapsed_seconds(now, item.recorded_at) <= Self::MAXIMUM_STALE_TTL
            });
            !items.is_empty()
        });
        inner.last_good_cost_items.retain(|_, items| {
            items.retain(|_, item| {
                elapsed_seconds(now, item.recorded_at) <= Self::MAXIMUM_STALE_TTL
            });
            !items.is_empty()
        });
        let valid_cost_keys: HashSet<String> = inner.last_good_cost_items.keys().cloned().collect();
        inner
            .last_good_cost_order
            .retain(|k, _| valid_cost_keys.contains(k));
    }

    fn stale_response(
        inner: &mut ResponseCacheInner,
        key: &str,
        stale_ttl: f64,
        now: Timestamp,
    ) -> Option<CliLocalHttpResponse> {
        if stale_ttl <= 0.0 {
            return None;
        }
        if key.starts_with("usage:") {
            return None;
        }
        if key.starts_with("cost:") {
            return Self::stale_cost_response(inner, key, stale_ttl, now);
        }
        if let Some(entry) = inner.last_good.get(key)
            && elapsed_seconds(now, entry.recorded_at) <= stale_ttl
        {
            return Some(entry.response.clone());
        }
        inner.last_good.remove(key);
        None
    }

    fn merge_last_good_usage_items(
        inner: &mut ResponseCacheInner,
        response: &CliLocalHttpResponse,
        key: &str,
        stale_ttl: f64,
        now: Timestamp,
        replace_cached_items: bool,
    ) -> Option<CliLocalHttpResponse> {
        if !key.starts_with("usage:") || response.status_code != 200 || stale_ttl <= 0.0 {
            return None;
        }
        let mut items: Vec<serde_json::Value> = serde_json::from_slice(&response.body).ok()?;
        let mut cached_items = if replace_cached_items {
            HashMap::new()
        } else {
            inner.last_good_usage_items.remove(key).unwrap_or_default()
        };
        if !replace_cached_items {
            cached_items.retain(|_, v| elapsed_seconds(now, v.recorded_at) <= stale_ttl);
        }

        let mut counts: HashMap<UsageItemKey, usize> = HashMap::new();
        for (idx, item) in items.iter().enumerate() {
            if let Some(item_key) = Self::usage_item_key(
                item,
                Self::cache_account_key_at(idx, &response.usage_cache_keys),
            ) {
                *counts.entry(item_key).or_insert(0) += 1;
            }
        }
        let duplicate_keys: HashSet<UsageItemKey> = counts
            .into_iter()
            .filter_map(|(k, c)| (c > 1).then_some(k))
            .collect();
        for dup in &duplicate_keys {
            cached_items.remove(dup);
        }

        let mut replaced_error = false;
        for (idx, item) in items.iter_mut().enumerate() {
            let Some(item_key) = Self::usage_item_key(
                item,
                Self::cache_account_key_at(idx, &response.usage_cache_keys),
            ) else {
                continue;
            };
            if duplicate_keys.contains(&item_key) {
                continue;
            }
            if value_has_error(item) {
                if let Some(cached) = cached_items.get(&item_key) {
                    *item = cached.value.clone();
                    replaced_error = true;
                }
            } else {
                cached_items.insert(
                    item_key,
                    LastGoodItem {
                        recorded_at: now,
                        value: item.clone(),
                    },
                );
            }
        }
        inner
            .last_good_usage_items
            .insert(key.to_string(), cached_items);

        if !replaced_error {
            return Some(response.clone());
        }
        let body = serde_json::to_vec(&items).ok()?;
        Some(CliLocalHttpResponse {
            status_code: response.status_code,
            reason_phrase: response.reason_phrase.clone(),
            content_type: response.content_type.clone(),
            headers: response.headers.clone(),
            body,
            usage_cache_keys: response.usage_cache_keys.clone(),
        })
    }

    fn merge_last_good_cost_items(
        inner: &mut ResponseCacheInner,
        response: &CliLocalHttpResponse,
        key: &str,
        stale_ttl: f64,
        now: Timestamp,
        replace_cached_items: bool,
    ) -> Option<CliLocalHttpResponse> {
        if !key.starts_with("cost:") || response.status_code != 200 || stale_ttl <= 0.0 {
            return None;
        }
        let mut items: Vec<serde_json::Value> = serde_json::from_slice(&response.body).ok()?;
        let providers: Vec<String> = items
            .iter()
            .filter_map(|item| {
                item.get("provider")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
            })
            .collect();
        let unique_providers: HashSet<&String> = providers.iter().collect();
        if providers.len() != items.len() || unique_providers.len() != providers.len() {
            return Some(response.clone());
        }

        let mut cached_items = if replace_cached_items {
            HashMap::new()
        } else {
            inner.last_good_cost_items.remove(key).unwrap_or_default()
        };
        if !replace_cached_items {
            cached_items.retain(|_, v| elapsed_seconds(now, v.recorded_at) <= stale_ttl);
        }

        for (idx, item) in items.iter_mut().enumerate() {
            let provider = &providers[idx];
            if value_has_error(item) {
                if let Some(cached) = cached_items.get(provider) {
                    *item = cached.value.clone();
                }
            } else {
                cached_items.insert(
                    provider.clone(),
                    LastGoodItem {
                        recorded_at: now,
                        value: item.clone(),
                    },
                );
            }
        }
        inner
            .last_good_cost_items
            .insert(key.to_string(), cached_items);
        inner
            .last_good_cost_order
            .insert(key.to_string(), providers);

        let body = serde_json::to_vec(&items).ok()?;
        Some(CliLocalHttpResponse {
            status_code: response.status_code,
            reason_phrase: response.reason_phrase.clone(),
            content_type: response.content_type.clone(),
            headers: response.headers.clone(),
            body,
            usage_cache_keys: response.usage_cache_keys.clone(),
        })
    }

    fn stale_cost_response(
        inner: &ResponseCacheInner,
        key: &str,
        stale_ttl: f64,
        now: Timestamp,
    ) -> Option<CliLocalHttpResponse> {
        let order = inner.last_good_cost_order.get(key)?;
        if order.is_empty() {
            return None;
        }
        let cached_items = inner.last_good_cost_items.get(key)?;
        let mut rows = Vec::with_capacity(order.len());
        for provider in order {
            let cached = cached_items.get(provider)?;
            if elapsed_seconds(now, cached.recorded_at) > stale_ttl {
                return None;
            }
            rows.push(cached.value.clone());
        }
        let body = serde_json::to_vec(&rows).ok()?;
        Some(CliLocalHttpResponse::json(200, "OK", body))
    }

    fn usage_item_key(item: &serde_json::Value, account_id: Option<&str>) -> Option<UsageItemKey> {
        let provider = item
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())?;
        let account_id = account_id.filter(|s| !s.is_empty())?;
        Some(UsageItemKey {
            provider: provider.to_string(),
            account_id: account_id.to_string(),
        })
    }

    fn cache_account_key_at(index: usize, keys: &Option<Vec<Option<String>>>) -> Option<&str> {
        keys.as_ref()?.get(index)?.as_deref()
    }
}

fn elapsed_seconds(now: Timestamp, earlier: Timestamp) -> f64 {
    let diff_ms = now.as_millisecond() - earlier.as_millisecond();
    (diff_ms as f64) / 1000.0
}

#[derive(Debug, Clone)]
pub struct ServeResponseRequest {
    pub key: String,
    pub config_fingerprint: String,
    pub refresh_interval: f64,
    pub deadline: Option<Instant>,
    pub allows_stale_while_revalidate: bool,
}

pub async fn cached_serve_response<F, Fut>(
    key: &str,
    cache: &CliServeResponseCache,
    refresh_interval: f64,
    request_timeout: f64,
    config_fingerprint: &str,
    stale_while_revalidate: bool,
    make_response: F,
) -> CliLocalHttpResponse
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CliLocalHttpResponse> + Send + 'static,
{
    let request = ServeResponseRequest {
        key: key.to_string(),
        config_fingerprint: config_fingerprint.to_string(),
        refresh_interval,
        deadline: serve_request_deadline(Instant::now(), request_timeout),
        allows_stale_while_revalidate: stale_while_revalidate,
    };
    cached_serve_response_with_request(request, cache, Arc::new(move || Box::pin(make_response())))
        .await
}

pub async fn cached_serve_response_with_request(
    request: ServeResponseRequest,
    cache: &CliServeResponseCache,
    make_response: Arc<
        dyn Fn() -> Pin<Box<dyn Future<Output = CliLocalHttpResponse> + Send>> + Send + Sync,
    >,
) -> CliLocalHttpResponse {
    let cache_key = serve_cache_key(&request.key, &request.config_fingerprint);
    let now = (cache.wall_clock)();
    if let Some(cached) = cache.cached_response(&cache_key, now) {
        return cached;
    }

    let policy = CachePolicy {
        ttl: request.refresh_interval,
        stale_ttl: serve_stale_ttl(request.refresh_interval),
    };
    if request.allows_stale_while_revalidate
        && let Some(stale) = cache.stale_response_for_revalidation(
            &cache_key,
            policy.stale_ttl,
            (cache.wall_clock)(),
        )
    {
        let bg_req = request.clone();
        let bg_cache = cache.clone();
        let bg_key = cache_key.clone();
        let bg_make = Arc::clone(&make_response);
        tokio::spawn(async move {
            let _ = refresh_serve_response(&bg_req, &bg_key, policy, &bg_cache, bg_make).await;
        });
        return stale;
    }

    refresh_serve_response(&request, &cache_key, policy, cache, make_response).await
}

async fn refresh_serve_response(
    request: &ServeResponseRequest,
    cache_key: &str,
    policy: CachePolicy,
    cache: &CliServeResponseCache,
    make_response: Arc<
        dyn Fn() -> Pin<Box<dyn Future<Output = CliLocalHttpResponse> + Send>> + Send + Sync,
    >,
) -> CliLocalHttpResponse {
    let timeout_response = serve_timeout_response();
    let cache_for_accept = cache.clone();
    let cache_key_for_accept = cache_key.to_string();
    let cache_for_op = cache.clone();
    let cache_key_for_op = cache_key.to_string();
    let outcome = cache
        .operations
        .value(
            &request.key,
            Some(&request.config_fingerprint),
            request.deadline,
            CliServeCoordinatedResponse {
                response: timeout_response,
                is_committed: false,
            },
            move |fetched| {
                let cache = cache_for_accept.clone();
                let ck = cache_key_for_accept.clone();
                async move {
                    let should_cache = should_cache_serve_response(&fetched.response);
                    let committed = cache.complete_fetch(
                        fetched.response,
                        &ck,
                        policy,
                        (cache.wall_clock)(),
                        should_cache,
                    );
                    CliServeCoordinatedResponse {
                        response: committed,
                        is_committed: true,
                    }
                }
            },
            move |_sink| {
                let cache = cache_for_op.clone();
                let ck = cache_key_for_op.clone();
                let make = Arc::clone(&make_response);
                async move {
                    if let Some(hit) = cache.cached_response(&ck, (cache.wall_clock)()) {
                        return CliServeCoordinatedResponse {
                            response: hit,
                            is_committed: false,
                        };
                    }
                    let fetched = make().await;
                    CliServeCoordinatedResponse {
                        response: fetched,
                        is_committed: false,
                    }
                }
            },
        )
        .await;

    if outcome.is_committed {
        return outcome.response;
    }
    let should_cache = should_cache_serve_response(&outcome.response);
    cache.complete_fetch(
        outcome.response,
        cache_key,
        policy,
        (cache.wall_clock)(),
        should_cache,
    )
}

#[must_use]
pub fn serve_provider_timeout_output(provider: &str) -> UsageCommandOutput {
    let mut item = ProviderPayload::new(provider, "auto");
    item.error = Some(ProviderErrorPayload::provider(format!(
        "{provider} usage timed out"
    )));
    UsageCommandOutput {
        sections: Vec::new(),
        payload: vec![item],
        exit_code: 1,
    }
}

pub async fn serve_collect_usage_outputs<F, Fut>(
    providers: &[String],
    config_fingerprint: &str,
    deadline: Option<Instant>,
    operations: &CliServeOperationCoordinator<UsageCommandOutput>,
    fetch: F,
) -> UsageCommandOutput
where
    F: Fn(String, CliServeOperationProgressSink<UsageCommandOutput>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = UsageCommandOutput> + Send + 'static,
{
    let fetch = Arc::new(fetch);
    let mut handles = Vec::with_capacity(providers.len());
    for (idx, provider) in providers.iter().cloned().enumerate() {
        let ops = operations.clone();
        let fp = config_fingerprint.to_string();
        let fetch = Arc::clone(&fetch);
        handles.push(tokio::spawn(async move {
            let timeout = serve_provider_timeout_output(&provider);
            let p_clone = provider.clone();
            let out = ops
                .value(
                    &provider,
                    Some(&fp),
                    deadline,
                    timeout,
                    |v| async move { v },
                    move |sink| fetch(p_clone.clone(), sink),
                )
                .await;
            (idx, out)
        }));
    }

    let mut indexed = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(pair) = handle.await {
            indexed.push(pair);
        }
    }
    indexed.sort_by_key(|(idx, _)| *idx);
    let mut merged = UsageCommandOutput::default();
    for (_, out) in indexed {
        merged.merge(out);
    }
    merged
}

#[derive(Clone)]
pub struct ServeCostCollectionContext {
    pub config_fingerprint: String,
    pub provider_timeout: Option<f64>,
    pub request_deadline: Option<Instant>,
    pub now: Arc<dyn Fn() -> Instant + Send + Sync>,
    pub provider_operations: CliServeOperationCoordinator<CostPayload>,
}

#[must_use]
pub fn serve_cost_error_payload(provider: &str, message: &str) -> CostPayload {
    let mut item = CostPayload::new(provider, "local");
    item.updated_at = Some(Timestamp::now());
    item.error = Some(ProviderErrorPayload::provider(message));
    item
}

#[must_use]
pub fn serve_cost_timeout_payload(provider: &str) -> CostPayload {
    serve_cost_error_payload(provider, &format!("{provider} cost refresh timed out"))
}

pub async fn serve_collect_cost_payloads<F, Fut>(
    providers: &[String],
    context: &ServeCostCollectionContext,
    fetch: F,
) -> Vec<CostPayload>
where
    F: Fn(String, CliServeOperationProgressSink<CostPayload>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CostPayload> + Send + 'static,
{
    let fetch = Arc::new(fetch);
    let has_pi = providers.iter().any(|p| p == "pi");
    let mut payload = Vec::with_capacity(providers.len());

    for provider in providers.iter().cloned() {
        let now_instant = (context.now)();
        let deadline = serve_cost_provider_deadline(
            now_instant,
            context.provider_timeout,
            context.request_deadline,
        );
        let timeout = serve_cost_timeout_payload(&provider);
        let includes_pi = (provider == "claude" || provider == "codex") && !has_pi;
        let mode = if includes_pi { "inclusive" } else { "native" };
        let op_fp = format!(
            "{}|piAccounting={provider}:{mode}",
            context.config_fingerprint
        );
        let fetch = Arc::clone(&fetch);
        let p_clone = provider.clone();
        let item = context
            .provider_operations
            .value(
                &provider,
                Some(&op_fp),
                deadline,
                timeout,
                |v| async move { v },
                move |sink| fetch(p_clone.clone(), sink),
            )
            .await;
        payload.push(item);
    }
    payload
}

#[derive(Debug, Clone, Serialize)]
struct ServeErrorPayload<'a> {
    error: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct ServeHealthPayload<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
}

#[must_use]
pub fn serve_health_response(version: Option<&str>) -> CliLocalHttpResponse {
    serve_json(&ServeHealthPayload {
        status: "ok",
        version,
    })
}

#[must_use]
pub fn serve_timeout_response() -> CliLocalHttpResponse {
    serve_error(504, "Gateway Timeout", "request timed out")
}

#[must_use]
pub fn serve_error(status_code: u16, reason_phrase: &str, message: &str) -> CliLocalHttpResponse {
    let body = serde_json::to_vec(&ServeErrorPayload { error: message })
        .unwrap_or_else(|_| br#"{"error":"internal error"}"#.to_vec());
    CliLocalHttpResponse::json(status_code, reason_phrase, body)
}

#[must_use]
pub fn serve_json<T: Serialize>(payload: &T) -> CliLocalHttpResponse {
    let body = serde_json::to_vec(payload).unwrap_or_else(|_| b"{}".to_vec());
    CliLocalHttpResponse::json(200, "OK", body)
}

#[must_use]
pub fn adding_no_store(mut response: CliLocalHttpResponse) -> CliLocalHttpResponse {
    if !response
        .headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("cache-control"))
    {
        response
            .headers
            .insert("Cache-Control".to_string(), "no-store".to_string());
    }
    response
}

pub type UsageFetchCallback = Arc<
    dyn Fn(
            String,
            Config,
            CliServeOperationProgressSink<UsageCommandOutput>,
        ) -> Pin<Box<dyn Future<Output = UsageCommandOutput> + Send>>
        + Send
        + Sync,
>;

pub type CostFetchCallback = Arc<
    dyn Fn(
            String,
            Config,
            CliServeOperationProgressSink<CostPayload>,
        ) -> Pin<Box<dyn Future<Output = CostPayload> + Send>>
        + Send
        + Sync,
>;

/// Runtime state for `ariadusage serve` (`ServeRuntime`).
#[derive(Clone)]
pub struct ServeRuntime {
    pub config_store: Arc<ConfigStore>,
    pub managed_accounts_store: FileManagedCodexAccountStore,
    pub cache: CliServeResponseCache,
    pub provider_operations: CliServeOperationCoordinator<UsageCommandOutput>,
    pub cost_operations: CliServeOperationCoordinator<CostPayload>,
    pub refresh_interval: f64,
    pub request_timeout: f64,
    pub health_version: Option<String>,
    pub dashboard_auth: CliServeDashboardAuth,
    pub dashboard_identity_mode: Option<DashboardIdentityMode>,
    pub dashboard_all_accounts: bool,
    pub data_routes_require_auth: bool,
    pub hides_personal_info_override: Option<bool>,
    pub usage_bars_show_used_override: Option<bool>,
    pub usage_fetcher: UsageFetchCallback,
    pub cost_fetcher: CostFetchCallback,
}

impl ServeRuntime {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config_store: ConfigStore,
        refresh_interval: f64,
        request_timeout: f64,
        health_version: Option<String>,
        dashboard_auth: CliServeDashboardAuth,
        dashboard_identity_mode: Option<DashboardIdentityMode>,
        dashboard_all_accounts: bool,
        bind_host: &str,
    ) -> Self {
        Self {
            config_store: Arc::new(config_store),
            managed_accounts_store: FileManagedCodexAccountStore::with_optional_path(
                FileManagedCodexAccountStore::default_path(),
            ),
            cache: CliServeResponseCache::new(),
            provider_operations: CliServeOperationCoordinator::new(),
            cost_operations: CliServeOperationCoordinator::new(),
            refresh_interval,
            request_timeout,
            health_version,
            dashboard_auth,
            dashboard_identity_mode,
            dashboard_all_accounts,
            data_routes_require_auth: !CliServeSecurity::is_loopback_host(bind_host),
            hides_personal_info_override: None,
            usage_bars_show_used_override: None,
            usage_fetcher: Arc::new(|provider, _cfg, _sink| {
                Box::pin(async move {
                    let mut item = ProviderPayload::new(&provider, "auto");
                    item.error = Some(ProviderErrorPayload::provider(format!(
                        "{provider} usage fetcher not configured"
                    )));
                    UsageCommandOutput {
                        sections: Vec::new(),
                        payload: vec![item],
                        exit_code: 1,
                    }
                })
            }),
            cost_fetcher: Arc::new(|provider, _cfg, _sink| {
                Box::pin(async move {
                    serve_cost_error_payload(
                        &provider,
                        &format!("{provider} cost fetcher not configured"),
                    )
                })
            }),
        }
    }

    pub fn load_config(&self) -> Result<Config, String> {
        self.config_store
            .load_effective()
            .map_err(|err| err.to_string())
    }

    pub fn load_managed_codex_accounts(&self) -> Result<Vec<ManagedCodexAccount>, String> {
        self.managed_accounts_store
            .load_account_metadata()
            .map(|set| set.accounts)
            .map_err(|err| err.to_string())
    }

    #[must_use]
    pub fn hides_personal_info(&self, config: &Config) -> bool {
        if let Some(ov) = self.hides_personal_info_override {
            return ov;
        }
        config
            .settings
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok())
            .and_then(|v| {
                v.get("hidePersonalInfo")
                    .and_then(serde_json::Value::as_bool)
            })
            .unwrap_or(true)
    }

    #[must_use]
    pub fn usage_bars_show_used(&self, config: &Config) -> bool {
        if let Some(ov) = self.usage_bars_show_used_override {
            return ov;
        }
        config
            .settings
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok())
            .and_then(|v| {
                v.get("usageBarsShowUsed")
                    .and_then(serde_json::Value::as_bool)
            })
            .unwrap_or(false)
    }

    pub async fn shutdown(&self) {
        self.cache.operations.shutdown().await;
        self.provider_operations.shutdown().await;
        self.cost_operations.shutdown().await;
    }
}

pub async fn handle_serve_request(
    request: CliLocalHttpRequest,
    runtime: &ServeRuntime,
) -> CliLocalHttpResponse {
    let started_at = Instant::now();
    let request_deadline = serve_request_deadline(started_at, runtime.request_timeout);
    let provider_timeout = serve_provider_timeout(runtime.request_timeout);
    let provider_deadline = serve_provider_deadline(started_at, runtime.request_timeout);
    let no_store = request.path == "/accounts"
        || request.path.starts_with("/accounts/")
        || request.path.starts_with("/dashboard/v1/");

    let route = match CliServeRouter::route(&request.method, &request.path, &request.query_items) {
        Ok(r) => r,
        Err(CliServeRouteError::MethodNotAllowed) => {
            let resp = serve_error(405, "Method Not Allowed", "method not allowed");
            return if no_store {
                adding_no_store(resp)
            } else {
                resp
            };
        }
        Err(CliServeRouteError::NotFound) => {
            let resp = serve_error(404, "Not Found", "not found");
            return if no_store {
                adding_no_store(resp)
            } else {
                resp
            };
        }
    };

    match route {
        CliServeRoute::WebUi => web_ui::response(),
        CliServeRoute::ProviderIcon { name } => web_ui::icon_response(&name)
            .unwrap_or_else(|| serve_error(404, "Not Found", "not found")),
        CliServeRoute::Health => serve_health_response(runtime.health_version.as_deref()),
        CliServeRoute::Accounts { id } => serve_accounts_route(&request, id.as_deref(), runtime),
        CliServeRoute::Usage { provider } => {
            if runtime.data_routes_require_auth
                && !runtime
                    .dashboard_auth
                    .authorizes(request.headers.get("authorization").map(String::as_str))
            {
                return serve_unauthorized_response();
            }
            let snapshot = match load_serve_config_snapshot(&runtime.config_store) {
                Ok(s) => s,
                Err(msg) => {
                    return adding_no_store(serve_error(500, "Internal Server Error", &msg));
                }
            };
            let operation_key = match serve_operation_key("usage", provider.as_deref()) {
                Ok(k) => k,
                Err(err) => {
                    return adding_no_store(serve_error(400, "Bad Request", &err.to_string()));
                }
            };
            let selected_providers =
                match resolve_selected_providers(provider.as_deref(), &snapshot.config) {
                    Ok(p) => p,
                    Err(err) => {
                        return adding_no_store(serve_error(400, "Bad Request", &err.to_string()));
                    }
                };
            let rt = runtime.clone();
            let cfg = snapshot.config.clone();
            let fp = snapshot.cache_token.clone();
            let resp = cached_serve_response_with_request(
                ServeResponseRequest {
                    key: operation_key,
                    config_fingerprint: snapshot.cache_token,
                    refresh_interval: runtime.refresh_interval,
                    deadline: request_deadline,
                    allows_stale_while_revalidate: true,
                },
                &runtime.cache,
                Arc::new(move || {
                    let rt = rt.clone();
                    let cfg = cfg.clone();
                    let fp = fp.clone();
                    let provs = selected_providers.clone();
                    Box::pin(async move {
                        let usage_fp = serve_usage_operation_fingerprint(&fp, true, false);
                        let fetcher = Arc::clone(&rt.usage_fetcher);
                        let cfg_for_fetch = cfg.clone();
                        let out = serve_collect_usage_outputs(
                            &provs,
                            &usage_fp,
                            provider_deadline,
                            &rt.provider_operations,
                            move |prov, sink| fetcher(prov, cfg_for_fetch.clone(), sink),
                        )
                        .await;
                        let keys: Vec<Option<String>> = out
                            .payload
                            .iter()
                            .map(|p| p.cache_account_key.clone())
                            .collect();
                        serve_json(&out.payload).with_usage_cache_keys(Some(keys))
                    })
                }),
            )
            .await;
            adding_no_store(resp)
        }
        CliServeRoute::Cost { provider } => {
            if runtime.data_routes_require_auth
                && !runtime
                    .dashboard_auth
                    .authorizes(request.headers.get("authorization").map(String::as_str))
            {
                return serve_unauthorized_response();
            }
            let snapshot = match load_serve_config_snapshot(&runtime.config_store) {
                Ok(s) => s,
                Err(msg) => {
                    return adding_no_store(serve_error(500, "Internal Server Error", &msg));
                }
            };
            let operation_key = match serve_operation_key("cost", provider.as_deref()) {
                Ok(k) => k,
                Err(err) => {
                    return adding_no_store(serve_error(400, "Bad Request", &err.to_string()));
                }
            };
            let selected_providers =
                match resolve_selected_providers(provider.as_deref(), &snapshot.config) {
                    Ok(p) => p,
                    Err(err) => {
                        return adding_no_store(serve_error(400, "Bad Request", &err.to_string()));
                    }
                };
            let cost_providers: Vec<String> = selected_providers
                .into_iter()
                .filter(|p| COST_SUPPORTED_PROVIDERS.contains(&p.as_str()))
                .collect();
            if cost_providers.is_empty() {
                return adding_no_store(serve_error(
                    400,
                    "Bad Request",
                    &format!(
                        "cost is only supported for {}",
                        COST_SUPPORTED_PROVIDERS.join(", ")
                    ),
                ));
            }
            let rt = runtime.clone();
            let cfg = snapshot.config.clone();
            let fp = snapshot.cache_token.clone();
            let resp = cached_serve_response_with_request(
                ServeResponseRequest {
                    key: operation_key,
                    config_fingerprint: snapshot.cache_token,
                    refresh_interval: runtime.refresh_interval,
                    deadline: request_deadline,
                    allows_stale_while_revalidate: true,
                },
                &runtime.cache,
                Arc::new(move || {
                    let rt = rt.clone();
                    let cfg = cfg.clone();
                    let fp = fp.clone();
                    let provs = cost_providers.clone();
                    Box::pin(async move {
                        let ctx = ServeCostCollectionContext {
                            config_fingerprint: fp,
                            provider_timeout,
                            request_deadline,
                            now: Arc::new(Instant::now),
                            provider_operations: rt.cost_operations.clone(),
                        };
                        let fetcher = Arc::clone(&rt.cost_fetcher);
                        let items = serve_collect_cost_payloads(&provs, &ctx, move |prov, sink| {
                            fetcher(prov, cfg.clone(), sink)
                        })
                        .await;
                        serve_json(&items)
                    })
                }),
            )
            .await;
            adding_no_store(resp)
        }
        CliServeRoute::DashboardSnapshot {
            provider,
            detail: raw_detail,
        } => {
            if !runtime
                .dashboard_auth
                .authorizes(request.headers.get("authorization").map(String::as_str))
            {
                return serve_unauthorized_response();
            }
            let snapshot = match load_serve_config_snapshot(&runtime.config_store) {
                Ok(s) => s,
                Err(msg) => {
                    return adding_no_store(serve_error(500, "Internal Server Error", &msg));
                }
            };
            let detail = match raw_detail.as_deref() {
                None => DashboardSnapshotDetail::Full,
                Some(d) => match DashboardSnapshotDetail::parse(d) {
                    Some(parsed) => parsed,
                    None => {
                        return adding_no_store(serve_error(
                            400,
                            "Bad Request",
                            &CliServeArgumentError::InvalidDashboardDetail(d.to_string())
                                .to_string(),
                        ));
                    }
                },
            };
            let explicit_providers = match provider.as_deref() {
                None => None,
                Some(raw_p) => match resolve_selected_providers(Some(raw_p), &snapshot.config) {
                    Ok(list) => Some(list),
                    Err(err) => {
                        return adding_no_store(serve_error(400, "Bad Request", &err.to_string()));
                    }
                },
            };
            let identity_mode = resolve_dashboard_identity_mode(
                runtime.dashboard_identity_mode,
                runtime.hides_personal_info(&snapshot.config),
                runtime.dashboard_all_accounts,
            );
            let usage_bars_show_used = runtime.usage_bars_show_used(&snapshot.config);
            let operation_key = match serve_dashboard_operation_key(
                identity_mode,
                usage_bars_show_used,
                provider.as_deref(),
                runtime.dashboard_all_accounts,
            ) {
                Ok(k) => k,
                Err(err) => {
                    return adding_no_store(serve_error(400, "Bad Request", &err.to_string()));
                }
            };

            if detail == DashboardSnapshotDetail::Shell {
                let shell: DashboardSnapshotPayload = DashboardSnapshotBuilder::make_shell_snapshot(
                    &snapshot.config,
                    explicit_providers.as_deref(),
                    (runtime.cache.wall_clock)(),
                    runtime.refresh_interval,
                    runtime.health_version.clone(),
                    usage_bars_show_used,
                );
                return adding_no_store(serve_json(&shell));
            }

            let rt = runtime.clone();
            let cfg = snapshot.config.clone();
            let fp = snapshot.cache_token.clone();
            let resp = cached_serve_response_with_request(
                ServeResponseRequest {
                    key: operation_key,
                    config_fingerprint: snapshot.cache_token,
                    refresh_interval: runtime.refresh_interval,
                    deadline: request_deadline,
                    allows_stale_while_revalidate: true,
                },
                &runtime.cache,
                Arc::new(move || {
                    let rt = rt.clone();
                    let cfg = cfg.clone();
                    let fp = fp.clone();
                    let explicit_provs = explicit_providers.clone();
                    Box::pin(async move {
                        let provs = explicit_provs
                            .clone()
                            .unwrap_or_else(|| cfg.enabled_providers());
                        let usage_fp = serve_usage_operation_fingerprint(
                            &fp,
                            rt.dashboard_all_accounts,
                            rt.dashboard_all_accounts,
                        );
                        let u_fetcher = Arc::clone(&rt.usage_fetcher);
                        let cfg_u = cfg.clone();
                        let usage_out = serve_collect_usage_outputs(
                            &provs,
                            &usage_fp,
                            provider_deadline,
                            &rt.provider_operations,
                            move |prov, sink| u_fetcher(prov, cfg_u.clone(), sink),
                        )
                        .await;

                        let cost_provs: Vec<String> = provs
                            .iter()
                            .filter(|p| COST_SUPPORTED_PROVIDERS.contains(&p.as_str()))
                            .cloned()
                            .collect();
                        let cost_ctx = ServeCostCollectionContext {
                            config_fingerprint: fp,
                            provider_timeout,
                            request_deadline,
                            now: Arc::new(Instant::now),
                            provider_operations: rt.cost_operations.clone(),
                        };
                        let c_fetcher = Arc::clone(&rt.cost_fetcher);
                        let cfg_c = cfg.clone();
                        let cost_items = serve_collect_cost_payloads(
                            &cost_provs,
                            &cost_ctx,
                            move |prov, sink| c_fetcher(prov, cfg_c.clone(), sink),
                        )
                        .await;

                        let mut account_collections = BTreeMap::new();
                        if !rt.dashboard_all_accounts
                            && provs.iter().any(|p| p == "codex")
                            && let Some(store_path) = rt.managed_accounts_store.path()
                            && let Some(coll) =
                                dashboard::DashboardManagedCodexAccounts::collect(&cfg, store_path)
                        {
                            account_collections.insert("codex".to_string(), coll);
                        }
                        let snap = DashboardSnapshotBuilder::make_snapshot(
                            &usage_out.payload,
                            &cost_items,
                            &cfg,
                            identity_mode,
                            (rt.cache.wall_clock)(),
                            rt.refresh_interval,
                            rt.health_version.clone(),
                            &account_collections,
                            usage_bars_show_used,
                            rt.dashboard_all_accounts,
                        );
                        let keys: Vec<Option<String>> =
                            DashboardSnapshotBuilder::selected_payloads(
                                &usage_out.payload,
                                rt.dashboard_all_accounts,
                            )
                            .into_iter()
                            .map(|p| usage_cache_account_key(&p.provider, &p))
                            .collect();
                        serve_json(&snap).with_usage_cache_keys(Some(keys))
                    })
                }),
            )
            .await;
            adding_no_store(resp)
        }
    }
}

pub fn resolve_selected_providers(
    raw_provider: Option<&str>,
    config: &Config,
) -> Result<Vec<String>, CliServeArgumentError> {
    let Some(raw) = raw_provider.filter(|s| !s.trim().is_empty()) else {
        return Ok(dashboard::provider_selection_list(
            &config.enabled_providers(),
        ));
    };
    let parsed = parse_provider_selection(raw)?;
    if parsed.len() == 1 && parsed[0] == "all" {
        return Ok(config.ordered_providers());
    }
    if parsed.len() == 1 && parsed[0] == "both" {
        return Ok(vec!["codex".to_string(), "claude".to_string()]);
    }
    Ok(parsed)
}

/// Safe background timer handle for `CLIServeHeapTrimmer.swift` (`unsafe_code = "deny"`).
pub struct CliServeHeapTrimmer {
    abort_handle: AbortHandle,
}

impl CliServeHeapTrimmer {
    pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(300);

    #[must_use]
    pub fn start(interval: Duration, trim: Arc<dyn Fn() + Send + Sync>) -> Option<Self> {
        if interval.is_zero() {
            return None;
        }
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                trim();
            }
        });
        Some(Self {
            abort_handle: handle.abort_handle(),
        })
    }

    pub fn cancel(&self) {
        self.abort_handle.abort();
    }
}

impl Drop for CliServeHeapTrimmer {
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

/// Returns the `serve` command help text (`CLIHelp.serveHelp`).
#[must_use]
pub fn serve_help(version: &str) -> String {
    format!(
        "AriadUsage {version}\n\n\
         Usage:\n  \
         ariadusage serve [--host <host>] [--port <port>] [--refresh-interval <seconds>]\n                 \
         [--request-timeout <seconds>] [--allow-plain-http]\n                 \
         [--identity <redacted|full>] [--all-accounts]\n                 \
         [--json-output] [--log-level <trace|verbose|debug|info|warning|error|critical>]\n                 \
         [-v|--verbose]\n\n\
         Description:\n  \
         Start a foreground HTTP server that exposes existing CLI JSON payloads and a\n  \
         token-gated dashboard snapshot, with a built-in web UI at /. The static web UI\n  \
         is always open; it sends a browser-entered token only when fetching snapshot data.\n  \
         The server binds to 127.0.0.1 by default; `localhost` is normalized to 127.0.0.1.\n  \
         GET /dashboard/v1/snapshot requires \"Authorization: Bearer YOUR_TOKEN\" and fails\n  \
         closed (401) when no token is configured. Store the token in SecretStore with\n  \
         `ariadusage secret set --id serve.dashboardToken` or set the\n  \
         ARIADUSAGE_DASHBOARD_TOKEN environment variable (secrets are never passed on argv).\n  \
         Transport is plain HTTP: the token crosses the network in cleartext on every\n  \
         request. A non-loopback --host therefore requires both a dashboard token and\n  \
         --allow-plain-http, which records that you accept that trade-off. On a\n  \
         non-loopback host the token also gates /accounts, /usage, and /cost (account data);\n  \
         / and /health are always open. Use a TLS-terminating reverse proxy for anything\n  \
         beyond a trusted network segment.\n\n  \
         Account IDs are stable opaque identifiers; clients must not parse their internal format.\n  \
         Account discovery reads metadata only and never returns credentials or refreshes usage.\n\n\
         Endpoints:\n  \
         GET /                    Built-in web dashboard\n  \
         GET /health\n  \
         GET /accounts             Discover AriadUsage-managed accounts\n  \
         GET /accounts/<id>        Fetch one account by opaque ID\n  \
         GET /usage\n  \
         GET /usage?provider=claude\n  \
         GET /usage?provider=all\n  \
         GET /cost\n  \
         GET /cost?provider=codex\n  \
         GET /dashboard/v1/snapshot\n\n\
         Examples:\n  \
         ariadusage serve\n  \
         ariadusage serve --port 8080 --refresh-interval 60 --request-timeout 30\n  \
         ariadusage secret set --id serve.dashboardToken\n  \
         ARIADUSAGE_DASHBOARD_TOKEN=YOUR_TOKEN ariadusage serve\n  \
         ARIADUSAGE_DASHBOARD_TOKEN=... ariadusage serve --host 0.0.0.0 --allow-plain-http\n  \
         curl http://127.0.0.1:8080/usage?provider=all\n  \
         curl http://127.0.0.1:8080/accounts\n  \
         curl -H \"Authorization: Bearer $ARIADUSAGE_DASHBOARD_TOKEN\" \\\n    \
         http://127.0.0.1:8080/dashboard/v1/snapshot"
    )
}

/// Returns the `dashboard` command help text (`CLIHelp.dashboardHelp`).
#[must_use]
pub fn dashboard_help(version: &str) -> String {
    format!(
        "AriadUsage {version}\n\n\
         Usage:\n  \
         ariadusage dashboard [--pretty] [--timeout <seconds>] [--output <path>]\n                     \
         [--identity <redacted|full>]\n                     \
         [--json-output] [--log-level <trace|verbose|debug|info|warning|error|critical>]\n                     \
         [-v|--verbose]\n\n\
         Description:\n  \
         Print one dashboard-v1 snapshot as JSON, then exit. Honors enabled providers\n  \
         in stable order and keeps provider failures as row-level errors without\n  \
         dropping healthy rows. Account identity defaults to full emails;\n  \
         --identity redacted hides email local parts.\n  \
         Stdout contains only the JSON document; diagnostics are written to stderr.\n  \
         --timeout accepts 0...86400 seconds and defaults to 30; 0 disables the deadline.\n  \
         --output atomically writes the snapshot to a file (0644) instead of stdout;\n  \
         the parent directory must already exist (it is not created), and nothing is\n  \
         printed to stdout on success."
    )
}

/// Returns the root CLI help text (`CLIHelp.rootHelp`).
#[must_use]
pub fn root_help(version: &str) -> String {
    format!(
        "AriadUsage {version}\n\n\
         Usage:\n  \
         ariadusage dashboard [--pretty] [--timeout <seconds>] [--output <path>]\n  \
         ariadusage serve [--host <host>] [--port <port>] [--refresh-interval <seconds>]\n                 \
         [--request-timeout <seconds>] [--allow-plain-http]\n                 \
         [--json-output] [--log-level <trace|verbose|debug|info|warning|error|critical>] [-v|--verbose]"
    )
}

/// Resolves whether `--all-accounts` is accepted for a CLI subcommand (`DashboardServeScopeTests`).
pub fn resolve_serve_or_dashboard_all_accounts_flag(argv: &[&str]) -> Result<bool, String> {
    let Some((&cmd, rest)) = argv.split_first() else {
        return Err("missing command".to_string());
    };
    let mut all_accounts = false;
    for &arg in rest {
        if arg == "--all-accounts" {
            if cmd != "serve" {
                return Err(format!("unexpected flag '{arg}' for command '{cmd}'"));
            }
            all_accounts = true;
        } else {
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    match cmd {
        "serve" | "dashboard" => Ok(all_accounts),
        other => Err(format!("unknown command '{other}'")),
    }
}
