// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfig.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashSet;

use super::types::{Config, ModelRateLimit, ProviderConfig, ProviderEntry};
use crate::providers;

/// Normalizes configuration:
/// 1. Drops duplicate provider entries (first wins).
/// 2. Re-validates each typed provider's model rate limits, and those of its
///    token accounts (CodexBar validates account limits whenever an account is
///    built; Rust callers can set the field directly).
/// 3. Appends missing first-party providers in enum order (codex, claude, antigravity)
///    with explicit `enabled = default_enabled`.
/// 4. Sets `version = 1`.
pub fn normalize(mut config: Config) -> Config {
    let mut seen = HashSet::new();
    let mut normalized_providers = Vec::with_capacity(config.providers.len().max(3));

    for mut entry in config.providers {
        if seen.insert(entry.id().to_string()) {
            if let Some(provider) = entry.as_typed_mut() {
                revalidate_model_rate_limits(provider);
            }
            normalized_providers.push(entry);
        }
    }

    for descriptor in providers::first_party_order() {
        if !seen.contains(descriptor.id.as_str()) {
            let mut provider = ProviderConfig::new(descriptor.id.clone());
            provider.enabled = Some(descriptor.default_enabled);
            normalized_providers.push(ProviderEntry::Typed(provider));
        }
    }

    config.version = Config::CURRENT_VERSION;
    config.providers = normalized_providers;
    config
}

fn revalidate_model_rate_limits(provider: &mut ProviderConfig) {
    let revalidate = |limits: &mut Option<Vec<ModelRateLimit>>| {
        if let Some(rows) = limits.take() {
            *limits = Some(ModelRateLimit::validated(rows));
        }
    };
    revalidate(&mut provider.model_rate_limits);
    if let Some(accounts) = provider.token_accounts.as_mut() {
        for account in &mut accounts.accounts {
            revalidate(&mut account.model_rate_limits);
        }
    }
}
