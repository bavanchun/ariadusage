use ariadusage_core::config::{Config, ConfigError, decode, encode};
use serde_json::value::RawValue;

#[test]
fn test_duplicate_top_level_key_rejected() {
    // Duplicate "version"
    let json_dup_version = br#"{
        "version": 1,
        "version": 1,
        "providers": []
    }"#;
    let err1 = decode(json_dup_version).unwrap_err();
    assert_eq!(
        err1,
        ConfigError::DuplicateKey {
            key: "version".to_string()
        }
    );

    // Duplicate "providers"
    let json_dup_providers = br#"{
        "version": 1,
        "providers": [],
        "providers": []
    }"#;
    let err2 = decode(json_dup_providers).unwrap_err();
    assert_eq!(
        err2,
        ConfigError::DuplicateKey {
            key: "providers".to_string()
        }
    );

    // Duplicate custom top key
    let json_dup_custom = br#"{
        "version": 1,
        "providers": [],
        "customKey": "a",
        "customKey": "b"
    }"#;
    let err3 = decode(json_dup_custom).unwrap_err();
    assert_eq!(
        err3,
        ConfigError::DuplicateKey {
            key: "customKey".to_string()
        }
    );
}

#[test]
fn test_utf8_bom_stripped() {
    let mut bom_bytes = vec![0xEF, 0xBB, 0xBF];
    bom_bytes.extend_from_slice(br#"{"version": 1, "providers": []}"#);

    let decoded = decode(&bom_bytes)
        .expect("decode ok")
        .expect("config present");
    assert_eq!(decoded.version, 1);
    assert!(decoded.providers.is_empty());
}

#[test]
fn test_blank_input_returns_none() {
    assert_eq!(decode(b"").unwrap(), None);
    assert_eq!(decode(b"   ").unwrap(), None);
    assert_eq!(decode(b"\t\r\n  \n\t").unwrap(), None);
}

#[test]
fn test_sorted_key_guard() {
    let mut config = Config::new(1, vec![]);
    config.extra_top.insert(
        "zeta".to_string(),
        RawValue::from_string("1".to_string()).unwrap(),
    );
    config.extra_top.insert(
        "alpha".to_string(),
        RawValue::from_string("2".to_string()).unwrap(),
    );
    config.extra_top.insert(
        "beta".to_string(),
        RawValue::from_string("3".to_string()).unwrap(),
    );

    let encoded = String::from_utf8(encode(&config)).unwrap();

    let alpha_pos = encoded.find("\"alpha\"").unwrap();
    let beta_pos = encoded.find("\"beta\"").unwrap();
    let providers_pos = encoded.find("\"providers\"").unwrap();
    let version_pos = encoded.find("\"version\"").unwrap();
    let zeta_pos = encoded.find("\"zeta\"").unwrap();

    // Verify strictly ascending character positions in output
    assert!(alpha_pos < beta_pos);
    assert!(beta_pos < providers_pos);
    assert!(providers_pos < version_pos);
    assert!(version_pos < zeta_pos);
}

#[test]
fn test_hooks_settings_and_unknown_top_level_keys_survive_byte_for_byte() {
    let raw_hooks = r#"{"custom_hook":{"action":"notify","flag":true}}"#;
    let raw_settings = r#"{"general":{"cadence_seconds":60,"enabled":true}}"#;
    let raw_extra1 = r#"{"inner":[1,2,3]}"#;
    let raw_extra2 = r#""plain-string""#;

    let json = format!(
        r#"{{
  "extraOne": {raw_extra1},
  "extraTwo": {raw_extra2},
  "hooks": {raw_hooks},
  "providers": [],
  "settings": {raw_settings},
  "version": 1
}}"#
    );

    let config = decode(json.as_bytes()).unwrap().unwrap();

    assert_eq!(config.hooks.as_ref().unwrap().get(), raw_hooks);
    assert_eq!(config.settings.as_ref().unwrap().get(), raw_settings);
    assert_eq!(config.extra_top.get("extraOne").unwrap().get(), raw_extra1);
    assert_eq!(config.extra_top.get("extraTwo").unwrap().get(), raw_extra2);

    let encoded = String::from_utf8(encode(&config)).unwrap();
    assert_eq!(encoded, json);
}
