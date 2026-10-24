// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeSourcePlanner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::config::CookieSource;
use ariadusage_core::cookie as cookie_core;
use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::oauth::{ExpiredAction, TokenOwner, decide_on_expired};
use ariadusage_core::pipeline::{
    BoxFuture, ClassifiedError, FetchContext, FetchError, FetchInteraction, FetchResult,
    FetchRuntime, FetchStrategy, ProviderPlan, TransportClass,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use cookie_core::normalize;
use jiff::Timestamp;

use crate::brokers::call::BrokerCall;
use crate::brokers::cookie_cache::{CacheScope, CookieCache};
use crate::brokers::credential_file::CredentialDecl;
use crate::brokers::exec_resolver::{RealResolverFs, ResolverFs};
use crate::brokers::net::NetBroker;
use crate::brokers::oauth::{DelegatedCoordinatorResult, DelegatedRefreshCoordinator};
use crate::brokers::process::ProcessRegistry;
use crate::brokers::secret_store::SecretStore;

use super::admin_api::{
    ClaudeAdminAPIEndpointConfig, ClaudeAdminAPISettingsError, ClaudeAdminAPISettingsReader,
    ClaudeAdminAPIUsageFetcher,
};
use super::cli_gates::{
    ClaudeCLIAuthStatusProbe, ClaudeCLIAuthenticationStatus, ClaudeCLIBackgroundAvailabilityStore,
    ClaudeCLIRateLimitGate, ClaudeCLIUsageSpawnThrottle, ClaudeCLIUsageSpawnThrottleKey,
    ClaudeKeychainPromptMode,
};
use super::cli_parser::ClaudeStatusProbeParser;
use super::cli_session::{
    ClaudeCLISession, ClaudeCLIUsageLoader, ClaudeStatusProbe, ClaudeStatusProbeFetchOverride,
    default_dedicated_probe_working_directory,
};
use super::config_paths::ClaudeConfigPaths;
use super::credential_routing::{ClaudeCredentialRoute, ClaudeCredentialRouting};
use super::credentials::{
    ClaudeOAuthCredentialRecord, ClaudeOAuthCredentialSource, ClaudeOAuthCredentials,
    ClaudeOAuthCredentialsError, credentials_profile_identifier, load_from_credentials_file,
    load_from_environment,
};
use super::planner::{
    ClaudeFetchPlanStep, ClaudeSourcePlan, ClaudeSourcePlanner, ClaudeSourcePlanningInput,
    ClaudeSourceSelectedStrategy, ClaudeSourceSelectionReason, ClaudeUsageStrategy,
};
use super::rate_limit_gate::ClaudeOAuthUsageRateLimitGate;
use super::subscription_metadata::{
    ClaudeSubscriptionFetchResult, ClaudeSubscriptionMetadataFetcher,
    DEFAULT_SUBSCRIPTION_METADATA_TIMEOUT,
};
use super::usage_fetcher::{ClaudeOAuthEndpointConfig, ClaudeOAuthUsageFetcher};
use super::web_extras::{WebExtrasApplyParams, apply_web_extras_if_needed};
use super::web_fetcher::{
    ClaudeWebAPIFetcher, ClaudeWebBrowserFetchGate, ClaudeWebEndpointConfig, ClaudeWebFetchError,
    ClaudeWebFetchOptions, ClaudeWebSessionImporter, WebUsageData,
};

pub const CLAUDE_OAUTH_STRATEGY_ID: &str = "claude.oauth";
pub const CLAUDE_CLI_STRATEGY_ID: &str = "claude.cli";
pub const CLAUDE_WEB_STRATEGY_ID: &str = "claude.web";
pub const CLAUDE_ADMIN_API_STRATEGY_ID: &str = "claude.admin-api";

pub const ADMIN_API_KEY_ENV_KEYS: &[&str] = &["ANTHROPIC_ADMIN_KEY", "ANTHROPIC_ADMIN_API_KEY"];
pub const DEFAULT_CLAUDE_WEB_TIMEOUT_SECONDS: f64 = 15.0;

pub type ClaudeWebAvailabilityProbe =
    Arc<dyn Fn(&FetchContext) -> BoxFuture<'static, bool> + Send + Sync>;
pub type ClaudeWebUsageLoader = Arc<
    dyn Fn(&FetchContext) -> BoxFuture<'static, Result<WebUsageData, ClaudeWebFetchError>>
        + Send
        + Sync,
>;
pub type ClaudeCLIAuthStatusOverride = Arc<
    dyn Fn(&Path, &FetchContext) -> BoxFuture<'static, ClaudeCLIAuthenticationStatus> + Send + Sync,
>;

/// Web enrichment cookie access resolved from `CookieSource`, manual cookie header, or `CookieCache`.
#[derive(Clone)]
pub struct ClaudeWebEnrichmentAccess {
    pub is_available: bool,
    pub manual_cookie_header: Option<SecretString>,
}

impl fmt::Debug for ClaudeWebEnrichmentAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeWebEnrichmentAccess")
            .field("is_available", &self.is_available)
            .field(
                "has_manual_cookie_header",
                &self.manual_cookie_header.is_some(),
            )
            .finish()
    }
}

/// Shared runtime dependencies and configuration for the Claude strategies and provider plan.
#[derive(Clone)]
pub struct ClaudeOAuthStrategyConfig {
    pub net: Arc<NetBroker>,
    pub endpoint: ClaudeOAuthEndpointConfig,
    pub web_endpoint: ClaudeWebEndpointConfig,
    pub admin_api_endpoint: ClaudeAdminAPIEndpointConfig,
    pub rate_limit_gate: Arc<ClaudeOAuthUsageRateLimitGate>,
    pub delegated_refresh: Option<Arc<DelegatedRefreshCoordinator>>,
    pub cookie_cache: Arc<CookieCache>,
    pub session_importer: ClaudeWebSessionImporter,
    pub browser_fetch_gate: Arc<ClaudeWebBrowserFetchGate>,
    pub working_directory: PathBuf,
    pub home_directory: PathBuf,
    pub allow_background_delegated_refresh: bool,
    pub include_account_identity: bool,
    pub include_optional_usage: bool,
    pub token_account_token: Option<SecretString>,
    pub admin_api_key: Option<SecretString>,
    pub workspace_spend_enabled: bool,
    pub secret_store: Option<Arc<SecretStore>>,
    pub manual_cookie_header: Option<SecretString>,
    pub cookie_source: Option<CookieSource>,
    pub use_desktop_cookies: bool,
    pub web_organization_id: Option<String>,
    pub web_timeout_seconds: f64,
    pub subscription_metadata_timeout: Duration,
    pub cache_scope: Option<CacheScope>,
    pub has_web_session: bool,
    pub has_cli: bool,
    pub web_extras_enabled: bool,
    pub cli_binary: String,
    pub cli_probe_timeout: Duration,
    pub keep_cli_sessions_alive: bool,
    pub cli_keychain_disabled: bool,
    pub cli_prompt_mode: ClaudeKeychainPromptMode,
    pub process_registry: Arc<ProcessRegistry>,
    pub launch_gate: Arc<LaunchGate>,
    pub cli_session: Arc<ClaudeCLISession>,
    pub cli_rate_limit_gate: Arc<ClaudeCLIRateLimitGate>,
    pub cli_spawn_throttle: Arc<ClaudeCLIUsageSpawnThrottle>,
    pub cli_background_availability: Arc<ClaudeCLIBackgroundAvailabilityStore>,
    pub cli_status_probe_override: Option<ClaudeStatusProbeFetchOverride>,
    pub cli_direct_status_probe_override: Option<ClaudeStatusProbeFetchOverride>,
    pub cli_auth_status_override: Option<ClaudeCLIAuthStatusOverride>,
    pub clock: Arc<dyn Fn() -> Timestamp + Send + Sync>,
    pub deadline_now: Arc<dyn Fn() -> Instant + Send + Sync>,
    pub web_availability_probe: Option<ClaudeWebAvailabilityProbe>,
    pub web_usage_loader: Option<ClaudeWebUsageLoader>,
}

