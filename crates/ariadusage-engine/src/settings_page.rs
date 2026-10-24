// Ported from CodexBar Sources/CodexBarCore/Config/PreferencesDocument.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SettingsStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;

use ariadusage_core::config::{
    Config, CookieSource, ProviderConfig, SecretPresence, set_provider_enabled,
};
use ariadusage_core::providers::{ProviderDescriptor, SourceMode};
use ariadusage_core::refresh::RefreshFrequency;
use ariadusage_core::settings_value::SettingsValue;
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::ids::{ActionId, SettingId};
use ariadusage_protocol::settings::{
    ActionItem, ActionStyle, ChoiceOption, DescriptorKind, MultiChoiceEntry, NumberConfig,
    SettingCondition, SettingDescriptor, SettingsPage, SettingsScope, SettingsSection,
    TokenAccountRow, TokenAccountsConfig,
};
use serde_json::Value;
use serde_json::value::RawValue;
use thiserror::Error;

use crate::registry::ProviderRegistry;

/// Errors returned when reading or mutating a setting through the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SettingsMutationError {
    #[error("unknown provider '{0}'")]
    UnknownProvider(String),
    #[error("unknown setting '{0}'")]
    UnknownSetting(String),
    #[error("invalid value for setting '{id}': {reason}")]
    InvalidValue { id: String, reason: String },
    #[error("secret setting '{0}' must be set via `ariadusage secret set`")]
    SecretNotWritableOverIpc(String),
}

/// Extracted app-level preferences stored in `Config.settings`.
#[derive(Debug, Clone, PartialEq)]
pub struct AppPreferences {
    pub refresh_frequency: RefreshFrequency,
    pub refresh_interval_seconds: f64,
    pub status_checks_enabled: bool,
    pub show_optional_credits_and_extra_usage: bool,
    pub cost_usage_enabled: bool,
    pub hide_personal_info: bool,
    pub notifications_enabled: bool,
    pub source_mode: String,
    pub menu_bar_icon_style: String,
    pub menu_bar_show_used: bool,
    pub menu_bar_merge_icons: bool,
    pub accent_color: String,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            refresh_frequency: RefreshFrequency::FiveMinutes,
            refresh_interval_seconds: 300.0,
            status_checks_enabled: true,
            show_optional_credits_and_extra_usage: true,
            cost_usage_enabled: false,
            hide_personal_info: false,
            notifications_enabled: true,
            source_mode: "auto".to_string(),
            menu_bar_icon_style: "bars".to_string(),
            menu_bar_show_used: false,
            menu_bar_merge_icons: false,
            accent_color: "#CACCCC".to_string(),
        }
    }
}

impl AppPreferences {
    /// Extracts app-level preferences from `config.settings` without losing unknown fields.
    pub fn from_config(config: &Config) -> Self {
        let mut prefs = Self::default();
        let Some(raw) = &config.settings else {
            return prefs;
        };
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw.get()) else {
            return prefs;
        };

