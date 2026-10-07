// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderEnvironmentResolver.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;

use ariadusage_protocol::secret::SecretString;

/// Precedence rule for combining config-stored credentials and ambient environment variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvPrecedence {
    /// Config value takes precedence over ambient environment.
    Config,
    /// Ambient environment takes precedence over config value.
    Environment,
}

/// Rule for projecting a configuration value into the process environment.
#[derive(Clone)]
pub struct EnvProjection<C> {
    pub key: String,
    pub precedence: EnvPrecedence,
    pub value_from_config: fn(&C) -> Option<String>,
}

/// Scrubbing and injection rules applied when a specific token account is selected.
#[derive(Clone, Default)]
pub struct AccountTokenProjection {
    /// Environment keys scrubbed to eliminate ambient credential confusion.
    pub scrub_keys: Vec<String>,
    /// Environment key injected with the account token (if any).
    pub target_key: Option<String>,
}

/// Environment resolution descriptor for a provider.
#[derive(Clone, Default)]
pub struct ProviderEnvDescriptor<C> {
    pub projections: Vec<EnvProjection<C>>,
    pub account_projection: Option<AccountTokenProjection>,
}

/// Pure environment resolver implementing credential precedence and account isolation.
pub struct ProviderEnvironmentResolver;

impl ProviderEnvironmentResolver {
    pub fn resolve<C>(
        base: &BTreeMap<String, SecretString>,
        config: Option<&C>,
        selected_account_token: Option<&SecretString>,
        descriptor: &ProviderEnvDescriptor<C>,
    ) -> BTreeMap<String, SecretString> {
        let mut environment = base.clone();

        if let Some(cfg) = config {
            for proj in &descriptor.projections {
                if let Some(val) = (proj.value_from_config)(cfg) {
                    match proj.precedence {
                        EnvPrecedence::Config => {
                            environment.insert(proj.key.clone(), SecretString::new(val));
                        }
                        EnvPrecedence::Environment => {
                            if !environment.contains_key(&proj.key) {
                                environment.insert(proj.key.clone(), SecretString::new(val));
                            }
                        }
                    }
                }
            }
        }

        if let (Some(token), Some(acct_proj)) =
            (selected_account_token, &descriptor.account_projection)
        {
            for key in &acct_proj.scrub_keys {
                environment.remove(key);
            }
            if let Some(ref target_key) = acct_proj.target_key {
                environment.insert(target_key.clone(), token.clone());
            }
        }

        environment
    }
}
