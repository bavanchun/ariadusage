// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityUsageDataSource.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderConfig.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::model::{
    NamedRateWindow, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_core::pipeline::{
    BoxFuture, FetchContext, FetchError, FetchResult, FetchStrategy, ProviderPlan,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::brokers::call::BrokerCall;
use crate::brokers::local_probe::LocalProbe;
use crate::brokers::net::NetBroker;

use super::family_visibility::contains_token;
use super::oauth_store::AntigravityOAuthCredentialsStore;
use super::quota_summary::parse_quota_summary_window_minutes;
use super::remote_fetcher::{
    AntigravityRemoteFetchError, AntigravityRemoteUsageFetcher, CredentialsUpdateHandler,
    OAuthClientResolver, RemoteDataLoader,
};
use super::status_probe::{
    AntigravityStatusProbe, AntigravityStatusProbeError, AntigravityStatusSnapshot,
    COMPACT_FALLBACK_WINDOW_ID_PREFIX, ProcessScope, QUOTA_SUMMARY_WINDOW_ID_PREFIX,
};

pub const ANTIGRAVITY_APP_LOCAL_STRATEGY_ID: &str = "antigravity.app-local";
pub const ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID: &str = "antigravity.cli-https";
pub const ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID: &str = "antigravity.ide-local";
pub const ANTIGRAVITY_OAUTH_STRATEGY_ID: &str = "antigravity.oauth";
pub const ANTIGRAVITY_OFFLINE_STRATEGY_ID: &str = "antigravity.offline";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AntigravityUsageDataSource {
    Auto,
    Cli,
    Oauth,
}

impl AntigravityUsageDataSource {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Cli, Self::Oauth];

    pub const fn source_name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cli => "cli",
            Self::Oauth => "oauth",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Cli => "CLI",
            Self::Oauth => "OAuth",
        }
    }

    pub const fn to_source_mode(self) -> SourceMode {
        match self {
            Self::Auto => SourceMode::Auto,
            Self::Cli => SourceMode::Cli,
            Self::Oauth => SourceMode::Oauth,
        }
    }

    /// Local CLI mode keeps saved OAuth token accounts passive without deleting them.
    pub fn effective_selected_token_account<'a, T>(
        &self,
        selected: Option<&'a T>,
    ) -> Option<&'a T> {
        match self {
            Self::Cli => None,
            Self::Auto | Self::Oauth => selected,
        }
    }

    /// Builds the provider environment for Antigravity, projecting the selected token account
    /// only when the data source allows OAuth (`Auto` or `Oauth`).
    pub fn make_environment(
        &self,
        base: &BTreeMap<String, String>,
        selected_token: Option<&str>,
    ) -> BTreeMap<String, String> {
        let mut env = base.clone();
        if let Some(token) = self
            .effective_selected_token_account(selected_token.as_ref())
            .copied()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            env.insert(
                AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
                token.to_owned(),
            );
        }
        env
    }
}

pub struct AntigravityProviderConfig;

impl AntigravityProviderConfig {
    pub const DEFAULT_COMMAND_NAME: &str = "agy";
    pub const CLI_ALIASES: &[&str] = &["agy", "antigravity"];
    pub const COMMAND_NAME_ENV_KEYS: &[&str] =
        &["ARIADUSAGE_ANTIGRAVITY_CLI_NAME", "ANTIGRAVITY_CLI_NAME"];
}

