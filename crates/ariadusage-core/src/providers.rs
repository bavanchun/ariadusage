// Ported from CodexBar Sources/CodexBarCore/Providers/Providers.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::LazyLock;

use ariadusage_protocol::ProviderId;
use serde::{Deserialize, Serialize};

use crate::pace::{ProviderPaceCapability, ProviderPaceDurationRule, ProviderPaceWindowRule};

/// Mode through which usage metrics are acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceMode {
    Auto,
    Web,
    Cli,
    Oauth,
    Api,
}

/// Static descriptor holding metadata and capabilities for a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub display_name: &'static str,
    pub default_enabled: bool,
    pub source_modes: &'static [SourceMode],
    pub supports_api_key_override: bool,
    pub requires_api_key_for_api_source: bool,
    pub uses_region: bool,
    pub uses_secret_key: bool,
    pub supports_workspace: bool,
    pub supports_enterprise_host: bool,
    pub token_account_support: bool,
    pub pace_capability: ProviderPaceCapability,
}

static FIRST_PARTY_DESCRIPTORS: LazyLock<[ProviderDescriptor; 3]> = LazyLock::new(|| {
    [
        // Codex (CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift:43, 138, 31)
        ProviderDescriptor {
            id: ProviderId::new("codex").expect("valid id"),
            display_name: "Codex",
            default_enabled: true,
            source_modes: &[
                SourceMode::Auto,
                SourceMode::Web,
                SourceMode::Cli,
                SourceMode::Oauth,
                SourceMode::Api,
            ],
            supports_api_key_override: false,
            requires_api_key_for_api_source: false,
            uses_region: false,
            uses_secret_key: false,
            supports_workspace: false,
            supports_enterprise_host: false,
            token_account_support: false,
            pace_capability: ProviderPaceCapability {
                reset_window_pace: ProviderPaceWindowRule::Unsupported,
                inferred_monthly_duration: ProviderPaceDurationRule::Unsupported,
                shows_headroom_hint: true,
            },
        },
        // Claude (CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift:134, 234)
        ProviderDescriptor {
            id: ProviderId::new("claude").expect("valid id"),
            display_name: "Claude",
            default_enabled: false,
            source_modes: &[
                SourceMode::Auto,
                SourceMode::Api,
                SourceMode::Web,
                SourceMode::Cli,
                SourceMode::Oauth,
            ],
            supports_api_key_override: true,
            requires_api_key_for_api_source: true,
            uses_region: false,
            uses_secret_key: false,
            supports_workspace: false,
            supports_enterprise_host: false,
            token_account_support: true,
            pace_capability: ProviderPaceCapability::unsupported(),
        },
        // Antigravity (CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderDescriptor.swift:36, 102)
        ProviderDescriptor {
            id: ProviderId::new("antigravity").expect("valid id"),
            display_name: "Antigravity",
            default_enabled: false,
            source_modes: &[SourceMode::Auto, SourceMode::Cli, SourceMode::Oauth],
            supports_api_key_override: false,
            requires_api_key_for_api_source: true,
            uses_region: false,
            uses_secret_key: false,
            supports_workspace: false,
            supports_enterprise_host: false,
            token_account_support: true,
            pace_capability: ProviderPaceCapability::unsupported(),
        },
    ]
});

/// Returns all first-party provider descriptors in CodexBar enum order.
pub fn first_party_order() -> &'static [ProviderDescriptor] {
    &FIRST_PARTY_DESCRIPTORS[..]
}

/// Looks up a provider descriptor by provider ID.
pub fn find_by_id(id: &ProviderId) -> Option<&'static ProviderDescriptor> {
    first_party_order().iter().find(|d| d.id == *id)
}

/// Looks up a provider descriptor by provider ID string slice.
pub fn find_by_id_str(id: &str) -> Option<&'static ProviderDescriptor> {
    first_party_order().iter().find(|d| d.id.as_str() == id)
}
