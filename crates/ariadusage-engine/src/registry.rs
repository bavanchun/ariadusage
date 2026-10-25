// Ported from CodexBar Sources/CodexBar/ProviderRegistry.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use ariadusage_core::config::{Config, CookieSource, ProviderConfig, TokenAccountMeta};
use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::{
    AccountTokenProjection, BoxFuture, EnvPrecedence, EnvProjection, FetchContext, FetchStrategy,
    ProviderEnvDescriptor, ProviderEnvironmentResolver, ProviderPlan,
};
use ariadusage_core::providers::{
    ProviderDescriptor, SourceMode, find_by_id, find_by_id_str, first_party_order,
};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::secret::SecretString;

use crate::brokers::net::{NetBroker, NetConfig};
use crate::brokers::process::ProcessRegistry;
use crate::brokers::secret_store::SecretStore;
use crate::config_store::ConfigStore;
use crate::error::StoreError;
use crate::providers::claude::{
    CLAUDE_OAUTH_TOKEN_ENV_KEY, ClaudeAdminAPISettingsReader, ClaudeCredentialRoute,
    ClaudeCredentialRouting, ClaudeOAuthStrategyConfig, ClaudeProviderPlan,
    claude_provider_config_env_descriptor,
};
use crate::providers::codex::{CodexProviderPlan, CodexStrategyConfig};

/// Per-refresh inputs resolved from configuration, secret store, and runtime state when
/// building a provider's execution plan and `FetchContext`.
#[derive(Clone, Default)]
pub struct ProviderPlanInputs {
    pub base_environment: BTreeMap<String, SecretString>,
    pub api_key: Option<SecretString>,
    pub manual_cookie_header: Option<SecretString>,
    pub selected_account_token: Option<SecretString>,
    pub include_optional_usage: bool,
    pub secret_store: Option<Arc<SecretStore>>,
}

impl std::fmt::Debug for ProviderPlanInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderPlanInputs")
            .field(
                "base_env_keys",
                &self.base_environment.keys().collect::<Vec<_>>(),
            )
            .field("has_api_key", &self.api_key.is_some())
            .field(
                "has_manual_cookie_header",
                &self.manual_cookie_header.is_some(),
            )
            .field(
                "has_selected_account_token",
                &self.selected_account_token.is_some(),
            )
            .field("include_optional_usage", &self.include_optional_usage)
            .field("has_secret_store", &self.secret_store.is_some())
            .finish()
    }
}

/// Factory for constructing a `ProviderPlan` given the provider's current configuration and secrets.
pub trait ProviderPlanFactory: Send + Sync {
    fn build_plan(
        &self,
        provider_config: Option<&ProviderConfig>,
        selected_account: Option<&TokenAccountMeta>,
        inputs: &ProviderPlanInputs,
    ) -> Arc<dyn ProviderPlan>;
}

struct StaticPlanFactory {
    plan: Arc<dyn ProviderPlan>,
}

impl ProviderPlanFactory for StaticPlanFactory {
    fn build_plan(
        &self,
        _provider_config: Option<&ProviderConfig>,
        _selected_account: Option<&TokenAccountMeta>,
        _inputs: &ProviderPlanInputs,
    ) -> Arc<dyn ProviderPlan> {
        Arc::clone(&self.plan)
    }
}

/// Empty fallback plan used for providers whose strategy modules have not yet been registered.
/// Yields an empty strategy list so `fetch_outcome` returns `FetchError::NoAvailableStrategy`.
pub struct UnregisteredProviderPlan;

impl ProviderPlan for UnregisteredProviderPlan {
    fn resolve_strategies<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(async { Vec::new() })
    }
}

/// Factory that builds a `ClaudeProviderPlan` from a shared `ClaudeOAuthStrategyConfig` template,
/// preserving stateful gates/caches/sessions across refreshes while applying per-refresh config.
pub struct ClaudeProviderPlanFactory {
    base_config: ClaudeOAuthStrategyConfig,
}

impl ClaudeProviderPlanFactory {
    pub fn new(base_config: ClaudeOAuthStrategyConfig) -> Self {
        Self { base_config }
    }
}

