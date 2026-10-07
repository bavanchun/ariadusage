// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfig.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Config/ProviderConfigCoding.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/TokenAccounts.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt;

use ariadusage_protocol::ProviderId;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::quota::QuotaWarnings;
use crate::providers::{self, SourceMode};
use crate::settings_value::SettingsValue;

fn raw_maps_equal(
    a: &BTreeMap<String, Box<RawValue>>,
    b: &BTreeMap<String, Box<RawValue>>,
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .all(|(k, v)| b.get(k).is_some_and(|ov| v.get() == ov.get()))
}

/// Cookie acquisition policy for web-authenticated providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CookieSource {
    Auto,
    Manual,
    Off,
}

/// Metadata and non-secret configuration for a stored token account.
#[derive(Clone)]
pub struct TokenAccountMeta {
    pub id: String,
    pub label: String,
    pub added_at: f64,
    pub last_used: Option<f64>,
    pub external_identifier: Option<String>,
    pub usage_scope: Option<String>,
    pub organization_id: Option<String>,
    pub workspace_id: Option<String>,
    pub seat_credit_entitlement: Option<String>,
    pub extra: BTreeMap<String, Box<RawValue>>,
}

impl PartialEq for TokenAccountMeta {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.label == other.label
            && self.added_at.to_bits() == other.added_at.to_bits()
            && self.last_used.map(f64::to_bits) == other.last_used.map(f64::to_bits)
            && self.external_identifier == other.external_identifier
            && self.usage_scope == other.usage_scope
            && self.organization_id == other.organization_id
            && self.workspace_id == other.workspace_id
            && self.seat_credit_entitlement == other.seat_credit_entitlement
            && raw_maps_equal(&self.extra, &other.extra)
    }
}

impl Eq for TokenAccountMeta {}

impl TokenAccountMeta {
    pub fn sanitized_organization_id(&self) -> Option<String> {
        SettingsValue::cleaned(self.organization_id.as_deref())
    }

    pub fn sanitized_usage_scope(&self) -> Option<String> {
        SettingsValue::cleaned(self.usage_scope.as_deref())
    }

    pub fn sanitized_workspace_id(&self) -> Option<String> {
        SettingsValue::cleaned(self.workspace_id.as_deref())
    }

    pub fn sanitized_seat_credit_entitlement(&self) -> Option<String> {
        SettingsValue::cleaned(self.seat_credit_entitlement.as_deref())
    }
}

impl fmt::Debug for TokenAccountMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenAccountMeta")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("added_at", &self.added_at)
            .field("last_used", &self.last_used)
            .field("external_identifier", &self.external_identifier)
            .field("usage_scope", &self.usage_scope)
            .field("organization_id", &self.organization_id)
            .field("workspace_id", &self.workspace_id)
            .field("seat_credit_entitlement", &self.seat_credit_entitlement)
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Token accounts collection metadata.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenAccountsMeta {
    pub version: u32,
    pub active_index: i64,
    pub accounts: Vec<TokenAccountMeta>,
}

impl TokenAccountsMeta {
    pub fn clamped_active_index(&self) -> usize {
        if self.accounts.is_empty() {
            0
        } else {
            let max_idx = (self.accounts.len() - 1) as i64;
            self.active_index.clamp(0, max_idx) as usize
        }
    }
}

impl fmt::Debug for TokenAccountsMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenAccountsMeta")
            .field("version", &self.version)
            .field("active_index", &self.active_index)
            .field("accounts", &self.accounts)
            .finish()
    }
}

/// Typed configuration for a first-party provider.
#[derive(Clone)]
pub struct ProviderConfig {
    pub id: ProviderId,
    pub enabled: Option<bool>,
    pub source: Option<SourceMode>,
    pub extras_enabled: Option<bool>,
    pub cookie_source: Option<CookieSource>,
    pub region: Option<String>,
    pub workspace_id: Option<String>,
    pub enterprise_host: Option<String>,
    pub token_accounts: Option<TokenAccountsMeta>,
    pub quota_warnings: Option<QuotaWarnings>,
    pub accent_color: Option<String>,
    pub hidden_usage_item_ids: Option<Vec<String>>,
    pub extra: BTreeMap<String, Box<RawValue>>,
}

impl PartialEq for ProviderConfig {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.enabled == other.enabled
            && self.source == other.source
            && self.extras_enabled == other.extras_enabled
            && self.cookie_source == other.cookie_source
            && self.region == other.region
            && self.workspace_id == other.workspace_id
            && self.enterprise_host == other.enterprise_host
            && self.token_accounts == other.token_accounts
            && self.quota_warnings == other.quota_warnings
            && self.accent_color == other.accent_color
            && self.hidden_usage_item_ids == other.hidden_usage_item_ids
            && raw_maps_equal(&self.extra, &other.extra)
    }
}

impl Eq for ProviderConfig {}

impl ProviderConfig {
    pub fn new(id: ProviderId) -> Self {
        Self {
            id,
            enabled: None,
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
        }
    }

    pub fn sanitized_region(&self) -> Option<String> {
        SettingsValue::cleaned(self.region.as_deref())
    }

    pub fn sanitized_workspace_id(&self) -> Option<String> {
        SettingsValue::cleaned(self.workspace_id.as_deref())
    }

