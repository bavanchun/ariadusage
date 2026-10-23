// Ported from CodexBar Tests/CodexBarTests/ProviderConfigModelRateLimitsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexBarConfigHooksTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::config::{
    Config, ConfigError, ModelRateLimit, ProviderConfig, ProviderEntry, TokenAccountMeta,
    TokenAccountsMeta, decode, encode, normalize,
};
use ariadusage_protocol::ProviderId;
use serde_json::Value;

const ACCOUNT_ID: &str = "00000000-0000-0000-0000-000000000001";

fn decode_json(json: &str) -> Config {
    decode(json.as_bytes())
        .expect("config decodes")
        .expect("config is not blank")
}

fn encoded_json(config: &Config) -> Value {
    serde_json::from_slice(&encode(config)).expect("encoded config is JSON")
}

fn single_provider(provider: ProviderConfig) -> Config {
    Config::new(1, vec![ProviderEntry::Typed(provider)])
}

fn codex() -> ProviderConfig {
    ProviderConfig::new(ProviderId::new("codex").unwrap())
}

fn account(limits: Option<Vec<ModelRateLimit>>) -> TokenAccountMeta {
    TokenAccountMeta {
        id: ACCOUNT_ID.to_string(),
        label: "Synthetic".to_string(),
        added_at: 1.0,
        last_used: Some(2.0),
        external_identifier: Some("synthetic-id".to_string()),
        usage_scope: Some("synthetic-scope".to_string()),
        organization_id: Some("synthetic-org".to_string()),
        workspace_id: Some("synthetic-workspace".to_string()),
        claude_config_directory: None,
        seat_credit_entitlement: Some("100".to_string()),
        model_rate_limits: limits,
        extra: Default::default(),
    }
}

fn with_account(account: TokenAccountMeta) -> ProviderConfig {
    let mut provider = codex();
    provider.token_accounts = Some(TokenAccountsMeta {
        version: 1,
        active_index: 0,
        accounts: vec![account],
    });
    provider
}

fn unvalidated_rows() -> Vec<ModelRateLimit> {
    vec![
        ModelRateLimit::new(" ", 1, None),
        ModelRateLimit::new("zero", 0, None),
        ModelRateLimit::new("negative", -1, None),
        ModelRateLimit::new(" gpt-5 ", 100, Some(10)),
        ModelRateLimit::new("gpt-5", 200, Some(0)),
        ModelRateLimit::new("gpt-4o", 300, Some(-1)),
    ]
}

fn expected_rows() -> Vec<ModelRateLimit> {
    vec![
        ModelRateLimit::new("gpt-5", 200, None),
        ModelRateLimit::new("gpt-4o", 300, None),
    ]
}

#[test]
fn model_rate_limits_round_trip_and_empty_values_are_omitted() {
    let mut provider = codex();
    provider.model_rate_limits = Some(vec![
        ModelRateLimit::new("gpt-5", 40_000_000, Some(15_000)),
        ModelRateLimit::new("gpt-4o", 100, None),
    ]);
    let config = single_provider(provider.clone());

    let decoded = decode(&encode(&config)).unwrap().unwrap();
    assert_eq!(
        decoded.provider_config("codex").unwrap().model_rate_limits,
        provider.model_rate_limits
    );

    let json = encoded_json(&config);
    let rows = json["providers"][0]["modelRateLimits"]
        .as_array()
        .expect("rows are written");
    assert_eq!(rows[0]["model"], "gpt-5");
    assert_eq!(rows[0]["tokensPerMinute"], 40_000_000);
    assert_eq!(rows[0]["requestsPerMinute"], 15_000);
    assert!(rows[1].get("requestsPerMinute").is_none());

    for value in [None, Some(Vec::new())] {
        let mut empty = codex();
        empty.model_rate_limits = value;
        let bytes = encode(&single_provider(empty));
        assert!(
            !String::from_utf8(bytes)
                .unwrap()
                .contains("modelRateLimits")
        );
    }

    let old = decode_json(r#"{"version":1,"providers":[{"id":"codex"}]}"#);
    assert_eq!(
        old.provider_config("codex").unwrap().model_rate_limits,
        None
    );
    assert_eq!(encode(&old), encode(&single_provider(codex())));
}

#[test]
fn loading_trims_drops_invalid_counts_and_uses_the_last_duplicate() {
    let config = decode_json(
        r#"{"version":1,"providers":[{"id":"codex","modelRateLimits":[
          {"model":" ","tokensPerMinute":10},
          {"model":"zero","tokensPerMinute":0},
          {"model":"negative","tokensPerMinute":-1},
          {"model":" gpt-5 ","tokensPerMinute":100,"requestsPerMinute":5},
          {"model":"gpt-5","tokensPerMinute":200,"requestsPerMinute":0},
          {"model":"gpt-4o","tokensPerMinute":100,"requestsPerMinute":-1}
        ]}]}"#,
    );
    assert_eq!(
        config.provider_config("codex").unwrap().model_rate_limits,
        Some(vec![
            ModelRateLimit::new("gpt-5", 200, None),
            ModelRateLimit::new("gpt-4o", 100, None),
        ])
    );
}

