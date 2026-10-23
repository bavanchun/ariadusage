// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeSourcePlanner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use ariadusage_core::oauth::{ExpiredAction, TokenOwner, decide_on_expired};
use ariadusage_core::pipeline::{
    BoxFuture, ClassifiedError, FetchContext, FetchError, FetchResult, FetchRuntime, FetchStrategy,
    ProviderPlan,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use jiff::Timestamp;

use crate::brokers::call::BrokerCall;
use crate::brokers::credential_file::CredentialDecl;
use crate::brokers::net::NetBroker;
use crate::brokers::oauth::{DelegatedCoordinatorResult, DelegatedRefreshCoordinator};

use super::config_paths::ClaudeConfigPaths;
use super::credential_routing::{ClaudeCredentialRoute, ClaudeCredentialRouting};
use super::credentials::{
    ClaudeOAuthCredentialRecord, ClaudeOAuthCredentialSource, ClaudeOAuthCredentials,
    ClaudeOAuthCredentialsError, credentials_profile_identifier, load_from_credentials_file,
    load_from_environment,
};
use super::planner::{
    ClaudeFetchPlanStep, ClaudeSourcePlan, ClaudeSourcePlanner, ClaudeSourcePlanningInput,
    ClaudeSourceSelectedStrategy, ClaudeUsageStrategy,
};
use super::rate_limit_gate::ClaudeOAuthUsageRateLimitGate;
use super::usage_fetcher::{ClaudeOAuthEndpointConfig, ClaudeOAuthUsageFetcher};

pub const CLAUDE_OAUTH_STRATEGY_ID: &str = "claude.oauth";
pub const CLAUDE_CLI_STRATEGY_ID: &str = "claude.cli";
pub const CLAUDE_WEB_STRATEGY_ID: &str = "claude.web";
pub const CLAUDE_ADMIN_API_STRATEGY_ID: &str = "claude.admin-api";

pub const ADMIN_API_KEY_ENV_KEYS: &[&str] = &["ANTHROPIC_ADMIN_KEY", "ANTHROPIC_ADMIN_API_KEY"];

/// Shared runtime dependencies and configuration for the Claude OAuth strategy and provider plan.
#[derive(Clone)]
pub struct ClaudeOAuthStrategyConfig {
    pub net: Arc<NetBroker>,
    pub endpoint: ClaudeOAuthEndpointConfig,
    pub rate_limit_gate: Arc<ClaudeOAuthUsageRateLimitGate>,
    pub delegated_refresh: Option<Arc<DelegatedRefreshCoordinator>>,
    pub working_directory: PathBuf,
    pub home_directory: PathBuf,
    pub allow_background_delegated_refresh: bool,
    pub include_account_identity: bool,
    pub include_optional_usage: bool,
    pub token_account_token: Option<SecretString>,
    pub manual_cookie_header: Option<SecretString>,
    pub has_web_session: bool,
    pub has_cli: bool,
    pub web_extras_enabled: bool,
    pub clock: Arc<dyn Fn() -> Timestamp + Send + Sync>,
}

impl ClaudeOAuthStrategyConfig {
    pub fn new(
        net: Arc<NetBroker>,
        working_directory: impl Into<PathBuf>,
        home_directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            net,
            endpoint: ClaudeOAuthEndpointConfig::default(),
            rate_limit_gate: Arc::new(ClaudeOAuthUsageRateLimitGate::new()),
            delegated_refresh: None,
            working_directory: working_directory.into(),
            home_directory: home_directory.into(),
            allow_background_delegated_refresh: false,
            include_account_identity: true,
            include_optional_usage: true,
            token_account_token: None,
            manual_cookie_header: None,
            has_web_session: false,
            has_cli: false,
            web_extras_enabled: false,
            clock: Arc::new(Timestamp::now),
        }
    }

    pub fn with_endpoint(mut self, endpoint: ClaudeOAuthEndpointConfig) -> Self {
        self.endpoint = endpoint;
        self
    }

    pub fn with_rate_limit_gate(mut self, gate: Arc<ClaudeOAuthUsageRateLimitGate>) -> Self {
        self.rate_limit_gate = gate;
        self
    }

    pub fn with_delegated_refresh(mut self, coordinator: Arc<DelegatedRefreshCoordinator>) -> Self {
        self.delegated_refresh = Some(coordinator);
        self
    }

    pub fn with_allow_background_delegated_refresh(mut self, allow: bool) -> Self {
        self.allow_background_delegated_refresh = allow;
        self
    }

    pub fn with_include_account_identity(mut self, include: bool) -> Self {
        self.include_account_identity = include;
        self
    }

    pub fn with_include_optional_usage(mut self, include: bool) -> Self {
        self.include_optional_usage = include;
        self
    }

    pub fn with_clock<F>(mut self, clock: F) -> Self
    where
        F: Fn() -> Timestamp + Send + Sync + 'static,
    {
        self.clock = Arc::new(clock);
        self
    }

    pub fn credentials_path(&self, cx: &FetchContext) -> PathBuf {
        ClaudeConfigPaths::credentials_path_from_secret_env(
            &cx.env,
            &self.working_directory,
            &self.home_directory,
        )
    }

    pub fn is_cli_available(&self) -> bool {
        self.has_cli || self.delegated_refresh.is_some()
    }
}

