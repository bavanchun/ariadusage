// Ported from CodexBar Tests/CodexBarTests/CodexPATTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::{pipeline::*, providers::SourceMode};
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::{ProviderId, metric::SourceKind};
use jiff::Timestamp;
use std::{collections::BTreeMap, sync::Arc};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
fn context(mode: SourceMode) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "fixture".into(),
        source_mode: mode,
        env: BTreeMap::new(),
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}

// upstream: CodexPATTests.swift:24 OAuth parse ignores a PAT-only auth file
#[test]
fn oauth_parse_ignores_a_pat_only_auth_file() {
    assert_eq!(
        CodexOAuthCredentialsStore::parse(
            br#"{"OPENAI_API_KEY":null,"personal_access_token":"at-test-token"}"#,
            CodexOAuthCredentialSource::CodexHome
        )
        .unwrap_err(),
        CodexOAuthCredentialsError::MissingTokens
    );
}

// upstream: CodexPATTests.swift:76 auto usage strategy prefers PAT over OAuth
#[test]
fn auto_usage_strategy_prefers_pat_over_oauth() {
    for (selected, has_pat, expected) in [
        (CodexUsageDataSource::Auto, true, CodexUsageDataSource::Pat),
        (
            CodexUsageDataSource::Auto,
            false,
            CodexUsageDataSource::OAuth,
        ),
        (CodexUsageDataSource::Pat, false, CodexUsageDataSource::Pat),
    ] {
        assert_eq!(
            resolve_usage_strategy(selected, true, has_pat).data_source,
            expected
        );
    }
}

// upstream: CodexPATTests.swift:206 PAT usage mapping keeps pat source and does not request reset credits
#[test]
fn pat_usage_mapping_keeps_pat_source_and_does_not_request_reset_credits() {
    let raw=br#"{"plan_type":"team","rate_limit":{"primary_window":{"used_percent":68,"reset_at":1766948068,"limit_window_seconds":604800},"secondary_window":null}}"#;
    let whoami = CodexPATWhoami {
        email: Some("pat@example.com".into()),
        plan_type: Some("team".into()),
    };
    let r = CodexPATFetchStrategy::map_result_for_testing(raw, Some(&whoami), Timestamp::now())
        .unwrap();
    assert_eq!(r.source_label, "pat");
    assert_eq!(r.strategy_id, "codex.pat");
    assert_eq!(r.strategy_kind, SourceKind::Api);
    assert!(r.codex_reset_credits_attempted);
    assert!(r.codex_reset_credits.is_none());
    assert!(r.usage.primary.is_none());
    assert_eq!(r.usage.secondary.unwrap().used_percent, 68.);
    let id = r.usage.identity.unwrap();
    assert_eq!(id.account_email.as_deref(), Some("pat@example.com"));
    assert_eq!(id.login_method.as_deref(), Some("team"));
}

// upstream: CodexPATTests.swift:236 explicit PAT source does not include OAuth or CLI strategies
#[tokio::test]
async fn explicit_pat_source_does_not_include_oauth_or_cli_strategies() {
    let home = TempDir::new().unwrap();
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let plan = CodexProviderPlan::new(CodexStrategyConfig::new(
        net,
        BTreeMap::new(),
        home.path(),
        home.path(),
    ));
    let strategies = plan.resolve_strategies(&context(SourceMode::Api)).await;
    assert_eq!(
        strategies.iter().map(|s| s.id()).collect::<Vec<_>>(),
        ["codex.pat"]
    );
    assert_eq!(
        strategies.iter().map(|s| s.kind()).collect::<Vec<_>>(),
        [SourceKind::Api]
    );
}

// upstream: CodexPATTests.swift:321 explicit PAT source does not fall back, Auto does after an unusable PAT
#[test]
fn explicit_pat_source_does_not_fall_back_auto_does_after_an_unusable_pat() {
    let strategy = CodexPATFetchStrategy::new();
    let explicit = context(SourceMode::Api);
    let auto = context(SourceMode::Auto);
    let unauthorized = CodexOAuthFetchError::Unauthorized.into_fetch_error();
    let missing =
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::MissingTokens).into_fetch_error();
    let not_found =
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::NotFound).into_fetch_error();
    assert!(!strategy.should_fallback(&unauthorized, &explicit));
    assert!(!strategy.should_fallback(&missing, &explicit));
    assert!(strategy.should_fallback(&unauthorized, &auto));
    assert!(strategy.should_fallback(&not_found, &auto));
    assert!(strategy.should_fallback(&missing, &auto));
    assert!(!strategy.should_fallback(
        &CodexOAuthFetchError::InvalidResponse.into_fetch_error(),
        &auto
    ));
    assert!(!strategy.should_fallback(
        &CodexOAuthFetchError::ServerError(500, None).into_fetch_error(),
        &auto
    ));
}