    pub fn sanitized_enterprise_host(&self) -> Option<String> {
        SettingsValue::cleaned(self.enterprise_host.as_deref())
    }
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("enabled", &self.enabled)
            .field("source", &self.source)
            .field("extras_enabled", &self.extras_enabled)
            .field("cookie_source", &self.cookie_source)
            .field("region", &self.region)
            .field("workspace_id", &self.workspace_id)
            .field("enterprise_host", &self.enterprise_host)
            .field("token_accounts", &self.token_accounts)
            .field("quota_warnings", &self.quota_warnings)
            .field("accent_color", &self.accent_color)
            .field("hidden_usage_item_ids", &self.hidden_usage_item_ids)
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// An entry in the ordered provider list: either typed (for first-party providers)
/// or opaque (for unknown / third-party / legacy providers preserving exact bytes).
#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ProviderEntry {
    Typed(ProviderConfig),
    Opaque {
        id: String,
        enabled: bool,
        raw: Box<RawValue>,
    },
}

impl PartialEq for ProviderEntry {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Typed(a), Self::Typed(b)) => a == b,
            (
                Self::Opaque {
                    id: id1,
                    enabled: e1,
                    raw: r1,
                },
                Self::Opaque {
                    id: id2,
                    enabled: e2,
                    raw: r2,
                },
            ) => id1 == id2 && e1 == e2 && r1.get() == r2.get(),
            _ => false,
        }
    }
}

impl Eq for ProviderEntry {}

impl ProviderEntry {
    pub fn id(&self) -> &str {
        match self {
            Self::Typed(c) => c.id.as_str(),
            Self::Opaque { id, .. } => id.as_str(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Typed(c) => c
                .enabled
                .unwrap_or_else(|| providers::find_by_id(&c.id).is_some_and(|d| d.default_enabled)),
            Self::Opaque { enabled, .. } => *enabled,
        }
    }

    pub fn as_typed(&self) -> Option<&ProviderConfig> {
        match self {
            Self::Typed(c) => Some(c),
            Self::Opaque { .. } => None,
        }
    }

    pub fn as_typed_mut(&mut self) -> Option<&mut ProviderConfig> {
        match self {
            Self::Typed(c) => Some(c),
            Self::Opaque { .. } => None,
        }
    }
}

impl fmt::Debug for ProviderEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Typed(c) => c.fmt(f),
            Self::Opaque { id, enabled, .. } => f
                .debug_struct("ProviderEntry::Opaque")
                .field("id", id)
                .field("enabled", enabled)
                .field("raw", &"[opaque]")
                .finish(),
        }
    }
}

/// Top-level configuration document.
#[derive(Clone)]
pub struct Config {
    pub version: u32,
    pub providers: Vec<ProviderEntry>,
    pub hooks: Option<Box<RawValue>>,
    pub settings: Option<Box<RawValue>>,
    pub secret_file_fallback: Option<bool>,
    pub extra_top: BTreeMap<String, Box<RawValue>>,
}

impl PartialEq for Config {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.providers == other.providers
            && self.hooks.as_deref().map(RawValue::get) == other.hooks.as_deref().map(RawValue::get)
            && self.settings.as_deref().map(RawValue::get)
                == other.settings.as_deref().map(RawValue::get)
            && self.secret_file_fallback == other.secret_file_fallback
            && raw_maps_equal(&self.extra_top, &other.extra_top)
    }
}

impl Eq for Config {}

impl Config {
    pub const CURRENT_VERSION: u32 = 1;

    pub fn new(version: u32, providers: Vec<ProviderEntry>) -> Self {
        Self {
            version,
            providers,
            hooks: None,
            settings: None,
            secret_file_fallback: None,
            extra_top: BTreeMap::new(),
        }
    }

    pub fn ordered_providers(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.id().to_string()).collect()
    }

    pub fn enabled_providers(&self) -> Vec<String> {
        self.providers
            .iter()
            .filter_map(|p| {
                if p.is_enabled() {
                    Some(p.id().to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn provider_entry(&self, id: &str) -> Option<&ProviderEntry> {
        self.providers.iter().find(|p| p.id() == id)
    }

    pub fn provider_entry_mut(&mut self, id: &str) -> Option<&mut ProviderEntry> {
        self.providers.iter_mut().find(|p| p.id() == id)
    }

    pub fn provider_config(&self, id: &str) -> Option<&ProviderConfig> {
        self.provider_entry(id).and_then(ProviderEntry::as_typed)
    }

    pub fn provider_config_mut(&mut self, id: &str) -> Option<&mut ProviderConfig> {
        self.provider_entry_mut(id)
            .and_then(ProviderEntry::as_typed_mut)
    }

    pub fn remove_provider(&mut self, id: &str) {
        self.providers.retain(|p| p.id() != id);
    }

    pub fn set_provider_config(&mut self, config: ProviderConfig) {
        if let Some(existing) = self
            .providers
            .iter_mut()
            .find(|p| p.id() == config.id.as_str())
        {
            *existing = ProviderEntry::Typed(config);
        } else {
            self.providers.push(ProviderEntry::Typed(config));
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("version", &self.version)
            .field("providers", &self.providers)
            .field("has_hooks", &self.hooks.is_some())
            .field("has_settings", &self.settings.is_some())
            .field("secret_file_fallback", &self.secret_file_fallback)
            .field("extra_top_keys", &self.extra_top.keys().collect::<Vec<_>>())
            .finish()
    }
}