impl ClaudeOAuthStrategyConfig {
    pub fn new(
        net: Arc<NetBroker>,
        working_directory: impl Into<PathBuf>,
        home_directory: impl Into<PathBuf>,
    ) -> Self {
        let working_directory = working_directory.into();
        let home_directory = home_directory.into();
        let process_registry = Arc::new(ProcessRegistry::new());
        let launch_gate = Arc::new(LaunchGate::new(Instant::now));
        let dedicated_probe_dir = default_dedicated_probe_working_directory(&home_directory);
        let cli_session = Arc::new(
            ClaudeCLISession::new(
                dedicated_probe_dir,
                Arc::clone(&process_registry),
                Arc::clone(&launch_gate),
            )
            .with_home_directory(home_directory.clone()),
        );
        Self {
            net,
            endpoint: ClaudeOAuthEndpointConfig::default(),
            web_endpoint: ClaudeWebEndpointConfig::default(),
            admin_api_endpoint: ClaudeAdminAPIEndpointConfig::default(),
            rate_limit_gate: Arc::new(ClaudeOAuthUsageRateLimitGate::new()),
            delegated_refresh: None,
            cookie_cache: Arc::new(CookieCache::new()),
            session_importer: ClaudeWebSessionImporter::None,
            browser_fetch_gate: Arc::new(ClaudeWebBrowserFetchGate::new()),
            working_directory,
            home_directory,
            allow_background_delegated_refresh: false,
            include_account_identity: true,
            include_optional_usage: true,
            token_account_token: None,
            admin_api_key: None,
            workspace_spend_enabled: false,
            secret_store: None,
            manual_cookie_header: None,
            cookie_source: None,
            use_desktop_cookies: false,
            web_organization_id: None,
            web_timeout_seconds: DEFAULT_CLAUDE_WEB_TIMEOUT_SECONDS,
            subscription_metadata_timeout: DEFAULT_SUBSCRIPTION_METADATA_TIMEOUT,
            cache_scope: None,
            has_web_session: false,
            has_cli: false,
            web_extras_enabled: false,
            cli_binary: "claude".to_owned(),
            cli_probe_timeout: ClaudeCLIUsageLoader::CLI_PROBE_TIMEOUT,
            keep_cli_sessions_alive: false,
            cli_keychain_disabled: false,
            cli_prompt_mode: ClaudeKeychainPromptMode::OnlyOnUserAction,
            process_registry,
            launch_gate,
            cli_session,
            cli_rate_limit_gate: Arc::new(ClaudeCLIRateLimitGate::new()),
            cli_spawn_throttle: Arc::new(ClaudeCLIUsageSpawnThrottle::new()),
            cli_background_availability: Arc::new(ClaudeCLIBackgroundAvailabilityStore::new()),
            cli_status_probe_override: None,
            cli_direct_status_probe_override: None,
            cli_auth_status_override: None,
            clock: Arc::new(Timestamp::now),
            deadline_now: Arc::new(Instant::now),
            web_availability_probe: None,
            web_usage_loader: None,
        }
    }

    pub fn with_endpoint(mut self, endpoint: ClaudeOAuthEndpointConfig) -> Self {
        self.endpoint = endpoint;
        self
    }

    pub fn with_web_endpoint(mut self, web_endpoint: ClaudeWebEndpointConfig) -> Self {
        self.web_endpoint = web_endpoint;
        self
    }

    pub fn with_admin_api_endpoint(
        mut self,
        admin_api_endpoint: ClaudeAdminAPIEndpointConfig,
    ) -> Self {
        self.admin_api_endpoint = admin_api_endpoint;
        self
    }

    pub fn with_admin_api_key(mut self, admin_api_key: Option<SecretString>) -> Self {
        self.admin_api_key = admin_api_key;
        self
    }

    pub fn with_token_account_token(mut self, token: Option<SecretString>) -> Self {
        self.token_account_token = token;
        self
    }

    pub fn with_workspace_spend_enabled(mut self, enabled: bool) -> Self {
        self.workspace_spend_enabled = enabled;
        self
    }

