// Ported from CodexBar Tests/CodexBarTests/CodexOAuthManagedWorkspaceRecoveryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthResetCreditFetchTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::providers::codex::*;
use jiff::Timestamp;
use std::collections::BTreeMap;
use std::time::Duration;
fn now() -> Timestamp {
    Timestamp::from_second(1_786_000_000).unwrap()
}
fn credentials(account: Option<&str>) -> CodexOAuthCredentials {
    CodexOAuthCredentials::new(
        "fixture-access",
        "fixture-refresh",
        None,
        account.map(str::to_owned),
        Some(now()),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    )
}

#[path = "support/https.rs"]
mod https;
use ariadusage_core::pipeline::{FetchContext, FetchInteraction, FetchPhase, FetchRuntime};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::net::NetConfig;
use ariadusage_protocol::ProviderId;
use std::sync::Arc;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
fn context(mode: SourceMode) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "synthetic".into(),
        source_mode: mode,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}
fn config(home: &std::path::Path) -> CodexStrategyConfig {
    let net = Arc::new(https::broker(NetConfig::default()));
    CodexStrategyConfig::new(
        net,
        BTreeMap::from([
            ("CODEX_HOME".into(), home.to_string_lossy().into_owned()),
            ("PATH".into(), String::new()),
        ]),
        home,
        home,
    )
}

// upstream: CodexOAuthTests.swift:735 auto mode only falls back from O auth on auth failures
#[test]
fn auto_mode_only_falls_back_from_o_auth_on_auth_failures() {
    let mode = SourceMode::Auto;
    for error in [
        CodexOAuthError::Fetch(CodexOAuthFetchError::Unauthorized),
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::NotFound),
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::Unreadable),
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::MissingTokens),
        CodexOAuthError::Refresh(CodexRefreshError::Expired),
        CodexOAuthError::Refresh(CodexRefreshError::Revoked),
        CodexOAuthError::Refresh(CodexRefreshError::Reused),
    ] {
        assert!(CodexOAuthFetchStrategy::should_fallback_on_error(
            &error, mode
        ));
    }
    for error in [
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::ReadOnlySource),
        CodexOAuthError::NoRateLimitsFound,
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::DecodeFailed("bad json".into())),
        CodexOAuthError::Fetch(CodexOAuthFetchError::InvalidResponse),
        CodexOAuthError::Fetch(CodexOAuthFetchError::ServerError(
            500,
            Some("offline".into()),
        )),
        CodexOAuthError::Fetch(CodexOAuthFetchError::NetworkError(
            "notConnectedToInternet".into(),
        )),
        CodexOAuthError::Refresh(CodexRefreshError::NetworkError("timedOut".into())),
    ] {
        assert!(!CodexOAuthFetchStrategy::should_fallback_on_error(
            &error, mode
        ));
    }
}

// upstream: CodexOAuthTests.swift:787 explicit O auth mode only falls back to CLI for native refresh recovery
#[test]
fn explicit_o_auth_mode_only_falls_back_to_cli_for_native_refresh_recovery() {
    for (error, expected) in [
        (
            CodexOAuthError::Fetch(CodexOAuthFetchError::Unauthorized),
            false,
        ),
        (
            CodexOAuthError::Credentials(CodexOAuthCredentialsError::NativeRefreshRequired),
            true,
        ),
        (
            CodexOAuthError::Credentials(CodexOAuthCredentialsError::ReadOnlySource),
            false,
        ),
        (CodexOAuthError::Refresh(CodexRefreshError::Expired), false),
    ] {
        assert_eq!(
            CodexOAuthFetchStrategy::should_fallback_on_error(&error, SourceMode::Oauth),
            expected
        );
    }
}

// upstream: CodexOAuthTests.swift:798 credential recovery errors direct users to codex login
#[test]
fn credential_recovery_errors_direct_users_to_codex_login() {
    for description in [
        CodexOAuthCredentialsError::NativeRefreshRequired.to_string(),
        CodexOAuthCredentialsError::ReadOnlySource.to_string(),
        CodexOAuthFetchError::Unauthorized.to_string(),
    ] {
        assert!(description.contains("codex login"));
    }
}

// upstream: CodexOAuthTests.swift:805 explicit O auth mode includes CLI recovery after native credentials expire
#[test]
fn explicit_o_auth_mode_includes_cli_recovery_after_native_credentials_expire() {
    let home = TempDir::new().unwrap();
    let plan = CodexProviderPlan::new(config(home.path()));
    assert_eq!(
        plan.resolve_strategy_ids(SourceMode::Oauth),
        vec!["codex.oauth", "codex.oauth-native-refresh-cli"]
    );
}

