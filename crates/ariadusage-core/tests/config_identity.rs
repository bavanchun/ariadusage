// Ported from CodexBar Tests/CodexBarTests/ProviderInstanceIdentityCharacterizationTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::config::{Config, ProviderConfig, ProviderEntry, decode, encode};
use ariadusage_protocol::ProviderId;

#[test]
fn test_config_provider_ids_and_enablement_encode_as_their_existing_bare_strings() {
    let input = br#"{"version":1,"providers":[{"id":"claude","enabled":true},{"id":"codex","enabled":false}]}"#;
    let config = decode(input).expect("decode ok").expect("config present");

    assert_eq!(config.ordered_providers(), vec!["claude", "codex"]);
    assert_eq!(config.enabled_providers(), vec!["claude"]);

    let encoded = encode(&config);
    let expected = r#"{
  "providers": [
    {
      "enabled": true,
      "id": "claude"
    },
    {
      "enabled": false,
      "id": "codex"
    }
  ],
  "version": 1
}"#;
    assert_eq!(std::str::from_utf8(&encoded).unwrap(), expected);
}

#[test]
fn test_provider_config_ordering_remains_the_menu_and_status_ordering_contract() {
    let mut agy = ProviderConfig::new(ProviderId::new("antigravity").unwrap());
    agy.enabled = Some(true);

    let mut claude = ProviderConfig::new(ProviderId::new("claude").unwrap());
    claude.enabled = Some(false);

    let mut codex = ProviderConfig::new(ProviderId::new("codex").unwrap());
    codex.enabled = Some(true);

    let config = Config::new(
        1,
        vec![
            ProviderEntry::Typed(agy),
            ProviderEntry::Typed(claude),
            ProviderEntry::Typed(codex),
        ],
    );

    assert_eq!(
        config.ordered_providers(),
        vec!["antigravity", "claude", "codex"]
    );
    assert_eq!(config.enabled_providers(), vec!["antigravity", "codex"]);
}