    pub fn with_secret_store(mut self, secret_store: Arc<SecretStore>) -> Self {
        self.secret_store = Some(secret_store);
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

    pub fn with_cookie_cache(mut self, cookie_cache: Arc<CookieCache>) -> Self {
        self.cookie_cache = cookie_cache;
        self
    }

    pub fn with_session_importer(mut self, importer: ClaudeWebSessionImporter) -> Self {
        self.session_importer = importer;
        self
    }

    pub fn with_browser_fetch_gate(mut self, gate: Arc<ClaudeWebBrowserFetchGate>) -> Self {
        self.browser_fetch_gate = gate;
        self
    }

    pub fn with_cookie_source(mut self, cookie_source: Option<CookieSource>) -> Self {
        self.cookie_source = cookie_source;
        self
    }

    pub fn with_manual_cookie_header(mut self, header: Option<SecretString>) -> Self {
        self.manual_cookie_header = header;
        self
    }

    pub fn with_use_desktop_cookies(mut self, enabled: bool) -> Self {
        self.use_desktop_cookies = enabled;
        self
    }

    pub fn with_web_organization_id(mut self, org_id: Option<String>) -> Self {
        self.web_organization_id = org_id;
        self
    }

    pub fn with_web_timeout_seconds(mut self, seconds: f64) -> Self {
        self.web_timeout_seconds = seconds;
        self
    }

    pub fn with_subscription_metadata_timeout(mut self, timeout: Duration) -> Self {
        self.subscription_metadata_timeout = timeout;
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

    pub fn with_web_extras_enabled(mut self, enabled: bool) -> Self {
        self.web_extras_enabled = enabled;
        self
    }

    pub fn with_clock<F>(mut self, clock: F) -> Self
    where
        F: Fn() -> Timestamp + Send + Sync + 'static,
    {
        self.clock = Arc::new(clock);
        self
    }

    pub fn with_deadline_now<F>(mut self, deadline_now: F) -> Self
    where
        F: Fn() -> Instant + Send + Sync + 'static,
    {
        self.deadline_now = Arc::new(deadline_now);
        self
    }

    pub fn with_web_availability_probe<F>(mut self, probe: F) -> Self
    where
        F: Fn(&FetchContext) -> BoxFuture<'static, bool> + Send + Sync + 'static,
    {
        self.web_availability_probe = Some(Arc::new(probe));
        self
    }

    pub fn with_web_usage_loader<F>(mut self, loader: F) -> Self
    where
        F: Fn(&FetchContext) -> BoxFuture<'static, Result<WebUsageData, ClaudeWebFetchError>>
            + Send
            + Sync
            + 'static,
    {
        self.web_usage_loader = Some(Arc::new(loader));
        self
    }

    pub fn with_has_cli(mut self, has_cli: bool) -> Self {
        self.has_cli = has_cli;
        self
    }

    pub fn with_cli_binary(mut self, binary: impl Into<String>) -> Self {
        self.cli_binary = binary.into();
        self
    }

    pub fn with_cli_probe_timeout(mut self, timeout: Duration) -> Self {
        self.cli_probe_timeout = timeout;
        self
    }

    pub fn with_keep_cli_sessions_alive(mut self, keep_alive: bool) -> Self {
        self.keep_cli_sessions_alive = keep_alive;
        self
    }

    pub fn with_cli_keychain_disabled(mut self, disabled: bool) -> Self {
        self.cli_keychain_disabled = disabled;
        self
    }

    pub fn with_cli_prompt_mode(mut self, mode: ClaudeKeychainPromptMode) -> Self {
        self.cli_prompt_mode = mode;
        self
    }

    pub fn with_cli_session(mut self, session: Arc<ClaudeCLISession>) -> Self {
        self.cli_session = session;
        self
    }

    pub fn with_cli_rate_limit_gate(mut self, gate: Arc<ClaudeCLIRateLimitGate>) -> Self {
        self.cli_rate_limit_gate = gate;
        self
    }

    pub fn with_cli_spawn_throttle(mut self, throttle: Arc<ClaudeCLIUsageSpawnThrottle>) -> Self {
        self.cli_spawn_throttle = throttle;
        self
    }

    pub fn with_cli_background_availability(
        mut self,
        store: Arc<ClaudeCLIBackgroundAvailabilityStore>,
    ) -> Self {
        self.cli_background_availability = store;
        self
    }

    pub fn with_cli_status_probe_override(
        mut self,
        probe_override: ClaudeStatusProbeFetchOverride,
    ) -> Self {
        self.cli_status_probe_override = Some(probe_override);
        self
    }

    pub fn with_cli_direct_status_probe_override(
        mut self,
        probe_override: ClaudeStatusProbeFetchOverride,
    ) -> Self {
        self.cli_direct_status_probe_override = Some(probe_override);
        self
    }

    pub fn with_cli_auth_status_override<F>(mut self, auth_override: F) -> Self
    where
        F: Fn(&Path, &FetchContext) -> BoxFuture<'static, ClaudeCLIAuthenticationStatus>
            + Send
            + Sync
            + 'static,
    {
        self.cli_auth_status_override = Some(Arc::new(auth_override));
        self
    }

    pub fn resolve_cli_binary(&self, cx: &FetchContext) -> Option<PathBuf> {
        if self.has_cli
            && (self.cli_status_probe_override.is_some()
                || self.cli_direct_status_probe_override.is_some())
        {
            return Some(PathBuf::from(&self.cli_binary));
        }
        if self.cli_binary.contains('/') {
            let path = PathBuf::from(&self.cli_binary);
            return RealResolverFs.is_executable(&path).then_some(path);
        }
        if cx.env.contains_key("CLAUDE_CLI_PATH") || cx.env.contains_key("PATH") || self.has_cli {
            if let Some(resolved) = ClaudeStatusProbe::resolve_binary_path(
                &self.cli_binary,
                &cx.env,
                &self.home_directory,
            ) {
                return Some(resolved);
            }
            return None;
        }
        for rel in [
            ".local/bin/claude",
            ".claude/local/claude",
            ".claude/bin/claude",
            ".local/share/mise/shims/claude",
            ".asdf/shims/claude",
        ] {
            let candidate = self.home_directory.join(rel);
            if RealResolverFs.is_executable(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    pub fn has_cli_in_context(&self, cx: &FetchContext) -> bool {
        self.has_cli || self.resolve_cli_binary(cx).is_some()
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

    pub fn effective_cookie_source(&self) -> CookieSource {
        if let Some(source) = self.cookie_source {
            return source;
        }
        if self.manual_cookie_header.is_some()
            || matches!(
                ClaudeCredentialRouting::resolve(
                    self.token_account_token
                        .as_ref()
                        .map(SecretString::expose_secret),
                    None,
                ),
                ClaudeCredentialRoute::WebCookie { .. }
            )
        {
            return CookieSource::Manual;
        }
        CookieSource::Auto
    }

    pub fn effective_manual_cookie_header(&self) -> Option<SecretString> {
        if self.effective_cookie_source() != CookieSource::Manual {
            return None;
        }
        if let Some(header) = &self.manual_cookie_header {
            return normalize(header.expose_secret()).map(SecretString::new);
        }
        if let ClaudeCredentialRoute::WebCookie { header } = ClaudeCredentialRouting::resolve(
            self.token_account_token
                .as_ref()
                .map(SecretString::expose_secret),
            None,
        ) {
            return Some(header);
        }
        None
    }

    pub fn has_manual_session_key(&self) -> bool {
        ClaudeWebAPIFetcher::has_session_key_in_header(
            self.effective_manual_cookie_header()
                .as_ref()
                .map(SecretString::expose_secret),
        )
    }

    pub fn has_plausible_web_session(&self, cx: &FetchContext) -> bool {
        match cx.source_mode {
            SourceMode::Api | SourceMode::Oauth | SourceMode::Cli => return false,
            SourceMode::Web => return self.effective_cookie_source() != CookieSource::Off,
            SourceMode::Auto => {}
        }

        if self.has_web_session {
            return true;
        }

        match self.effective_cookie_source() {
            CookieSource::Off => false,
            CookieSource::Manual => self.has_manual_session_key(),
            CookieSource::Auto => {
                // Keep planning synchronous and let the web step perform the bounded availability
                // check if Daemon Auto reaches its fallback, or inside the bounded fetch for CLI Auto.
                true
            }
        }
    }

    pub fn web_enrichment_access(&self) -> ClaudeWebEnrichmentAccess {
        match self.effective_cookie_source() {
            CookieSource::Off => ClaudeWebEnrichmentAccess {
                is_available: false,
                manual_cookie_header: None,
            },
            CookieSource::Manual => {
                let header = self.effective_manual_cookie_header();
                let is_available = ClaudeWebAPIFetcher::has_session_key_in_header(
                    header.as_ref().map(SecretString::expose_secret),
                );
                ClaudeWebEnrichmentAccess {
                    is_available,
                    manual_cookie_header: header,
                }
            }
            CookieSource::Auto => {
                if self.use_desktop_cookies {
                    return ClaudeWebEnrichmentAccess {
                        is_available: false,
                        manual_cookie_header: None,
                    };
                }
                let provider_id = ProviderId::new("claude").expect("valid provider id");
                let header = self
                    .cookie_cache
                    .load(&provider_id, self.cache_scope.as_ref())
                    .map(|entry| entry.header.clone());
                let is_available = ClaudeWebAPIFetcher::has_session_key_in_header(
                    header.as_ref().map(SecretString::expose_secret),
                );
                ClaudeWebEnrichmentAccess {
                    is_available,
                    manual_cookie_header: header,
                }
            }
        }
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

    pub fn direct_credential_is_missing(&self, cx: &FetchContext) -> bool {
        let call = BrokerCall {
            interaction: cx.interaction,
            cancel: cx.cancel.clone(),
            request_id: cx.request_id.clone(),
        };
        matches!(
            self.resolve_credential_record(cx, &call),
            Err(ClaudeOAuthCredentialsError::NotFound
                | ClaudeOAuthCredentialsError::MissingOAuth
                | ClaudeOAuthCredentialsError::MissingAccessToken)
        )
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

            let enrichment = self.config.web_enrichment_access();
            let use_web_extras = cx.runtime == FetchRuntime::Daemon
                && self.config.web_extras_enabled
                && enrichment.is_available;
            let include_prepaid_balance = cx.runtime == FetchRuntime::Daemon
                && self.config.include_optional_usage
                && enrichment.is_available;

            let mut snapshot = apply_web_extras_if_needed(
                mapped.snapshot,
                WebExtrasApplyParams {
                    net: &self.config.net,
                    oauth_endpoint: &self.config.endpoint,
                    web_endpoint: &self.config.web_endpoint,
                    cookie_cache: &self.config.cookie_cache,
                    importer: &self.config.session_importer,
                    browser_gate: &self.config.browser_fetch_gate,
                    manual_cookie_header: enrichment
                        .manual_cookie_header
                        .as_ref()
                        .map(SecretString::expose_secret),
                    oauth_access_token: Some(&record.credentials.access_token),
                    oauth_profile: account_identity.as_ref(),
                    use_web_extras,
                    include_prepaid_balance,
                    web_extras_timeout: ClaudeWebFetchStrategy::timeout_duration(
                        self.config.web_timeout_seconds,
                    ),
                    options: ClaudeWebFetchOptions {
                        target_organization_id: self.config.web_organization_id.clone(),
                        include_usage_details: use_web_extras,
                        include_prepaid_balance,
                        use_desktop_cookies: self.config.use_desktop_cookies,
                        cache_scope: self.config.cache_scope.clone(),
                        now: fetch_now,
                    },
                    call: &call,
                },
            )
            .await?;

            if enrichment.is_available
                && let Some(cookie_secret) = enrichment.manual_cookie_header.as_ref()
            {
                let expected_owner_id = snapshot
                    .identity
                    .as_ref()
                    .and_then(|i| i.account_id.clone());
                if let ClaudeSubscriptionFetchResult::Available(metadata) =
                    ClaudeSubscriptionMetadataFetcher::fetch(
                        &self.config.net,
                        &self.config.web_endpoint,
                        &self.config.endpoint,
                        cookie_secret,
                        expected_owner_id.as_deref(),
                        Some(&record.credentials.access_token),
                        self.config.subscription_metadata_timeout,
                        &call,
                    )
                    .await
                {
                    snapshot = metadata.applying(&snapshot);
                }
            }

            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }

            Ok(FetchResult::new(
                snapshot,
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

/// Shared deadline budget state across `is_available` and `fetch` for a single `ClaudeWebFetchStrategy` run.
#[derive(Default)]
pub struct ClaudeWebFetchDeadlineState {
    deadline: Mutex<Option<Instant>>,
}

impl ClaudeWebFetchDeadlineState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remaining_budget(&self, full_budget: Duration, now: Instant) -> Duration {
        let mut guard = self
            .deadline
            .lock()
            .expect("web fetch deadline lock not poisoned");
        let target = match *guard {
            Some(existing) => existing,
            None => {
                let computed = now.checked_add(full_budget).unwrap_or(now);
                *guard = Some(computed);
                computed
            }
        };
        target.saturating_duration_since(now)
    }
}

/// Deadline and timeout errors emitted by `ClaudeWebFetchStrategy`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClaudeWebFetchStrategyError {
    #[error(
        "Claude web usage fetch timeout must be a finite, nonnegative value within the supported range."
    )]
    InvalidTimeout,
    #[error("Claude web usage fetch timed out after {seconds} seconds.")]
    TimedOut { seconds: f64 },
}

impl ClaudeWebFetchStrategyError {
    pub fn into_classified_error(self) -> ClassifiedError {
        let msg = self.to_string();
        match self {
            Self::InvalidTimeout => ClassifiedError::new(ProviderErrorKind::ApiFailure, msg),
            Self::TimedOut { .. } => ClassifiedError::new(ProviderErrorKind::NetworkFailure, msg)
                .with_transport(TransportClass::Timeout),
        }
    }

    pub fn into_fetch_error(self) -> FetchError {
        FetchError::Classified(self.into_classified_error())
    }
}

/// Strategy executing Claude Web usage fetching (`claude.web`).
pub struct ClaudeWebFetchStrategy {
    config: ClaudeOAuthStrategyConfig,
    planned_step: Option<ClaudeFetchPlanStep>,
    deadline_state: Arc<ClaudeWebFetchDeadlineState>,
}

impl ClaudeWebFetchStrategy {
    pub fn new(config: ClaudeOAuthStrategyConfig) -> Self {
        Self {
            config,
            planned_step: None,
            deadline_state: Arc::new(ClaudeWebFetchDeadlineState::new()),
        }
    }

    pub fn with_planned_step(mut self, step: ClaudeFetchPlanStep) -> Self {
        self.planned_step = Some(step);
        self
    }

    pub fn timeout_duration(timeout: f64) -> Option<Duration> {
        if !timeout.is_finite() || timeout < 0.0 || timeout > (i64::MAX as f64) {
            return None;
        }
        Duration::try_from_secs_f64(timeout).ok()
    }

    async fn has_browser_session_key(&self, cx: &FetchContext, timeout: Duration) -> bool {
        if cx.cancel.is_cancelled() || timeout.is_zero() {
            return false;
        }
        let child_cancel = cx.cancel.child_token();
        let sub_call = BrokerCall {
            interaction: cx.interaction,
            cancel: child_cancel.clone(),
            request_id: cx.request_id.clone(),
        };

        let fut = async {
            if let Some(probe) = &self.config.web_availability_probe {
                probe(cx).await
            } else {
                ClaudeWebAPIFetcher::has_session_key(
                    &self.config.cookie_cache,
                    self.config.cache_scope.as_ref(),
                    &self.config.session_importer,
                    self.config.use_desktop_cookies,
                    &sub_call,
                )
                .await
            }
        };

        match tokio::time::timeout(timeout, cx.cancel.run_until_cancelled(fut)).await {
            Ok(Some(is_available)) => is_available,
            Ok(None) | Err(_) => {
                child_cancel.cancel();
                false
            }
        }
    }

    pub async fn load_web_usage_data(&self, cx: &FetchContext) -> Result<WebUsageData, FetchError> {
        let timeout_secs = self.config.web_timeout_seconds;
        let Some(full_budget) = Self::timeout_duration(timeout_secs) else {
            return Err(ClaudeWebFetchStrategyError::InvalidTimeout.into_fetch_error());
        };
        let remaining_budget = self
            .deadline_state
            .remaining_budget(full_budget, (self.config.deadline_now)());
        if remaining_budget.is_zero() {
            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }
            return Err(ClaudeWebFetchStrategyError::TimedOut {
                seconds: timeout_secs,
            }
            .into_fetch_error());
        }

        let child_cancel = cx.cancel.child_token();
        let sub_call = BrokerCall {
            interaction: cx.interaction,
            cancel: child_cancel.clone(),
            request_id: cx.request_id.clone(),
        };
        let fetch_now = (self.config.clock)();

        let fut = async {
            if let Some(loader) = &self.config.web_usage_loader {
                return loader(cx).await.map_err(|e| e.into_fetch_error());
            }

            let options = ClaudeWebFetchOptions {
                target_organization_id: self.config.web_organization_id.clone(),
                include_usage_details: true,
                include_prepaid_balance: self.config.include_optional_usage,
                use_desktop_cookies: self.config.use_desktop_cookies,
                cache_scope: self.config.cache_scope.clone(),
                now: fetch_now,
            };

            if let Some(manual_header) = self.config.effective_manual_cookie_header() {
                ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
                    &self.config.net,
                    &self.config.web_endpoint,
                    manual_header.expose_secret(),
                    &options,
                    &sub_call,
                )
                .await
                .map_err(|e| e.into_fetch_error())
            } else {
                ClaudeWebAPIFetcher::fetch_usage_serialized(
                    &self.config.net,
                    &self.config.web_endpoint,
                    &self.config.cookie_cache,
                    &self.config.session_importer,
                    &self.config.browser_fetch_gate,
                    &options,
                    &sub_call,
                )
                .await
                .map_err(|e| e.into_fetch_error())
            }
        };

        match tokio::time::timeout(remaining_budget, cx.cancel.run_until_cancelled(fut)).await {
            Ok(Some(Ok(usage))) => {
                if cx.cancel.is_cancelled() {
                    return Err(FetchError::Cancelled);
                }
                Ok(usage)
            }
            Ok(Some(Err(err))) => Err(err),
            Ok(None) => {
                child_cancel.cancel();
                Err(FetchError::Cancelled)
            }
            Err(_) => {
                child_cancel.cancel();
                if cx.cancel.is_cancelled() {
                    return Err(FetchError::Cancelled);
                }
                Err(ClaudeWebFetchStrategyError::TimedOut {
                    seconds: timeout_secs,
                }
                .into_fetch_error())
            }
        }
    }
}

impl FetchStrategy for ClaudeWebFetchStrategy {
    fn id(&self) -> &'static str {
        CLAUDE_WEB_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Web
    }

    fn is_available<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            if let Some(step) = &self.planned_step
                && cx.source_mode == SourceMode::Auto
            {
                if !step.is_plausibly_available {
                    return false;
                }
                // In CLI Auto mode, defer cookie import to the bounded `fetch` step.
                if cx.runtime == FetchRuntime::Cli {
                    return true;
                }
            }

            let app_auto_remaining_budget =
                if cx.runtime == FetchRuntime::Daemon && cx.source_mode == SourceMode::Auto {
                    let Some(full_budget) = Self::timeout_duration(self.config.web_timeout_seconds)
                    else {
                        return false;
                    };
                    let remaining = self
                        .deadline_state
                        .remaining_budget(full_budget, (self.config.deadline_now)());
                    if remaining.is_zero() {
                        return false;
                    }
                    Some(remaining)
                } else {
                    None
                };

            match self.config.effective_cookie_source() {
                CookieSource::Off => false,
                CookieSource::Manual => self.config.has_manual_session_key(),
                CookieSource::Auto => {
                    if let Some(remaining) = app_auto_remaining_budget {
                        self.has_browser_session_key(cx, remaining).await
                    } else {
                        // Explicit web and CLI auto perform the real browser import inside the bounded fetch.
                        true
                    }
                }
            }
        })
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }

            let web_data = self.load_web_usage_data(cx).await?;
            let fetch_now = (self.config.clock)();
            // Billing uses the owner already present in the authenticated web response.
            let mut snapshot = web_data
                .to_usage_snapshot(true, self.config.include_optional_usage, fetch_now)
                .map_err(FetchError::Classified)?;

            let enrichment = self.config.web_enrichment_access();
            if enrichment.is_available
                && let Some(cookie_secret) = enrichment.manual_cookie_header.as_ref()
            {
                let expected_owner_id = snapshot
                    .identity
                    .as_ref()
                    .and_then(|i| i.account_id.clone());
                let call = BrokerCall {
                    interaction: cx.interaction,
                    cancel: cx.cancel.clone(),
                    request_id: cx.request_id.clone(),
                };
                if let ClaudeSubscriptionFetchResult::Available(metadata) =
                    ClaudeSubscriptionMetadataFetcher::fetch(
                        &self.config.net,
                        &self.config.web_endpoint,
                        &self.config.endpoint,
                        cookie_secret,
                        expected_owner_id.as_deref(),
                        None,
                        self.config.subscription_metadata_timeout,
                        &call,
                    )
                    .await
                {
                    snapshot = metadata.applying(&snapshot);
                }
            }

            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }

            Ok(FetchResult::new(
                snapshot,
                "web",
                CLAUDE_WEB_STRATEGY_ID,
                SourceKind::Web,
            ))
        })
    }

    fn should_fallback(&self, error: &FetchError, cx: &FetchContext) -> bool {
        if cx.source_mode != SourceMode::Auto {
            return false;
        }
        if cx.cancel.is_cancelled() || matches!(error, FetchError::Cancelled) {
            return false;
        }
        if let FetchError::Classified(c) = error
            && c.transport == Some(TransportClass::Cancelled)
        {
            return false;
        }
        // In CLI runtime auto mode, web comes before CLI so fallback is required.
        // In Daemon (app) runtime auto mode, web is terminal and surfaces its concrete error.
        cx.runtime == FetchRuntime::Cli
    }
}

