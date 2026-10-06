use ariadusage_core::config::{
    Config, ProviderEntry, REDACTED_PLACEHOLDER, SecretPresence, decode, encode,
    sanitized_for_dump, set_provider_enabled, validate,
};
use serde_json::value::RawValue;

fn make_runtime_secret(kind: &str) -> String {
    format!("runtime-secret-{}-{}", kind, 98765)
}

#[test]
fn test_secret_in_config_fires_for_each_secret_key() {
    let api_secret = make_runtime_secret("apikey");
    let cookie_secret = make_runtime_secret("cookie");
    let secret_key_secret = make_runtime_secret("secretkey");
    let plugin_secret = make_runtime_secret("plugin");
    let account_token_secret = make_runtime_secret("token");

    // 1. apiKey
    let json_api = format!(
        r#"{{"version":1,"providers":[{{"id":"claude","apiKey":"{}"}}]}}"#,
        api_secret
    );
    let cfg = decode(json_api.as_bytes()).unwrap().unwrap();
    let issues = validate(&cfg, &SecretPresence::none());
    assert!(
        issues
            .iter()
            .any(|i| i.code == "secret_in_config" && i.field.as_deref() == Some("apiKey"))
    );

    // 2. cookieHeader
    let json_cookie = format!(
        r#"{{"version":1,"providers":[{{"id":"claude","cookieHeader":"{}"}}]}}"#,
        cookie_secret
    );
    let cfg = decode(json_cookie.as_bytes()).unwrap().unwrap();
    let issues = validate(&cfg, &SecretPresence::none());
    assert!(
        issues
            .iter()
            .any(|i| i.code == "secret_in_config" && i.field.as_deref() == Some("cookieHeader"))
    );

    // 3. secretKey
    let json_secret = format!(
        r#"{{"version":1,"providers":[{{"id":"claude","secretKey":"{}"}}]}}"#,
        secret_key_secret
    );
    let cfg = decode(json_secret.as_bytes()).unwrap().unwrap();
    let issues = validate(&cfg, &SecretPresence::none());
    assert!(
        issues
            .iter()
            .any(|i| i.code == "secret_in_config" && i.field.as_deref() == Some("secretKey"))
    );

    // 4. pluginSecrets
    let json_plugin = format!(
        r#"{{"version":1,"providers":[{{"id":"claude","pluginSecrets":{{"MY_TOKEN":"{}"}}}}]}}"#,
        plugin_secret
    );
    let cfg = decode(json_plugin.as_bytes()).unwrap().unwrap();
    let issues = validate(&cfg, &SecretPresence::none());
    assert!(
        issues
            .iter()
            .any(|i| i.code == "secret_in_config" && i.field.as_deref() == Some("pluginSecrets"))
    );

    // 5. tokenAccounts account token
    let json_token = format!(
        r#"{{"version":1,"providers":[{{"id":"claude","tokenAccounts":{{"version":1,"activeIndex":0,"accounts":[{{"id":"acc-1","label":"acc","addedAt":0.0,"token":"{}"}}]}}}}]}}"#,
        account_token_secret
    );
    let cfg = decode(json_token.as_bytes()).unwrap().unwrap();
    let issues = validate(&cfg, &SecretPresence::none());
    assert!(
        issues
            .iter()
            .any(|i| i.code == "secret_in_config" && i.field.as_deref() == Some("tokenAccounts"))
    );
}