/// Strategy executing Claude OAuth usage fetching (`claude.oauth`).
pub struct ClaudeOAuthFetchStrategy {
    config: ClaudeOAuthStrategyConfig,
    planned_step: Option<ClaudeFetchPlanStep>,
}

impl ClaudeOAuthFetchStrategy {
    pub fn new(config: ClaudeOAuthStrategyConfig) -> Self {
        Self {
            config,
            planned_step: None,
        }
    }

    pub fn with_planned_step(mut self, step: ClaudeFetchPlanStep) -> Self {
        self.planned_step = Some(step);
        self
    }

    fn resolve_credential_record(
        &self,
        cx: &FetchContext,
        call: &BrokerCall,
    ) -> Result<ClaudeOAuthCredentialRecord, ClaudeOAuthCredentialsError> {
        if let Some(token) = &self.config.token_account_token {
            return match ClaudeCredentialRouting::resolve(
                Some(token.expose_secret()),
                self.config
                    .manual_cookie_header
                    .as_ref()
                    .map(SecretString::expose_secret),
            ) {
                ClaudeCredentialRoute::Oauth { access_token } => Ok(ClaudeOAuthCredentialRecord {
                    credentials: ClaudeOAuthCredentials {
                        access_token,
                        refresh_token: None,
                        expires_at: Some(Timestamp::MAX),
                        scopes: vec!["user:profile".to_owned()],
                        rate_limit_tier: None,
                        subscription_type: None,
                    },
                    owner: TokenOwner::Environment,
                    source: ClaudeOAuthCredentialSource::Environment,
                    stat_fingerprint: None,
                }),
                _ => Err(ClaudeOAuthCredentialsError::NotFound),
            };
        }

        if let Some(env_record) = load_from_environment(&cx.env) {
            return Ok(env_record);
        }

        let cred_path = self.config.credentials_path(cx);
        load_from_credentials_file(&cred_path, call)
    }

    pub fn is_plausibly_available(&self, cx: &FetchContext) -> bool {
        if cx.selected_token_account.is_some() {
            return matches!(
                ClaudeCredentialRouting::resolve(
                    self.config
                        .token_account_token
                        .as_ref()
                        .map(SecretString::expose_secret),
                    self.config
                        .manual_cookie_header
                        .as_ref()
                        .map(SecretString::expose_secret),
                ),
                ClaudeCredentialRoute::Oauth { .. }
            ) || load_from_environment(&cx.env).is_some();
        }

        if load_from_environment(&cx.env).is_some() {
            return true;
        }

        // Explicit OAuth executes at least once so its concrete credential error is preserved.
        if cx.source_mode != SourceMode::Auto {
            return true;
        }

        let call = BrokerCall {
            interaction: cx.interaction,
            cancel: cx.cancel.clone(),
            request_id: cx.request_id.clone(),
        };
        let cred_path = self.config.credentials_path(cx);
        let Ok(record) = load_from_credentials_file(&cred_path, &call) else {
            return false;
        };

        if !record.credentials.has_scope("user:profile") {
            return false;
        }

        let now = (self.config.clock)();
        if !record.credentials.is_expired_at(now) {
            return true;
        }

        match decide_on_expired(
            record.owner,
            record.credentials.has_refresh_token(),
            cx.runtime,
            cx.interaction,
            self.config.allow_background_delegated_refresh,
        ) {
            ExpiredAction::Delegate => self.config.is_cli_available(),
            ExpiredAction::Refresh => record.credentials.has_refresh_token(),
            ExpiredAction::Fail(_) => false,
        }
    }
}