        if let Some(freq_str) = map
            .get("refreshFrequency")
            .or_else(|| map.get("cadence"))
            .and_then(Value::as_str)
            && let Some(freq) = RefreshFrequency::parse(freq_str)
        {
            prefs.refresh_frequency = freq;
            prefs.refresh_interval_seconds = freq.duration().map_or(300.0, |d| d.as_secs_f64());
        }
        if let Some(interval) = map.get("refreshInterval").and_then(Value::as_f64)
            && interval.is_finite()
            && interval >= 0.0
        {
            prefs.refresh_interval_seconds = interval;
            if !map.contains_key("refreshFrequency") && !map.contains_key("cadence") {
                prefs.refresh_frequency = frequency_from_interval_seconds(interval);
            }
        }
        if let Some(enabled) = map.get("statusChecksEnabled").and_then(Value::as_bool) {
            prefs.status_checks_enabled = enabled;
        }
        if let Some(show) = map
            .get("showOptionalCreditsAndExtraUsage")
            .or_else(|| map.get("menu.showOptionalCreditsAndExtraUsage"))
            .and_then(Value::as_bool)
        {
            prefs.show_optional_credits_and_extra_usage = show;
        }
        if let Some(cost) = map.get("costUsageEnabled").and_then(Value::as_bool) {
            prefs.cost_usage_enabled = cost;
        }
        if let Some(hide) = map.get("hidePersonalInfo").and_then(Value::as_bool) {
            prefs.hide_personal_info = hide;
        }
        if let Some(notif) = map
            .get("notificationsEnabled")
            .or_else(|| map.get("notifications.enabled"))
            .and_then(Value::as_bool)
        {
            prefs.notifications_enabled = notif;
        }
        if let Some(mode) = map.get("sourceMode").and_then(Value::as_str)
            && !mode.trim().is_empty()
        {
            prefs.source_mode = mode.trim().to_string();
        }
        if let Some(style) = map
            .get("menuBarIconStyle")
            .or_else(|| map.get("menuBar.iconStyle"))
            .and_then(Value::as_str)
            && !style.trim().is_empty()
        {
            prefs.menu_bar_icon_style = style.trim().to_string();
        }
        if let Some(show_used) = map
            .get("menuBarShowUsed")
            .or_else(|| map.get("menuBar.showUsed"))
            .and_then(Value::as_bool)
        {
            prefs.menu_bar_show_used = show_used;
        }
        if let Some(merge) = map
            .get("menuBarMergeIcons")
            .or_else(|| map.get("menuBar.mergeIcons"))
            .and_then(Value::as_bool)
        {
            prefs.menu_bar_merge_icons = merge;
        }
        if let Some(color) = map.get("accentColor").and_then(Value::as_str)
            && !color.trim().is_empty()
        {
            prefs.accent_color = color.trim().to_string();
        }

        prefs
    }
}

fn frequency_from_interval_seconds(seconds: f64) -> RefreshFrequency {
    if seconds <= 0.0 {
        RefreshFrequency::Manual
    } else if seconds <= 60.0 {
        RefreshFrequency::OneMinute
    } else if seconds <= 120.0 {
        RefreshFrequency::TwoMinutes
    } else if seconds <= 300.0 {
        RefreshFrequency::FiveMinutes
    } else if seconds <= 900.0 {
        RefreshFrequency::FifteenMinutes
    } else {
        RefreshFrequency::ThirtyMinutes
    }
}

fn update_settings_json_field(config: &mut Config, key: &str, value: Value) {
    let mut map: BTreeMap<String, Box<RawValue>> = config
        .settings
        .as_ref()
        .and_then(|raw| serde_json::from_str(raw.get()).ok())
        .unwrap_or_default();

    if let Ok(serialized_val) = serde_json::to_string(&value)
        && let Ok(raw_val) = RawValue::from_string(serialized_val)
    {
        map.insert(key.to_string(), raw_val);
    }

    if let Ok(serialized_map) = serde_json::to_string(&map)
        && let Ok(raw_map) = RawValue::from_string(serialized_map)
    {
        config.settings = Some(raw_map);
    }
}

/// Builds a validated `SettingsPage` for `scope` from `config` and `presence`.
///
/// Contains only real configuration values and secret presence flags (`is_set`);
/// never emits invented accounts, CLI versions, or placeholder credentials.
pub fn build_settings_page(
    registry: &ProviderRegistry,
    config: &Config,
    presence: &SecretPresence,
    scope: &SettingsScope,
) -> Result<SettingsPage, SettingsMutationError> {
    match scope {
        SettingsScope::App => Ok(build_app_settings_page(registry, config)),
        SettingsScope::Provider(id) => {
            let descriptor = registry
                .descriptor(id)
                .ok_or_else(|| SettingsMutationError::UnknownProvider(id.as_str().to_string()))?;
            Ok(build_provider_settings_page(descriptor, config, presence))
        }
    }
}

