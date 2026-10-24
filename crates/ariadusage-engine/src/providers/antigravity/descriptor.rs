// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityUsageDataSource.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityOfflineFetchStrategy.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::model::{
    NamedRateWindow, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_core::pipeline::{
    BoxFuture, ClassifiedError, FetchContext, FetchError, FetchResult, FetchStrategy, ProviderPlan,
    TransportClass,
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

use super::oauth_store::{AntigravityOAuthCredentialsStore, EnvValue};
use super::offline_store::AntigravityOfflineStore;
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
    Oauth,
    Cli,
}

impl AntigravityUsageDataSource {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Oauth, Self::Cli];

    pub const fn source_name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Oauth => "oauth",
            Self::Cli => "cli",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Oauth => "Google OAuth",
            Self::Cli => "Local API / agy CLI",
        }
    }

    pub const fn to_source_mode(self) -> SourceMode {
        match self {
            Self::Auto => SourceMode::Auto,
            Self::Oauth => SourceMode::Oauth,
            Self::Cli => SourceMode::Cli,
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
    Automatic,
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
        let rows: Vec<&NamedRateWindow> = snapshot
            .extra_rate_windows
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter(|w| w.id.starts_with(QUOTA_SUMMARY_WINDOW_ID_PREFIX))
            .collect();
        if rows.is_empty() {
            return SemanticWindows {
                session: snapshot.primary.clone(),
                weekly: snapshot.secondary.clone(),
            };
        }
        let known: Vec<&NamedRateWindow> = rows.into_iter().filter(|w| w.usage_known).collect();
        SemanticWindows {
            session: Self::most_constrained_by_minutes(&known, 300),
            weekly: Self::most_constrained_by_minutes(&known, 10_080),
        }
    }

    pub fn icon_windows(context: IconWindowsContext<'_>) -> IconWindowsPair {
        let windows: Vec<&NamedRateWindow> = context
            .snapshot
            .extra_rate_windows
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter(|w| w.usage_known && w.id.starts_with(QUOTA_SUMMARY_WINDOW_ID_PREFIX))
            .collect();
        if windows.is_empty() {
            return IconWindowsPair {
                primary: None,
                secondary: None,
            };
        }
        IconWindowsPair {
            primary: Self::most_constrained_by_minutes(&windows, 300),
            secondary: Self::most_constrained_by_minutes(&windows, 10_080),
        }
    }

    fn most_constrained_by_minutes(
        windows: &[&NamedRateWindow],
        minutes: i64,
    ) -> Option<RateWindow> {
        windows
            .iter()
            .copied()
            .filter(|w| w.window.window_minutes == Some(minutes))
            .max_by(|lhs, rhs| {
                if lhs.window.used_percent != rhs.window.used_percent {
                    return lhs.window.used_percent.total_cmp(&rhs.window.used_percent);
                }
                rhs.id.cmp(&lhs.id)
            })
            .map(|w| w.window.clone())
    }

    pub fn menu_bar_window(context: MenuBarWindowContext<'_>) -> MenuBarWindowSelection {
        match context.metric {
            MenuBarMetric::Primary => context
                .snapshot
                .primary
                .clone()
                .or_else(|| context.snapshot.secondary.clone())
                .or_else(|| context.snapshot.tertiary.clone())
                .or_else(|| {
                    Self::most_constrained_extra_window_by_prefix(
                        context.snapshot,
                        COMPACT_FALLBACK_WINDOW_ID_PREFIX,
                    )
                })
                .map_or(
                    MenuBarWindowSelection::Unresolved,
                    MenuBarWindowSelection::Resolved,
                ),
            MenuBarMetric::Secondary => context
                .snapshot
                .secondary
                .clone()
                .or_else(|| context.snapshot.primary.clone())
                .or_else(|| context.snapshot.tertiary.clone())
                .or_else(|| {
                    Self::most_constrained_extra_window_by_prefix(
                        context.snapshot,
                        COMPACT_FALLBACK_WINDOW_ID_PREFIX,
                    )
                })
                .map_or(
                    MenuBarWindowSelection::Unresolved,
                    MenuBarWindowSelection::Resolved,
                ),
            MenuBarMetric::Tertiary => context
                .snapshot
                .tertiary
                .clone()
                .or_else(|| context.snapshot.primary.clone())
                .or_else(|| context.snapshot.secondary.clone())
                .or_else(|| {
                    Self::most_constrained_extra_window_by_prefix(
                        context.snapshot,
                        COMPACT_FALLBACK_WINDOW_ID_PREFIX,
                    )
                })
                .map_or(
                    MenuBarWindowSelection::Unresolved,
                    MenuBarWindowSelection::Resolved,
                ),
            MenuBarMetric::Average => {
                if !context.supports_average {
                    return context
                        .snapshot
                        .primary
                        .clone()
                        .or_else(|| context.snapshot.secondary.clone())
                        .or_else(|| context.snapshot.tertiary.clone())
                        .map_or(
                            MenuBarWindowSelection::Unresolved,
                            MenuBarWindowSelection::Resolved,
                        );
                }
                MenuBarWindowSelection::Unresolved
            }
            MenuBarMetric::Automatic => {
                if context.prioritizes_exhausted_quotas
                    && let Some(ranked) =
                        Self::ranked_quota_summary_window(context.snapshot, context.now)
                {
                    return MenuBarWindowSelection::Resolved(ranked);
                }
                Self::most_constrained_extra_window_by_prefix(
                    context.snapshot,
                    QUOTA_SUMMARY_WINDOW_ID_PREFIX,
                )
                .or_else(|| {
                    Self::most_constrained(&[
                        context.snapshot.primary.clone(),
                        context.snapshot.secondary.clone(),
                        context.snapshot.tertiary.clone(),
                    ])
                })
                .or_else(|| {
                    Self::most_constrained_extra_window_by_prefix(
                        context.snapshot,
                        COMPACT_FALLBACK_WINDOW_ID_PREFIX,
                    )
                })
                .map_or(
                    MenuBarWindowSelection::Unresolved,
                    MenuBarWindowSelection::Resolved,
                )
            }
        }
    }

    pub fn most_constrained(windows: &[Option<RateWindow>]) -> Option<RateWindow> {
        let available: Vec<&RateWindow> = windows.iter().filter_map(Option::as_ref).collect();
        let usable: Vec<&RateWindow> = available
            .iter()
            .copied()
            .filter(|w| w.used_percent < 100.0)
            .collect();
        let pool = if usable.is_empty() { available } else { usable };
        let mut best: Option<&RateWindow> = None;
        for w in pool {
            if best.is_none_or(|b| w.used_percent >= b.used_percent) {
                best = Some(w);
            }
        }
        best.cloned()
    }

    pub fn most_constrained_extra_window_by_prefix(
        snapshot: &UsageSnapshot,
        prefix: &str,
    ) -> Option<RateWindow> {
        let windows: Vec<&RateWindow> = snapshot
            .extra_rate_windows
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter(|w| w.usage_known && w.id.starts_with(prefix))
            .map(|w| &w.window)
            .collect();
        let usable: Vec<&RateWindow> = windows
            .iter()
            .copied()
            .filter(|w| w.used_percent < 100.0)
            .collect();
        let pool = if usable.is_empty() { windows } else { usable };
        let mut best: Option<&RateWindow> = None;
        for w in pool {
            if best.is_none_or(|b| w.used_percent >= b.used_percent) {
                best = Some(w);
            }
        }
        best.cloned()
    }

    pub fn ranked_quota_summary_window(
        snapshot: &UsageSnapshot,
        now: Timestamp,
    ) -> Option<RateWindow> {
        snapshot
            .extra_rate_windows
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter(|w| {
                w.usage_known
                    && w.id.starts_with(QUOTA_SUMMARY_WINDOW_ID_PREFIX)
                    && w.window.used_percent.is_finite()
                    && matches!(w.window.window_minutes, Some(300 | 10_080))
            })
            .max_by(|lhs, rhs| {
                if lhs.window.used_percent != rhs.window.used_percent {
                    return lhs.window.used_percent.total_cmp(&rhs.window.used_percent);
                }
                let lhs_reset = lhs.window.resets_at.filter(|&r| r > now);
                let rhs_reset = rhs.window.resets_at.filter(|&r| r > now);
                match (lhs_reset, rhs_reset) {
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (Some(l), Some(r)) if l != r => r.cmp(&l),
                    _ => lhs.id.cmp(&rhs.id),
                }
            })
            .map(|w| w.window.clone())
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

    pub fn matches(
        snapshot_account_email: Option<&str>,
        expected_account_email: Option<&str>,
    ) -> bool {
        let Some(expected) = expected_account_email
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return true;
        };
        let Some(found) = snapshot_account_email
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return false;
        };
        found.eq_ignore_ascii_case(expected)
    }

    pub fn selected_account_email(cx: &FetchContext) -> Option<String> {
        let creds =
            AntigravityOAuthCredentialsStore::load_selected_account_credentials(&cx.env).ok()??;
        creds.resolved_account_email()
    }

    pub fn validate(usage: &UsageSnapshot, cx: &FetchContext) -> Result<(), FetchError> {
        if cx.source_mode != SourceMode::Auto || cx.selected_token_account.is_none() {
            return Ok(());
        }
        let expected = Self::selected_account_email(cx);
        let found = usage
            .identity
            .as_ref()
            .and_then(|id| id.account_email.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if let (Some(expected_email), Some(found_email)) = (&expected, &found)
            && found_email.eq_ignore_ascii_case(expected_email)
        {
            return Ok(());
        }
        Err(FetchError::Classified(Self::account_mismatch_error(
            expected.as_deref(),
            found.as_deref(),
        )))
    }

    pub fn account_mismatch_error(expected: Option<&str>, found: Option<&str>) -> ClassifiedError {
        let selected = expected.unwrap_or("the selected account");
        let message = if let Some(found) = found {
            format!(
                "Antigravity local session is signed in as {found}, not {selected}; local usage cannot be used for the selected account."
            )
        } else {
            format!(
                "Antigravity local session did not report an account matching {selected}; local usage cannot be used for the selected account."
            )
        };
        ClassifiedError::new(ProviderErrorKind::ProviderUnavailable, message)
    }

    pub fn validate_selected_account_email<V: EnvValue>(
        actual_email: Option<&str>,
        env: &BTreeMap<String, V>,
    ) -> Result<(), AntigravityStatusProbeError> {
        let selected =
            match AntigravityOAuthCredentialsStore::load_selected_account_credentials(env) {
                Ok(Some(creds)) => creds,
                Ok(None) => return Ok(()),
                Err(err) => return Err(AntigravityStatusProbeError::ParseFailed(err.to_string())),
            };
        let Some(expected) = selected.resolved_account_email() else {
            return Err(AntigravityStatusProbeError::ParseFailed(
                "Antigravity local session did not report an account matching the selected account; local usage cannot be used for the selected account.".to_owned(),
            ));
        };
        let Some(actual) = actual_email.map(str::trim).filter(|s| !s.is_empty()) else {
            return Err(AntigravityStatusProbeError::ParseFailed(format!(
                "Antigravity local session did not report an account matching {expected}; local usage cannot be used for the selected account."
            )));
        };
        if actual.eq_ignore_ascii_case(&expected) {
            Ok(())
        } else {
            Err(AntigravityStatusProbeError::ParseFailed(format!(
                "Antigravity local session is signed in as {actual}, not {expected}; local usage cannot be used for the selected account."
            )))
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
    pub cli: Arc<super::cli_https::CliRuntime>,
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
            cli: Arc::new(super::cli_https::CliRuntime::default()),
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
            let selected_account_email = if resolved_cx.source_mode == SourceMode::Auto
                && resolved_cx.selected_token_account.is_some()
            {
                AntigravitySelectedAccountGuard::selected_account_email(&resolved_cx)
            } else {
                None
            };

            let snapshot = if let Some(probe_override) = &self.config.status_probe_override {
                probe_override(self.scope, &resolved_cx)
                    .await
                    .map_err(|e| match e {
                        AntigravityStatusProbeError::Cancelled => FetchError::Cancelled,
                        other => FetchError::Classified(other.into_classified_error()),
                    })?
            } else {
                let call = BrokerCall {
                    interaction: resolved_cx.interaction,
                    cancel: resolved_cx.cancel.clone(),
                    request_id: resolved_cx.request_id.clone(),
                };
                AntigravityStatusProbe::fetch_with_expected_account(
                    &self.config.local_probe,
                    &self.config.net,
                    &call,
                    self.config.probe_timeout,
                    self.scope,
                    selected_account_email.as_deref(),
                )
                .await
                .map_err(|e| match e {
                    AntigravityStatusProbeError::Cancelled => FetchError::Cancelled,
                    other => FetchError::Classified(other.into_classified_error()),
                })?
            };

            let now = (self.config.clock)();
            let usage = snapshot
                .to_usage_snapshot_at(now)
                .map_err(|e| FetchError::Classified(e.into_classified_error()))?;

            AntigravitySelectedAccountGuard::validate(&usage, &resolved_cx)?;

            let source_label = match self.scope {
                ProcessScope::AppOnly | ProcessScope::IdeAndCli => "app",
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

    fn should_fallback(&self, _error: &FetchError, cx: &FetchContext) -> bool {
        cx.source_mode == SourceMode::Auto || cx.source_mode == SourceMode::Cli
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
        if snapshot.model_quotas.is_empty() && snapshot.quota_summary.is_none() {
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

            if self.config.oauth_client_resolver.is_none()
                && self.config.remote_data_loader.is_none()
            {
                fetcher = fetcher.with_installed_client_discovery();
            }

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
        let home = resolve_home_directory(&self.config.home_directory, &cx.env);
        AntigravityOfflineStore::has_offline_data(&home, &cx.env)
    }
}

fn resolve_home_directory<V: EnvValue>(default_home: &Path, env: &BTreeMap<String, V>) -> PathBuf {
    env.get("HOME")
        .map(EnvValue::as_env_str)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_home.to_path_buf())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityOfflinePriorError {
    Probe(AntigravityStatusProbeError),
    Remote(AntigravityRemoteFetchError),
    CliReportFailedExit(i32),
    AuthenticationRequired,
    AccountMismatch {
        expected: Option<String>,
        found: Option<String>,
    },
    Transport {
        class: TransportClass,
        caller_detail: Option<String>,
    },
    ForeignSubprocess {
        code: i32,
        stderr: String,
    },
}

/// Offline fallback (tokscale lesson): when live probes and OAuth have no data,
/// surface the local Antigravity CLI conversation count from
/// `~/.gemini/antigravity-cli/conversations/*.db` as a non-quota snapshot.
pub struct AntigravityOfflineFetchStrategy {
    home_directory: PathBuf,
    clock: Arc<dyn Fn() -> Timestamp + Send + Sync>,
}

impl Default for AntigravityOfflineFetchStrategy {
    fn default() -> Self {
        Self {
            home_directory: PathBuf::new(),
            clock: Arc::new(Timestamp::now),
        }
    }
}

impl AntigravityOfflineFetchStrategy {
    pub fn new(config: AntigravityStrategyConfig) -> Self {
        Self {
            home_directory: config.home_directory,
            clock: config.clock,
        }
    }

    pub fn with_home_directory(home_directory: impl Into<PathBuf>) -> Self {
        Self {
            home_directory: home_directory.into(),
            clock: Arc::new(Timestamp::now),
        }
    }

    pub fn diagnostic_for_prior_failure(error: &AntigravityRemoteFetchError) -> Option<String> {
        let reason = Self::reason_for_remote_error(error);
        Some(format!(
            "Live Antigravity usage is unavailable; showing offline data. {reason}"
        ))
    }

    pub fn diagnostic_for_prior_probe_failure(
        error: &AntigravityStatusProbeError,
    ) -> Option<String> {
        let reason = Self::reason_for_probe_error(error)?;
        Some(format!(
            "Live Antigravity usage is unavailable; showing offline data. {reason}"
        ))
    }

    pub fn diagnostic_for_prior_error(error: &AntigravityOfflinePriorError) -> Option<String> {
        let reason = match error {
            AntigravityOfflinePriorError::Probe(probe_err) => {
                Self::reason_for_probe_error(probe_err)?
            }
            AntigravityOfflinePriorError::Remote(remote_err) => {
                Self::reason_for_remote_error(remote_err)
            }
            AntigravityOfflinePriorError::CliReportFailedExit(code) => {
                format!("Antigravity CLI usage report failed: agy exited {code}")
            }
            AntigravityOfflinePriorError::AuthenticationRequired => {
                "Antigravity authentication required. Sign in and retry.".to_owned()
            }
            AntigravityOfflinePriorError::AccountMismatch { .. } => {
                "the local Antigravity session does not match the selected account".to_owned()
            }
            AntigravityOfflinePriorError::Transport { class, .. } => {
                Self::transport_class_description(*class).to_owned()
            }
            AntigravityOfflinePriorError::ForeignSubprocess { .. } => {
                "check Diagnostics for per-source details".to_owned()
            }
        };
        Some(format!(
            "Live Antigravity usage is unavailable; showing offline data. {reason}"
        ))
    }

    pub const fn transport_class_description(class: TransportClass) -> &'static str {
        match class {
            TransportClass::NotConnected | TransportClass::CannotConnect => {
                "The Internet connection appears to be offline."
            }
            TransportClass::Timeout => "The request timed out.",
            TransportClass::ConnectionLost => "The network connection was lost.",
            TransportClass::CannotFindHost | TransportClass::Dns => {
                "A server with the specified hostname could not be found."
            }
            TransportClass::Cancelled => "The operation couldn\u{2019}t be completed.",
        }
    }

    fn reason_for_remote_error(error: &AntigravityRemoteFetchError) -> String {
        match error {
            AntigravityRemoteFetchError::NotLoggedIn
            | AntigravityRemoteFetchError::ReauthenticationRequired => error.to_string(),
            AntigravityRemoteFetchError::PermissionDenied(_)
            | AntigravityRemoteFetchError::RefreshFailed(_)
            | AntigravityRemoteFetchError::ApiError(_)
            | AntigravityRemoteFetchError::ParseFailed(_)
            | AntigravityRemoteFetchError::NetworkError(_) => {
                "the Antigravity API request failed".to_owned()
            }
        }
    }

    fn reason_for_probe_error(error: &AntigravityStatusProbeError) -> Option<String> {
        let reason = match error {
            AntigravityStatusProbeError::ApiError(msg) => {
                if msg.contains("HTTP 401") || msg.contains("HTTP 403") {
                    error.to_string()
                } else if let Some(idx) = msg.find("HTTP ")
                    && msg.len() >= idx + 8
                    && msg[idx + 5..idx + 8].chars().all(|c| c.is_ascii_digit())
                {
                    format!("the usage request failed ({})", &msg[idx..idx + 8])
                } else {
                    "the usage request failed".to_owned()
                }
            }
            AntigravityStatusProbeError::AuthenticationRequired
            | AntigravityStatusProbeError::CliReportFailed(_) => error.to_string(),
            AntigravityStatusProbeError::AccountMismatch { .. } => {
                "the local Antigravity session does not match the selected account".to_owned()
            }
            AntigravityStatusProbeError::PortDiscoveryPending { .. }
            | AntigravityStatusProbeError::PortDiscoveryFailed { .. } => {
                "check Diagnostics for per-source details".to_owned()
            }
            AntigravityStatusProbeError::Transport(_) => "the usage request failed".to_owned(),
            AntigravityStatusProbeError::NotRunning
            | AntigravityStatusProbeError::MissingCSRFToken
            | AntigravityStatusProbeError::TimedOut => error.to_string(),
            AntigravityStatusProbeError::Cancelled => return None,
            AntigravityStatusProbeError::ParseFailed(_)
            | AntigravityStatusProbeError::PortDetectionFailed(_) => {
                "check Diagnostics for per-source details".to_owned()
            }
        };
        Some(reason)
    }
}

impl FetchStrategy for AntigravityOfflineFetchStrategy {
    fn id(&self) -> &'static str {
        ANTIGRAVITY_OFFLINE_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::LocalProbe
    }

    fn is_available<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let home = resolve_home_directory(&self.home_directory, &cx.env);
            AntigravityOfflineStore::has_offline_data(&home, &cx.env)
        })
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }
            let home = resolve_home_directory(&self.home_directory, &cx.env);
            let count = AntigravityOfflineStore::count_conversations(&home, &cx.env);
            if count == 0 {
                return Err(FetchError::Classified(
                    AntigravityStatusProbeError::NotRunning.into_classified_error(),
                ));
            }
            let window = RateWindow::new(0.0, None, None, None, None, false).map_err(|e| {
                FetchError::Classified(
                    AntigravityStatusProbeError::ParseFailed(e.to_string()).into_classified_error(),
                )
            })?;
            let title = format!(
                "Offline \u{00b7} {count} conversation{}",
                if count == 1 { "" } else { "s" }
            );
            let offline_window =
                NamedRateWindow::new("antigravity-offline-conversations", title, window)
                    .with_usage_known(false);
            let identity = ProviderIdentitySnapshot::new(
                Some(ProviderId::new("antigravity").expect("valid provider id")),
                None,
                None,
                Some("offline".to_owned()),
                None,
            );
            let now = (self.clock)();
            let snapshot = UsageSnapshot::new(
                None,
                None,
                None,
                Some(vec![offline_window]),
                None,
                Vec::new(),
                None,
                None,
                now,
                Some(identity),
                Confidence::Exact,
            )
            .map_err(|e| {
                FetchError::Classified(
                    AntigravityStatusProbeError::ParseFailed(e.to_string()).into_classified_error(),
                )
            })?;
            Ok(FetchResult::new(
                snapshot,
                "offline",
                ANTIGRAVITY_OFFLINE_STRATEGY_ID,
                SourceKind::LocalProbe,
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
                Box::new(super::cli_https::AntigravityCLIHTTPSFetchStrategy::new(
                    self.config.clone(),
                )),
                Box::new(AntigravityStatusFetchStrategy::ide_local(
                    self.config.clone(),
                )),
                Box::new(AntigravityOfflineFetchStrategy::new(self.config.clone())),
            ],
            SourceMode::Oauth => vec![Box::new(AntigravityOAuthFetchStrategy::new(
                self.config.clone(),
            ))],
            SourceMode::Auto => {
                let mut strategies: Vec<Box<dyn FetchStrategy>> = vec![
                    Box::new(AntigravityStatusFetchStrategy::app_local(
                        self.config.clone(),
                    )),
                    Box::new(super::cli_https::AntigravityCLIHTTPSFetchStrategy::new(
                        self.config.clone(),
                    )),
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
                strategies.push(Box::new(AntigravityOfflineFetchStrategy::new(
                    self.config.clone(),
                )));
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