// MARK: - Presentation & Window Selection (AntigravityProviderDescriptor.swift)

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticWindows {
    pub session: Option<RateWindow>,
    pub weekly: Option<RateWindow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IconWindowsPair {
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
}

#[derive(Debug, Clone, Copy)]
pub struct IconWindowsContext<'a> {
    pub snapshot: &'a UsageSnapshot,
    pub secondary_override_window_id: Option<&'a str>,
    pub now: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuBarMetric {
    Primary,
    Secondary,
    Tertiary,
    Average,
}

#[derive(Debug, Clone, Copy)]
pub struct MenuBarWindowContext<'a> {
    pub metric: MenuBarMetric,
    pub snapshot: &'a UsageSnapshot,
    pub supports_average: bool,
    pub prioritizes_exhausted_quotas: bool,
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MenuBarWindowSelection {
    Resolved(RateWindow),
    Average(RateWindow),
    Unresolved,
}

pub struct AntigravityProviderPresentation;

impl AntigravityProviderPresentation {
    pub const IGNORES_OBSERVED_QUOTA_RESET_BOUNDARIES: bool = true;
    pub const TOKEN_ACCOUNT_TITLE: &str = "Google accounts";
    pub const TOKEN_ACCOUNT_REQUIRES_MANUAL_COOKIE_SOURCE: bool = false;

    pub fn token_account_env_override(token: &str) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert(
            AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
            token.to_owned(),
        );
        map
    }

    pub fn semantic_windows(snapshot: &UsageSnapshot) -> SemanticWindows {
        let summary_session = Self::most_constrained_extra_window(snapshot, |window| {
            Self::is_quota_summary_window(window)
                && Self::quota_summary_window_minutes(window) == Some(300)
        });
        let summary_weekly = Self::most_constrained_extra_window(snapshot, |window| {
            Self::is_quota_summary_window(window)
                && Self::quota_summary_window_minutes(window) == Some(10_080)
        });
        let summary_fallback =
            Self::most_constrained_extra_window(snapshot, Self::is_quota_summary_window);
        let compact_fallback =
            Self::most_constrained_extra_window(snapshot, Self::is_compact_fallback_window);

        if summary_session.is_some() || summary_weekly.is_some() {
            return SemanticWindows {
                session: summary_session
                    .clone()
                    .or(summary_weekly.clone())
                    .or(summary_fallback.clone()),
                weekly: summary_weekly.or(summary_session).or(summary_fallback),
            };
        }

        SemanticWindows {
            session: snapshot
                .primary
                .clone()
                .or(summary_fallback)
                .or(compact_fallback)
                .or_else(|| snapshot.secondary.clone())
                .or_else(|| snapshot.tertiary.clone()),
            weekly: snapshot.secondary.clone(),
        }
    }

    pub fn icon_windows(context: IconWindowsContext<'_>) -> IconWindowsPair {
        let windows = Self::semantic_windows(context.snapshot);
        IconWindowsPair {
            primary: windows.session.clone(),
            secondary: windows.weekly.or(windows.session),
        }
    }

    pub fn menu_bar_window(context: MenuBarWindowContext<'_>) -> MenuBarWindowSelection {
        let primary_summary = Self::ranked_quota_summary_window(context.snapshot, |window| {
            !Self::is_third_party_quota_summary_window(window)
        });
        let secondary_summary = Self::ranked_quota_summary_window(
            context.snapshot,
            Self::is_third_party_quota_summary_window,
        );
        let fallback_summary =
            Self::most_constrained_extra_window(context.snapshot, Self::is_quota_summary_window);
        let compact_fallback =
            Self::most_constrained_extra_window(context.snapshot, Self::is_compact_fallback_window);

        let primary = context
            .snapshot
            .primary
            .clone()
            .or(primary_summary)
            .or(fallback_summary)
            .or(compact_fallback);
        let secondary = context.snapshot.secondary.clone().or(secondary_summary);
        let tertiary = context.snapshot.tertiary.clone();

        if context.prioritizes_exhausted_quotas
            && let Some(exhausted) = [primary.as_ref(), secondary.as_ref(), tertiary.as_ref()]
                .into_iter()
                .flatten()
                .find(|w| w.remaining_percent() <= 0.0)
        {
            return MenuBarWindowSelection::Resolved(exhausted.clone());
        }

        match context.metric {
            MenuBarMetric::Primary => primary.or(secondary).or(tertiary).map_or(
                MenuBarWindowSelection::Unresolved,
                MenuBarWindowSelection::Resolved,
            ),
            MenuBarMetric::Secondary => secondary.or(primary).or(tertiary).map_or(
                MenuBarWindowSelection::Unresolved,
                MenuBarWindowSelection::Resolved,
            ),
            MenuBarMetric::Tertiary => tertiary.or(primary).or(secondary).map_or(
                MenuBarWindowSelection::Unresolved,
                MenuBarWindowSelection::Resolved,
            ),
            MenuBarMetric::Average => {
                if context.supports_average
                    && let (Some(p), Some(s)) = (&primary, &secondary)
                {
                    let avg_used = (p.used_percent + s.used_percent) / 2.0;
                    if let Ok(avg_win) = RateWindow::new(avg_used, None, None, None, None, false) {
                        return MenuBarWindowSelection::Average(avg_win);
                    }
                }
                Self::most_constrained(&[primary, secondary, tertiary]).map_or(
                    MenuBarWindowSelection::Unresolved,
                    MenuBarWindowSelection::Resolved,
                )
            }
        }
    }

    pub fn most_constrained(windows: &[Option<RateWindow>]) -> Option<RateWindow> {
        let available: Vec<&RateWindow> = windows.iter().filter_map(Option::as_ref).collect();
        let most_constrained = available
            .iter()
            .copied()
            .min_by(|lhs, rhs| lhs.remaining_percent().total_cmp(&rhs.remaining_percent()))?;
        if most_constrained.remaining_percent() < 100.0 {
            return Some(most_constrained.clone());
        }
        available.first().copied().cloned()
    }

    pub fn most_constrained_extra_window<F>(
        snapshot: &UsageSnapshot,
        matching: F,
    ) -> Option<RateWindow>
    where
        F: Fn(&NamedRateWindow) -> bool,
    {
        let extras = snapshot.extra_rate_windows.as_deref()?;
        let available: Vec<&RateWindow> = extras
            .iter()
            .filter(|w| w.usage_known && matching(w))
            .map(|w| &w.window)
            .collect();
        let most_constrained = available
            .iter()
            .copied()
            .min_by(|lhs, rhs| lhs.remaining_percent().total_cmp(&rhs.remaining_percent()))?;
        if most_constrained.remaining_percent() < 100.0 {
            return Some(most_constrained.clone());
        }
        available.first().copied().cloned()
    }

    pub fn ranked_quota_summary_window<F>(
        snapshot: &UsageSnapshot,
        matching: F,
    ) -> Option<RateWindow>
    where
        F: Fn(&NamedRateWindow) -> bool,
    {
        let extras = snapshot.extra_rate_windows.as_deref()?;
        let candidates: Vec<&NamedRateWindow> = extras
            .iter()
            .filter(|w| w.usage_known && Self::is_quota_summary_window(w) && matching(w))
            .collect();
        let ordinary: Vec<&NamedRateWindow> = candidates
            .iter()
            .copied()
            .filter(|w| Self::is_ordinary_gemini_quota_summary_window(w))
            .collect();
        let effective = if ordinary.is_empty() {
            &candidates
        } else {
            &ordinary
        };

        effective
            .iter()
            .copied()
            .min_by(|lhs, rhs| {
                let l_rem = lhs.window.remaining_percent();
                let r_rem = rhs.window.remaining_percent();
                if (l_rem - r_rem).abs() > 0.01 {
                    return l_rem.total_cmp(&r_rem);
                }
                Self::quota_summary_window_precedence(lhs)
                    .cmp(&Self::quota_summary_window_precedence(rhs))
            })
            .map(|w| w.window.clone())
    }

    pub fn quota_summary_window_precedence(window: &NamedRateWindow) -> i32 {
        match Self::quota_summary_window_minutes(window) {
            Some(300) => 0,
            Some(10_080) => 1,
            Some(minutes) if minutes > 0 => 2,
            _ => 3,
        }
    }

    pub fn quota_summary_window_minutes(window: &NamedRateWindow) -> Option<i64> {
        if let Some(minutes) = window.window.window_minutes
            && minutes > 0
        {
            return Some(minutes);
        }
        let text = format!("{} {}", window.id, window.title).to_lowercase();
        for token in text.split(|c: char| !c.is_alphanumeric()) {
            if let Some(minutes) = parse_quota_summary_window_minutes(token) {
                return Some(i64::from(minutes));
            }
        }
        None
    }

    pub fn is_quota_summary_window(window: &NamedRateWindow) -> bool {
        window.id.starts_with(QUOTA_SUMMARY_WINDOW_ID_PREFIX)
    }

    pub fn is_ordinary_gemini_quota_summary_window(window: &NamedRateWindow) -> bool {
        if !Self::is_quota_summary_window(window) {
            return false;
        }
        let raw_id = window
            .id
            .strip_prefix(QUOTA_SUMMARY_WINDOW_ID_PREFIX)
            .unwrap_or(&window.id);
        let id = raw_id.split('#').next().unwrap_or(raw_id);
        let lower_id = id.to_lowercase();
        let tokens: Vec<&str> = lower_id
            .split(|c: char| !c.is_alphanumeric())
            .filter(|part| !part.is_empty())
            .collect();
        if tokens.first().copied() != Some("gemini") || tokens.len() < 2 {
            return false;
        }
        const ALLOWED: &[&str] = &[
            "5h", "5hr", "5hrs", "hour", "hours", "session", "weekly", "week", "1w", "7d", "day",
            "daily", "1d", "24h", "quota", "limit",
        ];
        tokens[1..].iter().all(|q| ALLOWED.contains(q))
    }

    pub fn is_third_party_quota_summary_window(window: &NamedRateWindow) -> bool {
        let text = format!("{} {}", window.id, window.title).to_lowercase();
        let tokens: Vec<&str> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|part| !part.is_empty())
            .collect();
        text.contains("claude")
            || text.contains("gpt")
            || text.contains("third-party")
            || text.contains("third party")
            || tokens.contains(&"3p")
    }

    pub fn is_compact_fallback_window(window: &NamedRateWindow) -> bool {
        if window.id.starts_with(COMPACT_FALLBACK_WINDOW_ID_PREFIX) {
            let text = format!("{} {}", window.id, window.title).to_lowercase();
            return !text.contains("tab")
                && !contains_token("image", &text)
                && !text.contains("lite");
        }
        false
    }
}

