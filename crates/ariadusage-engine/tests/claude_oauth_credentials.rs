// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;

use ariadusage_core::oauth::TokenOwner;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::providers::claude::{
    CLAUDE_OAUTH_SCOPES_ENV_KEY, CLAUDE_OAUTH_TOKEN_ENV_KEY, ClaudeOAuthCredentialSource,
    ClaudeOAuthCredentials, ClaudeOAuthCredentialsError, credentials_profile_identifier,
    load_from_credentials_file, load_from_environment,
};
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use tokio_util::sync::CancellationToken;

#[test]
fn parses_oauth_credentials_and_redacts_debug() {
    let raw = br#"{
        "claudeAiOauth": {
            "accessToken": "sk-ant-oat01-tok-1",
            "refreshToken": "sk-ant-ort01-ref-1",
            "expiresAt": 4102444800000.0,
            "scopes": ["usage:read", "user:profile"],
            "rateLimitTier": "default_claude_max_20x",
            "subscriptionType": "pro"
        }
    }"#;

    let creds = ClaudeOAuthCredentials::parse(raw).expect("parse credentials");
    assert_eq!(creds.access_token.expose_secret(), "sk-ant-oat01-tok-1");
    assert!(creds.has_refresh_token());
    assert_eq!(
        creds.scopes,
        vec!["usage:read".to_owned(), "user:profile".to_owned()]
    );
    assert!(creds.has_scope("user:profile"));
    assert_eq!(
        creds.rate_limit_tier.as_deref(),
        Some("default_claude_max_20x")
    );
    assert_eq!(creds.subscription_type.as_deref(), Some("pro"));

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    assert!(!creds.is_expired_at(now));

    let future_now = Timestamp::from_second(4_200_000_000).unwrap();
    assert!(creds.is_expired_at(future_now));

    let debug_text = format!("{creds:?}");
    assert!(!debug_text.contains("sk-ant-oat01-tok-1"));
    assert!(!debug_text.contains("sk-ant-ort01-ref-1"));
}

#[test]
fn missing_access_token_and_missing_oauth_block_fail() {
    assert_eq!(
        ClaudeOAuthCredentials::parse(b"not-json"),
        Err(ClaudeOAuthCredentialsError::DecodeFailed)
    );
    assert_eq!(
        ClaudeOAuthCredentials::parse(br#"{ "other": { "accessToken": "nope" } }"#),
        Err(ClaudeOAuthCredentialsError::MissingOAuth)
    );
    assert_eq!(
        ClaudeOAuthCredentials::parse(
            br#"{
                "claudeAiOauth": {
                    "accessToken": "",
                    "refreshToken": "test-refresh",
                    "expiresAt": 1735689600000
                }
            }"#
        ),
        Err(ClaudeOAuthCredentialsError::MissingAccessToken)
    );
}

#[test]
fn detects_and_rejects_mcp_oauth_only_keychain_payload() {
    let raw = br#"{
        "mcpOAuth": {
            "plugin:slack:slack": {
                "accessToken": ""
            }
        }
    }"#;

    assert!(ClaudeOAuthCredentials::is_mcp_oauth_only_payload(raw));
    assert_eq!(
        ClaudeOAuthCredentials::parse(raw),
        Err(ClaudeOAuthCredentialsError::McpOAuthOnly)
    );
}

#[test]
fn treats_missing_expiry_as_expired() {
    let creds = ClaudeOAuthCredentials {
        access_token: SecretString::new("token"),
        refresh_token: None,
        expires_at: None,
        scopes: Vec::new(),
        rate_limit_tier: None,
        subscription_type: None,
    };
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    assert!(creds.is_expired_at(now));
}

#[test]
fn computes_history_owner_identifier_preferring_refresh_token_over_access_token() {
    let creds_a = ClaudeOAuthCredentials::parse(
        br#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-rot-1",
                "refreshToken": "sk-ant-ort01-ref-1"
            }
        }"#,
    )
    .unwrap();
    let creds_b = ClaudeOAuthCredentials::parse(
        br#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-rot-2",
                "refreshToken": "sk-ant-ort01-ref-1"
            }
        }"#,
    )
    .unwrap();
    let creds_access_only = ClaudeOAuthCredentials::parse(
        br#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-rot-1"
            }
        }"#,
    )
    .unwrap();

    let id_a = creds_a.history_owner_identifier().expect("owner id a");
    let id_b = creds_b.history_owner_identifier().expect("owner id b");
    let id_access = creds_access_only
        .history_owner_identifier()
        .expect("owner id access");

    assert_eq!(id_a.len(), 64);
    assert_eq!(id_a, id_b);
    assert_ne!(id_a, id_access);
    assert_eq!(
        ClaudeOAuthCredentials::normalized_history_owner_identifier(Some(&id_a.to_uppercase())),
        Some(id_a)
    );
    assert_eq!(
        ClaudeOAuthCredentials::normalized_history_owner_identifier(Some("short")),
        None
    );
}

#[test]
fn loads_credentials_from_environment_with_default_and_custom_scopes() {
    let mut env = BTreeMap::new();
    env.insert(
        CLAUDE_OAUTH_TOKEN_ENV_KEY.to_owned(),
        SecretString::new("  sk-ant-oat01-env-1  "),
    );

    let record = load_from_environment(&env).expect("env record");
    assert_eq!(record.owner, TokenOwner::Environment);
    assert_eq!(record.source, ClaudeOAuthCredentialSource::Environment);
    assert_eq!(
        record.credentials.access_token.expose_secret(),
        "sk-ant-oat01-env-1"
    );
    assert_eq!(record.credentials.scopes, vec!["user:profile".to_owned()]);

    env.insert(
        CLAUDE_OAUTH_SCOPES_ENV_KEY.to_owned(),
        SecretString::new("user:inference, user:profile "),
    );
    let custom = load_from_environment(&env).expect("env record with custom scopes");
    assert_eq!(
        custom.credentials.scopes,
        vec!["user:inference".to_owned(), "user:profile".to_owned()]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn reads_credentials_file_via_broker_and_computes_profile_identifier() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join(".credentials.json");
    std::fs::write(
        &cred_path,
        br#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-file-1",
                "refreshToken": "sk-ant-ort01-file-1",
                "expiresAt": 1893456000000.0,
                "scopes": ["user:profile"]
            }
        }"#,
    )
    .unwrap();

    let call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "req-1".to_owned(),
    };

    let record = load_from_credentials_file(&cred_path, &call).expect("load from file");
    assert_eq!(record.owner, TokenOwner::Cli);
    assert_eq!(record.source, ClaudeOAuthCredentialSource::CredentialsFile);
    assert!(record.stat_fingerprint.is_some());

    let profile_id = credentials_profile_identifier(&cred_path);
    assert_eq!(profile_id.len(), 64);
}