/// Strategy executing Claude CLI usage fetching (`claude.cli`).
pub struct ClaudeCLIFetchStrategy {
    config: ClaudeOAuthStrategyConfig,
    planned_step: Option<ClaudeFetchPlanStep>,
    owner_cli_recovery_only: bool,
}

impl ClaudeCLIFetchStrategy {
    pub fn new(config: ClaudeOAuthStrategyConfig) -> Self {
        Self {
            config,
            planned_step: None,
            owner_cli_recovery_only: false,
        }
    }

    pub fn with_planned_step(mut self, step: ClaudeFetchPlanStep) -> Self {
        if step.inclusion_reason == ClaudeSourceSelectionReason::ExplicitOAuthOwnerCliFallback {
            self.owner_cli_recovery_only = true;
        }
        self.planned_step = Some(step);
        self
    }

    pub fn with_owner_cli_recovery_only(mut self, owner_cli_recovery_only: bool) -> Self {
        self.owner_cli_recovery_only = owner_cli_recovery_only;
        self
    }

    fn effective_enrichment(&self, cx: &FetchContext) -> (bool, bool, Option<SecretString>) {
        if self.owner_cli_recovery_only {
            return (false, false, None);
        }
        let enrichment = self.config.web_enrichment_access();
        let use_web_extras = cx.runtime == FetchRuntime::Daemon
            && self.config.web_extras_enabled
            && enrichment.is_available;
        let include_prepaid_balance = cx.runtime == FetchRuntime::Daemon
            && self.config.include_optional_usage
            && enrichment.is_available;
        (
            use_web_extras,
            include_prepaid_balance,
            enrichment.manual_cookie_header,
        )
    }