impl ProviderPlanFactory for ClaudeProviderPlanFactory {
    fn build_plan(
        &self,
        provider_config: Option<&ProviderConfig>,
        selected_account: Option<&TokenAccountMeta>,
        inputs: &ProviderPlanInputs,
    ) -> Arc<dyn ProviderPlan> {
        let mut cfg = self.base_config.clone();
        cfg.include_optional_usage = inputs.include_optional_usage;
        cfg.web_extras_enabled = provider_config
            .and_then(|c| c.extras_enabled)
            .unwrap_or(self.base_config.web_extras_enabled);
        cfg.workspace_spend_enabled = provider_config
            .and_then(ProviderConfig::claude_workspace_spend_enabled_opt)
            .unwrap_or(self.base_config.workspace_spend_enabled);
        cfg.secret_store = inputs
            .secret_store
            .clone()
            .or_else(|| self.base_config.secret_store.clone());
        cfg.admin_api_key = if selected_account.is_none() {
            ClaudeAdminAPISettingsReader::clean_token(
                inputs.api_key.as_ref().map(SecretString::expose_secret),
            )
            .or_else(|| self.base_config.admin_api_key.clone())
        } else {
            None
        };

        let manual_cookie_header = if selected_account.is_none() {
            inputs
                .manual_cookie_header
                .clone()
                .or_else(|| self.base_config.manual_cookie_header.clone())
        } else {
            None
        };
        let selected_token = inputs
            .selected_account_token
            .clone()
            .or_else(|| self.base_config.token_account_token.clone());

        let route = ClaudeCredentialRouting::resolve(
            selected_token.as_ref().map(SecretString::expose_secret),
            manual_cookie_header
                .as_ref()
                .map(SecretString::expose_secret),
        );

        let configured_cookie_source = provider_config
            .and_then(|c| c.cookie_source)
            .or(self.base_config.cookie_source);
        let effective_cookie_source = match &route {
            ClaudeCredentialRoute::Oauth { .. } | ClaudeCredentialRoute::AdminApiKey(_) => {
                Some(CookieSource::Off)
            }
            ClaudeCredentialRoute::WebCookie { .. } | ClaudeCredentialRoute::None => {
                configured_cookie_source
            }
        };

        cfg.cookie_source = effective_cookie_source;
        cfg.token_account_token = selected_token;
        cfg.manual_cookie_header = match route {
            ClaudeCredentialRoute::WebCookie { header } => Some(header),
            _ => manual_cookie_header,
        };
        if let Some(account) = selected_account {
            cfg.web_organization_id = account.sanitized_organization_id();
        }
        Arc::new(ClaudeProviderPlan::new(cfg))
    }
}

/// Factory that builds a `CodexProviderPlan` from a shared `CodexStrategyConfig` template,
/// preserving stateful launch gates and process registries across refreshes while applying
/// per-refresh environment and workspace configuration.
pub struct CodexProviderPlanFactory {
    base_config: CodexStrategyConfig,
}

impl CodexProviderPlanFactory {
    pub fn new(base_config: CodexStrategyConfig) -> Self {
        Self { base_config }
    }
}

