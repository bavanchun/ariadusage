// Ported from CodexBar Tests/CodexBarTests/ConfigValidationTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::config::{
    Config, ProviderConfig, SecretPresence, TokenAccountMeta, TokenAccountsMeta, validate,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::ProviderId;

#[test]
fn test_allows_codex_api_source_without_config_api_key() {
    let mut config = Config::new(1, vec![]);
    let mut codex = ProviderConfig::new(ProviderId::new("codex").unwrap());
    codex.source = Some(SourceMode::Api);
    config.set_provider_config(codex);

    let issues = validate(&config, &SecretPresence::none());
    assert!(!issues.iter().any(
        |i| i.provider.as_ref().is_some_and(|p| p.as_str() == "codex")
            && i.code == "unsupported_source"
    ));
    assert!(!issues.iter().any(
        |i| i.provider.as_ref().is_some_and(|p| p.as_str() == "codex")
            && i.code == "api_key_missing"
    ));
}

#[test]
fn test_reports_unsupported_source() {
    let mut config = Config::new(1, vec![]);
    // Antigravity does not support web mode
    let mut agy = ProviderConfig::new(ProviderId::new("antigravity").unwrap());
    agy.source = Some(SourceMode::Web);
    config.set_provider_config(agy);

    let issues = validate(&config, &SecretPresence::none());
    assert!(issues.iter().any(|i| {
        i.provider
            .as_ref()
            .is_some_and(|p| p.as_str() == "antigravity")
            && i.code == "unsupported_source"
            && i.field.as_deref() == Some("source")
    }));
}

#[test]
fn test_reports_missing_api_key_when_source_api() {
    let mut config = Config::new(1, vec![]);
    // Claude requires API key when source is api
    let mut claude = ProviderConfig::new(ProviderId::new("claude").unwrap());
    claude.source = Some(SourceMode::Api);
    config.set_provider_config(claude);

    // Without secret presence -> emits api_key_missing
    let issues = validate(&config, &SecretPresence::none());
    assert!(issues.iter().any(|i| {
        i.provider.as_ref().is_some_and(|p| p.as_str() == "claude")
            && i.code == "api_key_missing"
            && i.field.as_deref() == Some("apiKey")
    }));

    // With secret presence configured -> does not emit api_key_missing
    let secrets = SecretPresence::none().with_api_key(ProviderId::new("claude").unwrap());
    let issues_with_key = validate(&config, &secrets);
    assert!(!issues_with_key.iter().any(|i| {
        i.provider.as_ref().is_some_and(|p| p.as_str() == "claude") && i.code == "api_key_missing"
    }));
}

#[test]
fn test_warns_on_unsupported_token_accounts() {
    let mut config = Config::new(1, vec![]);
    // Codex does not support token accounts
    let mut codex = ProviderConfig::new(ProviderId::new("codex").unwrap());
    codex.token_accounts = Some(TokenAccountsMeta {
        version: 1,
        active_index: 0,
        accounts: vec![TokenAccountMeta {
            id: "acc-1".to_string(),
            label: "Test".to_string(),
            added_at: 0.0,
            last_used: None,
            external_identifier: None,
            usage_scope: None,
            organization_id: None,
            workspace_id: None,
            seat_credit_entitlement: None,
            extra: Default::default(),
        }],
    });
    config.set_provider_config(codex);

    let issues = validate(&config, &SecretPresence::none());
    assert!(issues.iter().any(
        |i| i.provider.as_ref().is_some_and(|p| p.as_str() == "codex")
            && i.code == "token_accounts_unused"
            && i.field.as_deref() == Some("tokenAccounts")
    ));
}

#[test]
fn test_reports_unused_workspace() {
    let mut config = Config::new(1, vec![]);
    // Claude does not support workspaceID at provider level
    let mut claude = ProviderConfig::new(ProviderId::new("claude").unwrap());
    claude.workspace_id = Some("ws-project-1".to_string());
    config.set_provider_config(claude);

    let issues = validate(&config, &SecretPresence::none());
    assert!(issues.iter().any(|i| {
        i.provider.as_ref().is_some_and(|p| p.as_str() == "claude")
            && i.code == "workspace_unused"
            && i.field.as_deref() == Some("workspaceID")
    }));
}