impl FetchStrategy for ClaudeOAuthFetchStrategy {
    fn id(&self) -> &'static str {
        CLAUDE_OAUTH_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Oauth
    }

    fn is_available<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            if let Some(step) = &self.planned_step {
                if cx.runtime == FetchRuntime::Daemon
                    && cx.source_mode == SourceMode::Oauth
                    && step.data_source == ClaudeSourceSelectedStrategy::Oauth
                {
                    return true;
                }
                if cx.source_mode == SourceMode::Auto {
                    if !step.is_plausibly_available {
                        return false;
                    }
                    // CLI Auto OAuth uses the noninteractive credential check so a logged-out CLI
                    // user gets `noAvailableStrategy` rather than an OAuth credential error, while
                    // Daemon Auto executes the planned OAuth step directly without preflighting.
                    if cx.runtime == FetchRuntime::Cli
                        && step.data_source == ClaudeSourceSelectedStrategy::Oauth
                    {
                        return self.is_plausibly_available(cx);
                    }
                    return true;
                }
            }
            self.is_plausibly_available(cx)
        })
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }

            let call = BrokerCall {
                interaction: cx.interaction,
                cancel: cx.cancel.clone(),
                request_id: cx.request_id.clone(),
            };

            let mut record = match self.resolve_credential_record(cx, &call) {
                Ok(rec) => rec,
                Err(ClaudeOAuthCredentialsError::Cancelled) => return Err(FetchError::Cancelled),
                Err(err) => return Err(FetchError::Classified(err.into_classified_error())),
            };

            if !record.credentials.has_scope("user:profile") {
                return Err(FetchError::Classified(
                    ClaudeOAuthCredentialsError::MissingRequiredScope.into_classified_error(),
                ));
            }

            let now = (self.config.clock)();
            if record.credentials.is_expired_at(now) {
                match decide_on_expired(
                    record.owner,
                    record.credentials.has_refresh_token(),
                    cx.runtime,
                    cx.interaction,
                    self.config.allow_background_delegated_refresh,
                ) {
                    ExpiredAction::Delegate => {
                        let Some(coordinator) = &self.config.delegated_refresh else {
                            return Err(FetchError::Classified(
                                ClaudeOAuthCredentialsError::RefreshDelegatedToClaudeCli
                                    .into_classified_error(),
                            ));
                        };
                        let cred_path = self.config.credentials_path(cx);
                        let decl = CredentialDecl::new(&cred_path);
                        let profile_digest = credentials_profile_identifier(&cred_path);
                        let refresh_outcome = coordinator
                            .refresh(
                                &profile_digest,
                                &decl,
                                &call,
                                self.config.allow_background_delegated_refresh,
                            )
                            .await;
                        if call.cancel.is_cancelled() {
                            return Err(FetchError::Cancelled);
                        }
                        match refresh_outcome {
                            DelegatedCoordinatorResult::Success => {
                                record = match load_from_credentials_file(&cred_path, &call) {
                                    Ok(reloaded) => reloaded,
                                    Err(ClaudeOAuthCredentialsError::Cancelled) => {
                                        return Err(FetchError::Cancelled);
                                    }
                                    Err(err) => {
                                        return Err(FetchError::Classified(
                                            err.into_classified_error(),
                                        ));
                                    }
                                };
                                let after_now = (self.config.clock)();
                                if !record.credentials.has_scope("user:profile") {
                                    return Err(FetchError::Classified(
                                        ClaudeOAuthCredentialsError::MissingRequiredScope
                                            .into_classified_error(),
                                    ));
                                }
                                if record.credentials.is_expired_at(after_now) {
                                    return Err(FetchError::Classified(
                                        ClaudeOAuthCredentialsError::Expired
                                            .into_classified_error(),
                                    ));
                                }
                            }
                            DelegatedCoordinatorResult::CliUnavailable
                            | DelegatedCoordinatorResult::SkippedBackgroundDisabled
                            | DelegatedCoordinatorResult::SkippedByCooldown { .. }
                            | DelegatedCoordinatorResult::UnreadableResult
                            | DelegatedCoordinatorResult::FailedOrUnchanged => {
                                return Err(FetchError::Classified(
                                    ClaudeOAuthCredentialsError::RefreshDelegatedToClaudeCli
                                        .into_classified_error(),
                                ));
                            }
                        }
                    }
                    ExpiredAction::Refresh | ExpiredAction::Fail(_) => {
                        return Err(FetchError::Classified(
                            ClaudeOAuthCredentialsError::Expired.into_classified_error(),
                        ));
                    }
                }
            }

            let fetch_now = (self.config.clock)();
            let usage_response = ClaudeOAuthUsageFetcher::fetch_usage_response(
                &self.config.net,
                &self.config.endpoint,
                &self.config.rate_limit_gate,
                &record.credentials.access_token,
                &call,
                fetch_now,
            )
            .await
            .map_err(|e| e.into_fetch_error())?;

            let account_identity = if self.config.include_account_identity {
                ClaudeOAuthUsageFetcher::fetch_profile_identity(
                    &self.config.net,
                    &self.config.endpoint,
                    &record.credentials.access_token,
                    &call,
                )
                .await
                .map_err(|e| e.into_fetch_error())?
            } else {
                None
            };

            let mapped = ClaudeOAuthUsageFetcher::map_usage_response(
                &usage_response,
                record.credentials.subscription_type.as_deref(),
                record.credentials.rate_limit_tier.as_deref(),
                account_identity.as_ref(),
                record.history_owner_identifier(),
                self.config.include_optional_usage,
                fetch_now,
            )
            .map_err(FetchError::Classified)?;

            Ok(FetchResult::new(
                mapped.snapshot,
                "oauth",
                CLAUDE_OAUTH_STRATEGY_ID,
                SourceKind::Oauth,
            ))
        })
    }

    fn should_fallback(&self, error: &FetchError, cx: &FetchContext) -> bool {
        if cx.cancel.is_cancelled() || matches!(error, FetchError::Cancelled) {
            return false;
        }
        if cx.runtime == FetchRuntime::Daemon
            && cx.source_mode == SourceMode::Oauth
            && let FetchError::Classified(classified) = error
            && matches!(
                classified.kind,
                ProviderErrorKind::MissingCredential | ProviderErrorKind::AuthenticationExpired
            )
        {
            return true;
        }
        cx.source_mode == SourceMode::Auto
    }
}