impl ProviderPlanFactory for CodexProviderPlanFactory {
    fn build_plan(
        &self,
        provider_config: Option<&ProviderConfig>,
        _selected_account: Option<&TokenAccountMeta>,
        inputs: &ProviderPlanInputs,
    ) -> Arc<dyn ProviderPlan> {
        let mut cfg = self.base_config.clone();
        for (k, v) in &inputs.base_environment {
            cfg.environment
                .insert(k.clone(), v.expose_secret().to_string());
        }
        if let Some(ws) = provider_config.and_then(ProviderConfig::sanitized_workspace_id) {
            cfg.environment
                .insert("CODEX_WORKSPACE_ID".to_string(), ws.clone());
            cfg.managed_workspace_account_id = Some(ws);
        } else if let Some(ws) = cfg
            .environment
            .get("CODEX_WORKSPACE_ID")
            .map(String::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
        {
            cfg.managed_workspace_account_id = Some(ws);
        }
        if let Some(home) = cfg.environment.get("HOME").map(PathBuf::from) {
            cfg.home_directory = home.clone();
            cfg.cli_fetcher.home_dir = Some(home);
        }
        cfg.cli_fetcher.environment = cfg.environment.clone();
        Arc::new(CodexProviderPlan::new(cfg))
    }
}

/// Resolved execution specification for a single provider in a refresh pass.
pub struct ResolvedProviderSpec {
    pub descriptor: &'static ProviderDescriptor,
    pub enabled: bool,
    pub source_mode: SourceMode,
    pub selected_account: Option<TokenAccountMeta>,
    /// Account discriminator used by the coordinator to detect account switches and isolate state.
    pub account_key: Option<String>,
    pub environment: BTreeMap<String, SecretString>,
    pub plan: Arc<dyn ProviderPlan>,
}

impl std::fmt::Debug for ResolvedProviderSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedProviderSpec")
            .field("id", &self.descriptor.id)
            .field("enabled", &self.enabled)
            .field("source_mode", &self.source_mode)
            .field(
                "selected_account_id",
                &self.selected_account.as_ref().map(|a| a.id.as_str()),
            )
            .field("account_key", &self.account_key)
            .finish()
    }
}

/// Registry of merged first-party provider descriptors and their runtime execution plans.
///
/// Wires `ClaudeProviderPlan` and `CodexProviderPlan` out of the box and allows `Antigravity` (#52)
/// or test doubles to register their `ProviderPlan` or `ProviderPlanFactory`.
#[derive(Clone)]
pub struct ProviderRegistry {
    descriptors: &'static [ProviderDescriptor],
    factories: HashMap<ProviderId, Arc<dyn ProviderPlanFactory>>,
    working_directory: PathBuf,
    home_directory: PathBuf,
    explicit_directories: bool,
    process_registry: Arc<ProcessRegistry>,
    launch_gate: Arc<LaunchGate>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        let net = Arc::new(
            NetBroker::new(NetConfig::default()).expect("default NetBroker should initialize"),
        );
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
        let home_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| working_dir.clone());
        let claude_config =
            ClaudeOAuthStrategyConfig::new(Arc::clone(&net), working_dir.clone(), home_dir.clone());
        let codex_config = CodexStrategyConfig::new(net, BTreeMap::new(), home_dir, working_dir)
            .with_process_broker(
                Arc::clone(&claude_config.process_registry),
                Arc::clone(&claude_config.launch_gate),
            );
        let mut registry = Self::with_claude_and_codex_config(claude_config, codex_config);
        registry.explicit_directories = false;
        registry
    }
}

impl ProviderRegistry {
    /// Creates a registry with the default descriptors and a `ClaudeProviderPlanFactory`
    /// backed by `claude_config`.
    pub fn with_claude_config(claude_config: ClaudeOAuthStrategyConfig) -> Self {
        let mut registry = Self::empty()
            .with_directories(
                claude_config.working_directory.clone(),
                claude_config.home_directory.clone(),
            )
            .with_process_broker(
                Arc::clone(&claude_config.process_registry),
                Arc::clone(&claude_config.launch_gate),
            );
        let claude_id = ProviderId::new("claude").expect("valid provider id");
        registry.register_factory(
            claude_id,
            Arc::new(ClaudeProviderPlanFactory::new(claude_config)),
        );
        registry
    }

    /// Creates a registry with the default descriptors and both `ClaudeProviderPlanFactory`
    /// and `CodexProviderPlanFactory` registered.
    pub fn with_claude_and_codex_config(
        claude_config: ClaudeOAuthStrategyConfig,
        codex_config: CodexStrategyConfig,
    ) -> Self {
        let mut registry = Self::with_claude_config(claude_config);
        let codex_id = ProviderId::new("codex").expect("valid provider id");
        registry.register_factory(
            codex_id,
            Arc::new(CodexProviderPlanFactory::new(codex_config)),
        );
        registry
    }