    fn throttle_key(
        &self,
        binary: &str,
        cx: &FetchContext,
        call: &BrokerCall,
    ) -> Option<ClaudeCLIUsageSpawnThrottleKey> {
        let (use_web_extras, include_prepaid_balance, _) = self.effective_enrichment(cx);
        ClaudeCLIUsageSpawnThrottle::key(
            binary,
            &cx.env,
            &self.config.working_directory,
            &self.config.home_directory,
            use_web_extras,
            include_prepaid_balance,
            call,
        )
    }

    async fn authentication_status(
        &self,
        binary: &std::path::Path,
        cx: &FetchContext,
        call: &BrokerCall,
    ) -> ClaudeCLIAuthenticationStatus {
        if let Some(ref ov) = self.config.cli_auth_status_override {
            return (ov)(binary, cx).await;
        }
        ClaudeCLIAuthStatusProbe::authentication_status(
            call.clone(),
            &self.config.process_registry,
            &self.config.launch_gate,
            binary,
            &cx.env,
            &self.config.working_directory,
            ClaudeCLIAuthStatusProbe::PROBE_TIMEOUT,
        )
        .await
    }
}

impl FetchStrategy for ClaudeCLIFetchStrategy {
    fn id(&self) -> &'static str {
        CLAUDE_CLI_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Cli
    }

    fn is_available<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            if cx.source_mode == SourceMode::Auto
                && let Some(step) = &self.planned_step
                && !step.is_plausibly_available
            {
                return false;
            }

            let Some(binary) = self.config.resolve_cli_binary(cx) else {
                return false;
            };
            let binary_str = binary.to_string_lossy().into_owned();
            let call = BrokerCall {
                interaction: cx.interaction,
                cancel: cx.cancel.clone(),
                request_id: cx.request_id.clone(),
            };

            if cx.runtime == FetchRuntime::Cli {
                return self.authentication_status(&binary, cx, &call).await
                    != ClaudeCLIAuthenticationStatus::LoggedOut;
            }

            let is_background_app_refresh = cx.interaction == FetchInteraction::Background;
            if is_background_app_refresh && cx.source_mode == SourceMode::Oauth {
                return false;
            }

            let is_background_auto_refresh =
                is_background_app_refresh && cx.source_mode == SourceMode::Auto;
            if is_background_auto_refresh {
                let now = (self.config.clock)();
                if let Some(key) = self.throttle_key(&binary_str, cx, &call)
                    && self
                        .config
                        .cli_spawn_throttle
                        .cached_result(&key, now)
                        .is_some()
                {
                    return true;
                }
                return self
                    .config
                    .cli_background_availability
                    .allows_background_auto_usage_fetch(
                        &binary_str,
                        &cx.env,
                        &self.config.working_directory,
                        &self.config.home_directory,
                        cx.interaction,
                        self.config.cli_keychain_disabled,
                        self.config.cli_prompt_mode,
                        &call,
                        || {
                            ClaudeOAuthFetchStrategy::new(self.config.clone())
                                .direct_credential_is_missing(cx)
                        },
                    );
            }

            true
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
            let now = (self.config.clock)();
            let binary = self.config.resolve_cli_binary(cx);
            let binary_str = binary.as_ref().map(|p| p.to_string_lossy().into_owned());
            let throttle_key = binary_str
                .as_deref()
                .and_then(|b| self.throttle_key(b, cx, &call));

            if cx.runtime == FetchRuntime::Daemon
                && cx.interaction == FetchInteraction::Background
                && !self.owner_cli_recovery_only
                && let Some(ref key) = throttle_key
                && let Some(cached) = self.config.cli_spawn_throttle.cached_result(key, now)
            {
                return Ok(cached);
            }

            let background_availability_marker = binary_str.as_deref().and_then(|b| {
                ClaudeCLIBackgroundAvailabilityStore::capture_marker(
                    b,
                    &cx.env,
                    &self.config.working_directory,
                    &self.config.home_directory,
                    &call,
                )
            });

            let initial_timeout = if cx.source_mode == SourceMode::Auto
                && self.config.cli_probe_timeout == ClaudeCLIUsageLoader::CLI_PROBE_TIMEOUT
            {
                ClaudeCLIUsageLoader::CLI_AUTO_PROBE_TIMEOUT
            } else {
                self.config.cli_probe_timeout
            };
            let binary_arg = binary_str
                .clone()
                .unwrap_or_else(|| self.config.cli_binary.clone());
            let mut probe = ClaudeStatusProbe::new(
                cx.env.clone(),
                self.config.home_directory.clone(),
                Arc::clone(&self.config.cli_session),
            )
            .with_working_directory(self.config.working_directory.clone())
            .with_binary(binary_arg)
            .with_timeout(initial_timeout)
            .with_keep_cli_sessions_alive(self.config.keep_cli_sessions_alive);
            if let Some(ref ov) = self.config.cli_status_probe_override {
                probe = probe.with_fetch_override(Arc::clone(ov));
            }
            if let Some(ref ov) = self.config.cli_direct_status_probe_override {
                probe = probe.with_direct_fetch_override(Arc::clone(ov));
            }

            let loader = ClaudeCLIUsageLoader {
                probe,
                rate_limit_gate: Arc::clone(&self.config.cli_rate_limit_gate),
                registry: Arc::clone(&self.config.process_registry),
                launch_gate: Arc::clone(&self.config.launch_gate),
            };

            let status_snapshot = match loader
                .load_with_retry(call.clone(), initial_timeout, now)
                .await
            {
                Ok(snap) => snap,
                Err(err) => {
                    if matches!(err, super::cli_parser::ClaudeStatusProbeError::Cancelled)
                        || cx.cancel.is_cancelled()
                        || call.cancel.is_cancelled()
                    {
                        return Err(FetchError::Cancelled);
                    }
                    if err.is_retryable()
                        && self
                            .config
                            .cli_background_availability
                            .is_established(background_availability_marker.as_ref())
                    {
                        return Err(FetchError::Classified(err.into_classified_error()));
                    }
                    if let Some(ref marker) = background_availability_marker {
                        self.config.cli_background_availability.revoke(marker);
                    }
                    return Err(FetchError::Classified(err.into_classified_error()));
                }
            };

            let fetch_now = (self.config.clock)();
            let mut snapshot = match status_snapshot.to_usage_snapshot(fetch_now) {
                Ok(snap) => snap,
                Err(classified) => {
                    if cx.cancel.is_cancelled() {
                        return Err(FetchError::Cancelled);
                    }
                    if let Some(ref marker) = background_availability_marker {
                        self.config.cli_background_availability.revoke(marker);
                    }
                    return Err(FetchError::Classified(classified));
                }
            };

            let (use_web_extras, include_prepaid_balance, manual_cookie_header) =
                self.effective_enrichment(cx);
            snapshot = apply_web_extras_if_needed(
                snapshot,
                WebExtrasApplyParams {
                    net: &self.config.net,
                    oauth_endpoint: &self.config.endpoint,
                    web_endpoint: &self.config.web_endpoint,
                    cookie_cache: &self.config.cookie_cache,
                    importer: &self.config.session_importer,
                    browser_gate: &self.config.browser_fetch_gate,
                    manual_cookie_header: manual_cookie_header
                        .as_ref()
                        .map(SecretString::expose_secret),
                    oauth_access_token: None,
                    oauth_profile: None,
                    use_web_extras,
                    include_prepaid_balance,
                    web_extras_timeout: ClaudeWebFetchStrategy::timeout_duration(
                        self.config.web_timeout_seconds,
                    ),
                    options: ClaudeWebFetchOptions {
                        target_organization_id: self.config.web_organization_id.clone(),
                        include_usage_details: use_web_extras,
                        include_prepaid_balance,
                        use_desktop_cookies: self.config.use_desktop_cookies,
                        cache_scope: self.config.cache_scope.clone(),
                        now: fetch_now,
                    },
                    call: &call,
                },
            )
            .await?;

            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }

            if cx.runtime == FetchRuntime::Daemon
                && cx.interaction == FetchInteraction::UserInitiated
                && let Some(marker) = background_availability_marker
            {
                self.config.cli_background_availability.establish(marker);
            }

            let result =
                FetchResult::new(snapshot, "claude", CLAUDE_CLI_STRATEGY_ID, SourceKind::Cli);
            if let Some(key) = throttle_key {
                self.config
                    .cli_spawn_throttle
                    .record(result.clone(), key, fetch_now);
            }
            Ok(result)
        })
    }

    fn should_fallback(&self, error: &FetchError, cx: &FetchContext) -> bool {
        if cx.cancel.is_cancelled() || matches!(error, FetchError::Cancelled) {
            return false;
        }
        if let FetchError::Classified(c) = error
            && c.transport == Some(TransportClass::Cancelled)
        {
            return false;
        }
        if cx.runtime != FetchRuntime::Daemon
            || cx.source_mode != SourceMode::Auto
            || self.owner_cli_recovery_only
        {
            return false;
        }
        if let FetchError::Classified(c) = error
            && ClaudeStatusProbeParser::is_subscription_quota_unavailable_description(Some(
                &c.message,
            ))
        {
            return false;
        }
        self.config.has_plausible_web_session(cx)
    }
}