#[test]
fn test_secrets_survive_decode_set_enabled_encode_byte_for_byte() {
    let api_secret = make_runtime_secret("apikey-pres");
    let cookie_secret = make_runtime_secret("cookie-pres");
    let secret_key_secret = make_runtime_secret("secretkey-pres");
    let plugin_secret = make_runtime_secret("plugin-pres");
    let account_token_secret = make_runtime_secret("token-pres");

    let original_json = format!(
        r#"{{
  "providers": [
    {{
      "apiKey": "{api_secret}",
      "cookieHeader": "{cookie_secret}",
      "enabled": false,
      "id": "claude",
      "pluginSecrets": {{
        "SUB_TOKEN": "{plugin_secret}"
      }},
      "secretKey": "{secret_key_secret}",
      "tokenAccounts": {{
        "accounts": [
          {{
            "addedAt": 100.0,
            "id": "acc-pres-1",
            "label": "My Account",
            "token": "{account_token_secret}"
          }}
        ],
        "activeIndex": 0,
        "version": 1
      }}
    }}
  ],
  "version": 1
}}"#
    );

    let mut config = decode(original_json.as_bytes()).unwrap().unwrap();

    // Mutate enablement
    set_provider_enabled(&mut config, "claude", true);

    let encoded = String::from_utf8(encode(&config)).unwrap();

    // Verify all secrets survived in the encoded document
    assert!(encoded.contains(&api_secret));
    assert!(encoded.contains(&cookie_secret));
    assert!(encoded.contains(&secret_key_secret));
    assert!(encoded.contains(&plugin_secret));
    assert!(encoded.contains(&account_token_secret));
    assert!(encoded.contains("\"enabled\": true"));

    // Verify round-trip decode
    let re_decoded = decode(encoded.as_bytes()).unwrap().unwrap();
    let claude = re_decoded.provider_config("claude").unwrap();
    assert_eq!(
        claude.extra.get("apiKey").unwrap().get(),
        format!("\"{api_secret}\"")
    );
    assert_eq!(
        claude.extra.get("cookieHeader").unwrap().get(),
        format!("\"{cookie_secret}\"")
    );
    assert_eq!(
        claude.extra.get("secretKey").unwrap().get(),
        format!("\"{secret_key_secret}\"")
    );
    assert!(
        claude
            .extra
            .get("pluginSecrets")
            .unwrap()
            .get()
            .contains(&plugin_secret)
    );
    let ta = claude.token_accounts.as_ref().unwrap();
    assert_eq!(
        ta.accounts[0].extra.get("token").unwrap().get(),
        format!("\"{account_token_secret}\"")
    );
}

#[test]
fn test_secrets_appear_in_neither_dump_nor_debug() {
    let api_secret = make_runtime_secret("apikey-dump");
    let cookie_secret = make_runtime_secret("cookie-dump");
    let secret_key_secret = make_runtime_secret("secretkey-dump");
    let plugin_secret = make_runtime_secret("plugin-dump");
    let account_token_secret = make_runtime_secret("token-dump");
    let opaque_secret = make_runtime_secret("opaque-dump");

    let json = format!(
        r#"{{
  "providers": [
    {{
      "apiKey": "{api_secret}",
      "cookieHeader": "{cookie_secret}",
      "enabled": true,
      "id": "claude",
      "pluginSecrets": {{
        "KEY": "{plugin_secret}"
      }},
      "secretKey": "{secret_key_secret}",
      "tokenAccounts": {{
        "accounts": [
          {{
            "addedAt": 200.0,
            "id": "acc-dump-1",
            "label": "Acc Dump",
            "token": "{account_token_secret}"
          }}
        ],
        "activeIndex": 0,
        "version": 1
      }}
    }},
    {{
      "enabled": true,
      "id": "opaque-provider",
      "secretField": "{opaque_secret}"
    }}
  ],
  "version": 1
}}"#
    );

    let config = decode(json.as_bytes()).unwrap().unwrap();

    // 1. Sanitized for dump:
    let dumped = sanitized_for_dump(&config, false);
    let dumped_str = String::from_utf8(encode(&dumped)).unwrap();

    assert!(!dumped_str.contains(&api_secret));
    assert!(!dumped_str.contains(&cookie_secret));
    assert!(!dumped_str.contains(&secret_key_secret));
    assert!(!dumped_str.contains(&plugin_secret));
    assert!(!dumped_str.contains(&account_token_secret));
    assert!(!dumped_str.contains(&opaque_secret));

    assert!(dumped_str.contains(REDACTED_PLACEHOLDER));

    // 2. show_secrets: true preserves secrets:
    let full_dump = sanitized_for_dump(&config, true);
    let full_dump_str = String::from_utf8(encode(&full_dump)).unwrap();
    assert!(full_dump_str.contains(&api_secret));
    assert!(full_dump_str.contains(&opaque_secret));
}

