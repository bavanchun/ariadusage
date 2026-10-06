// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfig.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashSet};

use super::types::{Config, ProviderConfig, ProviderEntry};
use crate::providers;

/// Normalizes configuration:
/// 1. Drops duplicate provider entries (first wins).
/// 2. Appends missing first-party providers in enum order (codex, claude, antigravity)
///    with explicit `enabled = default_enabled`.
/// 3. Sets `version = 1`.
pub fn normalize(mut config: Config) -> Config {
    let mut seen = HashSet::new();
    let mut normalized_providers = Vec::with_capacity(config.providers.len().max(3));

    for entry in config.providers {
        if seen.insert(entry.id().to_string()) {
            normalized_providers.push(entry);
        }
    }

    for descriptor in providers::first_party_order() {
        if !seen.contains(descriptor.id.as_str()) {
            normalized_providers.push(ProviderEntry::Typed(ProviderConfig {
                id: descriptor.id.clone(),
                enabled: Some(descriptor.default_enabled),
                source: None,
                extras_enabled: None,
                cookie_source: None,
                region: None,
                workspace_id: None,
                enterprise_host: None,
                token_accounts: None,
                quota_warnings: None,
                accent_color: None,
                hidden_usage_item_ids: None,
                extra: BTreeMap::new(),
            }));
        }
    }

    config.version = Config::CURRENT_VERSION;
    config.providers = normalized_providers;
    config
}
