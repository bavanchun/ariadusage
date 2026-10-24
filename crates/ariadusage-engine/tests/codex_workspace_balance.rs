// Ported from CodexBar Tests/CodexBarTests/CodexWorkspaceBalanceTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::providers::codex::*;
use jiff::Timestamp;
use serde_json::json;
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
fn result(raw: &[u8], account: Option<&str>, include: bool) -> CodexFetchOutcome {
    CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(account),
        None,
        false,
        include,
        now(),
    )
    .unwrap()
}

#[path = "support/https.rs"]
mod https;
use ariadusage_engine::brokers::net::NetConfig;
use tempfile::TempDir;
fn fixture(
    account: Option<&str>,
    has: bool,
    unlimited: bool,
    balance: serde_json::Value,
) -> (CodexUsageResponse, CodexFetchOutcome) {
    let raw = serde_json::to_vec(&json!({"account_id":account,"plan_type":"business","rate_limit":{"primary_window":{"used_percent":10,"reset_at":1786161204,"limit_window_seconds":18000}},"credits":{"has_credits":has,"unlimited":unlimited,"balance":balance}})).unwrap();
    (
        CodexOAuthUsageFetcher::decode_usage_response(&raw).unwrap(),
        result(&raw, None, true),
    )
}
fn balance(raw: &str) -> Result<CodexWorkspaceRemainingBalanceResponse, CodexOAuthFetchError> {
    CodexOAuthUsageFetcher::decode_workspace_remaining_balance_response(raw.as_bytes())
}
fn credit_refresh(balance: f64, has_cap: bool, include: bool, missing: bool) -> CodexFetchOutcome {
    let mut raw = json!({"account_id":"workspace-fixture","plan_type":"business","credits":{"has_credits":true,"unlimited":false,"balance":if missing {serde_json::Value::Null} else {json!(balance)}}});
    if has_cap {
        raw["individual_limit"] = json!({"limit":400,"used":300});
    }
    result(&serde_json::to_vec(&raw).unwrap(), None, include)
}

// upstream: CodexWorkspaceBalanceTests.swift:106 deferred balance refresh preserves a workspace balance while a requested missing read invalidates it
#[test]
fn deferred_balance_refresh_preserves_a_workspace_balance_while_a_requested_missing_read_invalidates_it()
 {
    for include in [false, true] {
        for has_cap in [false, true] {
            let result = credit_refresh(0., has_cap, include, true);
            let fresh = result.credits.as_ref().unwrap();
            let retained = CreditsSnapshot::new(
                1234.,
                vec![],
                fresh
                    .updated_at
                    .checked_sub(Duration::from_secs(60))
                    .unwrap(),
            )
            .with_balance_is_workspace(true);
            let display = CodexExtraUsageCost::credits_for_display(
                Some(&retained),
                result.usage.provider_cost.as_ref(),
            )
            .unwrap();
            assert!(result.usage.primary.is_none());
            assert!(result.usage.secondary.is_none());
            assert!(!fresh.balance_read_succeeded);
            assert_eq!(
                fresh.credits_available,
                if include { Some(true) } else { None }
            );
            assert_eq!(
                fresh.codex_credit_limit.as_ref().map(|v| v.remaining()),
                if has_cap { Some(100.) } else { None }
            );
            assert_eq!(
                display.display_remaining(),
                if include {
                    if has_cap { Some(100.) } else { None }
                } else {
                    Some(1234.)
                }
            );
            assert_eq!(display.balance_read_succeeded, !include);
            assert_eq!(display.has_workspace_balance(), !include);
        }
    }
}