// MARK: - Selected Account Guard

pub struct AntigravitySelectedAccountGuard;

impl AntigravitySelectedAccountGuard {
    pub fn resolve_context(cx: &FetchContext, token_override: Option<&str>) -> FetchContext {
        if cx.selected_token_account.is_none()
            || AntigravityOAuthCredentialsStore::has_selected_account_credentials(&cx.env)
        {
            return cx.clone();
        }
        let Some(token) = token_override.map(str::trim).filter(|s| !s.is_empty()) else {
            return cx.clone();
        };
        let mut updated = cx.clone();
        updated.env.insert(
            AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
            SecretString::new(token.to_owned()),
        );
        updated
    }

    pub fn validate_selected_account_email<V: super::oauth_store::EnvValue>(
        actual_email: Option<&str>,
        env: &BTreeMap<String, V>,
    ) -> Result<(), AntigravityStatusProbeError> {
        let selected =
            match AntigravityOAuthCredentialsStore::load_selected_account_credentials(env) {
                Ok(Some(creds)) => creds,
                Ok(None) => return Ok(()),
                Err(err) => return Err(AntigravityStatusProbeError::ParseFailed(err.to_string())),
            };
        let Some(expected) = selected
            .email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Ok(());
        };
        let Some(actual) = actual_email.map(str::trim).filter(|s| !s.is_empty()) else {
            return Err(AntigravityStatusProbeError::ParseFailed(
                "Antigravity local source did not report the selected account email.".to_owned(),
            ));
        };
        if actual.eq_ignore_ascii_case(expected) {
            Ok(())
        } else {
            Err(AntigravityStatusProbeError::ParseFailed(
                "Antigravity local source is signed into a different account than the selected account."
                    .to_owned(),
            ))
        }
    }
}