#[test]
fn legacy_provider_level_limits_decode_as_the_provider_keys_limits() {
    let config = decode_json(&format!(
        r#"{{"version":1,"providers":[{{"id":"codex",
          "modelRateLimits":[{{"model":"gpt-5","tokensPerMinute":100}}],
          "tokenAccounts":{{"version":1,"activeIndex":0,"accounts":[{{
            "id":"{ACCOUNT_ID}","label":"Legacy","token":"synthetic","addedAt":0
          }}]}}}}]}}"#
    ));
    let provider = config.provider_config("codex").unwrap();
    assert_eq!(
        provider.model_rate_limits,
        Some(vec![ModelRateLimit::new("gpt-5", 100, None)])
    );
    let account = &provider.token_accounts.as_ref().unwrap().accounts[0];
    assert_eq!(account.model_rate_limits, None);
}

#[test]
fn account_limits_validate_at_decoding_and_encode_inside_the_account() {
    let mut account_json = encoded_json(&single_provider(with_account(account(None))))["providers"]
        [0]["tokenAccounts"]["accounts"][0]
        .clone();
    account_json["modelRateLimits"] = serde_json::to_value(unvalidated_rows()).unwrap();
    let document = serde_json::json!({
        "version": 1,
        "providers": [{
            "id": "codex",
            "tokenAccounts": {"version": 1, "activeIndex": 0, "accounts": [account_json]},
        }],
    });
    let decoded = decode_json(&document.to_string());
    let provider = decoded.provider_config("codex").unwrap();
    let decoded_account = &provider.token_accounts.as_ref().unwrap().accounts[0];
    assert_eq!(decoded_account.model_rate_limits, Some(expected_rows()));

    let json = encoded_json(&decoded);
    assert!(json["providers"][0].get("modelRateLimits").is_none());
    let encoded_account = &json["providers"][0]["tokenAccounts"]["accounts"][0];
    assert_eq!(
        encoded_account["modelRateLimits"].as_array().map(Vec::len),
        Some(2)
    );

    let normalized = normalize(decoded.clone());
    let normalized_account = &normalized
        .provider_config("codex")
        .unwrap()
        .token_accounts
        .as_ref()
        .unwrap()
        .accounts[0];
    assert_eq!(normalized_account.model_rate_limits, Some(expected_rows()));

    let invalid_only = vec![ModelRateLimit::new("bad", 0, None)];
    for rows in [None, Some(Vec::new()), Some(invalid_only)] {
        let validated = rows.clone().map(ModelRateLimit::validated);
        let config = single_provider(with_account(account(validated.clone())));
        let json = encoded_json(&config);
        let written = &json["providers"][0]["tokenAccounts"]["accounts"][0];
        match &rows {
            None => assert!(written.get("modelRateLimits").is_none()),
            Some(_) => assert_eq!(written["modelRateLimits"], serde_json::json!([])),
        }

        let mut raw_account = account_json.clone();
        match &rows {
            None => {
                raw_account
                    .as_object_mut()
                    .unwrap()
                    .remove("modelRateLimits");
            }
            Some(rows) => raw_account["modelRateLimits"] = serde_json::to_value(rows).unwrap(),
        }
        let document = serde_json::json!({
            "version": 1,
            "providers": [{
                "id": "codex",
                "tokenAccounts": {"version": 1, "activeIndex": 0, "accounts": [raw_account]},
            }],
        });
        let decoded = decode_json(&document.to_string());
        let decoded_account = &decoded
            .provider_config("codex")
            .unwrap()
            .token_accounts
            .as_ref()
            .unwrap()
            .accounts[0];
        assert_eq!(decoded_account.model_rate_limits, validated);
    }
}