fn build_app_settings_page(registry: &ProviderRegistry, config: &Config) -> SettingsPage {
    let prefs = AppPreferences::from_config(config);
    let ordered = registry.ordered_descriptors(config);

    let cadence_options = vec![
        ChoiceOption::new("manual", "Manual"),
        ChoiceOption::new("1m", "Every 1 minute"),
        ChoiceOption::new("2m", "Every 2 minutes"),
        ChoiceOption::new("5m", "Every 5 minutes"),
        ChoiceOption::new("15m", "Every 15 minutes"),
        ChoiceOption::new("30m", "Every 30 minutes"),
        ChoiceOption::new("adaptive", "Adaptive"),
        ChoiceOption::new("adaptiveAgentAware", "Adaptive (agent-aware)"),
    ];

    let general_descriptors = vec![
        SettingDescriptor::new(
            SettingId::new("app.refresh.cadence").expect("valid setting id"),
            "Refresh Cadence",
            DescriptorKind::Choice {
                selected: prefs.refresh_frequency.as_str().to_string(),
                options: cadence_options,
            },
        )
        .with_help("Background usage polling frequency."),
        SettingDescriptor::new(
            SettingId::new("refreshInterval").expect("valid setting id"),
            "Refresh Interval",
            DescriptorKind::Number {
                value: prefs.refresh_interval_seconds,
                config: NumberConfig {
                    min: Some(0.0),
                    max: Some(1800.0),
                    step: Some(60.0),
                    unit: Some("s".to_string()),
                },
            },
        )
        .with_help("Background polling cadence in seconds."),
        SettingDescriptor::new(
            SettingId::new("app.statusChecksEnabled").expect("valid setting id"),
            "Check Provider Status",
            DescriptorKind::Toggle {
                value: prefs.status_checks_enabled,
            },
        )
        .with_help("Poll Statuspage and Google Workspace status feeds."),
        SettingDescriptor::new(
            SettingId::new("sourceMode").expect("valid setting id"),
            "Source Mode",
            DescriptorKind::Choice {
                selected: prefs.source_mode,
                options: vec![
                    ChoiceOption::new("auto", "Auto"),
                    ChoiceOption::new("cli", "CLI Tool"),
                    ChoiceOption::new("web", "Web Session"),
                    ChoiceOption::new("oauth", "OAuth"),
                    ChoiceOption::new("api", "API Token"),
                ],
            },
        ),
    ];

    let menu_bar_descriptors = vec![
        SettingDescriptor::new(
            SettingId::new("menuBar.iconStyle").expect("valid setting id"),
            "Status Icon Style",
            DescriptorKind::Choice {
                selected: prefs.menu_bar_icon_style,
                options: vec![
                    ChoiceOption::new("bars", "Bars"),
                    ChoiceOption::new("critters", "Critters"),
                ],
            },
        ),
        SettingDescriptor::new(
            SettingId::new("menuBar.showUsed").expect("valid setting id"),
            "Show Used Quota Instead of Remaining",
            DescriptorKind::Toggle {
                value: prefs.menu_bar_show_used,
            },
        ),
        SettingDescriptor::new(
            SettingId::new("menuBar.mergeIcons").expect("valid setting id"),
            "Merge Provider Status Items",
            DescriptorKind::Toggle {
                value: prefs.menu_bar_merge_icons,
            },
        ),
        SettingDescriptor::new(
            SettingId::new("accentColor").expect("valid setting id"),
            "Custom Accent Color",
            DescriptorKind::Color {
                value: prefs.accent_color,
                default: "#CACCCC".to_string(),
            },
        ),
    ];

    let menu_descriptors = vec![
        SettingDescriptor::new(
            SettingId::new("menu.showOptionalCreditsAndExtraUsage").expect("valid setting id"),
            "Show Credits and Extra Usage",
            DescriptorKind::Toggle {
                value: prefs.show_optional_credits_and_extra_usage,
            },
        ),
        SettingDescriptor::new(
            SettingId::new("notifications.enabled").expect("valid setting id"),
            "Desktop Notifications",
            DescriptorKind::Toggle {
                value: prefs.notifications_enabled,
            },
        ),
    ];

    let active_entries: Vec<MultiChoiceEntry> = ordered
        .iter()
        .map(|descriptor| {
            let enabled = registry.is_provider_enabled(config, &descriptor.id);
            MultiChoiceEntry::new(
                descriptor.id.as_str(),
                descriptor.display_name,
                false,
                enabled,
            )
        })
        .collect();

    let mut provider_descriptors = vec![SettingDescriptor::new(
        SettingId::new("providers.active").expect("valid setting id"),
        "Active Providers",
        DescriptorKind::MultiChoice {
            entries: active_entries,
        },
    )];

    for descriptor in ordered {
        let enabled = registry.is_provider_enabled(config, &descriptor.id);
        provider_descriptors.push(
            SettingDescriptor::new(
                SettingId::new(format!("providers.{}.enabled", descriptor.id.as_str()))
                    .expect("valid setting id"),
                descriptor.display_name,
                DescriptorKind::Toggle { value: enabled },
            )
            .with_help(descriptor.toggle_title),
        );
    }

    let advanced_descriptors = vec![
        SettingDescriptor::new(
            SettingId::new("app.shareUsageWithCli").expect("valid setting id"),
            "Share Usage with CLI",
            DescriptorKind::Toggle {
                value: config.share_usage_with_cli,
            },
        ),
        SettingDescriptor::new(
            SettingId::new("app.secretFileFallback").expect("valid setting id"),
            "Encrypted Secret File Fallback",
            DescriptorKind::Toggle {
                value: config.secret_file_fallback.unwrap_or(false),
            },
        ),
        SettingDescriptor::new(
            SettingId::new("diagnostics").expect("valid setting id"),
            "Maintenance Actions",
            DescriptorKind::Actions {
                actions: vec![ActionItem {
                    id: ActionId::new("refreshAll").expect("valid action id"),
                    label: "Refresh All Providers".to_string(),
                    style: ActionStyle::Button,
                    confirmation: None,
                }],
            },
        ),
    ];

    SettingsPage::new(
        SettingsScope::App,
        vec![
            SettingsSection::new("general", "General Settings", general_descriptors)
                .with_pane("general"),
            SettingsSection::new("menuBar", "Menu Bar", menu_bar_descriptors).with_pane("menuBar"),
            SettingsSection::new("menu", "Menu Content", menu_descriptors).with_pane("menu"),
            SettingsSection::new("providers", "Providers", provider_descriptors)
                .with_pane("providers"),
            SettingsSection::new("advanced", "Advanced", advanced_descriptors)
                .with_pane("advanced"),
        ],
    )
}