#[test]
fn test_debug_does_not_leak_raw_values() {
    let api_secret = make_runtime_secret("apikey-dbg");
    let cookie_secret = make_runtime_secret("cookie-dbg");
    let secret_key_secret = make_runtime_secret("secretkey-dbg");
    let plugin_secret = make_runtime_secret("plugin-dbg");
    let account_token_secret = make_runtime_secret("token-dbg");
    let top_secret = make_runtime_secret("top-dbg");
    let opaque_secret = make_runtime_secret("opaque-dbg");

    let json = format!(
        r#"{{
  "extraTopSecret": "{top_secret}",
  "providers": [
    {{
      "apiKey": "{api_secret}",
      "cookieHeader": "{cookie_secret}",
      "enabled": true,
      "id": "claude",
      "pluginSecrets": {{
        "KEY": "{plugin_secret}"
      }},
      "secretKey": "{secret_key_secret}",
      "tokenAccounts": {{
        "accounts": [
          {{
            "addedAt": 300.0,
            "id": "acc-dbg-1",
            "label": "Acc Dbg",
            "token": "{account_token_secret}"
          }}
        ],
        "activeIndex": 0,
        "version": 1
      }}
    }},
    {{
      "enabled": true,
      "id": "opaque-provider",
      "secretField": "{opaque_secret}"
    }}
  ],
  "version": 1
}}"#
    );

    let config = decode(json.as_bytes()).unwrap().unwrap();

    let debug_config = format!("{:?}", config);
    let debug_pretty_config = format!("{:#?}", config);

    for debug_output in [&debug_config, &debug_pretty_config] {
        assert!(!debug_output.contains(&api_secret));
        assert!(!debug_output.contains(&cookie_secret));
        assert!(!debug_output.contains(&secret_key_secret));
        assert!(!debug_output.contains(&plugin_secret));
        assert!(!debug_output.contains(&account_token_secret));
        assert!(!debug_output.contains(&top_secret));
        assert!(!debug_output.contains(&opaque_secret));

        // Must show keys only
        assert!(debug_output.contains("extra_keys"));
        assert!(debug_output.contains("apiKey"));
        assert!(debug_output.contains("extra_top_keys"));
        assert!(debug_output.contains("extraTopSecret"));
    }
}

#[test]
fn test_plugin_secrets_redaction_across_all_json_types() {
    let str_sec = make_runtime_secret("ps-str");
    let arr_sec1 = make_runtime_secret("ps-arr1");
    let arr_sec2 = make_runtime_secret("ps-arr2");
    let obj_sec1 = make_runtime_secret("ps-obj1");
    let obj_sec2 = make_runtime_secret("ps-obj2");

    let test_cases = [
        // 1. String
        (
            format!(
                r#"{{"version":1,"providers":[{{"id":"codex","enabled":true,"pluginSecrets":"{}"}}]}}"#,
                str_sec
            ),
            vec![str_sec.as_str()],
            true, // is_non_object
        ),
        // 2. Number
        (
            r#"{"version":1,"providers":[{"id":"codex","enabled":true,"pluginSecrets":987654}]}"#
                .to_string(),
            vec!["987654"],
            true,
        ),
        // 3. Array
        (
            format!(
                r#"{{"version":1,"providers":[{{"id":"codex","enabled":true,"pluginSecrets":["{}","{}"]}}]}}"#,
                arr_sec1, arr_sec2
            ),
            vec![arr_sec1.as_str(), arr_sec2.as_str()],
            true,
        ),
        // 4. Object
        (
            format!(
                r#"{{"version":1,"providers":[{{"id":"codex","enabled":true,"pluginSecrets":{{"K1":"{}","K2":123,"K3":{{"NESTED":"{}"}}}}}}]}}"#,
                obj_sec1, obj_sec2
            ),
            vec![obj_sec1.as_str(), obj_sec2.as_str()],
            false, // is_object
        ),
        // 5. Bool
        (
            r#"{"version":1,"providers":[{"id":"codex","enabled":true,"pluginSecrets":true}]}"#
                .to_string(),
            vec![],
            true,
        ),
        // 6. Null
        (
            r#"{"version":1,"providers":[{"id":"codex","enabled":true,"pluginSecrets":null}]}"#
                .to_string(),
            vec![],
            true,
        ),
    ];

    for (json_str, secrets_to_check, is_non_object) in test_cases {
        let cfg = decode(json_str.as_bytes()).unwrap().unwrap();

        // secret_in_config must fire
        let issues = validate(&cfg, &SecretPresence::none());
        assert!(
            issues.iter().any(
                |i| i.code == "secret_in_config" && i.field.as_deref() == Some("pluginSecrets")
            ),
            "secret_in_config must fire for json: {}",
            json_str
        );

        // sanitized_for_dump must redact
        let dumped = sanitized_for_dump(&cfg, false);
        let dumped_str = String::from_utf8(encode(&dumped)).unwrap();

        for s in secrets_to_check {
            assert!(
                !dumped_str.contains(s),
                "Secret {} must not appear in dump: {}",
                s,
                dumped_str
            );
        }

        let codex = dumped.provider_config("codex").unwrap();
        let ps_raw = codex
            .extra
            .get("pluginSecrets")
            .expect("pluginSecrets exists");

        if is_non_object {
            assert_eq!(
                ps_raw.get().trim(),
                format!("\"{}\"", REDACTED_PLACEHOLDER),
                "Non-object pluginSecrets must be redacted to \"[REDACTED]\""
            );
        } else {
            // Object: each key must be kept with value [REDACTED]
            let map: std::collections::BTreeMap<String, String> =
                serde_json::from_str(ps_raw.get()).expect("valid object");
            assert_eq!(map.len(), 3);
            assert_eq!(
                map.get("K1").map(String::as_str),
                Some(REDACTED_PLACEHOLDER)
            );
            assert_eq!(
                map.get("K2").map(String::as_str),
                Some(REDACTED_PLACEHOLDER)
            );
            assert_eq!(
                map.get("K3").map(String::as_str),
                Some(REDACTED_PLACEHOLDER)
            );
        }
    }
}

