// Ported from CodexBar Tests/CodexBarTests/ProviderEnvironmentResolverTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;

use ariadusage_core::pipeline::*;
use ariadusage_protocol::secret::SecretString;

struct DummyConfig {
    api_key: Option<String>,
    workspace_id: Option<String>,
}

#[test]
fn test_selected_api_account_overrides_saved_and_ambient_credentials() {
    let mut base = BTreeMap::new();
    base.insert(
        "TEST_API_TOKEN".to_string(),
        SecretString::new("ambient-token"),
    );

    let config = DummyConfig {
        api_key: Some("saved-token".to_string()),
        workspace_id: None,
    };

    let account_token = SecretString::new("account-token");

    let descriptor = ProviderEnvDescriptor {
        projections: vec![EnvProjection {
            key: "TEST_API_TOKEN".to_string(),
            precedence: EnvPrecedence::Config,
            value_from_config: |c: &DummyConfig| c.api_key.clone(),
        }],
        account_projection: Some(AccountTokenProjection {
            scrub_keys: vec!["TEST_API_TOKEN".to_string()],
            target_key: Some("TEST_API_TOKEN".to_string()),
        }),
    };

    let env = ProviderEnvironmentResolver::resolve(
        &base,
        Some(&config),
        Some(&account_token),
        &descriptor,
    );

    assert_eq!(
        env.get("TEST_API_TOKEN").map(|s| s.expose_secret()),
        Some("account-token")
    );
}

#[test]
fn test_account_removes_project_scoping_from_saved_config() {
    let mut base = BTreeMap::new();
    base.insert(
        "ADMIN_API_KEY".to_string(),
        SecretString::new("ambient-token"),
    );
    base.insert(
        "PROJECT_ID".to_string(),
        SecretString::new("ambient-project"),
    );

    let config = DummyConfig {
        api_key: Some("saved-token".to_string()),
        workspace_id: Some("saved-project".to_string()),
    };

    let account_token = SecretString::new("sk-admin-account");

    let descriptor = ProviderEnvDescriptor {
        projections: vec![
            EnvProjection {
                key: "ADMIN_API_KEY".to_string(),
                precedence: EnvPrecedence::Config,
                value_from_config: |c: &DummyConfig| c.api_key.clone(),
            },
            EnvProjection {
                key: "PROJECT_ID".to_string(),
                precedence: EnvPrecedence::Config,
                value_from_config: |c: &DummyConfig| c.workspace_id.clone(),
            },
        ],
        account_projection: Some(AccountTokenProjection {
            scrub_keys: vec!["PROJECT_ID".to_string()],
            target_key: Some("ADMIN_API_KEY".to_string()),
        }),
    };

    let env = ProviderEnvironmentResolver::resolve(
        &base,
        Some(&config),
        Some(&account_token),
        &descriptor,
    );

    assert_eq!(
        env.get("ADMIN_API_KEY").map(|s| s.expose_secret()),
        Some("sk-admin-account")
    );
    assert_eq!(env.get("PROJECT_ID"), None);
}

#[test]
fn test_session_account_removes_conflicting_credentials() {
    let mut base = BTreeMap::new();
    base.insert(
        "ADMIN_API_KEY".to_string(),
        SecretString::new("ambient-admin"),
    );
    base.insert(
        "OAUTH_TOKEN".to_string(),
        SecretString::new("ambient-oauth"),
    );

    let config = DummyConfig {
        api_key: Some("saved-admin".to_string()),
        workspace_id: None,
    };

    let session_token = SecretString::new("session-token");

    let descriptor = ProviderEnvDescriptor {
        projections: vec![EnvProjection {
            key: "ADMIN_API_KEY".to_string(),
            precedence: EnvPrecedence::Config,
            value_from_config: |c: &DummyConfig| c.api_key.clone(),
        }],
        account_projection: Some(AccountTokenProjection {
            scrub_keys: vec!["ADMIN_API_KEY".to_string(), "OAUTH_TOKEN".to_string()],
            target_key: None,
        }),
    };

    let env = ProviderEnvironmentResolver::resolve(
        &base,
        Some(&config),
        Some(&session_token),
        &descriptor,
    );

    assert_eq!(env.get("ADMIN_API_KEY"), None);
    assert_eq!(env.get("OAUTH_TOKEN"), None);
}

#[test]
fn test_cookie_account_leaves_unrelated_environment_intact() {
    let mut base = BTreeMap::new();
    base.insert("FOO".to_string(), SecretString::new("bar"));

    let config = DummyConfig {
        api_key: None,
        workspace_id: None,
    };

    let account_token = SecretString::new("session=account");

    let descriptor = ProviderEnvDescriptor {
        projections: vec![],
        account_projection: Some(AccountTokenProjection {
            scrub_keys: vec!["OTHER_KEY".to_string()],
            target_key: None,
        }),
    };

    let env = ProviderEnvironmentResolver::resolve(
        &base,
        Some(&config),
        Some(&account_token),
        &descriptor,
    );

    assert_eq!(env.get("FOO").map(|s| s.expose_secret()), Some("bar"));
}

#[test]
fn test_environment_precedence_preserves_ambient_value() {
    let mut base = BTreeMap::new();
    base.insert(
        "API_KEY".to_string(),
        SecretString::new("ambient-preserved"),
    );

    let config = DummyConfig {
        api_key: Some("config-ignored".to_string()),
        workspace_id: None,
    };

    let descriptor = ProviderEnvDescriptor {
        projections: vec![EnvProjection {
            key: "API_KEY".to_string(),
            precedence: EnvPrecedence::Environment,
            value_from_config: |c: &DummyConfig| c.api_key.clone(),
        }],
        account_projection: None,
    };

    let env = ProviderEnvironmentResolver::resolve(&base, Some(&config), None, &descriptor);

    assert_eq!(
        env.get("API_KEY").map(|s| s.expose_secret()),
        Some("ambient-preserved")
    );
}