fn build_provider_settings_page(
    descriptor: &ProviderDescriptor,
    config: &Config,
    presence: &SecretPresence,
) -> SettingsPage {
    let id_str = descriptor.id.as_str();
    let provider_config = config.provider_config(id_str);
    let enabled = config
        .provider_entry(id_str)
        .map_or(descriptor.default_enabled, |e| e.is_enabled());
    let current_source = provider_config
        .and_then(|c| c.source)
        .filter(|m| descriptor.source_modes.contains(m))
        .unwrap_or(SourceMode::Auto);

    let enabled_id = SettingId::new(format!("providers.{id_str}.enabled")).expect("valid id");
    let source_mode_id =
        SettingId::new(format!("providers.{id_str}.sourceMode")).expect("valid id");

    let source_options: Vec<ChoiceOption> = descriptor
        .source_modes
        .iter()
        .map(|mode| ChoiceOption::new(source_mode_str(*mode), source_mode_label(*mode)))
        .collect();

    let mut overview_descriptors = vec![
        SettingDescriptor::new(
            enabled_id.clone(),
            format!("Enable {}", descriptor.display_name),
            DescriptorKind::Toggle { value: enabled },
        )
        .with_help(descriptor.toggle_title),
        SettingDescriptor::new(
            source_mode_id,
            "Usage Source",
            DescriptorKind::Choice {
                selected: source_mode_str(current_source).to_string(),
                options: source_options,
            },
        )
        .with_enabled_when(SettingCondition::equals(enabled_id, true)),
    ];

    if id_str == "claude" {
        let extras_enabled = provider_config
            .and_then(|c| c.extras_enabled)
            .unwrap_or(false);
        overview_descriptors.push(
            SettingDescriptor::new(
                SettingId::new("providers.claude.extrasEnabled").expect("valid id"),
                "Fetch Web Extras",
                DescriptorKind::Toggle {
                    value: extras_enabled,
                },
            )
            .with_help("Augment usage with web session metadata when available."),
        );
    }

    let mut sections = vec![
        SettingsSection::new(
            "overview",
            format!("{} Overview", descriptor.display_name),
            overview_descriptors,
        )
        .with_pane("providers"),
    ];

    let mut credential_descriptors = Vec::new();
    if descriptor.supports_api_key_override {
        credential_descriptors.push(
            SettingDescriptor::new(
                SettingId::new(format!("providers.{id_str}.apiKey")).expect("valid id"),
                "API Key",
                DescriptorKind::Secret {
                    is_set: presence.api_keys.contains(&descriptor.id),
                    source: None,
                },
            )
            .with_help("Configured via `ariadusage secret set`; secret values never cross IPC."),
        );
    }

    if descriptor.source_modes.contains(&SourceMode::Web) {
        let cookie_source = provider_config
            .and_then(|c| c.cookie_source)
            .unwrap_or(CookieSource::Auto);
        credential_descriptors.push(SettingDescriptor::new(
            SettingId::new(format!("providers.{id_str}.cookieSource")).expect("valid id"),
            "Cookie Source",
            DescriptorKind::Choice {
                selected: cookie_source_str(cookie_source).to_string(),
                options: vec![
                    ChoiceOption::new("auto", "Automatic"),
                    ChoiceOption::new("manual", "Manual"),
                    ChoiceOption::new("off", "Off"),
                ],
            },
        ));
        credential_descriptors.push(
            SettingDescriptor::new(
                SettingId::new(format!("providers.{id_str}.cookieHeader")).expect("valid id"),
                "Cookie Header",
                DescriptorKind::Secret {
                    is_set: presence.cookie_headers.contains(&descriptor.id),
                    source: None,
                },
            )
            .with_help("Configured via `ariadusage secret set`; secret values never cross IPC."),
        );
    }

    if descriptor.token_account_support {
        let accounts: Vec<TokenAccountRow> = provider_config
            .and_then(|c| c.token_accounts.as_ref())
            .map(|data| {
                let active_idx = data.clamped_active_index();
                data.accounts
                    .iter()
                    .enumerate()
                    .map(|(idx, acct)| TokenAccountRow {
                        id: acct.id.clone(),
                        label: acct.label.clone(),
                        active: idx == active_idx,
                        token_is_set: presence
                            .token_account_tokens
                            .contains(&(descriptor.id.clone(), acct.id.clone())),
                    })
                    .collect()
            })
            .unwrap_or_default();

        credential_descriptors.push(SettingDescriptor::new(
            SettingId::new(format!("accounts.{id_str}")).expect("valid id"),
            format!("{} Accounts", descriptor.display_name),
            DescriptorKind::TokenAccounts {
                config: TokenAccountsConfig {
                    accounts,
                    supports_add: true,
                    supports_remove: true,
                    supports_activate: true,
                },
            },
        ));
    }

    if !credential_descriptors.is_empty() {
        sections.push(
            SettingsSection::new(
                "credentials",
                "Credentials & Accounts",
                credential_descriptors,
            )
            .with_pane("providers"),
        );
    }

    SettingsPage::new(SettingsScope::Provider(descriptor.id.clone()), sections)
}