/// Placeholder strategy for Claude sources pending implementation (`cli`, `web`, `admin-api`).
///
/// Reports `is_available = false` so `fetch_outcome` records `AttemptOutcome::Skipped` (`skipped: unavailable`),
/// and returns a typed `ProviderUnavailable` error if invoked directly.
pub struct ClaudeUnavailableFetchStrategy {
    strategy_id: &'static str,
    kind: SourceKind,
    source_label: &'static str,
}

impl ClaudeUnavailableFetchStrategy {
    pub const fn new(
        strategy_id: &'static str,
        kind: SourceKind,
        source_label: &'static str,
    ) -> Self {
        Self {
            strategy_id,
            kind,
            source_label,
        }
    }

    pub const fn cli() -> Self {
        Self::new(CLAUDE_CLI_STRATEGY_ID, SourceKind::Cli, "cli")
    }

    pub const fn web() -> Self {
        Self::new(CLAUDE_WEB_STRATEGY_ID, SourceKind::Web, "web")
    }

    pub const fn admin_api() -> Self {
        Self::new(CLAUDE_ADMIN_API_STRATEGY_ID, SourceKind::Api, "admin-api")
    }
}

impl FetchStrategy for ClaudeUnavailableFetchStrategy {
    fn id(&self) -> &'static str {
        self.strategy_id
    }

    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        let label = self.source_label;
        Box::pin(async move {
            Err(FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::ProviderUnavailable,
                format!("Claude {label} strategy is unavailable"),
            )))
        })
    }

    fn should_fallback(&self, error: &FetchError, cx: &FetchContext) -> bool {
        !cx.cancel.is_cancelled()
            && !matches!(error, FetchError::Cancelled)
            && cx.source_mode == SourceMode::Auto
    }
}

/// Provider fetch plan for Claude combining `ClaudeSourcePlanner` with executable strategies.
pub struct ClaudeProviderPlan {
    config: ClaudeOAuthStrategyConfig,
}

impl ClaudeProviderPlan {
    pub fn new(config: ClaudeOAuthStrategyConfig) -> Self {
        Self { config }
    }

    /// Computes the `ClaudeSourcePlan` for the given `FetchContext`.
    pub fn source_plan(&self, cx: &FetchContext) -> ClaudeSourcePlan {
        // Mirrors ClaudeProviderDescriptor.makePlanningInput on Linux:
        // App Auto, Linux CLI Auto, and App OAuth plan an OAuth step.
        let should_attempt_oauth = cx.source_mode == SourceMode::Auto
            || (cx.runtime == FetchRuntime::Daemon && cx.source_mode == SourceMode::Oauth);
        let input = ClaudeSourcePlanningInput {
            runtime: cx.runtime,
            selected_data_source: cx.source_mode,
            web_extras_enabled: self.config.web_extras_enabled,
            has_web_session: self.config.has_web_session,
            has_cli: self.config.has_cli,
            has_oauth_credentials: should_attempt_oauth,
        };
        ClaudeSourcePlanner::resolve(input)
    }

