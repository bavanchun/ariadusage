// Ported from CodexBar TestsLinux/PluginConfigPreservationLinuxTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::config::{decode, encode, set_provider_enabled};

#[test]
fn test_config_writes_retain_plugins_without_a_registered_runtime() {
    let raw = br#"{"version":1,"providers":[{"id":"portable-fixture","pluginSecrets":{"TOKEN":"fixture-secret"},
"pluginSettings":{"scope":"fixture"},"future":[null,true,18446744073709551615]}]}"#;

    let mut loaded = decode(raw).expect("decode ok").expect("config present");

    set_provider_enabled(&mut loaded, "grok", true);

    let saved = encode(&loaded);
    let re_decoded = decode(&saved)
        .expect("re-decode ok")
        .expect("config present");

    let first = &re_decoded.providers[0];
    assert_eq!(first.id(), "portable-fixture");
    // Verify exact bytes of opaque plugin record are retained
    if let ariadusage_core::config::ProviderEntry::Opaque { raw, .. } = first {
        let json: serde_json::Value =
            serde_json::from_str(raw.get()).expect("valid JSON in raw opaque");
        assert_eq!(json["id"], "portable-fixture");
        assert_eq!(json["pluginSecrets"]["TOKEN"], "fixture-secret");
    } else {
        panic!("expected opaque entry for portable-fixture");
    }

    let grok_entry = re_decoded
        .provider_entry("grok")
        .expect("grok entry present");
    assert!(grok_entry.is_enabled());
}
