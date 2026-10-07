use ariadusage_core::model::{ProviderCostSnapshot, RateWindow, UsagePercent};
use ariadusage_core::providers::{SourceMode, find_by_id_str};
use ariadusage_core::settings_value::SettingsValue;
use jiff::Timestamp;

#[test]
fn gap_non_finite_numbers_are_rejected() {
    assert!(RateWindow::new(f64::NAN, Some(300), None, None, None, false).is_err());
    assert!(RateWindow::new(f64::INFINITY, Some(300), None, None, None, false).is_err());
    assert!(RateWindow::new(f64::NEG_INFINITY, Some(300), None, None, None, false).is_err());
    assert!(RateWindow::new(50.0, Some(300), None, None, Some(f64::NAN), false).is_err());
    assert!(RateWindow::new(50.0, Some(300), None, None, Some(f64::INFINITY), false).is_err());

    let json_null = r#"{"usedPercent": null, "windowMinutes": 300}"#;
    assert!(serde_json::from_str::<RateWindow>(json_null).is_err());

    assert!(UsagePercent::from_ratio(f64::NAN, 100.0).is_err());
    assert!(UsagePercent::from_ratio(50.0, f64::NAN).is_err());
    assert!(UsagePercent::from_ratio(50.0, 0.0).is_err());
    assert!(UsagePercent::from_ratio(50.0, -10.0).is_err());

    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let nan_limit_cost = ProviderCostSnapshot::new(10.0, f64::NAN, "USD", now);
    assert!(nan_limit_cost.spend_limit_window().is_none());

    let nan_used_cost = ProviderCostSnapshot::new(f64::NAN, 100.0, "USD", now);
    assert!(nan_used_cost.spend_limit_window().is_none());
}

#[test]
fn gap_one_character_quote_inputs() {
    assert_eq!(SettingsValue::cleaned(Some("\"")), None);
    assert_eq!(SettingsValue::cleaned(Some("'")), None);
    assert_eq!(SettingsValue::cleaned(Some(" \" ")), None);
    assert_eq!(SettingsValue::cleaned(Some(" ' ")), None);
}

#[test]
fn gap_descriptor_values_match_codexbar_baseline() {
    // Codex Provider Descriptor
    let codex = find_by_id_str("codex").expect("codex descriptor");
    assert_eq!(codex.display_name, "Codex");
    // Codex/CodexProviderDescriptor.swift:43
    assert!(codex.default_enabled);
    // Codex/CodexProviderDescriptor.swift:138
    assert_eq!(
        codex.source_modes,
        &[
            SourceMode::Auto,
            SourceMode::Web,
            SourceMode::Cli,
            SourceMode::Oauth,
            SourceMode::Api,
        ]
    );
    // Codex/CodexProviderDescriptor.swift:31
    assert!(!codex.requires_api_key_for_api_source);
    assert!(!codex.supports_api_key_override);
    assert!(!codex.token_account_support);
    assert!(!codex.uses_region);
    assert!(!codex.uses_secret_key);
    assert!(!codex.supports_workspace);
    assert!(!codex.supports_enterprise_host);

    // Claude Provider Descriptor
    let claude = find_by_id_str("claude").expect("claude descriptor");
    assert_eq!(claude.display_name, "Claude");
    // Claude/ClaudeProviderDescriptor.swift:134
    assert!(!claude.default_enabled);
    // Claude/ClaudeProviderDescriptor.swift:234
    assert_eq!(
        claude.source_modes,
        &[
            SourceMode::Auto,
            SourceMode::Api,
            SourceMode::Web,
            SourceMode::Cli,
            SourceMode::Oauth,
        ]
    );
    assert!(claude.supports_api_key_override);
    assert!(claude.requires_api_key_for_api_source);
    assert!(claude.token_account_support);
    assert!(!claude.uses_region);
    assert!(!claude.uses_secret_key);
    assert!(!claude.supports_workspace);
    assert!(!claude.supports_enterprise_host);

    // Antigravity Provider Descriptor
    let antigravity = find_by_id_str("antigravity").expect("antigravity descriptor");
    assert_eq!(antigravity.display_name, "Antigravity");
    // Antigravity/AntigravityProviderDescriptor.swift:36
    assert!(!antigravity.default_enabled);
    // Antigravity/AntigravityProviderDescriptor.swift:102
    assert_eq!(
        antigravity.source_modes,
        &[SourceMode::Auto, SourceMode::Cli, SourceMode::Oauth]
    );
    assert!(!antigravity.supports_api_key_override);
    assert!(antigravity.requires_api_key_for_api_source);
    assert!(antigravity.token_account_support);
    assert!(!antigravity.uses_region);
    assert!(!antigravity.uses_secret_key);
    assert!(!antigravity.supports_workspace);
    assert!(!antigravity.supports_enterprise_host);
}
