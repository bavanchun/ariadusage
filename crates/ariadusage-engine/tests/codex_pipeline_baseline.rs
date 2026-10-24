// Ported from CodexBar Tests/CodexBarTests/CodexBaselineCharacterizationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

use ariadusage_core::{pipeline::*, providers::SourceMode};
use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::ProviderId;
use jiff::Timestamp;
use serde_json::json;
use std::{collections::BTreeMap, fs, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
#[path = "support/https.rs"]
mod https;
#[path = "support/codex_rpc.rs"]
mod rpc;
fn context(mode: SourceMode, runtime: FetchRuntime) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "fixture".into(),
        source_mode: mode,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}
fn config(home: &std::path::Path) -> CodexStrategyConfig {
    CodexStrategyConfig::new(
        Arc::new(https::broker(
            ariadusage_engine::brokers::net::NetConfig::default(),
        )),
        BTreeMap::from([
            ("CODEX_HOME".into(), home.to_string_lossy().into_owned()),
            ("PATH".into(), String::new()),
        ]),
        home,
        home,
    )
}
struct NoRetry;
impl Sleeper for NoRetry {
    fn sleep<'a>(&'a self, _: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async { panic!("No retry expected") })
    }
}
async fn outcome(cfg: CodexStrategyConfig, cx: &FetchContext) -> FetchOutcome {
    fetch_outcome(&cx.provider, &CodexProviderPlan::new(cfg), cx, &NoRetry).await
}

// upstream: CodexBaselineCharacterizationTests.swift:143 app auto pipeline order is PAT then OAuth then CLI without web
#[tokio::test]
async fn app_auto_pipeline_order_is_pat_then_oauth_then_cli_without_web() {
    let home = TempDir::new().unwrap();
    let plan = CodexProviderPlan::new(config(home.path()));
    let cx = context(SourceMode::Auto, FetchRuntime::Daemon);
    assert_eq!(
        plan.resolve_strategies(&cx)
            .await
            .iter()
            .map(|s| s.id())
            .collect::<Vec<_>>(),
        ["codex.pat", "codex.oauth", "codex.cli"]
    );
}

// upstream: CodexBaselineCharacterizationTests.swift:149 CLI auto pipeline order is PAT then OAuth then CLI without web
#[tokio::test]
async fn cli_auto_pipeline_order_is_pat_then_oauth_then_cli_without_web() {
    let home = TempDir::new().unwrap();
    let plan = CodexProviderPlan::new(config(home.path()));
    let cx = context(SourceMode::Auto, FetchRuntime::Cli);
    assert_eq!(
        plan.resolve_strategies(&cx)
            .await
            .iter()
            .map(|s| s.id())
            .collect::<Vec<_>>(),
        ["codex.pat", "codex.oauth", "codex.cli"]
    );
}