/// Applies a validated `SetSetting` mutation (`ClientMessage::SetSetting`) to `config` and returns
/// the affected `SettingsScope` and optional `ProviderId`.
pub fn apply_setting_mutation(
    registry: &ProviderRegistry,
    config: &mut Config,
    id: &SettingId,
    value: &Value,
) -> Result<(SettingsScope, Option<ProviderId>), SettingsMutationError> {
    let raw_id = id.as_str();

    if raw_id.ends_with(".apiKey")
        || raw_id.ends_with(".cookieHeader")
        || raw_id.ends_with(".token")
    {
        return Err(SettingsMutationError::SecretNotWritableOverIpc(
            raw_id.to_string(),
        ));
    }

    match raw_id {
        "app.refresh.cadence" | "refreshCadence" => {
            let Some(choice) = value.as_str() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a string choice value".to_string(),
                });
            };
            let Some(freq) = RefreshFrequency::parse(choice) else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: format!("unsupported refresh cadence '{choice}'"),
                });
            };
            update_settings_json_field(
                config,
                "refreshFrequency",
                Value::String(freq.as_str().to_string()),
            );
            if let Some(interval) = freq.duration() {
                update_settings_json_field(
                    config,
                    "refreshInterval",
                    Value::from(interval.as_secs_f64()),
                );
            } else if freq == RefreshFrequency::Manual {
                update_settings_json_field(config, "refreshInterval", Value::from(0.0));
            }
            return Ok((SettingsScope::App, None));
        }
        "refreshInterval" => {
            let Some(seconds) = value.as_f64() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a numeric interval in seconds".to_string(),
                });
            };
            if !seconds.is_finite() || seconds < 0.0 {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "refreshInterval must be a non-negative number".to_string(),
                });
            }
            let freq = frequency_from_interval_seconds(seconds);
            update_settings_json_field(config, "refreshInterval", Value::from(seconds));
            update_settings_json_field(
                config,
                "refreshFrequency",
                Value::String(freq.as_str().to_string()),
            );
            return Ok((SettingsScope::App, None));
        }
        "app.statusChecksEnabled" | "statusChecksEnabled" => {
            let Some(enabled) = value.as_bool() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a boolean value".to_string(),
                });
            };
            update_settings_json_field(config, "statusChecksEnabled", Value::Bool(enabled));
            return Ok((SettingsScope::App, None));
        }
        "menu.showOptionalCreditsAndExtraUsage" | "showOptionalCreditsAndExtraUsage" => {
            let Some(enabled) = value.as_bool() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a boolean value".to_string(),
                });
            };
            update_settings_json_field(
                config,
                "showOptionalCreditsAndExtraUsage",
                Value::Bool(enabled),
            );
            return Ok((SettingsScope::App, None));
        }
        "notifications.enabled" | "notificationsEnabled" => {
            let Some(enabled) = value.as_bool() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a boolean value".to_string(),
                });
            };
            update_settings_json_field(config, "notificationsEnabled", Value::Bool(enabled));
            return Ok((SettingsScope::App, None));
        }
        "sourceMode" => {
            let Some(mode_str) = value.as_str() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a string choice value".to_string(),
                });
            };
            if parse_source_mode(mode_str).is_none() {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: format!("unknown source mode '{mode_str}'"),
                });
            }
            update_settings_json_field(
                config,
                "sourceMode",
                Value::String(mode_str.trim().to_string()),
            );
            return Ok((SettingsScope::App, None));
        }
        "menuBar.iconStyle" => {
            let Some(style) = value.as_str() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a string choice value".to_string(),
                });
            };
            update_settings_json_field(
                config,
                "menuBarIconStyle",
                Value::String(style.trim().to_string()),
            );
            return Ok((SettingsScope::App, None));
        }
        "menuBar.showUsed" => {
            let Some(show_used) = value.as_bool() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a boolean value".to_string(),
                });
            };
            update_settings_json_field(config, "menuBarShowUsed", Value::Bool(show_used));
            return Ok((SettingsScope::App, None));
        }
        "menuBar.mergeIcons" => {
            let Some(merge) = value.as_bool() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a boolean value".to_string(),
                });
            };
            update_settings_json_field(config, "menuBarMergeIcons", Value::Bool(merge));
            return Ok((SettingsScope::App, None));
        }
        "accentColor" => {
            let Some(color) = value.as_str() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a color string".to_string(),
                });
            };
            update_settings_json_field(
                config,
                "accentColor",
                Value::String(color.trim().to_string()),
            );
            return Ok((SettingsScope::App, None));
        }
        "app.shareUsageWithCli" => {
            let Some(enabled) = value.as_bool() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a boolean value".to_string(),
                });
            };
            config.share_usage_with_cli = enabled;
            return Ok((SettingsScope::App, None));
        }
        "app.secretFileFallback" => {
            let Some(enabled) = value.as_bool() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a boolean value".to_string(),
                });
            };
            config.secret_file_fallback = Some(enabled);
            return Ok((SettingsScope::App, None));
        }
        "providers.active" => {
            let Some(arr) = value.as_array() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected an array of enabled provider IDs".to_string(),
                });
            };
            let mut active_ids = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(pid_str) = item.as_str() else {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: "provider ID entries must be strings".to_string(),
                    });
                };
                if registry.descriptor_by_str(pid_str).is_none() {
                    return Err(SettingsMutationError::UnknownProvider(pid_str.to_string()));
                }
                active_ids.push(pid_str.to_string());
            }
            for descriptor in registry.descriptors() {
                let enabled = active_ids.iter().any(|s| s == descriptor.id.as_str());
                set_provider_enabled(config, descriptor.id.as_str(), enabled);
            }
            return Ok((SettingsScope::App, None));
        }
        _ => {}
    }

    if let Some(rest) = raw_id.strip_prefix("providers.") {
        let Some((provider_str, field)) = rest.split_once('.') else {
            return Err(SettingsMutationError::UnknownSetting(raw_id.to_string()));
        };
        let descriptor = registry
            .descriptor_by_str(provider_str)
            .ok_or_else(|| SettingsMutationError::UnknownProvider(provider_str.to_string()))?;
        let provider_id = descriptor.id.clone();

        if field == "enabled" {
            let Some(enabled) = value.as_bool() else {
                return Err(SettingsMutationError::InvalidValue {
                    id: raw_id.to_string(),
                    reason: "expected a boolean value".to_string(),
                });
            };
            set_provider_enabled(config, provider_str, enabled);
            return Ok((
                SettingsScope::Provider(provider_id.clone()),
                Some(provider_id),
            ));
        }

        ensure_typed_provider_entry(config, &provider_id, descriptor.default_enabled);
        let provider_cfg = config
            .provider_config_mut(provider_str)
            .ok_or_else(|| SettingsMutationError::UnknownProvider(provider_str.to_string()))?;

        match field {
            "source" | "sourceMode" => {
                let Some(choice) = value.as_str() else {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: "expected a string choice value".to_string(),
                    });
                };
                let Some(mode) = parse_source_mode(choice) else {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: format!("unknown source mode '{choice}'"),
                    });
                };
                if !descriptor.source_modes.contains(&mode) {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: format!(
                            "source mode '{choice}' is not supported by {}",
                            descriptor.display_name
                        ),
                    });
                }
                provider_cfg.source = Some(mode);
            }
            "cookieSource" => {
                let Some(choice) = value.as_str() else {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: "expected a string choice value".to_string(),
                    });
                };
                let Some(cookie_source) = parse_cookie_source(choice) else {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: format!("unknown cookie source '{choice}'"),
                    });
                };
                provider_cfg.cookie_source = Some(cookie_source);
            }
            "extrasEnabled" => {
                let Some(enabled) = value.as_bool() else {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: "expected a boolean value".to_string(),
                    });
                };
                provider_cfg.extras_enabled = Some(enabled);
            }
            "region" => {
                let Some(region) = value.as_str() else {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: "expected a string value".to_string(),
                    });
                };
                provider_cfg.region = SettingsValue::cleaned(Some(region));
            }
            "workspaceId" => {
                let Some(ws) = value.as_str() else {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: "expected a string value".to_string(),
                    });
                };
                provider_cfg.workspace_id = SettingsValue::cleaned(Some(ws));
            }
            "enterpriseHost" => {
                let Some(host) = value.as_str() else {
                    return Err(SettingsMutationError::InvalidValue {
                        id: raw_id.to_string(),
                        reason: "expected a string value".to_string(),
                    });
                };
                provider_cfg.enterprise_host = SettingsValue::cleaned(Some(host));
            }
            _ => {
                return Err(SettingsMutationError::UnknownSetting(raw_id.to_string()));
            }
        }

        return Ok((
            SettingsScope::Provider(provider_id.clone()),
            Some(provider_id),
        ));
    }

    Err(SettingsMutationError::UnknownSetting(raw_id.to_string()))
}

