// Ported from CodexBar Tests/CodexBarTests/CodexBarConfigUnknownProviderTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::config::{decode, encode, normalize, sanitized_for_dump};
use ariadusage_core::providers::SourceMode;

#[test]
fn test_removed_provider_entries_do_not_invalidate_persisted_config() {
    let data = br#"{
      "version": 1,
      "providers": [
        {"id": "kimik2", "enabled": true},
        {"id": "crossmodel", "enabled": true},
        {"id": "crof", "enabled": true, "apiKey": "retired-fixture-key"},
        {"id": "codex", "enabled": false, "source": "oauth"}
      ]
    }"#;

    let config = decode(data).expect("decode ok").expect("config present");

    let typed_ids: Vec<&str> = config
        .providers
        .iter()
        .filter_map(|p| p.as_typed().map(|c| c.id.as_str()))
        .collect();
    assert_eq!(typed_ids, vec!["codex"]);

    let codex_cfg = config
        .provider_config("codex")
        .expect("codex config present");
    assert_eq!(codex_cfg.enabled, Some(false));
    assert_eq!(codex_cfg.source, Some(SourceMode::Oauth));
}

#[test]
fn test_reading_and_saving_retired_crof_config_preserves_its_record() {
    let original = br#"{
  "providers": [
    {
      "apiKey": "retired-fixture-key",
      "enabled": true,
      "id": "crof"
    },
    {
      "enabled": false,
      "id": "codex"
    }
  ],
  "version": 1
}"#;

    let config = decode(original)
        .expect("decode ok")
        .expect("config present");

    let saved = encode(&config);
    assert_eq!(
        std::str::from_utf8(&saved).unwrap(),
        std::str::from_utf8(original).unwrap()
    );
}

#[test]
fn test_unavailable_plugin_records_survive_normalization_and_existing_file_upgrades() {
    let fixture = br#"{
  "providers": [
    {
      "enabled": true,
      "future": {
        "fraction": 0.1234567890123456789012345678,
        "items": [
          true,
          42,
          "value"
        ],
        "large": 18446744073709551615,
        "null": null
      },
      "id": "fixture-unavailable",
      "pluginSecrets": {
        "TOKEN": "fixture-secret"
      },
      "pluginSettings": {
        "scope": "fixture"
      }
    },
    {
      "enabled": false,
      "id": "grok"
    },
    {
      "enabled": false,
      "id": "future.invalid/id",
      "pluginSecrets": {
        "OTHER": "fixture-other"
      },
      "source": "future-mode"
    }
  ],
  "version": 1
}"#;

    let config = decode(fixture).expect("decode ok").expect("config present");
    let normalized = normalize(normalize(config));
    let encoded = encode(&normalized);
    let encoded_str = std::str::from_utf8(&encoded).unwrap();

    // Verify unavailable plugin records are retained
    assert!(encoded_str.contains("\"fixture-unavailable\""));
    assert!(encoded_str.contains("\"future.invalid/id\""));
    assert!(encoded_str.contains("\"grok\""));
    // Verify first-party defaults were appended by normalization
    assert!(encoded_str.contains("\"codex\""));
    assert!(encoded_str.contains("\"claude\""));
    assert!(encoded_str.contains("\"antigravity\""));
}

#[test]
fn test_opaque_json_keeps_arbitrary_numeric_tokens_and_escaped_strings_byte_for_byte() {
    let record = r#"{"enabled":true,"future":[1e400,1e-400,-0,0.123456789012345678901234567890123456789],"id":"opaque-numbers","pluginSecrets":{"TOKEN":"fixture-secret"},"text":"braces } ] and escaped \" quote \\ slash"}"#;
    let raw = format!(r#"{{"providers":[{}],"version":1}}"#, record);

    let config = decode(raw.as_bytes())
        .expect("decode ok")
        .expect("config present");
    let normalized = normalize(config);
    let saved = String::from_utf8(encode(&normalized)).unwrap();
    assert!(saved.contains(record));

    let redacted_config = sanitized_for_dump(&normalized, false);
    let redacted = String::from_utf8(encode(&redacted_config)).unwrap();
    assert!(!redacted.contains("fixture-secret"));
    assert!(!redacted.contains("1e400"));
    assert!(redacted.contains("[REDACTED]"));
}

#[test]
fn test_explicit_deletion_shifts_remaining_opaque_entries_with_their_neighbors() {
    for deleted in ["first-opaque", "grok"] {
        let raw = br#"{"version":1,"providers":[{"id":"first-opaque"},{"id":"grok"},{"id":"second-opaque"},{"id":"groq"}]}"#;
        let mut config = decode(raw).expect("decode ok").expect("config present");

        config.remove_provider(deleted);

        let ids: Vec<String> = config
            .providers
            .iter()
            .map(|p| p.id().to_string())
            .collect();
        let expected: Vec<&str> = ["first-opaque", "grok", "second-opaque", "groq"]
            .into_iter()
            .filter(|&id| id != deleted)
            .collect();
        assert_eq!(ids, expected);
    }
}