    /// Creates a registry containing all first-party descriptors and no registered plan factories.
    pub fn empty() -> Self {
        let working_directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
        let home_directory = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| working_directory.clone());
        Self {
            descriptors: first_party_order(),
            factories: HashMap::new(),
            working_directory,
            home_directory,
            explicit_directories: false,
            process_registry: Arc::new(ProcessRegistry::new()),
            launch_gate: Arc::new(LaunchGate::default()),
        }
    }

    /// Returns the shared [`ProcessRegistry`] associated with this registry.
    pub fn process_registry(&self) -> &Arc<ProcessRegistry> {
        &self.process_registry
    }

    /// Returns the shared [`LaunchGate`] associated with this registry.
    pub fn launch_gate(&self) -> &Arc<LaunchGate> {
        &self.launch_gate
    }

    /// Overrides the shared [`ProcessRegistry`] and [`LaunchGate`] stored on this registry.
    pub fn with_process_broker(
        mut self,
        process_registry: Arc<ProcessRegistry>,
        launch_gate: Arc<LaunchGate>,
    ) -> Self {
        self.process_registry = process_registry;
        self.launch_gate = launch_gate;
        self
    }

    /// Overrides the working and home directories used when fingerprinting ambient provider credentials.
    pub fn with_directories(
        mut self,
        working_directory: impl Into<PathBuf>,
        home_directory: impl Into<PathBuf>,
    ) -> Self {
        self.working_directory = working_directory.into();
        self.home_directory = home_directory.into();
        self.explicit_directories = true;
        self
    }

    /// Registers a static `ProviderPlan` for `id` (used by provider modules or tests).
    pub fn register_plan(&mut self, id: ProviderId, plan: Arc<dyn ProviderPlan>) {
        self.factories
            .insert(id, Arc::new(StaticPlanFactory { plan }));
    }

    /// Registers a dynamic `ProviderPlanFactory` for `id`.
    pub fn register_factory(&mut self, id: ProviderId, factory: Arc<dyn ProviderPlanFactory>) {
        self.factories.insert(id, factory);
    }

    /// Returns true if a runtime `ProviderPlanFactory` is registered for `id`.
    pub fn has_registered_plan(&self, id: &ProviderId) -> bool {
        self.factories.contains_key(id)
    }

    /// Returns all first-party provider descriptors in canonical enum order (`codex`, `claude`, `antigravity`).
    pub fn descriptors(&self) -> &'static [ProviderDescriptor] {
        self.descriptors
    }

    /// Looks up a descriptor by `ProviderId`.
    pub fn descriptor(&self, id: &ProviderId) -> Option<&'static ProviderDescriptor> {
        find_by_id(id)
    }

    /// Looks up a descriptor by string slice.
    pub fn descriptor_by_str(&self, id: &str) -> Option<&'static ProviderDescriptor> {
        find_by_id_str(id)
    }

    /// Loads the effective normalized configuration from `ConfigStore` without mutating disk.
    pub fn load_config(&self, config_store: &ConfigStore) -> Result<Config, StoreError> {
        config_store.load_effective()
    }

    /// Returns all first-party provider descriptors ordered according to `config.providers`,
    /// appending any omitted first-party providers in canonical order.
    pub fn ordered_descriptors(&self, config: &Config) -> Vec<&'static ProviderDescriptor> {
        let mut seen = HashSet::new();
        let mut ordered = Vec::with_capacity(self.descriptors.len());

        for entry in &config.providers {
            if let Some(descriptor) = self.descriptor_by_str(entry.id())
                && seen.insert(descriptor.id.as_str())
            {
                ordered.push(descriptor);
            }
        }

        for descriptor in self.descriptors {
            if seen.insert(descriptor.id.as_str()) {
                ordered.push(descriptor);
            }
        }

        ordered
    }

    /// Returns the enabled first-party provider descriptors in `config` order.
    pub fn enabled_descriptors(&self, config: &Config) -> Vec<&'static ProviderDescriptor> {
        self.ordered_descriptors(config)
            .into_iter()
            .filter(|descriptor| self.is_provider_enabled(config, &descriptor.id))
            .collect()
    }

    /// Returns whether `id` is enabled in `config`, falling back to `descriptor.default_enabled`.
    pub fn is_provider_enabled(&self, config: &Config, id: &ProviderId) -> bool {
        if let Some(entry) = config.provider_entry(id.as_str()) {
            entry.is_enabled()
        } else {
            self.descriptor(id).is_some_and(|d| d.default_enabled)
        }
    }

    /// Resolves the active `TokenAccountMeta` for `provider_config`, if any accounts are configured.
    pub fn selected_account(provider_config: Option<&ProviderConfig>) -> Option<TokenAccountMeta> {
        let accounts = provider_config?.token_accounts.as_ref()?;
        if accounts.accounts.is_empty() {
            return None;
        }
        let idx = accounts.clamped_active_index();
        accounts.accounts.get(idx).cloned()
    }

    /// Resolves the effective `SourceMode` for a provider, applying account credential routing
    /// when a token account is selected (`selectedAccountSourceModeResolver`).
    pub fn effective_source_mode(
        descriptor: &ProviderDescriptor,
        provider_config: Option<&ProviderConfig>,
        selected_account: Option<&TokenAccountMeta>,
        selected_account_token: Option<&SecretString>,
    ) -> SourceMode {
        let base = provider_config
            .and_then(|c| c.source)
            .filter(|mode| descriptor.source_modes.contains(mode))
            .unwrap_or(SourceMode::Auto);

        if descriptor.id.as_str() == "claude" && selected_account.is_some() {
            return match ClaudeCredentialRouting::resolve(
                selected_account_token.map(SecretString::expose_secret),
                None,
            ) {
                ClaudeCredentialRoute::AdminApiKey(_) => SourceMode::Api,
                ClaudeCredentialRoute::Oauth { .. } => SourceMode::Oauth,
                ClaudeCredentialRoute::WebCookie { .. } => SourceMode::Web,
                ClaudeCredentialRoute::None => base,
            };
        }

        base
    }

    /// Resolves the provider environment using `ProviderEnvironmentResolver` and account scrubbing.
    pub fn resolve_environment(
        descriptor: &ProviderDescriptor,
        provider_config: Option<&ProviderConfig>,
        selected_account: Option<&TokenAccountMeta>,
        inputs: &ProviderPlanInputs,
    ) -> BTreeMap<String, SecretString> {
        let mut base = inputs.base_environment.clone();

        if descriptor.id.as_str() == "claude" {
            if let Some(api_key) = &inputs.api_key
                && let Some(cleaned) =
                    ClaudeAdminAPISettingsReader::clean_token(Some(api_key.expose_secret()))
            {
                base.insert(
                    ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY.to_string(),
                    cleaned,
                );
            }

            let target_key = if selected_account.is_some() {
                match ClaudeCredentialRouting::resolve(
                    inputs
                        .selected_account_token
                        .as_ref()
                        .map(SecretString::expose_secret),
                    None,
                ) {
                    ClaudeCredentialRoute::Oauth { .. } => {
                        Some(CLAUDE_OAUTH_TOKEN_ENV_KEY.to_string())
                    }
                    ClaudeCredentialRoute::AdminApiKey(_) => {
                        Some(ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY.to_string())
                    }
                    ClaudeCredentialRoute::WebCookie { .. } | ClaudeCredentialRoute::None => None,
                }
            } else {
                None
            };

            let mut env_descriptor = claude_provider_config_env_descriptor(|_| None);
            if let Some(acct_proj) = &mut env_descriptor.account_projection {
                acct_proj.target_key = target_key;
            }

            let token_for_projection = match ClaudeCredentialRouting::resolve(
                inputs
                    .selected_account_token
                    .as_ref()
                    .map(SecretString::expose_secret),
                None,
            ) {
                ClaudeCredentialRoute::Oauth { access_token } => Some(access_token),
                ClaudeCredentialRoute::AdminApiKey(key) => Some(key),
                ClaudeCredentialRoute::WebCookie { .. } | ClaudeCredentialRoute::None => {
                    inputs.selected_account_token.clone()
                }
            };

            let empty_token = SecretString::new(String::new());
            return ProviderEnvironmentResolver::resolve(
                &base,
                provider_config,
                if selected_account.is_some() {
                    token_for_projection
                        .as_ref()
                        .or(inputs.selected_account_token.as_ref())
                        .or(Some(&empty_token))
                } else {
                    None
                },
                &env_descriptor,
            );
        }

        if descriptor.id.as_str() == "antigravity" {
            let env_descriptor = ProviderEnvDescriptor::<ProviderConfig> {
                projections: Vec::new(),
                account_projection: Some(AccountTokenProjection {
                    scrub_keys: vec!["CODEXBAR_ANTIGRAVITY_OAUTH_CREDENTIALS".to_string()],
                    target_key: Some("CODEXBAR_ANTIGRAVITY_OAUTH_CREDENTIALS".to_string()),
                }),
            };
            return ProviderEnvironmentResolver::resolve(
                &base,
                provider_config,
                if selected_account.is_some() {
                    inputs.selected_account_token.as_ref()
                } else {
                    None
                },
                &env_descriptor,
            );
        }

        let env_descriptor = ProviderEnvDescriptor::<ProviderConfig> {
            projections: vec![EnvProjection {
                key: "CODEX_WORKSPACE_ID".to_string(),
                precedence: EnvPrecedence::Config,
                value_from_config: ProviderConfig::sanitized_workspace_id,
            }],
            account_projection: None,
        };
        ProviderEnvironmentResolver::resolve(&base, provider_config, None, &env_descriptor)
    }

    /// Computes the account/credential discriminator for `id` from `provider_config`, `environment`,
    /// and `inputs`.
    ///
    /// Used both before and after a provider fetch so credential swaps before or during a fetch
    /// retire the prior account's snapshot, reset-backfill state, and failure gate.
    pub fn compute_account_key(
        &self,
        id: &ProviderId,
        provider_config: Option<&ProviderConfig>,
        selected_account: Option<&TokenAccountMeta>,
        environment: &BTreeMap<String, SecretString>,
        inputs: &ProviderPlanInputs,
    ) -> Option<String> {
        if let Some(acct) = selected_account {
            if let Some(tok) = &inputs.selected_account_token {
                let digest = &ariadusage_core::digest::sha256_hex(
                    "ariadusage:account-key:v1",
                    &[tok.expose_secret().as_bytes()],
                )[..12];
                return Some(format!("token-account:{}:{digest}", acct.id));
            }
            return Some(format!("token-account:{}", acct.id));
        }

        let mut parts = Vec::new();
        if let Some(ws) = provider_config.and_then(ProviderConfig::sanitized_workspace_id) {
            parts.push(format!("workspace:{ws}"));
        }
        if let Some(api_key) = &inputs.api_key {
            let digest = &ariadusage_core::digest::sha256_hex(
                "ariadusage:account-key:v1",
                &[api_key.expose_secret().as_bytes()],
            )[..12];
            parts.push(format!("api-key:{digest}"));
        }
        if let Some(cookie) = &inputs.manual_cookie_header {
            let digest = &ariadusage_core::digest::sha256_hex(
                "ariadusage:account-key:v1",
                &[cookie.expose_secret().as_bytes()],
            )[..12];
            parts.push(format!("cookie:{digest}"));
        }
        for env_key in [
            CLAUDE_OAUTH_TOKEN_ENV_KEY,
            "ANTHROPIC_ADMIN_KEY",
            "ANTHROPIC_ADMIN_API_KEY",
            "CODEXBAR_ANTIGRAVITY_OAUTH_CREDENTIALS",
        ] {
            if let Some(val) = environment
                .get(env_key)
                .map(SecretString::expose_secret)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let digest = &ariadusage_core::digest::sha256_hex(
                    "ariadusage:account-key:v1",
                    &[val.as_bytes()],
                )[..12];
                parts.push(format!("env:{env_key}:{digest}"));
            }
        }

        if id.as_str() == "claude"
            && (self.explicit_directories
                || environment.contains_key("CLAUDE_CONFIG_DIR")
                || environment.contains_key("HOME"))
        {
            let home_dir = environment
                .get("HOME")
                .map(|s| PathBuf::from(s.expose_secret()))
                .unwrap_or_else(|| self.home_directory.clone());
            let call = crate::brokers::call::BrokerCall {
                interaction: ariadusage_core::pipeline::FetchInteraction::Background,
                cancel: tokio_util::sync::CancellationToken::new(),
                request_id: "account-key".to_string(),
            };
            let cred_path =
                crate::providers::claude::ClaudeConfigPaths::credentials_path_from_secret_env(
                    environment,
                    &self.working_directory,
                    &home_dir,
                );
            let decl = crate::brokers::credential_file::CredentialDecl::new(&cred_path);
            match crate::brokers::credential_file::read(&decl, &call) {
                Ok(read_res) => {
                    let fp = crate::brokers::credential_file::sha256_prefix(&read_res.bytes);
                    parts.push(format!("claude-cred:{fp}"));
                }
                Err(crate::brokers::credential_file::CredentialFileError::NotFound) => {}
                Err(_) => {
                    parts.push("claude-cred:unreadable".to_string());
                }
            }
            if let Some(uuid) = crate::providers::claude::ClaudeAccountProfile::account_uuid(
                environment,
                &self.working_directory,
                &home_dir,
                &call,
            ) {
                let digest = &ariadusage_core::digest::sha256_hex(
                    "ariadusage:account-key:v1",
                    &[uuid.as_bytes()],
                )[..12];
                parts.push(format!("claude-acct:{digest}"));
            }
        }

        if id.as_str() == "codex"
            && (self.explicit_directories
                || environment.contains_key("CODEX_HOME")
                || environment.contains_key("HOME"))
        {
            let home_dir = environment
                .get("HOME")
                .map(|s| PathBuf::from(s.expose_secret()))
                .unwrap_or_else(|| self.home_directory.clone());
            let plain_env: BTreeMap<String, String> = environment
                .iter()
                .map(|(k, v)| (k.clone(), v.expose_secret().to_string()))
                .collect();
            let call = crate::brokers::call::BrokerCall {
                interaction: ariadusage_core::pipeline::FetchInteraction::Background,
                cancel: tokio_util::sync::CancellationToken::new(),
                request_id: "account-key".to_string(),
            };
            if let Some(auth_path) =
                crate::providers::codex::CodexOAuthCredentialsStore::auth_file_path(
                    &plain_env,
                    Some(&home_dir),
                )
            {
                let decl = crate::brokers::credential_file::CredentialDecl::new(&auth_path);
                match crate::brokers::credential_file::read(&decl, &call) {
                    Ok(read_res) => {
                        let fp = crate::brokers::credential_file::sha256_prefix(&read_res.bytes);
                        parts.push(format!("codex-auth:{fp}"));
                    }
                    Err(crate::brokers::credential_file::CredentialFileError::NotFound) => {}
                    Err(_) => {
                        parts.push("codex-auth:unreadable".to_string());
                    }
                }
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("|"))
        }
    }

    /// Resolves the full `ResolvedProviderSpec` for `id` from `config` and `inputs`.
    pub fn resolve_spec(
        &self,
        id: &ProviderId,
        config: &Config,
        inputs: &ProviderPlanInputs,
    ) -> Option<ResolvedProviderSpec> {
        let descriptor = self.descriptor(id)?;
        let enabled = self.is_provider_enabled(config, id);
        let provider_config = config.provider_config(id.as_str());
        let selected_account = Self::selected_account(provider_config);
        let source_mode = Self::effective_source_mode(
            descriptor,
            provider_config,
            selected_account.as_ref(),
            inputs.selected_account_token.as_ref(),
        );
        let environment = Self::resolve_environment(
            descriptor,
            provider_config,
            selected_account.as_ref(),
            inputs,
        );
        let account_key = self.compute_account_key(
            id,
            provider_config,
            selected_account.as_ref(),
            &environment,
            inputs,
        );

        let plan = match self.factories.get(id) {
            Some(factory) => factory.build_plan(provider_config, selected_account.as_ref(), inputs),
            None => Arc::new(UnregisteredProviderPlan),
        };

        Some(ResolvedProviderSpec {
            descriptor,
            enabled,
            source_mode,
            selected_account,
            account_key,
            environment,
            plan,
        })
    }
}