// upstream: CodexWorkspaceBalanceTests.swift:132 usage only numeric balances remain authoritative over an older workspace balance
#[test]
fn usage_only_numeric_balances_remain_authoritative_over_an_older_workspace_balance() {
    for value in [0., 14.] {
        for cap in [false, true] {
            let result = credit_refresh(value, cap, false, false);
            let fresh = result.credits.as_ref().unwrap();
            let retained = CreditsSnapshot::new(
                1234.,
                vec![],
                fresh
                    .updated_at
                    .checked_sub(Duration::from_secs(60))
                    .unwrap(),
            )
            .with_balance_is_workspace(true);
            let display = CodexExtraUsageCost::credits_for_display(
                Some(&retained),
                result.usage.provider_cost.as_ref(),
            )
            .unwrap();
            assert!(fresh.balance_read_succeeded);
            assert_eq!(fresh.credits_available, Some(true));
            assert_eq!(
                fresh.codex_credit_limit.as_ref().map(|v| v.remaining()),
                if cap { Some(100.) } else { None }
            );
            assert_eq!(display.remaining, value);
            assert!(display.balance_read_succeeded);
            assert!(!display.has_workspace_balance());
            assert_eq!(display.updated_at, fresh.updated_at);
            let cost = result.usage.provider_cost.as_ref().unwrap();
            assert_eq!(cost.balance, if value > 0. { Some(value) } else { None });
            assert_eq!(cost.balance_updated_at, Some(fresh.updated_at));
        }
    }
}

// upstream: CodexWorkspaceBalanceTests.swift:157 workspace enrichment skips ineligible usage without invoking the endpoint
#[tokio::test]
async fn workspace_enrichment_skips_ineligible_usage_without_invoking_the_endpoint() {
    for scenario in [
        "disabled",
        "unavailable",
        "unlimited",
        "known balance",
        "known zero",
        "missing account",
    ] {
        let (usage, original) = fixture(
            if scenario == "missing account" {
                None
            } else {
                Some("usage-account")
            },
            scenario != "unavailable",
            scenario == "unlimited",
            match scenario {
                "known balance" => json!(14),
                "known zero" => json!(0),
                _ => json!(null),
            },
        );
        let enriched = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
            original.clone(),
            &usage,
            &credentials(None),
            scenario != "disabled",
            now(),
            |_| async { panic!("Ineligible usage must not fetch the workspace balance") },
        )
        .await;
        assert_eq!(enriched, original);
    }
}

// upstream: CodexWorkspaceBalanceTests.swift:177 workspace enrichment requires a response account matching any credential scope
#[tokio::test]
async fn workspace_enrichment_requires_a_response_account_matching_any_credential_scope() {
    for account in [
        "credential-account",
        "usage-account",
        " usage-account ",
        "",
        "   ",
    ] {
        let (usage, original) = fixture(
            if account == "credential-account" {
                None
            } else {
                Some("usage-account")
            },
            true,
            false,
            json!(null),
        );
        let enriched = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
            original.clone(),
            &usage,
            &credentials(Some(account)),
            true,
            now(),
            |id| async move {
                assert_ne!(account, "credential-account");
                assert_eq!(id, "usage-account");
                balance(r#"{"balance":1234}"#)
            },
        )
        .await;
        if account == "credential-account" {
            assert_eq!(enriched, original);
        } else {
            let c = enriched.credits.unwrap();
            assert_eq!(c.remaining, 1234.);
            assert!(c.has_workspace_balance());
        }
    }
}

// upstream: CodexWorkspaceBalanceTests.swift:202 workspace enrichment rejects conflicting credential and response accounts
#[tokio::test]
async fn workspace_enrichment_rejects_conflicting_credential_and_response_accounts() {
    let (usage, original) = fixture(Some("response-workspace"), true, false, json!(null));
    let enriched = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
        original.clone(),
        &usage,
        &credentials(Some("different-workspace")),
        true,
        now(),
        |_| async { panic!("Conflicting account scopes must not fetch a balance") },
    )
    .await;
    assert_eq!(enriched.credits, original.credits);
    assert_eq!(enriched.usage.provider_cost, original.usage.provider_cost);
}