// upstream: CodexBaselineCharacterizationTests.swift:155 explicit fetch plan modes keep Codex strategy selection
#[tokio::test]
async fn explicit_fetch_plan_modes_keep_codex_strategy_selection() {
    let home = TempDir::new().unwrap();
    let plan = CodexProviderPlan::new(config(home.path()));
    for runtime in [FetchRuntime::Daemon, FetchRuntime::Cli] {
        for (mode, expected) in [
            (
                SourceMode::Oauth,
                vec!["codex.oauth", "codex.oauth-native-refresh-cli"],
            ),
            (SourceMode::Api, vec!["codex.pat"]),
            (SourceMode::Cli, vec!["codex.cli"]),
            (SourceMode::Web, vec!["codex.web.dashboard"]),
        ] {
            let cx = context(mode, runtime);
            assert_eq!(
                plan.resolve_strategies(&cx)
                    .await
                    .iter()
                    .map(|s| s.id())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
}

// upstream: CodexBaselineCharacterizationTests.swift:175 app auto records unavailable OAuth before successful CLI fallback
#[tokio::test]
async fn app_auto_records_unavailable_oauth_before_successful_cli_fallback() {
    let (home, fetcher) = rpc::fixture(
        json!({"result":{"rateLimits":{"primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":1766948068_i64},"secondary":{"usedPercent":43,"windowDurationMins":10080,"resetsAt":1767407914_i64},"credits":{"hasCredits":true,"unlimited":false,"balance":"7"}}}}),
        "pro",
    );
    let mut cfg = config(home.path());
    cfg.cli_fetcher = fetcher;
    let out = outcome(cfg, &context(SourceMode::Auto, FetchRuntime::Daemon)).await;
    assert_eq!(
        out.attempts
            .iter()
            .map(|a| a.strategy_id.as_str())
            .collect::<Vec<_>>(),
        ["codex.pat", "codex.oauth", "codex.cli"]
    );
    assert_eq!(
        out.attempts
            .iter()
            .map(|a| a.was_available)
            .collect::<Vec<_>>(),
        [false, false, true]
    );
    let r = out.result.unwrap();
    assert_eq!(r.source_label, "codex-cli");
    let id = r.usage.identity.unwrap();
    assert_eq!(id.account_email.as_deref(), Some("stub@example.com"));
    assert_eq!(id.login_method.as_deref(), Some("pro"));
}

// upstream: CodexBaselineCharacterizationTests.swift:204 app auto does not fall back from non auth failing OAuth
#[tokio::test]
async fn app_auto_does_not_fall_back_from_non_auth_failing_oauth() {
    let home = TempDir::new().unwrap();
    let cfg = config(home.path());
    CodexOAuthCredentialsStore::save(
        &CodexOAuthCredentials::new(
            "access-token",
            "refresh-token",
            None,
            Some("account-id".into()),
            Some(Timestamp::now()),
            None,
            CodexOAuthCredentialSource::CodexHome,
            false,
        ),
        &cfg.environment,
        Some(home.path()),
    )
    .unwrap();
    fs::write(
        home.path().join("config.toml"),
        "chatgpt_base_url = \"http://127.0.0.1:9\"",
    )
    .unwrap();
    let out = outcome(cfg, &context(SourceMode::Auto, FetchRuntime::Daemon)).await;
    assert_eq!(
        out.attempts
            .iter()
            .map(|a| a.strategy_id.as_str())
            .collect::<Vec<_>>(),
        ["codex.pat", "codex.oauth"]
    );
    assert_eq!(
        out.attempts
            .iter()
            .map(|a| a.was_available)
            .collect::<Vec<_>>(),
        [false, true]
    );
    assert!(out.attempts[1].failure.is_some());
    let FetchError::Classified(c) = out.result.unwrap_err() else {
        panic!("expected network error")
    };
    assert_eq!(
        c.kind,
        ariadusage_protocol::usage::ProviderErrorKind::NetworkFailure
    );
}

// upstream: CodexBaselineCharacterizationTests.swift:240 Codex CLI strategy fetches usage and credits with one app-server process
#[tokio::test]
async fn codex_cli_strategy_fetches_usage_and_credits_with_one_app_server_process() {
    let mut limits = json!({"credits":{"hasCredits":true,"unlimited":false,"balance":"7"}});
    limits["primary"] =
        json!({"usedPercent":12,"windowDurationMins":300,"resetsAt":1766948068_i64});
    limits["secondary"] =
        json!({"usedPercent":43,"windowDurationMins":10080,"resetsAt":1767407914_i64});
    let (home, fetcher) = rpc::fixture(json!({"result":{"rateLimits":limits}}), "pro");
    let mut cfg = config(home.path());
    cfg.cli_fetcher = fetcher;
    let path = home.path().join("fixture-codex");
    let original = fs::read_to_string(&path).unwrap();
    let count = home.path().join("counter");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' start >> '{}'\n{}",
            count.display(),
            original
        ),
    )
    .unwrap();
    let r = CodexCLIUsageStrategy::new(cfg)
        .fetch_outcome(true, Timestamp::now())
        .await
        .unwrap();
    assert_eq!(r.source_label, "codex-cli");
    assert_eq!(r.usage.primary.unwrap().used_percent, 12.);
    assert_eq!(r.credits.unwrap().remaining, 7.);
    assert_eq!(fs::read_to_string(count).unwrap().lines().count(), 1);
}

// upstream: CodexBaselineCharacterizationTests.swift:274 Codex CLI strategy keeps credits when rate limit windows are absent
#[tokio::test]
async fn codex_cli_strategy_keeps_credits_when_rate_limit_windows_are_absent() {
    let limits = json!({"credits":{"hasCredits":true,"unlimited":false,"balance":"7"}});
    let (home, fetcher) = rpc::fixture(json!({"result":{"rateLimits":limits}}), "pro");
    let mut cfg = config(home.path());
    cfg.cli_fetcher = fetcher;
    let r = CodexCLIUsageStrategy::new(cfg)
        .fetch_outcome(true, Timestamp::now())
        .await
        .unwrap();
    assert_eq!(r.source_label, "codex-cli");
    assert!(r.usage.primary.is_none());
    assert!(r.usage.secondary.is_none());
    assert_eq!(
        r.usage.identity.unwrap().account_email.as_deref(),
        Some("stub@example.com")
    );
    assert_eq!(r.credits.unwrap().remaining, 7.);
}

// upstream: CodexBaselineCharacterizationTests.swift:300 Codex CLI strategy maps monthly credit limit
#[tokio::test]
async fn codex_cli_strategy_maps_monthly_credit_limit() {
    let mut limits = json!({"credits":{"hasCredits":true,"unlimited":false,"balance":"7"}});
    limits["primary"] =
        json!({"usedPercent":12,"windowDurationMins":300,"resetsAt":1766948068_i64});
    limits["secondary"] =
        json!({"usedPercent":43,"windowDurationMins":10080,"resetsAt":1767407914_i64});
    limits["individualLimit"] =
        json!({"limit":100000,"used":7761,"remainingPercent":92.239,"resetsAt":1782864000_i64});
    let (home, fetcher) = rpc::fixture(json!({"result":{"rateLimits":limits}}), "pro");
    let mut cfg = config(home.path());
    cfg.cli_fetcher = fetcher;
    let r = CodexCLIUsageStrategy::new(cfg)
        .fetch_outcome(true, Timestamp::now())
        .await
        .unwrap();
    let cap = r.credits.unwrap().codex_credit_limit.unwrap();
    assert_eq!(cap.limit, 100_000.);
    assert_eq!(cap.used, 7761.);
    assert_eq!(cap.remaining(), 92239.);
    assert_eq!(cap.remaining_percent, 92.239);
    assert_eq!(
        cap.resets_at,
        Some(Timestamp::from_second(1_782_864_000).unwrap())
    );
}