// upstream: CodexOAuthTests.swift:813 native refresh recovery is unavailable when Codex CLI is missing
#[test]
fn native_refresh_recovery_is_unavailable_when_codex_cli_is_missing() {
    let home = TempDir::new().unwrap();
    let c = CodexOAuthCredentials::new(
        "access-token",
        "refresh-token",
        None,
        Some("account-id".into()),
        Some(
            Timestamp::now()
                .checked_sub(Duration::from_secs(9 * 24 * 60 * 60))
                .unwrap(),
        ),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let mut cfg = config(home.path());
    cfg.environment
        .insert("CODEX_CLI_PATH".into(), "/missing/codex".into());
    CodexOAuthCredentialsStore::save(&c, &cfg.environment, Some(home.path())).unwrap();
    let strategy = CodexOAuthNativeRefreshCLIStrategy::new(cfg).with_binary_resolver(|| false);
    assert!(!strategy.is_available_sync(SourceMode::Oauth));
}

// upstream: CodexOAuthTests.swift:848 resolves chat GPT usage URL from config
#[test]
fn resolves_chat_gpt_usage_url_from_config() {
    let config = r#"chatgpt_base_url = "https://chatgpt.com/backend-api/""#;
    assert_eq!(
        CodexOAuthUsageFetcher::resolve_usage_url(&BTreeMap::new(), Some(config), None).as_str(),
        "https://chatgpt.com/backend-api/wham/usage"
    );
}

// upstream: CodexOAuthTests.swift:855 resolves codex usage URL from config
#[test]
fn resolves_codex_usage_url_from_config() {
    let config = r#"chatgpt_base_url = "https://api.openai.com""#;
    assert_eq!(
        CodexOAuthUsageFetcher::resolve_usage_url(&BTreeMap::new(), Some(config), None).as_str(),
        "https://api.openai.com/api/codex/usage"
    );
}

// upstream: CodexOAuthTests.swift:862 normalizes chat GPT base URL without backend API
#[test]
fn normalizes_chat_gpt_base_url_without_backend_api() {
    let config = r#"chatgpt_base_url = "https://chat.openai.com""#;
    assert_eq!(
        CodexOAuthUsageFetcher::resolve_usage_url(&BTreeMap::new(), Some(config), None).as_str(),
        "https://chat.openai.com/backend-api/wham/usage"
    );
}

// upstream: CodexOAuthResetCreditFetchTests.swift:6 app enrichment can rescue reset-credit-only O auth usage
#[test]
fn app_enrichment_can_rescue_reset_credit_only_o_auth_usage() {
    let raw = br#"{"rate_limit":{"primary_window":null,"secondary_window":null}}"#;
    let r = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(Some("account-123")),
        None,
        true,
        true,
        now(),
    )
    .unwrap();
    assert!(r.usage.primary.is_none());
    assert!(r.usage.secondary.is_none());
    assert!(r.codex_reset_credits.is_none());
    assert!(r.credits.is_none());
    assert_eq!(r.strategy_id, "codex.oauth");
}

// upstream: CodexOAuthResetCreditFetchTests.swift:61 reset credit inventory only O auth payload still returns usage result
#[test]
fn reset_credit_inventory_only_o_auth_payload_still_returns_usage_result() {
    let raw = br#"{"rate_limit":{"primary_window":null,"secondary_window":null}}"#;
    let reset = CodexRateLimitResetCreditsSnapshot::new(
        vec![CodexRateLimitResetCredit::new(
            "available-no-expiry",
            "codex_rate_limits",
            CodexRateLimitResetCreditStatus::Available,
            now(),
            None,
            None,
            None,
            None,
            None,
        )],
        1,
        now(),
    );
    let r = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(Some("account-123")),
        Some(reset),
        false,
        true,
        now(),
    )
    .unwrap();
    assert!(r.usage.primary.is_none());
    assert!(r.usage.secondary.is_none());
    assert_eq!(
        r.codex_reset_credits
            .unwrap()
            .available_inventory(now())
            .count(),
        1
    );
    assert!(r.credits.is_none());
    assert_eq!(r.source_label, "oauth");
}

// upstream: CodexOAuthResetCreditFetchTests.swift:93 empty reset credits do not mask missing O auth usage
#[test]
fn empty_reset_credits_do_not_mask_missing_o_auth_usage() {
    let reset = CodexRateLimitResetCreditsSnapshot::new(vec![], 0, now());
    assert!(matches!(
        CodexOAuthFetchStrategy::map_result_for_testing(
            br#"{"rate_limit":{"primary_window":null,"secondary_window":null}}"#,
            &credentials(Some("account-123")),
            Some(reset),
            false,
            true,
            now()
        ),
        Err(CodexOAuthError::NoRateLimitsFound)
    ));
}

// upstream: CodexOAuthResetCreditFetchTests.swift:109 O auth strategy fetches app inventory and CLI follows credits flag
#[test]
fn o_auth_strategy_fetches_app_inventory_and_cli_follows_credits_flag() {
    let mut app = context(SourceMode::Auto);
    app.runtime = FetchRuntime::Daemon;
    app.include_credits = false;
    let mut cli = context(SourceMode::Auto);
    cli.include_credits = false;
    assert!(CodexOAuthFetchStrategy::should_fetch_reset_credits(&app));
    assert!(!CodexOAuthFetchStrategy::should_fetch_reset_credits(&cli));
    cli.include_credits = true;
    assert!(CodexOAuthFetchStrategy::should_fetch_reset_credits(&cli));
}