// upstream: CodexWorkspaceBalanceTests.swift:218 workspace endpoint HTTP failures retain the usable OAuth result
#[tokio::test]
async fn workspace_endpoint_http_failures_retain_the_usable_oauth_result() {
    for status in [401, 403, 404, 500] {
        let (usage, original) = fixture(Some("usage-account"), true, false, json!(null));
        let server = https::start_server();
        let mock = server
            .mock_async(|when, then| {
                when.path("/backend-api/accounts/usage-account/remaining_balance");
                then.status(status).body(r#"{"error":"fixture failure"}"#);
            })
            .await;
        let home = TempDir::new().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
        )
        .unwrap();
        let env = BTreeMap::from([(
            "CODEX_HOME".to_owned(),
            home.path().to_string_lossy().into_owned(),
        )]);
        let net = https::broker(NetConfig::default());
        let enriched = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
            original.clone(),
            &usage,
            &credentials(None),
            true,
            now(),
            |id| async move {
                CodexOAuthUsageFetcher::fetch_workspace_remaining_balance(
                    &net,
                    "fixture-access",
                    &id,
                    &env,
                    Some(home.path()),
                    Duration::from_secs(5),
                )
                .await
            },
        )
        .await;
        mock.assert_calls_async(1).await;
        assert_eq!(enriched, original);
    }
}

// upstream: CodexWorkspaceBalanceTests.swift:247 workspace network failure retains the usable OAuth result
#[tokio::test]
async fn workspace_network_failure_retains_the_usable_oauth_result() {
    let (usage, original) = fixture(Some("usage-account"), true, false, json!(null));
    let enriched = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
        original.clone(),
        &usage,
        &credentials(None),
        true,
        now(),
        |_| async { Err(CodexOAuthFetchError::NetworkError("timed out".into())) },
    )
    .await;
    assert_eq!(enriched, original);
}

// upstream: CodexWorkspaceBalanceTests.swift:260 missing or invalid workspace balances retain the original result
#[tokio::test]
async fn missing_or_invalid_workspace_balances_retain_the_original_result() {
    for raw in [
        r#"{}"#,
        r#"{"balance":null}"#,
        r#"{"balance":"NaN"}"#,
        r#"{"balance":"Infinity"}"#,
        r#"{"balance":{"amount":14}}"#,
        r#"{"balance":true}"#,
    ] {
        let (usage, original) = fixture(Some("usage-account"), true, false, json!(null));
        let enriched = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
            original.clone(),
            &usage,
            &credentials(None),
            true,
            now(),
            |_| async move { balance(raw) },
        )
        .await;
        assert_eq!(enriched, original);
    }
}