    /// Compatibility helper matching `ClaudeProviderDescriptor.resolveUsageStrategy`.
    pub fn resolve_usage_strategy(
        selected_data_source: SourceMode,
        web_extras_enabled: bool,
        has_web_session: bool,
        has_cli: bool,
        has_oauth_credentials: bool,
    ) -> ClaudeUsageStrategy {
        let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
            runtime: FetchRuntime::Daemon,
            selected_data_source,
            web_extras_enabled,
            has_web_session,
            has_cli,
            has_oauth_credentials,
        });
        plan.compatibility_strategy()
            .unwrap_or(ClaudeUsageStrategy {
                data_source: match selected_data_source {
                    SourceMode::Api => ClaudeSourceSelectedStrategy::Api,
                    SourceMode::Oauth => ClaudeSourceSelectedStrategy::Oauth,
                    SourceMode::Web => ClaudeSourceSelectedStrategy::Web,
                    SourceMode::Cli | SourceMode::Auto => ClaudeSourceSelectedStrategy::Cli,
                },
                use_web_extras: false,
            })
    }

    fn resolve_selected_account_strategies(
        &self,
        cx: &FetchContext,
    ) -> Vec<Box<dyn FetchStrategy>> {
        let route = ClaudeCredentialRouting::resolve(
            self.config
                .token_account_token
                .as_ref()
                .map(SecretString::expose_secret),
            self.config
                .manual_cookie_header
                .as_ref()
                .map(SecretString::expose_secret),
        );

        if matches!(route, ClaudeCredentialRoute::AdminApiKey(_))
            || (matches!(cx.source_mode, SourceMode::Auto | SourceMode::Api)
                && has_admin_api_key(&cx.env))
        {
            return vec![Box::new(ClaudeUnavailableFetchStrategy::admin_api())];
        }

        if matches!(cx.source_mode, SourceMode::Auto | SourceMode::Oauth)
            && (matches!(route, ClaudeCredentialRoute::Oauth { .. })
                || load_from_environment(&cx.env).is_some())
        {
            return vec![Box::new(ClaudeOAuthFetchStrategy::new(self.config.clone()))];
        }

        if matches!(cx.source_mode, SourceMode::Auto | SourceMode::Web)
            && matches!(route, ClaudeCredentialRoute::WebCookie { .. })
        {
            return vec![Box::new(ClaudeUnavailableFetchStrategy::web())];
        }

        Vec::new()
    }
}

pub fn has_admin_api_key(env: &BTreeMap<String, SecretString>) -> bool {
    ADMIN_API_KEY_ENV_KEYS.iter().any(|key| {
        env.get(*key)
            .is_some_and(|v| !v.expose_secret().trim().is_empty())
    })
}

impl ProviderPlan for ClaudeProviderPlan {
    fn resolve_strategies<'a>(
        &'a self,
        cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(async move {
            // Selected account is an authority boundary: never fall through to ambient credentials.
            if cx.selected_token_account.is_some() {
                return self.resolve_selected_account_strategies(cx);
            }

            if cx.source_mode == SourceMode::Api
                || (cx.source_mode == SourceMode::Auto && has_admin_api_key(&cx.env))
            {
                return vec![Box::new(ClaudeUnavailableFetchStrategy::admin_api())];
            }

            let plan = self.source_plan(cx);
            plan.ordered_steps
                .into_iter()
                .map(|step| -> Box<dyn FetchStrategy> {
                    match step.data_source {
                        ClaudeSourceSelectedStrategy::Oauth => Box::new(
                            ClaudeOAuthFetchStrategy::new(self.config.clone())
                                .with_planned_step(step),
                        ),
                        ClaudeSourceSelectedStrategy::Cli => {
                            Box::new(ClaudeUnavailableFetchStrategy::cli())
                        }
                        ClaudeSourceSelectedStrategy::Web => {
                            Box::new(ClaudeUnavailableFetchStrategy::web())
                        }
                        ClaudeSourceSelectedStrategy::Api => {
                            Box::new(ClaudeUnavailableFetchStrategy::admin_api())
                        }
                    }
                })
                .collect()
        })
    }
}