#[test]
fn test_opaque_id_with_special_characters_through_dump_and_encode() {
    let special_id = "spec\"ial\\id/test🦀_ö";
    let opaque_secret = make_runtime_secret("opaque-spec");

    let json = format!(
        r#"{{"version":1,"providers":[{{"id":{},"enabled":true,"secretField":"{}"}}]}}"#,
        serde_json::to_string(special_id).unwrap(),
        opaque_secret
    );

    let cfg = decode(json.as_bytes())
        .unwrap()
        .expect("decodes valid config");

    // sanitized_for_dump must not panic and must properly escape special_id
    let dumped = sanitized_for_dump(&cfg, false);

    // encode must not panic
    let encoded_bytes = encode(&dumped);
    let encoded_str = String::from_utf8(encoded_bytes.clone()).unwrap();

    assert!(
        !encoded_str.contains(&opaque_secret),
        "Secret must not appear in dump"
    );
    assert!(
        encoded_str.contains(REDACTED_PLACEHOLDER),
        "Dump must contain redaction placeholder"
    );

    // Re-decode to ensure it round-trips
    let re_decoded = decode(&encoded_bytes).unwrap().expect("round-trips");
    assert_eq!(re_decoded.providers.len(), 1);
    assert_eq!(re_decoded.providers[0].id(), special_id);
    assert!(re_decoded.providers[0].is_enabled());
}

#[test]
fn test_typed_entry_secret_keys_non_string_values_redacted() {
    let obj_secret = make_runtime_secret("obj-sec");
    let arr_secret = make_runtime_secret("arr-sec");

    let json = format!(
        r#"{{"version":1,"providers":[{{"id":"claude","apiKey":{{"nested":"{}"}},"cookieHeader":["{}"],"secretKey":123456}}]}}"#,
        obj_secret, arr_secret
    );

    let cfg = decode(json.as_bytes()).unwrap().expect("valid config");
    let dumped = sanitized_for_dump(&cfg, false);
    let encoded_str = String::from_utf8(encode(&dumped)).unwrap();

    assert!(!encoded_str.contains(&obj_secret));
    assert!(!encoded_str.contains(&arr_secret));
    assert!(!encoded_str.contains("123456"));

    let claude = dumped.provider_config("claude").unwrap();
    assert_eq!(
        claude.extra.get("apiKey").unwrap().get().trim(),
        format!("\"{}\"", REDACTED_PLACEHOLDER)
    );
    assert_eq!(
        claude.extra.get("cookieHeader").unwrap().get().trim(),
        format!("\"{}\"", REDACTED_PLACEHOLDER)
    );
    assert_eq!(
        claude.extra.get("secretKey").unwrap().get().trim(),
        format!("\"{}\"", REDACTED_PLACEHOLDER)
    );
}

#[test]
fn test_opaque_entry_non_object_raw_value_redacted() {
    let prim_secret = make_runtime_secret("prim-sec");
    let mut cfg = Config::new(1, vec![]);
    let raw_val = RawValue::from_string(format!("\"{}\"", prim_secret)).unwrap();
    cfg.providers.push(ProviderEntry::Opaque {
        id: "non-obj-entry".to_string(),
        enabled: true,
        raw: raw_val,
    });

    let dumped = sanitized_for_dump(&cfg, false);
    let encoded_str = String::from_utf8(encode(&dumped)).unwrap();

    assert!(
        !encoded_str.contains(&prim_secret),
        "Non-object opaque raw value must be redacted"
    );
    assert!(
        encoded_str.contains(REDACTED_PLACEHOLDER),
        "Dump must contain redaction placeholder"
    );
}