// MARK: - Strategy Configuration & Strategies

pub type AntigravityStatusProbeOverride = Arc<
    dyn Fn(
            ProcessScope,
            &FetchContext,
        )
            -> BoxFuture<'static, Result<AntigravityStatusSnapshot, AntigravityStatusProbeError>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub struct AntigravityStrategyConfig {
    pub net: Arc<NetBroker>,
    pub local_probe: LocalProbe,
    pub home_directory: PathBuf,
    pub working_directory: PathBuf,
    pub probe_timeout: Duration,
    pub oauth_timeout: Duration,
    pub selected_token_override: Option<String>,
    pub status_probe_override: Option<AntigravityStatusProbeOverride>,
    pub remote_data_loader: Option<RemoteDataLoader>,
    pub credentials_update_handler: Option<CredentialsUpdateHandler>,
    pub oauth_client_resolver: Option<OAuthClientResolver>,
    pub clock: Arc<dyn Fn() -> Timestamp + Send + Sync>,
}

impl AntigravityStrategyConfig {
    pub fn new(
        net: Arc<NetBroker>,
        working_directory: impl Into<PathBuf>,
        home_directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            net,
            local_probe: LocalProbe::new(),
            home_directory: home_directory.into(),
            working_directory: working_directory.into(),
            probe_timeout: Duration::from_secs(8),
            oauth_timeout: Duration::from_secs(15),
            selected_token_override: None,
            status_probe_override: None,
            remote_data_loader: None,
            credentials_update_handler: None,
            oauth_client_resolver: None,
            clock: Arc::new(Timestamp::now),
        }
    }

    pub fn with_local_probe(mut self, local_probe: LocalProbe) -> Self {
        self.local_probe = local_probe;
        self
    }

    pub fn with_probe_timeout(mut self, timeout: Duration) -> Self {
        self.probe_timeout = timeout;
        self
    }

    pub fn with_oauth_timeout(mut self, timeout: Duration) -> Self {
        self.oauth_timeout = timeout;
        self
    }

    pub fn with_selected_token_override(mut self, token: Option<String>) -> Self {
        self.selected_token_override = token;
        self
    }

    pub fn with_status_probe_override<F>(mut self, probe_override: F) -> Self
    where
        F: Fn(
                ProcessScope,
                &FetchContext,
            )
                -> BoxFuture<'static, Result<AntigravityStatusSnapshot, AntigravityStatusProbeError>>
            + Send
            + Sync
            + 'static,
    {
        self.status_probe_override = Some(Arc::new(probe_override));
        self
    }

    pub fn with_remote_data_loader(mut self, loader: RemoteDataLoader) -> Self {
        self.remote_data_loader = Some(loader);
        self
    }

    pub fn with_credentials_update_handler(mut self, handler: CredentialsUpdateHandler) -> Self {
        self.credentials_update_handler = Some(handler);
        self
    }

    pub fn with_oauth_client_resolver(mut self, resolver: OAuthClientResolver) -> Self {
        self.oauth_client_resolver = Some(resolver);
        self
    }

    pub fn with_clock<F>(mut self, clock: F) -> Self
    where
        F: Fn() -> Timestamp + Send + Sync + 'static,
    {
        self.clock = Arc::new(clock);
        self
    }
}

