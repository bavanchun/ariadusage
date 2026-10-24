// Ported from CodexBar Tests/CodexBarTests/CodexSpendControlsMonthlyUsageTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

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
fn education(
    plan: Option<&str>,
    account: Option<&str>,
    spend: bool,
    limit: serde_json::Value,
    hidden: bool,
) -> Vec<u8> {
    let mut raw = json!({"rate_limit":{"primary_window":{"used_percent":10,"reset_at":1786161204,"limit_window_seconds":18000},"secondary_window":null},"credits":{"has_credits":true,"unlimited":false,"balance":if hidden {json!(null)} else {json!("14")}}});
    if let Some(p) = plan {
        raw["plan_type"] = json!(p);
    }
    if let Some(a) = account {
        raw["account_id"] = json!(a);
    }
    if spend {
        raw["spend_control"] = json!({"reached":false,"individual_limit":limit});
    }
    serde_json::to_vec(&raw).unwrap()
}
fn education_default() -> Vec<u8> {
    education(
        Some("education"),
        Some("acct-123"),
        true,
        json!(null),
        false,
    )
}
fn usage(raw: &[u8]) -> CodexUsageResponse {
    CodexOAuthUsageFetcher::decode_usage_response(raw).unwrap()
}
fn monthly(used: &str, limit: &str, mode: Option<&str>) -> CodexSpendControlsMonthlyUsageResponse {
    let mode = mode
        .map(|m| format!(",\"enforcement_mode\":\"{m}\""))
        .unwrap_or_default();
    decode(&format!(
        r#"{{"current_month_usage":{used},"effective_monthly_limit":{{"limit":{limit}{mode},"limit_mode":"amount_credits"}}}}"#
    ))
}
fn decode(raw: &str) -> CodexSpendControlsMonthlyUsageResponse {
    CodexOAuthUsageFetcher::decode_spend_controls_monthly_usage_response(raw.as_bytes()).unwrap()
}
fn payload() -> CodexSpendControlsMonthlyUsageResponse {
    monthly("3046.4506806135178", "7000", Some("HARD_CAP"))
}
fn malformed(mode: &str) -> CodexSpendControlsMonthlyUsageResponse {
    monthly(r#"12"#, r#"{"amount":7000}"#, Some(mode))
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:121 education usage fixture gates monthly endpoint and maps its response
#[test]
fn education_usage_fixture_gates_monthly_endpoint_and_maps_its_response() {
    let usage = usage(&education_default());
    assert_eq!(usage.account_id.as_deref(), Some("acct-123"));
    assert!(usage.resolved_individual_limit().is_none());
    assert!(usage.spend_control_present);
    assert!(CodexSpendControlsMonthlyUsageGate::should_fetch(&usage));
    let snap = payload().codex_credit_limit_snapshot(now()).unwrap();
    assert!((snap.used - 3046.4506806135178).abs() < 0.000_001);
    assert_eq!(snap.limit, 7000.);
    assert!((snap.remaining_percent - 56.479_276_0).abs() < 0.000_001);
    assert!(snap.resets_at.is_none());
    assert_eq!(snap.updated_at, now());
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:141 active and unknown enforcement modes produce a monthly limit
#[test]
fn active_and_unknown_enforcement_modes_produce_a_monthly_limit() {
    for mode in [
        Some("HARD_CAP"),
        Some("SOFT_CAP"),
        Some("future_mode"),
        None,
    ] {
        assert!(
            monthly("3046.4506806135178", "7000", mode)
                .codex_credit_limit_snapshot(now())
                .is_some()
        );
    }
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:152 inactive enforcement modes suppress the monthly limit case insensitively
#[test]
fn inactive_enforcement_modes_suppress_the_monthly_limit_case_insensitively() {
    for mode in ["NONE", "disabled", "OFF", "no_limit"] {
        assert!(
            monthly("3046.4506806135178", "7000", Some(mode))
                .codex_credit_limit_snapshot(now())
                .is_none()
        );
    }
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:160 missing or zero limit suppresses the monthly snapshot
#[test]
fn missing_or_zero_limit_suppresses_the_monthly_snapshot() {
    let zero = monthly("3046.4506806135178", "0", Some("HARD_CAP"));
    let missing = decode(
        r#"{"current_month_usage":12,"effective_monthly_limit":{"enforcement_mode":"HARD_CAP"}}"#,
    );
    for v in [zero, missing] {
        assert!(v.codex_credit_limit_snapshot(now()).is_none());
        assert!(!v.monthly_limit_mapping_failed());
    }
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:172 unmappable nested limit is an enrichment failure not confirmed absence
#[test]
fn unmappable_nested_limit_is_an_enrichment_failure_not_confirmed_absence() {
    let v = malformed("HARD_CAP");
    assert!(v.codex_credit_limit_snapshot(now()).is_none());
    assert!(v.monthly_limit_mapping_failed());
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:180 disabled enforcement wins over an unmappable nested limit
#[test]
fn disabled_enforcement_wins_over_an_unmappable_nested_limit() {
    let v = malformed("disabled");
    assert!(v.codex_credit_limit_snapshot(now()).is_none());
    assert!(!v.monthly_limit_mapping_failed());
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:189 unmappable current month usage is an enrichment failure
#[test]
fn unmappable_current_month_usage_is_an_enrichment_failure() {
    let v = monthly(r#"{"amount":12}"#, "7000", Some("HARD_CAP"));
    assert_eq!(v.codex_credit_limit_snapshot(now()).unwrap().limit, 7000.);
    assert!(v.monthly_limit_mapping_failed());
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:198 absent cap overrides malformed current month usage
#[test]
fn absent_cap_overrides_malformed_current_month_usage() {
    let v = decode(r#"{"current_month_usage":{"amount":12}}"#);
    assert!(v.codex_credit_limit_snapshot(now()).is_none());
    assert!(!v.monthly_limit_mapping_failed());
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:206 unmappable enforcement mode is an enrichment failure
#[test]
fn unmappable_enforcement_mode_is_an_enrichment_failure() {
    let v = decode(
        r#"{"current_month_usage":12,"effective_monthly_limit":{"limit":7000,"enforcement_mode":{"mode":"HARD_CAP"}}}"#,
    );
    assert_eq!(v.codex_credit_limit_snapshot(now()).unwrap().limit, 7000.);
    assert!(v.monthly_limit_mapping_failed());
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:223 zero limit confirms absence even with unmappable enforcement
#[test]
fn zero_limit_confirms_absence_even_with_unmappable_enforcement() {
    let v = decode(
        r#"{"current_month_usage":12,"effective_monthly_limit":{"limit":0,"enforcement_mode":{"mode":"HARD_CAP"}}}"#,
    );
    assert!(v.codex_credit_limit_snapshot(now()).is_none());
    assert!(!v.monthly_limit_mapping_failed());
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:240 non-finite usage and limit strings are mapping failures
#[test]
fn non_finite_usage_and_limit_strings_are_mapping_failures() {
    let v = monthly(r#""NaN""#, "7000", Some("HARD_CAP"));
    assert_eq!(v.codex_credit_limit_snapshot(now()).unwrap().limit, 7000.);
    assert!(v.monthly_limit_mapping_failed());
    let v = monthly("3046.4506806135178", r#""Infinity""#, Some("HARD_CAP"));
    assert!(v.codex_credit_limit_snapshot(now()).is_none());
    assert!(v.monthly_limit_mapping_failed());
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:251 monthly usage clamps negative usage and accepts numeric strings
#[test]
fn monthly_usage_clamps_negative_usage_and_accepts_numeric_strings() {
    let snap = monthly(r#""-20""#, r#""7000""#, Some("HARD_CAP"))
        .codex_credit_limit_snapshot(now())
        .unwrap();
    assert_eq!(snap.used, 0.);
    assert_eq!(snap.remaining_percent, 100.);
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:260 consumer plans never gate the monthly endpoint
#[test]
fn consumer_plans_never_gate_the_monthly_endpoint() {
    for plan in ["plus", "pro", "free"] {
        assert!(!CodexSpendControlsMonthlyUsageGate::should_fetch(&usage(
            &education(Some(plan), Some("acct-123"), true, json!(null), false)
        )));
    }
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:268 existing education individual limit prevents a second monthly limit request
#[test]
fn existing_education_individual_limit_prevents_a_second_monthly_limit_request() {
    let u = usage(&education(
        Some("education"),
        Some("acct-123"),
        true,
        json!({"limit":7000,"used":1000,"remaining_percent":85}),
        false,
    ));
    assert_eq!(u.resolved_individual_limit().unwrap().limit, Some(7000.));
    assert!(!CodexSpendControlsMonthlyUsageGate::should_fetch(&u));
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:277 education without spend control key does not gate the monthly endpoint
#[test]
fn education_without_spend_control_key_does_not_gate_the_monthly_endpoint() {
    let u = usage(&education(
        Some("education"),
        Some("acct-123"),
        false,
        json!(null),
        false,
    ));
    assert!(!u.spend_control_present);
    assert!(!CodexSpendControlsMonthlyUsageGate::should_fetch(&u));
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:285 missing or unknown plan still fetches when spend controls are present
#[test]
fn missing_or_unknown_plan_still_fetches_when_spend_controls_are_present() {
    let u = usage(&education(None, Some("acct-123"), true, json!(null), false));
    assert!(u.plan_type.is_none());
    assert!(CodexSpendControlsMonthlyUsageGate::should_fetch(&u));
    let u = usage(&education(
        Some("not-a-plan"),
        Some("acct-123"),
        true,
        json!(null),
        false,
    ));
    assert!(CodexSpendControlsMonthlyUsageGate::should_fetch(&u));
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:295 camel account id and null camel spend control retain presence
#[test]
fn camel_account_id_and_null_camel_spend_control_retain_presence() {
    let u = usage(br#"{"accountId":"camel-id","plan_type":"edu","spendControl":null}"#);
    assert_eq!(u.account_id.as_deref(), Some("camel-id"));
    assert!(u.spend_control_present);
    assert!(CodexSpendControlsMonthlyUsageGate::should_fetch(&u));
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:304 O auth helper injects monthly limit and preserves existing credit data
#[tokio::test]
async fn o_auth_helper_injects_monthly_limit_and_preserves_existing_credit_data() {
    let raw = education_default();
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        original.clone(),
        &u,
        &credentials(None),
        true,
        |id| async move {
            assert_eq!(id, "acct-123");
            Ok(payload())
        },
    )
    .await;
    let before = original.credits.as_ref().unwrap();
    let after = enriched.credits.as_ref().unwrap();
    assert_eq!(after.remaining, before.remaining);
    assert_eq!(after.events, before.events);
    assert_eq!(after.updated_at, before.updated_at);
    assert_eq!(after.codex_credit_limit.as_ref().unwrap().limit, 7000.);
    assert_eq!(enriched.source_label, original.source_label);
    assert_eq!(enriched.strategy_id, original.strategy_id);
    assert!(!enriched.codex_monthly_limit_enrichment_failed);
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:332 O auth helper adds monthly limit to credits with a hidden balance
#[tokio::test]
async fn o_auth_helper_adds_monthly_limit_to_credits_with_a_hidden_balance() {
    let raw = education(Some("education"), Some("acct-123"), true, json!(null), true);
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        original.clone(),
        &u,
        &credentials(Some("credential-account")),
        true,
        |id| async move {
            assert_eq!(id, "credential-account");
            Ok(payload())
        },
    )
    .await;
    let before = original.credits.unwrap();
    let after = enriched.credits.unwrap();
    assert!(!before.balance_read_succeeded);
    assert_eq!(before.credits_available, Some(true));
    assert_eq!(after.remaining, 0.);
    assert!(after.events.is_empty());
    assert_eq!(after.codex_credit_limit.unwrap().limit, 7000.);
    assert_eq!(after.credits_available, Some(true));
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:361 O auth helper replaces a hidden workspace balance with the owner visible balance
#[tokio::test]
async fn o_auth_helper_replaces_a_hidden_workspace_balance_with_the_owner_visible_balance() {
    let raw = education(
        Some("business"),
        Some("credential-account"),
        true,
        json!(null),
        true,
    );
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
        original,
        &u,
        &credentials(Some("credential-account")),
        true,
        now(),
        |id| async move {
            assert_eq!(id, "credential-account");
            CodexOAuthUsageFetcher::decode_workspace_remaining_balance_response(
                br#"{"balance":"1234"}"#,
            )
        },
    )
    .await;
    let c = enriched.credits.unwrap();
    assert_eq!(c.remaining, 1234.);
    assert!(c.balance_read_succeeded);
    assert_eq!(c.credits_available, Some(true));
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:388 O auth helper keeps original result on enrichment error
#[tokio::test]
async fn o_auth_helper_keeps_original_result_on_enrichment_error() {
    let raw = education_default();
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        original.clone(),
        &u,
        &credentials(Some("credential-account")),
        true,
        |_| async { Err(CodexOAuthFetchError::InvalidResponse) },
    )
    .await;
    assert_eq!(enriched.credits, original.credits);
    assert_eq!(enriched.source_label, original.source_label);
    assert_eq!(enriched.strategy_id, original.strategy_id);
    assert_eq!(enriched.strategy_kind, original.strategy_kind);
    assert!(!original.codex_monthly_limit_enrichment_failed);
    assert!(enriched.codex_monthly_limit_enrichment_failed);
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:412 O auth helper treats a missing account id as enrichment failure
#[tokio::test]
async fn o_auth_helper_treats_a_missing_account_id_as_enrichment_failure() {
    let raw = education(Some("education"), None, true, json!(null), false);
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        original.clone(),
        &u,
        &credentials(None),
        true,
        |_| async { panic!("missing account must not fetch") },
    )
    .await;
    assert_eq!(enriched.credits, original.credits);
    assert!(enriched.codex_monthly_limit_enrichment_failed);
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:436 O auth helper treats unmappable nested limit as enrichment failure
#[tokio::test]
async fn o_auth_helper_treats_unmappable_nested_limit_as_enrichment_failure() {
    let raw = education_default();
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        original.clone(),
        &u,
        &credentials(Some("credential-account")),
        true,
        |_| async { Ok(malformed("HARD_CAP")) },
    )
    .await;
    assert_eq!(enriched.credits, original.credits);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
    assert!(enriched.codex_monthly_limit_enrichment_failed);
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:457 O auth helper treats disabled unmappable limit as confirmed absence
#[tokio::test]
async fn o_auth_helper_treats_disabled_unmappable_limit_as_confirmed_absence() {
    let raw = education_default();
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        original.clone(),
        &u,
        &credentials(Some("credential-account")),
        true,
        |_| async { Ok(malformed("disabled")) },
    )
    .await;
    assert_eq!(enriched.credits, original.credits);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
    assert!(!enriched.codex_monthly_limit_enrichment_failed);
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:478 O auth helper treats unmappable current month usage as enrichment failure
#[tokio::test]
async fn o_auth_helper_treats_unmappable_current_month_usage_as_enrichment_failure() {
    let raw = education_default();
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        original.clone(),
        &u,
        &credentials(Some("credential-account")),
        true,
        |_| async { Ok(monthly(r#"{"amount":12}"#, "7000", Some("HARD_CAP"))) },
    )
    .await;
    assert_eq!(enriched.credits, original.credits);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
    assert!(enriched.codex_monthly_limit_enrichment_failed);
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:498 O auth helper treats absent cap as confirmed despite malformed usage
#[tokio::test]
async fn o_auth_helper_treats_absent_cap_as_confirmed_despite_malformed_usage() {
    let raw = education_default();
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        original.clone(),
        &u,
        &credentials(Some("credential-account")),
        true,
        |_| async { Ok(decode(r#"{"current_month_usage":{"amount":12}}"#)) },
    )
    .await;
    assert_eq!(enriched.credits, original.credits);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
    assert!(!enriched.codex_monthly_limit_enrichment_failed);
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:519 O auth helper treats disabled monthly cap as confirmed absence
#[tokio::test]
async fn o_auth_helper_treats_disabled_monthly_cap_as_confirmed_absence() {
    let raw = education_default();
    let u = usage(&raw);
    let original = result(&raw, None, true);
    let enriched = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        original.clone(),
        &u,
        &credentials(Some("credential-account")),
        true,
        |_| async { Ok(monthly("3046.4506806135178", "7000", Some("disabled"))) },
    )
    .await;
    assert_eq!(enriched.credits, original.credits);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
    assert!(!enriched.codex_monthly_limit_enrichment_failed);
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:558 O auth helper skips fetch when gate or credits setting is false
#[tokio::test]
async fn o_auth_helper_skips_fetch_when_gate_or_credits_setting_is_false() {
    let raw = education_default();
    let original = result(&raw, None, true);
    let consumer = usage(&education(
        Some("plus"),
        Some("acct-123"),
        true,
        json!(null),
        false,
    ));
    let edu = usage(&raw);
    for (u, include) in [(&consumer, true), (&edu, false)] {
        CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
            original.clone(),
            u,
            &credentials(Some("account")),
            include,
            |_| async { panic!("ineligible monthly usage must not fetch") },
        )
        .await;
    }
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:590 O auth monthly endpoint URL requires backend api base
#[test]
fn o_auth_monthly_endpoint_url_requires_backend_api_base() {
    let env = BTreeMap::new();
    let url = CodexOAuthUsageFetcher::resolve_spend_controls_monthly_usage_url(
        &env,
        Some(r#"chatgpt_base_url = "https://chatgpt.com/backend-api""#),
        None,
        "acct-123",
    )
    .unwrap();
    assert_eq!(
        url.as_str(),
        "https://chatgpt.com/backend-api/accounts/acct-123/spend-controls/current-user/monthly-usage"
    );
    assert!(
        CodexOAuthUsageFetcher::resolve_spend_controls_monthly_usage_url(
            &env,
            Some(r#"chatgpt_base_url = "https://example.com/api/codex""#),
            None,
            "acct-123"
        )
        .is_none()
    );
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:636 workspace balance endpoint decodes numeric strings and clamps negative values
#[test]
fn workspace_balance_endpoint_decodes_numeric_strings_and_clamps_negative_values() {
    for (raw, expected) in [
        (r#"{"balance":"1234"}"#, Some(1234.)),
        (r#"{"balance":-4}"#, Some(0.)),
        (r#"{"balance":"NaN"}"#, None),
        (r#"{"balance":null}"#, None),
    ] {
        assert_eq!(
            CodexOAuthUsageFetcher::decode_workspace_remaining_balance_response(raw.as_bytes())
                .unwrap()
                .balance,
            expected
        );
    }
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:604 O auth monthly endpoint request uses bearer account and bounded timeout
#[tokio::test]
async fn o_auth_monthly_endpoint_request_uses_bearer_account_and_bounded_timeout() {
    let server = https::start_server();
    let mock=server.mock_async(|when,then| {when.method(httpmock::Method::GET).path("/backend-api/accounts/acct-123/spend-controls/current-user/monthly-usage").header("authorization","Bearer access").header("chatgpt-account-id","acct-123").header("accept","application/json");then.status(200).body(r#"{"current_month_usage":3046.4506806135178,"effective_monthly_limit":{"limit":7000,"enforcement_mode":"HARD_CAP","limit_mode":"amount_credits"}}"#);}).await;
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
    let response = CodexOAuthUsageFetcher::fetch_spend_controls_monthly_usage(
        &net,
        "access",
        "acct-123",
        &env,
        Some(home.path()),
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    assert_eq!(response.effective_monthly_limit.unwrap().limit, Some(7000.));
    mock.assert_calls_async(1).await;
    let request=CodexOAuthUsageFetcher::build_account_scoped_get_request(reqwest::Url::parse("https://chatgpt.com/backend-api/accounts/acct-123/spend-controls/current-user/monthly-usage").unwrap(),"access","acct-123",Duration::from_secs(3)).unwrap();
    assert_eq!(request.timeout, Some(Duration::from_secs(3)));
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:644 O auth workspace balance endpoint uses bearer account and bounded timeout
#[tokio::test]
async fn o_auth_workspace_balance_endpoint_uses_bearer_account_and_bounded_timeout() {
    let server = https::start_server();
    let mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/accounts/acct-123/remaining_balance")
                .header("authorization", "Bearer access")
                .header("chatgpt-account-id", "acct-123")
                .header("accept", "application/json");
            then.status(200).body(r#"{"balance":1234}"#);
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
    let response = CodexOAuthUsageFetcher::fetch_workspace_remaining_balance(
        &net,
        "access",
        "acct-123",
        &env,
        Some(home.path()),
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    assert_eq!(response.balance, Some(1234.));
    mock.assert_calls_async(1).await;
    let request = CodexOAuthUsageFetcher::build_account_scoped_get_request(
        reqwest::Url::parse("https://chatgpt.com/backend-api/accounts/acct-123/remaining_balance")
            .unwrap(),
        "access",
        "acct-123",
        Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(request.timeout, Some(Duration::from_secs(3)));
}

// upstream: CodexSpendControlsMonthlyUsageTests.swift:540 O auth helper propagates cancellation
#[tokio::test]
async fn o_auth_helper_propagates_cancellation() {
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
    let _usage=server.mock_async(|when,then|{when.path("/backend-api/wham/usage");then.status(200).body(r####"{"account_id":"acct-123","plan_type":"education","rate_limit":{"primary_window":{"used_percent":10,"reset_at":1786161204,"limit_window_seconds":18000},"secondary_window":null},"credits":{"has_credits":true,"unlimited":false,"balance":"14"},"spend_control":{"reached":false,"individual_limit":null}}"####);}).await;
    let _reset = server
        .mock_async(|when, then| {
            when.path("/backend-api/wham/rate-limit-reset-credits");
            then.status(200)
                .body(r#"{"credits":[],"available_count":0}"#);
        })
        .await;
    let pending = server
        .mock_async(|when, then| {
            when.path("/backend-api/accounts/acct-123/spend-controls/current-user/monthly-usage");
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