/// Claude Admin API fetch strategy (`ClaudeAdminAPIFetchStrategy` in `ClaudeProviderDescriptor.swift:287-356`).
pub struct ClaudeAdminAPIFetchStrategy {
    config: ClaudeOAuthStrategyConfig,
}

impl ClaudeAdminAPIFetchStrategy {
    pub fn new(config: ClaudeOAuthStrategyConfig) -> Self {
        Self { config }
    }

    pub async fn resolve_token(&self, cx: &FetchContext) -> Option<SecretString> {
        let call = BrokerCall {
            interaction: cx.interaction,
            cancel: cx.cancel.clone(),
            request_id: cx.request_id.clone(),
        };

        if let Some(ref account_id) = cx.selected_token_account {
            if cx.source_mode != SourceMode::Api {
                return None;
            }
            let raw_account_token = match self.config.token_account_token.as_ref() {
                Some(tok) => Some(tok.clone()),
                None => match self.config.secret_store.as_ref() {
                    Some(store) => {
                        ClaudeAdminAPISettingsReader::token_account_from_secret_store(
                            store, account_id, call,
                        )
                        .await
                    }
                    None => None,
                },
            };
            return match ClaudeCredentialRouting::resolve(
                raw_account_token.as_ref().map(SecretString::expose_secret),
                None,
            ) {
                ClaudeCredentialRoute::AdminApiKey(token) => Some(token),
                _ => None,
            };
        }

        if let Some(config_token) = ClaudeCredentialRouting::normalize_admin_api_key(
            self.config
                .admin_api_key
                .as_ref()
                .map(SecretString::expose_secret),
        ) {
            return Some(config_token);
        }

        if let Some(ref store) = self.config.secret_store
            && let Some(store_token) =
                ClaudeAdminAPISettingsReader::api_key_from_secret_store(store, call).await
        {
            return Some(store_token);
        }

        ClaudeAdminAPISettingsReader::api_key(&cx.env)
    }

    pub fn resolve_token_sync(&self, cx: &FetchContext) -> Option<SecretString> {
        if cx.selected_token_account.is_some() {
            if cx.source_mode != SourceMode::Api {
                return None;
            }
            return match ClaudeCredentialRouting::resolve(
                self.config
                    .token_account_token
                    .as_ref()
                    .map(SecretString::expose_secret),
                None,
            ) {
                ClaudeCredentialRoute::AdminApiKey(token) => Some(token),
                _ => None,
            };
        }

        if let Some(config_token) = ClaudeCredentialRouting::normalize_admin_api_key(
            self.config
                .admin_api_key
                .as_ref()
                .map(SecretString::expose_secret),
        ) {
            return Some(config_token);
        }

        ClaudeAdminAPISettingsReader::api_key(&cx.env)
    }

    pub async fn is_selected_admin_api_account(&self, cx: &FetchContext) -> bool {
        cx.selected_token_account.is_some()
            && cx.source_mode == SourceMode::Api
            && self
                .resolve_token(cx)
                .await
                .is_some_and(|key| !key.expose_secret().is_empty())
    }