pub struct AntigravityStatusFetchStrategy {
    id: &'static str,
    scope: ProcessScope,
    config: AntigravityStrategyConfig,
}

impl AntigravityStatusFetchStrategy {
    pub fn app_local(config: AntigravityStrategyConfig) -> Self {
        Self {
            id: ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            scope: ProcessScope::AppOnly,
            config,
        }
    }

    pub fn ide_local(config: AntigravityStrategyConfig) -> Self {
        Self {
            id: ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            scope: ProcessScope::IdeOnly,
            config,
        }
    }

    pub fn scope(&self) -> ProcessScope {
        self.scope
    }
}

impl FetchStrategy for AntigravityStatusFetchStrategy {
    fn id(&self) -> &'static str {
        self.id
    }

    fn kind(&self) -> SourceKind {
        SourceKind::LocalProbe
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move { true })
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }

            let resolved_cx = AntigravitySelectedAccountGuard::resolve_context(
                cx,
                self.config.selected_token_override.as_deref(),
            );
            let snapshot = if let Some(probe_override) = &self.config.status_probe_override {
                probe_override(self.scope, &resolved_cx)
                    .await
                    .map_err(|e| FetchError::Classified(e.into_classified_error()))?
            } else {
                let call = BrokerCall {
                    interaction: resolved_cx.interaction,
                    cancel: resolved_cx.cancel.clone(),
                    request_id: resolved_cx.request_id.clone(),
                };
                AntigravityStatusProbe::fetch(
                    &self.config.local_probe,
                    &self.config.net,
                    &call,
                    self.config.probe_timeout,
                    self.scope,
                )
                .await
                .map_err(|e| FetchError::Classified(e.into_classified_error()))?
            };

            AntigravitySelectedAccountGuard::validate_selected_account_email(
                snapshot.account_email.as_deref(),
                &resolved_cx.env,
            )
            .map_err(|e| FetchError::Classified(e.into_classified_error()))?;

            let now = (self.config.clock)();
            let usage = snapshot
                .to_usage_snapshot_at(now)
                .map_err(|e| FetchError::Classified(e.into_classified_error()))?;

            let source_label = match self.scope {
                ProcessScope::AppOnly | ProcessScope::IdeAndCli => "local",
                ProcessScope::IdeOnly => "ide",
            };
            Ok(FetchResult::new(
                usage,
                source_label,
                self.id,
                SourceKind::LocalProbe,
            ))
        })
    }

    fn should_fallback(&self, _error: &FetchError, _cx: &FetchContext) -> bool {
        true
    }
}