// upstream: CodexWorkspaceBalanceTests.swift:351 workspace URL encodes reserved account characters as one path component
#[test]
fn workspace_url_encodes_reserved_account_characters_as_one_path_component() {
    let url = CodexOAuthUsageFetcher::resolve_workspace_remaining_balance_url(
        &BTreeMap::new(),
        Some(r#"chatgpt_base_url = "https://chatgpt.com/backend-api""#),
        None,
        "acct/a b?#%",
    )
    .unwrap();
    assert_eq!(
        url.as_str(),
        "https://chatgpt.com/backend-api/accounts/acct%2Fa%20b%3F%23%25/remaining_balance"
    );
}

// upstream: CodexWorkspaceBalanceTests.swift:361 workspace URL respects a custom backend and rejects unsupported bases or empty accounts
#[test]
fn workspace_url_respects_a_custom_backend_and_rejects_unsupported_bases_or_empty_accounts() {
    let env = BTreeMap::new();
    let custom = CodexOAuthUsageFetcher::resolve_workspace_remaining_balance_url(
        &env,
        Some(r#"chatgpt_base_url = "https://example.com/backend-api/""#),
        None,
        "fixture-account",
    );
    assert_eq!(
        custom.as_ref().map(|v| v.as_str()),
        Some("https://example.com/backend-api/accounts/fixture-account/remaining_balance")
    );
    assert!(
        CodexOAuthUsageFetcher::resolve_workspace_remaining_balance_url(
            &env,
            Some(r#"chatgpt_base_url = "https://example.com/api/codex""#),
            None,
            "fixture-account"
        )
        .is_none()
    );
    assert!(
        CodexOAuthUsageFetcher::resolve_workspace_remaining_balance_url(
            &env,
            Some(r#"chatgpt_base_url = "https://chatgpt.com""#),
            None,
            ""
        )
        .is_none()
    );
}

// upstream: CodexWorkspaceBalanceTests.swift:295 successful zero workspace balance retains provenance monthly limit and result metadata
#[tokio::test]
async fn successful_zero_workspace_balance_retains_provenance_monthly_limit_and_result_metadata() {
    let (usage, mut original) = fixture(Some("usage-account"), true, false, json!(null));
    let limit = CodexCreditLimitSnapshot::new("Monthly credit limit", 40., 100., 60., None, now());
    let events = vec![CreditEvent::new(now(), "fixture-service", 2.)];
    original.credits = Some(CreditsSnapshot::with_details(
        0.,
        events.clone(),
        now(),
        Some(limit.clone()),
        false,
        Some(true),
        false,
    ));
    original.codex_reset_credits_attempted = true;
    original.codex_monthly_limit_enrichment_failed = true;
    let enriched = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
        original.clone(),
        &usage,
        &credentials(None),
        true,
        now(),
        |_| async { balance(r#"{"balance":0}"#) },
    )
    .await;
    let c = enriched.credits.as_ref().unwrap();
    assert_eq!(c.remaining, 0.);
    assert!(c.balance_read_succeeded);
    assert_eq!(c.credits_available, Some(true));
    assert!(c.has_workspace_balance());
    assert_eq!(c.display_remaining(), Some(0.));
    assert_eq!(c.codex_credit_limit, Some(limit));
    assert_eq!(c.events, events);
    assert_eq!(enriched.usage.primary, original.usage.primary);
    assert_eq!(enriched.usage.identity, original.usage.identity);
    let cost = enriched.usage.provider_cost.as_ref().unwrap();
    assert_eq!(cost.balance_is_workspace, Some(true));
    assert_eq!(cost.balance_updated_at, Some(c.updated_at));
    assert_eq!(enriched.source_label, original.source_label);
    assert_eq!(enriched.strategy_id, original.strategy_id);
    assert_eq!(enriched.strategy_kind, original.strategy_kind);
    assert!(enriched.codex_reset_credits_attempted);
    assert!(enriched.codex_monthly_limit_enrichment_failed);
}

// upstream: CodexWorkspaceBalanceTests.swift:281 workspace enrichment propagates cancellation
#[tokio::test]
async fn workspace_enrichment_propagates_cancellation() {
    use ariadusage_core::{
        pipeline::{FetchContext, FetchInteraction, FetchPhase, FetchRuntime},
        providers::SourceMode,
    };
    use ariadusage_protocol::ProviderId;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    let server = https::start_server();
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
    )
    .unwrap();
    let env = BTreeMap::from([(
        "CODEX_HOME".into(),
        home.path().to_string_lossy().into_owned(),
    )]);
    let _usage=server.mock_async(|when,then|{when.path("/backend-api/wham/usage");then.status(200).body(r####"{"account_id":"usage-account","plan_type":"business","rate_limit":{"primary_window":{"used_percent":10,"reset_at":1786161204,"limit_window_seconds":18000}},"credits":{"has_credits":true,"unlimited":false,"balance":null}}"####);}).await;
    let _reset = server
        .mock_async(|when, then| {
            when.path("/backend-api/wham/rate-limit-reset-credits");
            then.status(200)
                .body(r#"{"credits":[],"available_count":0}"#);
        })
        .await;
    let pending = server
        .mock_async(|when, then| {
            when.path("/backend-api/accounts/usage-account/remaining_balance");
            then.status(200).delay(Duration::from_secs(5)).body("{}");
        })
        .await;
    let mut config = CodexStrategyConfig::new(
        Arc::new(https::broker(NetConfig::default())),
        env,
        home.path(),
        home.path(),
    );
    config.clock = Arc::new(now);
    let strategy = CodexOAuthFetchStrategy::new(config);
    let cx = FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "fixture-cancel".into(),
        source_mode: SourceMode::Oauth,
        env: BTreeMap::new(),
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };
    let cancel = cx.cancel.clone();
    let fetch = strategy.fetch_with_credentials(&cx, credentials(None));
    let canceller = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while pending.calls_async().await == 0 {
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
    pending.assert_calls_async(1).await;
}
