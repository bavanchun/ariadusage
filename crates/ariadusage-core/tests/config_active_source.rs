// Ported from CodexBar Tests/CodexBarTests/CodexActiveSourceConfigTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::config::{
    Config, ProviderConfig, QuotaWarningWindow, QuotaWarningWindowConfig, QuotaWarnings, decode,
    encode,
};
use ariadusage_protocol::ProviderId;

#[test]
fn test_legacy_config_without_codex_active_source_decodes_to_nil() {
    let legacy_json = br#"{
        "version": 1,
        "providers": [
            {
                "id": "codex"
            }
        ]
    }"#;

    let decoded = decode(legacy_json)
        .expect("decode ok")
        .expect("config present");
    let codex_cfg = decoded
        .provider_config("codex")
        .expect("codex config present");

    assert!(!codex_cfg.extra.contains_key("codexActiveSource"));
    assert!(codex_cfg.quota_warnings.is_none());
}

#[test]
fn test_provider_config_round_trips_quota_warning_overrides() {
    let mut config = Config::new(1, vec![]);
    let mut codex = ProviderConfig::new(ProviderId::new("codex").unwrap());
    codex.quota_warnings = Some(QuotaWarnings {
        session: Some(QuotaWarningWindowConfig::new(Some(vec![10]), None)),
        weekly: Some(QuotaWarningWindowConfig::new(Some(vec![50, 20]), None)),
    });
    config.set_provider_config(codex);

    let data = encode(&config);
    let decoded = decode(&data).expect("decode ok").expect("config present");
    let quota_warnings = decoded
        .provider_config("codex")
        .expect("codex config present")
        .quota_warnings
        .as_ref()
        .expect("quota warnings present");

    assert_eq!(
        quota_warnings.thresholds(QuotaWarningWindow::Session, &[80]),
        vec![10]
    );
    assert_eq!(
        quota_warnings.thresholds(QuotaWarningWindow::Weekly, &[80]),
        vec![50, 20]
    );
}

#[test]
fn test_quota_warning_window_enabled_defaults_stay_backward_compatible() {
    let legacy_json = br#"{
        "version": 1,
        "providers": [
            {
                "id": "codex",
                "quotaWarnings": {
                    "session": { "thresholds": [10] },
                    "weekly": { "enabled": false }
                }
            }
        ]
    }"#;

    let decoded = decode(legacy_json)
        .expect("decode ok")
        .expect("config present");
    let quota_warnings = decoded
        .provider_config("codex")
        .expect("codex config present")
        .quota_warnings
        .as_ref()
        .expect("quota warnings present");

    assert!(quota_warnings.is_enabled(QuotaWarningWindow::Session, false));
    assert!(!quota_warnings.is_enabled(QuotaWarningWindow::Weekly, true));
    assert!(quota_warnings.has_override(QuotaWarningWindow::Session));
    assert!(quota_warnings.has_override(QuotaWarningWindow::Weekly));
}