pub struct AntigravityOAuthFetchStrategy {
    config: AntigravityStrategyConfig,
}

impl AntigravityOAuthFetchStrategy {
    pub fn new(config: AntigravityStrategyConfig) -> Self {
        Self { config }
    }

    pub fn usage_snapshot_from(
        snapshot: &AntigravityStatusSnapshot,
    ) -> Result<UsageSnapshot, AntigravityStatusProbeError> {
        Self::usage_snapshot_from_at(snapshot, Timestamp::now())
    }

    pub fn usage_snapshot_from_at(
        snapshot: &AntigravityStatusSnapshot,
        now: Timestamp,
    ) -> Result<UsageSnapshot, AntigravityStatusProbeError> {
        if snapshot.model_quotas.is_empty()
            && !snapshot.has_known_quota_summary()
            && !snapshot.has_quota_summary_buckets()
        {
            let identity = ProviderIdentitySnapshot::new(
                Some(ProviderId::new("antigravity").expect("valid provider id")),
                snapshot.account_email.clone(),
                None,
                snapshot.account_plan.clone(),
                None,
            );
            return UsageSnapshot::new(
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
                now,
                Some(identity),
                Confidence::Exact,
            )
            .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()));
        }
        snapshot.to_usage_snapshot_at(now)
    }

    pub fn rate_limits_unavailable(snapshot: &UsageSnapshot) -> bool {
        snapshot.primary.is_none()
            && snapshot.secondary.is_none()
            && snapshot.tertiary.is_none()
            && snapshot
                .extra_rate_windows
                .as_ref()
                .is_none_or(Vec::is_empty)
            && snapshot.identity.is_some()
    }
}