fn ensure_typed_provider_entry(config: &mut Config, id: &ProviderId, default_enabled: bool) {
    if config.provider_config(id.as_str()).is_none() {
        let mut cfg = ProviderConfig::new(id.clone());
        cfg.enabled = Some(default_enabled);
        config.set_provider_config(cfg);
    }
}

fn source_mode_str(mode: SourceMode) -> &'static str {
    match mode {
        SourceMode::Auto => "auto",
        SourceMode::Web => "web",
        SourceMode::Cli => "cli",
        SourceMode::Oauth => "oauth",
        SourceMode::Api => "api",
    }
}

fn source_mode_label(mode: SourceMode) -> &'static str {
    match mode {
        SourceMode::Auto => "Auto",
        SourceMode::Web => "Web Cookie",
        SourceMode::Cli => "CLI",
        SourceMode::Oauth => "OAuth",
        SourceMode::Api => "API Token",
    }
}

fn parse_source_mode(raw: &str) -> Option<SourceMode> {
    match raw.trim() {
        "auto" => Some(SourceMode::Auto),
        "web" => Some(SourceMode::Web),
        "cli" => Some(SourceMode::Cli),
        "oauth" => Some(SourceMode::Oauth),
        "api" => Some(SourceMode::Api),
        _ => None,
    }
}

fn cookie_source_str(source: CookieSource) -> &'static str {
    match source {
        CookieSource::Auto => "auto",
        CookieSource::Manual => "manual",
        CookieSource::Off => "off",
    }
}

fn parse_cookie_source(raw: &str) -> Option<CookieSource> {
    match raw.trim() {
        "auto" => Some(CookieSource::Auto),
        "manual" => Some(CookieSource::Manual),
        "off" => Some(CookieSource::Off),
        _ => None,
    }
}