    pub fn is_selected_admin_api_account_sync(&self, cx: &FetchContext) -> bool {
        cx.selected_token_account.is_some()
            && cx.source_mode == SourceMode::Api
            && self
                .resolve_token_sync(cx)
                .is_some_and(|key| !key.expose_secret().is_empty())
    }

    pub fn workspace_spend_enabled(&self, cx: &FetchContext) -> bool {
        if self.config.workspace_spend_enabled {
            return true;
        }
        ClaudeAdminAPISettingsReader::workspace_spend_enabled(&cx.env)
    }
}

impl FetchStrategy for ClaudeAdminAPIFetchStrategy {
    fn id(&self) -> &'static str {
        CLAUDE_ADMIN_API_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Api
    }

    fn is_available<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            if cx.selected_token_account.is_some() {
                return self.is_selected_admin_api_account(cx).await;
            }
            if cx.source_mode == SourceMode::Api {
                return true;
            }
            self.resolve_token(cx).await.is_some()
        })
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            if cx.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }
            let Some(api_key) = self.resolve_token(cx).await else {
                return Err(ClaudeAdminAPISettingsError::MissingToken.into_fetch_error());
            };
            let workspace_spend_enabled = self.workspace_spend_enabled(cx);
            let now = (self.config.clock)();
            let call = BrokerCall {
                interaction: cx.interaction,
                cancel: cx.cancel.clone(),
                request_id: cx.request_id.clone(),
            };
            let usage = ClaudeAdminAPIUsageFetcher::fetch_usage(
                &self.config.net,
                &self.config.admin_api_endpoint,
                api_key.expose_secret(),
                workspace_spend_enabled,
                now,
                &call,
            )
            .await
            .map_err(|err| err.into_fetch_error())?;

            Ok(FetchResult::new(
                usage.to_usage_snapshot(),
                "admin-api",
                CLAUDE_ADMIN_API_STRATEGY_ID,
                SourceKind::Api,
            ))
        })
    }

    fn should_fallback(&self, error: &FetchError, cx: &FetchContext) -> bool {
        if cx.cancel.is_cancelled() || matches!(error, FetchError::Cancelled) {
            return false;
        }
        if let FetchError::Classified(c) = error
            && c.transport == Some(TransportClass::Cancelled)
        {
            return false;
        }
        cx.runtime == FetchRuntime::Daemon
            && cx.source_mode == SourceMode::Auto
            && !self.is_selected_admin_api_account_sync(cx)
    }
}

/// Unavailable strategy stub retained for explicit testing of disabled/unavailable sources.
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
            has_web_session: self.config.has_plausible_web_session(cx),
            has_cli: self.config.has_cli_in_context(cx),
            has_oauth_credentials: should_attempt_oauth,
        };
        ClaudeSourcePlanner::resolve(input)
    }

    /// Compatibility helper matching `ClaudeProviderDescriptor.resolveUsageStrategy`.
    /// Returns `None` when `selected_data_source` is `Auto` and no planned step is available,
    /// because `ClaudeSourceSelectedStrategy` excludes `Auto` by type.
    pub fn resolve_usage_strategy(
        selected_data_source: SourceMode,
        web_extras_enabled: bool,
        has_web_session: bool,
        has_cli: bool,
        has_oauth_credentials: bool,
    ) -> Option<ClaudeUsageStrategy> {
        let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
            runtime: FetchRuntime::Daemon,
            selected_data_source,
            web_extras_enabled,
            has_web_session,
            has_cli,
            has_oauth_credentials,
        });
        plan.compatibility_strategy().or_else(|| {
            let data_source = match selected_data_source {
                SourceMode::Auto => return None,
                SourceMode::Api => ClaudeSourceSelectedStrategy::Api,
                SourceMode::Oauth => ClaudeSourceSelectedStrategy::Oauth,
                SourceMode::Web => ClaudeSourceSelectedStrategy::Web,
                SourceMode::Cli => ClaudeSourceSelectedStrategy::Cli,
            };
            Some(ClaudeUsageStrategy {
                data_source,
                use_web_extras: false,
            })
        })
    }

    async fn resolve_selected_account_strategies(
        &self,
        cx: &FetchContext,
    ) -> Vec<Box<dyn FetchStrategy>> {
        let admin_strategy = ClaudeAdminAPIFetchStrategy::new(self.config.clone());
        if cx.source_mode == SourceMode::Api
            && admin_strategy.is_selected_admin_api_account(cx).await
        {
            return vec![Box::new(admin_strategy)];
        }

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

        if cx.source_mode == SourceMode::Api {
            return vec![Box::new(admin_strategy)];
        }

        if matches!(route, ClaudeCredentialRoute::AdminApiKey(_)) {
            return vec![Box::new(admin_strategy)];
        }

        if cx.source_mode == SourceMode::Oauth
            && (matches!(route, ClaudeCredentialRoute::Oauth { .. })
                || load_from_environment(&cx.env).is_some())
        {
            return vec![Box::new(ClaudeOAuthFetchStrategy::new(self.config.clone()))];
        }

        if cx.source_mode == SourceMode::Web
            && self.config.effective_cookie_source() == CookieSource::Manual
            && let ClaudeCredentialRoute::WebCookie { header } = route
        {
            let mut web_config = self.config.clone();
            web_config.cookie_source = Some(CookieSource::Manual);
            web_config.manual_cookie_header = Some(header);
            return vec![Box::new(ClaudeWebFetchStrategy::new(web_config))];
        }

        Vec::new()
    }
}

pub fn has_admin_api_key(env: &BTreeMap<String, SecretString>) -> bool {
    ClaudeAdminAPISettingsReader::api_key(env).is_some()
}

impl ProviderPlan for ClaudeProviderPlan {
    fn resolve_strategies<'a>(
        &'a self,
        cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(async move {
            // Selected account is an authority boundary: never fall through to ambient credentials.
            if cx.selected_token_account.is_some() {
                return self.resolve_selected_account_strategies(cx).await;
            }

            let admin_strategy = ClaudeAdminAPIFetchStrategy::new(self.config.clone());
            if cx.source_mode == SourceMode::Api
                || (cx.source_mode == SourceMode::Auto
                    && admin_strategy.resolve_token(cx).await.is_some())
            {
                return vec![Box::new(admin_strategy)];
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
                        ClaudeSourceSelectedStrategy::Cli => Box::new(
                            ClaudeCLIFetchStrategy::new(self.config.clone())
                                .with_planned_step(step),
                        ),
                        ClaudeSourceSelectedStrategy::Web => Box::new(
                            ClaudeWebFetchStrategy::new(self.config.clone())
                                .with_planned_step(step),
                        ),
                        ClaudeSourceSelectedStrategy::Api => {
                            Box::new(ClaudeAdminAPIFetchStrategy::new(self.config.clone()))
                        }
                    }
                })
                .collect()
        })
    }
}