impl FetchStrategy for AntigravityOAuthFetchStrategy {
    fn id(&self) -> &'static str {
        ANTIGRAVITY_OAUTH_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Oauth
    }

    fn is_available<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let resolved_cx = AntigravitySelectedAccountGuard::resolve_context(
                cx,
                self.config.selected_token_override.as_deref(),
            );
            if resolved_cx.source_mode == SourceMode::Oauth {
                return true;
            }
            if resolved_cx.selected_token_account.is_some() {
                return true;
            }
            if AntigravityOAuthCredentialsStore::has_selected_account_credentials(&resolved_cx.env)
            {
                return true;
            }
            let cred_url =
                AntigravityOAuthCredentialsStore::credentials_url(&self.config.home_directory);
            AntigravityOAuthCredentialsStore::exists(&cred_url)
        })
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }

            let resolved_cx = AntigravitySelectedAccountGuard::resolve_context(
                cx,
                self.config.selected_token_override.as_deref(),
            );
            let call = BrokerCall {
                interaction: resolved_cx.interaction,
                cancel: resolved_cx.cancel.clone(),
                request_id: resolved_cx.request_id.clone(),
            };

            let mut fetcher = AntigravityRemoteUsageFetcher::new(
                self.config.oauth_timeout,
                &self.config.home_directory,
            )
            .with_net(Arc::clone(&self.config.net))
            .with_secret_environment(&resolved_cx.env)
            .with_clock({
                let clock = Arc::clone(&self.config.clock);
                move || clock()
            });

            if let Some(loader) = &self.config.remote_data_loader {
                let loader = Arc::clone(loader);
                fetcher = fetcher.with_data_loader(move |req| loader(req));
            }
            if let Some(handler) = &self.config.credentials_update_handler {
                let handler = Arc::clone(handler);
                fetcher = fetcher.with_credentials_update_handler(move |creds| handler(creds));
            }
            if let Some(resolver) = &self.config.oauth_client_resolver {
                let resolver = Arc::clone(resolver);
                fetcher = fetcher.with_oauth_client_resolver(move || resolver());
            }

            let snapshot = fetcher
                .fetch_with_call(&call)
                .await
                .map_err(|e| FetchError::Classified(e.into_classified_error()))?;

            let now = (self.config.clock)();
            let usage = Self::usage_snapshot_from_at(&snapshot, now)
                .map_err(|e| FetchError::Classified(e.into_classified_error()))?;

            Ok(FetchResult::new(
                usage,
                "oauth",
                ANTIGRAVITY_OAUTH_STRATEGY_ID,
                SourceKind::Oauth,
            ))
        })
    }

    fn should_fallback(&self, _error: &FetchError, cx: &FetchContext) -> bool {
        cx.source_mode == SourceMode::Auto
    }
}

/// Part-1 placeholder strategy for `antigravity.cli-https` (`AntigravityCLIHTTPSFetchStrategy`).
///
/// Kept in the strategy pipeline so `resolve_strategies` and fallback ordering match CodexBar
/// while CLI HTTPS server bootstrap is out of scope for part 1.
pub struct AntigravityCLIHTTPSPlaceholderStrategy;

impl FetchStrategy for AntigravityCLIHTTPSPlaceholderStrategy {
    fn id(&self) -> &'static str {
        ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Cli
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move { false })
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            Err(FetchError::Classified(
                AntigravityStatusProbeError::NotRunning.into_classified_error(),
            ))
        })
    }

    fn should_fallback(&self, _error: &FetchError, _cx: &FetchContext) -> bool {
        true
    }
}

/// Part-1 placeholder strategy for `antigravity.offline` (`AntigravityOfflineFetchStrategy`).
///
/// Kept in the strategy pipeline so `resolve_strategies` and fallback ordering match CodexBar
/// while the offline SQLite/IndexedDB probe is out of scope for part 1.
pub struct AntigravityOfflinePlaceholderStrategy;

impl AntigravityOfflinePlaceholderStrategy {
    pub fn diagnostic_for_prior_failure(error: &AntigravityRemoteFetchError) -> Option<String> {
        if matches!(error, AntigravityRemoteFetchError::ReauthenticationRequired) {
            Some("Antigravity OAuth needs a new sign-in. Sign in to it again.".to_owned())
        } else {
            None
        }
    }
}

impl FetchStrategy for AntigravityOfflinePlaceholderStrategy {
    fn id(&self) -> &'static str {
        ANTIGRAVITY_OFFLINE_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Cli
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move { false })
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            Err(FetchError::Classified(
                AntigravityStatusProbeError::NotRunning.into_classified_error(),
            ))
        })
    }

    fn should_fallback(&self, _error: &FetchError, _cx: &FetchContext) -> bool {
        false
    }
}

// MARK: - ProviderPlan Implementation

pub struct AntigravityProviderPlan {
    config: AntigravityStrategyConfig,
}

impl AntigravityProviderPlan {
    pub const SOURCE_MODES: &[SourceMode] = &[SourceMode::Auto, SourceMode::Cli, SourceMode::Oauth];

