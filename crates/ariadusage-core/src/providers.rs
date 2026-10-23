// Ported from CodexBar Sources/CodexBarCore/Providers/Providers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::LazyLock;

use ariadusage_protocol::{ProviderId, ProviderPresentation};
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
    /// Compact name; equals `display_name` unless the provider sets one.
    pub short_display_name: &'static str,
    /// Title of the primary usage window.
    pub session_label: &'static str,
    /// Title of the secondary usage window.
    pub weekly_label: &'static str,
    /// Title of the tertiary usage window, when the provider has one.
    pub opus_label: Option<&'static str>,
    /// Branding color as 8-bit red, green and blue.
    pub accent_rgb: (u8, u8, u8),
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

impl ProviderDescriptor {
    /// Presentation block that snapshots carry for this provider.
    pub fn presentation(&self) -> ProviderPresentation {
        let (red, green, blue) = self.accent_rgb;
        let accent_color = ProviderPresentation::accent_color_from_rgb(red, green, blue);
        debug_assert!(ProviderPresentation::validate_accent_color(&accent_color).is_ok());
        ProviderPresentation {
            short_display_name: self.short_display_name.to_string(),
            session_label: self.session_label.to_string(),
            weekly_label: self.weekly_label.to_string(),
            tertiary_label: self.opus_label.map(str::to_string),
            accent_color: Some(accent_color),
        }
    }
}

static FIRST_PARTY_DESCRIPTORS: LazyLock<[ProviderDescriptor; 3]> = LazyLock::new(|| {
    [
        // Codex (CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift:31, 34-37, 43, 68, 86-93, 138)
        ProviderDescriptor {
            id: ProviderId::new("codex").expect("valid id"),
            display_name: "Codex",
            short_display_name: "Codex",
            session_label: "Session",
            weekly_label: "Weekly",
            opus_label: None,
            accent_rgb: (73, 163, 176),
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
        // Claude (CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift:126-129, 135, 158, 233, 239-245)
        ProviderDescriptor {
            id: ProviderId::new("claude").expect("valid id"),
            display_name: "Claude",
            short_display_name: "Claude",
            session_label: "Session",
            weekly_label: "Weekly",
            opus_label: Some("Sonnet"),
            accent_rgb: (204, 124, 94),
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
            pace_capability: ProviderPaceCapability::calendar_month_reset_window(),
        },
        // Antigravity (CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderDescriptor.swift:26-30, 36, 52, 71, 102)
        ProviderDescriptor {
            id: ProviderId::new("antigravity").expect("valid id"),
            display_name: "Antigravity",
            short_display_name: "Anti",
            session_label: "Gemini Models",
            weekly_label: "Claude and GPT",
            opus_label: None,
            accent_rgb: (96, 186, 126),
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
