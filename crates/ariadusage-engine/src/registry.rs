// Ported from CodexBar Sources/CodexBar/ProviderRegistry.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use ariadusage_core::config::{Config, CookieSource, ProviderConfig, TokenAccountMeta};
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
use crate::config_store::ConfigStore;
use crate::error::StoreError;
use crate::providers::claude::{
    CLAUDE_OAUTH_TOKEN_ENV_KEY, ClaudeCredentialRoute, ClaudeCredentialRouting,
    ClaudeOAuthStrategyConfig, ClaudeProviderPlan,
};

/// Per-refresh inputs resolved from configuration, secret store, and runtime state when
/// building a provider's execution plan and `FetchContext`.
#[derive(Clone, Default)]
pub struct ProviderPlanInputs {
    pub base_environment: BTreeMap<String, SecretString>,
    pub api_key: Option<SecretString>,
    pub manual_cookie_header: Option<SecretString>,
    pub selected_account_token: Option<SecretString>,
    pub include_optional_usage: bool,
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
            .or(self.base_config.cookie_source)
            .unwrap_or(CookieSource::Auto);
        let effective_cookie_source = match &route {
            ClaudeCredentialRoute::Oauth { .. } | ClaudeCredentialRoute::AdminApiKey(_) => {
                CookieSource::Off
            }
            ClaudeCredentialRoute::WebCookie { .. } | ClaudeCredentialRoute::None => {
                configured_cookie_source
            }
        };

        cfg.cookie_source = Some(effective_cookie_source);
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
/// Wires `ClaudeProviderPlan` out of the box and allows `Codex` (#49), `Antigravity` (#52),
/// or test doubles to register their `ProviderPlan` or `ProviderPlanFactory`.
#[derive(Clone)]
pub struct ProviderRegistry {
    descriptors: &'static [ProviderDescriptor],
    factories: HashMap<ProviderId, Arc<dyn ProviderPlanFactory>>,
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
        let claude_config = ClaudeOAuthStrategyConfig::new(net, working_dir, home_dir);
        Self::with_claude_config(claude_config)
    }
}

impl ProviderRegistry {
    /// Creates a registry with the default descriptors and a `ClaudeProviderPlanFactory`
    /// backed by `claude_config`.
    pub fn with_claude_config(claude_config: ClaudeOAuthStrategyConfig) -> Self {
        let mut registry = Self::empty();
        let claude_id = ProviderId::new("claude").expect("valid provider id");
        registry.register_factory(
            claude_id,
            Arc::new(ClaudeProviderPlanFactory::new(claude_config)),
        );
        registry
    }

    /// Creates a registry containing all first-party descriptors and no registered plan factories.
    pub fn empty() -> Self {
        Self {
            descriptors: first_party_order(),
            factories: HashMap::new(),
        }
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
                && !base.contains_key("ANTHROPIC_ADMIN_API_KEY")
            {
                base.insert("ANTHROPIC_ADMIN_API_KEY".to_string(), api_key.clone());
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
                        Some("ANTHROPIC_ADMIN_API_KEY".to_string())
                    }
                    ClaudeCredentialRoute::WebCookie { .. } | ClaudeCredentialRoute::None => None,
                }
            } else {
                None
            };

            let env_descriptor = ProviderEnvDescriptor::<ProviderConfig> {
                projections: Vec::new(),
                account_projection: Some(AccountTokenProjection {
                    scrub_keys: vec![
                        CLAUDE_OAUTH_TOKEN_ENV_KEY.to_string(),
                        "ANTHROPIC_ADMIN_API_KEY".to_string(),
                        "CLAUDE_ADMIN_API_KEY".to_string(),
                    ],
                    target_key,
                }),
            };

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

            return ProviderEnvironmentResolver::resolve(
                &base,
                provider_config,
                if selected_account.is_some() {
                    token_for_projection
                        .as_ref()
                        .or(inputs.selected_account_token.as_ref())
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
        let account_key = selected_account
            .as_ref()
            .map(|acct| format!("token-account:{}", acct.id))
            .or_else(|| {
                provider_config
                    .and_then(ProviderConfig::sanitized_workspace_id)
                    .map(|ws| format!("workspace:{ws}"))
            });

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