    pub fn new(config: AntigravityStrategyConfig) -> Self {
        Self { config }
    }

    pub fn resolve_strategy_ids(&self, cx: &FetchContext) -> Vec<String> {
        self.resolve_strategies_sync(cx)
            .into_iter()
            .map(|s| s.id().to_owned())
            .collect()
    }

    pub fn resolve_strategies_sync(&self, cx: &FetchContext) -> Vec<Box<dyn FetchStrategy>> {
        match cx.source_mode {
            SourceMode::Cli => vec![
                Box::new(AntigravityStatusFetchStrategy::app_local(
                    self.config.clone(),
                )),
                Box::new(AntigravityCLIHTTPSPlaceholderStrategy),
                Box::new(AntigravityStatusFetchStrategy::ide_local(
                    self.config.clone(),
                )),
                Box::new(AntigravityOfflinePlaceholderStrategy),
            ],
            SourceMode::Oauth => vec![Box::new(AntigravityOAuthFetchStrategy::new(
                self.config.clone(),
            ))],
            SourceMode::Auto => {
                let mut strategies: Vec<Box<dyn FetchStrategy>> = vec![
                    Box::new(AntigravityStatusFetchStrategy::app_local(
                        self.config.clone(),
                    )),
                    Box::new(AntigravityCLIHTTPSPlaceholderStrategy),
                    Box::new(AntigravityStatusFetchStrategy::ide_local(
                        self.config.clone(),
                    )),
                ];
                let resolved_cx = AntigravitySelectedAccountGuard::resolve_context(
                    cx,
                    self.config.selected_token_override.as_deref(),
                );
                let cred_url =
                    AntigravityOAuthCredentialsStore::credentials_url(&self.config.home_directory);
                if resolved_cx.selected_token_account.is_some()
                    || AntigravityOAuthCredentialsStore::has_selected_account_credentials(
                        &resolved_cx.env,
                    )
                    || AntigravityOAuthCredentialsStore::exists(&cred_url)
                {
                    strategies.push(Box::new(AntigravityOAuthFetchStrategy::new(
                        self.config.clone(),
                    )));
                }
                strategies.push(Box::new(AntigravityOfflinePlaceholderStrategy));
                strategies
            }
            SourceMode::Web | SourceMode::Api => Vec::new(),
        }
    }

    pub fn resolve_probe_fallback_error(
        previous: Option<&AntigravityStatusProbeError>,
        current: AntigravityStatusProbeError,
    ) -> AntigravityStatusProbeError {
        let Some(previous) = previous else {
            return current;
        };
        match current {
            AntigravityStatusProbeError::NotRunning
            | AntigravityStatusProbeError::MissingCSRFToken => {
                if matches!(previous, AntigravityStatusProbeError::NotRunning) {
                    current
                } else {
                    previous.clone()
                }
            }
            other => other,
        }
    }
}

impl ProviderPlan for AntigravityProviderPlan {
    fn resolve_strategies<'a>(
        &'a self,
        cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(async move { self.resolve_strategies_sync(cx) })
    }

    fn resolve_fallback_error(
        &self,
        previous: Option<FetchError>,
        current: &FetchError,
    ) -> FetchError {
        let Some(previous) = previous else {
            return current.clone_fallback();
        };
        if let FetchError::Classified(cur_c) = current {
            let is_current_not_running_or_missing_csrf = cur_c.kind
                == ProviderErrorKind::ProviderUnavailable
                && (cur_c
                    .message
                    .contains("Antigravity language server not detected")
                    || cur_c.message.contains("Antigravity CSRF token not found"));
            if is_current_not_running_or_missing_csrf {
                let is_previous_not_running = matches!(
                    &previous,
                    FetchError::Classified(prev_c)
                        if prev_c.kind == ProviderErrorKind::ProviderUnavailable
                            && prev_c
                                .message
                                .contains("Antigravity language server not detected")
                );
                if is_previous_not_running {
                    return current.clone_fallback();
                }
                return previous;
            }
        }
        current.clone_fallback()
    }
}