// upstream: CodexOAuthResetCreditFetchTests.swift:21 app uses winning OAuth credentials while CLI follows credits flag
#[tokio::test]
async fn app_uses_winning_oauth_credentials_while_cli_follows_credits_flag() {
    let server = https::start_server();
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
    )
    .unwrap();
    let mut cfg = config(home.path());
    cfg.clock = Arc::new(now);
    let strategy = CodexOAuthFetchStrategy::new(cfg);
    let _usage=server.mock_async(|w,t|{w.path("/backend-api/wham/usage").header("authorization","Bearer fixture-access").header("chatgpt-account-id","account-123");t.status(200).body(r#"{"rate_limit":{"primary_window":{"used_percent":22,"reset_at":4102444800,"limit_window_seconds":18000}}}"#);}).await;
    let reset = server
        .mock_async(|w, t| {
            w.path("/backend-api/wham/rate-limit-reset-credits")
                .header("authorization", "Bearer fixture-access")
                .header("chatgpt-account-id", "account-123");
            t.status(500);
        })
        .await;
    for (runtime, count) in [(FetchRuntime::Daemon, 1), (FetchRuntime::Cli, 2)] {
        let mut cx = context(SourceMode::Auto);
        cx.include_credits = true;
        cx.runtime = runtime;
        let r = strategy
            .fetch_with_credentials(&cx, credentials(Some("account-123")))
            .await
            .unwrap();
        assert!(r.codex_reset_credits.is_none());
        reset.assert_calls_async(count).await;
    }
}

// upstream: CodexOAuthResetCreditFetchTests.swift:45 CLI reset credit GET preserves cancellation without retry
#[tokio::test]
async fn cli_reset_credit_get_preserves_cancellation_without_retry() {
    let server = https::start_server();
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
    )
    .unwrap();
    let mut cfg = config(home.path());
    cfg.clock = Arc::new(now);
    let strategy = CodexOAuthFetchStrategy::new(cfg);
    let _usage=server.mock_async(|w,t|{w.path("/backend-api/wham/usage").header("authorization","Bearer fixture-access").header("chatgpt-account-id","account-123");t.status(200).body(r#"{"rate_limit":{"primary_window":{"used_percent":22,"reset_at":4102444800,"limit_window_seconds":18000}}}"#);}).await;
    let reset = server
        .mock_async(|w, t| {
            w.path("/backend-api/wham/rate-limit-reset-credits");
            t.status(200).delay(Duration::from_secs(5)).body("{}");
        })
        .await;
    let mut cx = context(SourceMode::Auto);
    cx.include_credits = true;
    let cancel = cx.cancel.clone();
    let fetch = strategy.fetch_with_credentials(&cx, credentials(Some("account-123")));
    let canceller = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while reset.calls_async().await == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
    };
    let (outcome, ()) = tokio::join!(fetch, canceller);
    assert!(matches!(
        outcome,
        Err(CodexOAuthError::Fetch(CodexOAuthFetchError::Cancelled))
    ));
    reset.assert_calls_async(1).await;
}

// upstream: CodexOAuthManagedWorkspaceRecoveryTests.swift:7 automatic mode does not expose unscoped CLI fallback for a managed workspace
#[tokio::test]
async fn automatic_mode_does_not_expose_unscoped_cli_fallback_for_a_managed_workspace() {
    use ariadusage_core::pipeline::ProviderPlan;
    let home = TempDir::new().unwrap();
    let mut cfg = config(home.path());
    cfg.managed_workspace_account_id = Some("workspace-team".into());
    let strategies = CodexProviderPlan::new(cfg)
        .resolve_strategies(&context(SourceMode::Auto))
        .await;
    assert_eq!(
        strategies.iter().map(|s| s.id()).collect::<Vec<_>>(),
        ["codex.pat", "codex.oauth"]
    );
}

// upstream: CodexOAuthManagedWorkspaceRecoveryTests.swift:15 native refresh recovery is unavailable when managed workspace scope is selected
#[test]
fn native_refresh_recovery_is_unavailable_when_managed_workspace_scope_is_selected() {
    let home = TempDir::new().unwrap();
    let mut cfg = config(home.path());
    cfg.managed_workspace_account_id = Some("workspace-team".into());
    let c = CodexOAuthCredentials::new(
        "access-token",
        "refresh-token",
        None,
        Some("auth-account".into()),
        Some(
            Timestamp::now()
                .checked_sub(Duration::from_secs(9 * 24 * 60 * 60))
                .unwrap(),
        ),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    CodexOAuthCredentialsStore::save(&c, &cfg.environment, Some(home.path())).unwrap();
    assert!(
        !CodexOAuthNativeRefreshCLIStrategy::new(cfg)
            .with_binary_resolver(|| true)
            .is_available_sync(SourceMode::Oauth)
    );
}