#[test]
fn null_limits_decode_as_absent_and_are_not_written() {
    let config = decode_json(&format!(
        r#"{{"version":1,"providers":[{{"id":"codex","modelRateLimits":null,
          "tokenAccounts":{{"version":1,"activeIndex":0,"accounts":[{{
            "id":"{ACCOUNT_ID}","label":"Synthetic","token":"synthetic","addedAt":0,
            "modelRateLimits":null
          }}]}}}}]}}"#
    ));
    let provider = config.provider_config("codex").unwrap();
    assert_eq!(provider.model_rate_limits, None);
    assert_eq!(
        provider.token_accounts.as_ref().unwrap().accounts[0].model_rate_limits,
        None
    );
    assert!(
        !String::from_utf8(encode(&config))
            .unwrap()
            .contains("modelRateLimits")
    );
}

#[test]
fn structurally_invalid_limits_fail_the_decode() {
    let provider_level = r#"{"version":1,"providers":[{"id":"codex",
        "modelRateLimits":[{"model":"gpt-5","tokensPerMinute":"many"}]}]}"#;
    assert!(matches!(
        decode(provider_level.as_bytes()),
        Err(ConfigError::Decode { .. })
    ));

    let account_level = format!(
        r#"{{"version":1,"providers":[{{"id":"codex",
          "tokenAccounts":{{"version":1,"activeIndex":0,"accounts":[{{
            "id":"{ACCOUNT_ID}","label":"Synthetic","token":"synthetic","addedAt":0,
            "modelRateLimits":[{{"tokensPerMinute":100}}]
          }}]}}}}]}}"#
    );
    assert!(matches!(
        decode(account_level.as_bytes()),
        Err(ConfigError::Decode { .. })
    ));
}

#[test]
fn normalize_revalidates_limits_set_directly() {
    let mut provider = with_account(account(Some(unvalidated_rows())));
    provider.model_rate_limits = Some(unvalidated_rows());
    let normalized = normalize(single_provider(provider));
    let provider = normalized.provider_config("codex").unwrap();
    assert_eq!(provider.model_rate_limits, Some(expected_rows()));
    assert_eq!(
        provider.token_accounts.as_ref().unwrap().accounts[0].model_rate_limits,
        Some(expected_rows())
    );
}

#[test]
fn claude_config_directory_round_trips_inside_the_account() {
    let mut with_directory = account(None);
    with_directory.claude_config_directory = Some("/fixtures/claude-personal".to_string());
    let config = single_provider(with_account(with_directory.clone()));

    let json = encoded_json(&config);
    let written = &json["providers"][0]["tokenAccounts"]["accounts"][0];
    assert_eq!(
        written["claudeConfigDirectory"],
        "/fixtures/claude-personal"
    );

    let decoded = decode(&encode(&config)).unwrap().unwrap();
    let decoded_account = &decoded
        .provider_config("codex")
        .unwrap()
        .token_accounts
        .as_ref()
        .unwrap()
        .accounts[0];
    assert_eq!(decoded_account, &with_directory);
    assert!(!decoded_account.extra.contains_key("claudeConfigDirectory"));

    let without = encoded_json(&single_provider(with_account(account(None))));
    assert!(
        without["providers"][0]["tokenAccounts"]["accounts"][0]
            .get("claudeConfigDirectory")
            .is_none()
    );
}

#[test]
fn sharing_is_opt_in_and_legacy_configs_omit_the_flag_on_round_trip() {
    let mut config = decode_json(r#"{"version":1,"providers":[{"id":"codex","enabled":true}]}"#);
    assert!(!config.share_usage_with_cli);
    assert_eq!(config.provider_config("codex").unwrap().enabled, Some(true));
    for enabled in [false, true, false] {
        config.share_usage_with_cli = enabled;
        let data = encode(&config);
        assert_eq!(
            String::from_utf8(data.clone())
                .unwrap()
                .contains("shareUsageWithCLI"),
            enabled
        );
        assert_eq!(
            decode(&data).unwrap().unwrap().share_usage_with_cli,
            enabled
        );
    }
}

#[test]
fn explicit_false_sharing_is_dropped_and_invalid_values_fail() {
    let explicit_false = decode_json(r#"{"version":1,"providers":[],"shareUsageWithCLI":false}"#);
    assert!(!explicit_false.share_usage_with_cli);
    assert!(explicit_false.extra_top.is_empty());
    assert!(
        !String::from_utf8(encode(&explicit_false))
            .unwrap()
            .contains("shareUsageWithCLI")
    );

    let null = decode_json(r#"{"version":1,"providers":[],"shareUsageWithCLI":null}"#);
    assert!(!null.share_usage_with_cli);

    assert!(matches!(
        decode(br#"{"version":1,"providers":[],"shareUsageWithCLI":"yes"}"#),
        Err(ConfigError::Decode { .. })
    ));
}
