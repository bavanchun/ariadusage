// Ported from CodexBar Tests/CodexBarTests/CodexOAuthCreditLimitTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::providers::codex::*;
use jiff::Timestamp;
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
use ariadusage_core::{model::identity::ProviderIdentitySnapshot, providers::SourceMode};
use ariadusage_protocol::{ProviderId, metric::SourceKind};
fn identity(email: &str) -> ProviderIdentitySnapshot {
    ProviderIdentitySnapshot::new(
        Some(ProviderId::new("codex").unwrap()),
        Some(email.into()),
        None,
        Some("enterprise".into()),
        None,
    )
}
fn zero() -> CodexFetchOutcome {
    result(br#"{"rate_limit":{"primary_window":{"used_percent":12,"reset_at":1766948068,"limit_window_seconds":18000},"secondary_window":null},"credits":{"has_credits":true,"unlimited":false,"balance":"0"}}"#,None,true)
}
fn cli(email: Option<&str>, cap: bool) -> CodexFetchOutcome {
    let mut c = zero();
    c.usage.primary = None;
    c.usage.identity = email.map(identity);
    c.source_label = "codex-cli".into();
    c.strategy_kind = SourceKind::Cli;
    if cap {
        let limit =
            CodexCreditLimitSnapshot::new("Monthly credit limit", 250., 1000., 75., None, now());
        c.credits = Some(CreditsSnapshot::new(750., vec![], now()).with_credit_limit(Some(limit)));
    }
    c
}

// upstream: CodexOAuthCreditLimitTests.swift:137 decodes monthly credit limit from rate limit payload
#[test]
fn decodes_monthly_credit_limit_from_rate_limit_payload() {
    let raw = br##"{
  "plan_type": "enterprise",
  "rate_limit": {
    "primary_window": null,
    "secondary_window": null,
    "individual_limit": {
      "limit": 100000,
      "used": "7761",
      "remaining_percent": 92.239,
      "resets_at": 1782864000
    }
  },
  "credits": {
    "has_credits": true,
    "unlimited": false,
    "balance": "0"
  }
}"##;
    let u = CodexOAuthUsageFetcher::decode_usage_response(raw).unwrap();
    let v = u.rate_limit.unwrap().individual_limit.unwrap();
    assert_eq!(v.limit, Some(100_000.));
    assert_eq!(v.used, Some(7761.));
    assert_eq!(v.remaining_percent, Some(92.239));
    assert_eq!(v.resets_at, Some(1_782_864_000));
}

// upstream: CodexOAuthCreditLimitTests.swift:166 monthly credit limit O auth payload displays limit when balance is zero
#[test]
fn monthly_credit_limit_o_auth_payload_displays_limit_when_balance_is_zero() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": null,
    "secondary_window": null,
    "individual_limit": {
      "limit": 100000,
      "used": 7761,
      "remaining_percent": 92.239,
      "resets_at": 1782864000
    }
  },
  "credits": {
    "has_credits": true,
    "unlimited": false,
    "balance": "0"
  }
}"##;
    let r = result(raw, None, true);
    let c = r.credits.unwrap();
    assert_eq!(c.remaining, 0.);
    let cap = c.codex_credit_limit.unwrap();
    assert_eq!(cap.remaining(), 92239.);
    assert_eq!(cap.remaining_percent, 92.239);
    assert_eq!(r.source_label, "oauth");
}

// upstream: CodexOAuthCreditLimitTests.swift:197 explicit O auth zero credits without monthly limit keeps partial result
#[test]
fn explicit_o_auth_zero_credits_without_monthly_limit_keeps_partial_result() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": null,
    "secondary_window": null
  },
  "credits": {
    "has_credits": true,
    "unlimited": false,
    "balance": "0"
  }
}"##;
    let r = result(raw, None, true);
    let c = r.credits.unwrap();
    assert_eq!(c.remaining, 0.);
    assert!(c.codex_credit_limit.is_none());
    assert_eq!(r.source_label, "oauth");
}

// upstream: CodexOAuthCreditLimitTests.swift:222 workspace credits with a hidden balance stay distinct from zero
#[test]
fn workspace_credits_with_a_hidden_balance_stay_distinct_from_zero() {
    let raw = br##"{
  "plan_type": "business",
  "rate_limit": {
    "primary_window": null,
    "secondary_window": null
  },
  "credits": {
    "has_credits": true,
    "unlimited": false,
    "balance": null
  }
}"##;
    let r = result(raw, None, true);
    let c = r.credits.unwrap();
    assert_eq!(c.remaining, 0.);
    assert!(!c.balance_read_succeeded);
    assert_eq!(c.credits_available, Some(true));
    assert!(c.codex_credit_limit.is_none());
}

// upstream: CodexOAuthCreditLimitTests.swift:249 auto O auth zero credits preserves O auth usage while adding CLI monthly limit
#[tokio::test]
async fn auto_o_auth_zero_credits_preserves_o_auth_usage_while_adding_cli_monthly_limit() {
    let mut original = zero();
    original.usage.identity = Some(identity("owner@example.com"));
    let before = original.clone();
    let enriched = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        original,
        SourceMode::Auto,
        None,
        true,
        true,
        || async { Ok(cli(Some("owner@example.com"), true)) },
    )
    .await;
    assert_eq!(enriched.source_label, "oauth");
    assert_eq!(enriched.usage.primary, before.usage.primary);
    assert_eq!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .as_ref()
            .unwrap()
            .remaining(),
        750.
    );
    assert_eq!(
        enriched.credits.as_ref().unwrap().remaining,
        before.credits.as_ref().unwrap().remaining
    );
    assert!(before.usage.primary.is_some());
    assert!(CodexOAuthFetchStrategy::should_try_cli_for_monthly_limit(
        &before
    ));
    assert_eq!(enriched.strategy_kind, SourceKind::Oauth);
}

// upstream: CodexOAuthCreditLimitTests.swift:274 usage-only O auth refresh does not launch CLI monthly limit enrichment
#[tokio::test]
async fn usage_only_o_auth_refresh_does_not_launch_cli_monthly_limit_enrichment() {
    let mut original = zero();
    original.usage.identity = Some(identity("owner@example.com"));
    let before = original.clone();
    let enriched = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        original,
        SourceMode::Auto,
        None,
        false,
        true,
        || async { Ok(cli(Some("owner@example.com"), true)) },
    )
    .await;
    assert_eq!(enriched.source_label, "oauth");
    assert_eq!(enriched.usage.primary, before.usage.primary);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
}

// upstream: CodexOAuthCreditLimitTests.swift:295 managed workspace O auth does not mix in unscoped CLI monthly limit
#[tokio::test]
async fn managed_workspace_o_auth_does_not_mix_in_unscoped_cli_monthly_limit() {
    let mut original = zero();
    original.usage.identity = Some(identity("owner@example.com"));
    let before = original.clone();
    let enriched = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        original,
        SourceMode::Auto,
        Some("workspace-team"),
        true,
        true,
        || async { Ok(cli(Some("owner@example.com"), true)) },
    )
    .await;
    assert_eq!(enriched.source_label, "oauth");
    assert_eq!(enriched.usage.primary, before.usage.primary);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
}

// upstream: CodexOAuthCreditLimitTests.swift:316 auto O auth zero credits rejects CLI monthly limit without verified identity
#[tokio::test]
async fn auto_o_auth_zero_credits_rejects_cli_monthly_limit_without_verified_identity() {
    let mut original = zero();
    original.usage.identity = Some(identity("owner@example.com"));
    let before = original.clone();
    let enriched = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        original,
        SourceMode::Auto,
        None,
        true,
        true,
        || async { Ok(cli(None, true)) },
    )
    .await;
    assert_eq!(enriched.source_label, "oauth");
    assert_eq!(enriched.usage.primary, before.usage.primary);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
    assert_eq!(
        enriched.usage.identity.unwrap().account_email.as_deref(),
        Some("owner@example.com")
    );
}

// upstream: CodexOAuthCreditLimitTests.swift:335 auto O auth zero credits rejects CLI monthly limit from another account
#[tokio::test]
async fn auto_o_auth_zero_credits_rejects_cli_monthly_limit_from_another_account() {
    let mut original = zero();
    original.usage.identity = Some(identity("owner@example.com"));
    let before = original.clone();
    let enriched = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        original,
        SourceMode::Auto,
        None,
        true,
        true,
        || async { Ok(cli(Some("other@example.com"), true)) },
    )
    .await;
    assert_eq!(enriched.source_label, "oauth");
    assert_eq!(enriched.usage.primary, before.usage.primary);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
    assert_eq!(
        enriched.usage.identity.unwrap().account_email.as_deref(),
        Some("owner@example.com")
    );
}

// upstream: CodexOAuthCreditLimitTests.swift:356 auto O auth zero credits accepts matching CLI account case insensitively
#[tokio::test]
async fn auto_o_auth_zero_credits_accepts_matching_cli_account_case_insensitively() {
    let mut original = zero();
    original.usage.identity = Some(identity("Owner@Example.com"));
    let before = original.clone();
    let enriched = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        original,
        SourceMode::Auto,
        None,
        true,
        true,
        || async { Ok(cli(Some(" owner@example.COM "), true)) },
    )
    .await;
    assert_eq!(enriched.source_label, "oauth");
    assert_eq!(enriched.usage.primary, before.usage.primary);
    assert_eq!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .as_ref()
            .unwrap()
            .remaining(),
        750.
    );
    assert_eq!(
        enriched.credits.as_ref().unwrap().remaining,
        before.credits.as_ref().unwrap().remaining
    );
    assert_eq!(
        enriched.usage.identity.unwrap().account_email.as_deref(),
        Some("Owner@Example.com")
    );
}

// upstream: CodexOAuthCreditLimitTests.swift:377 auto O auth zero credits keeps partial result when CLI is unavailable
#[tokio::test]
async fn auto_o_auth_zero_credits_keeps_partial_result_when_cli_is_unavailable() {
    let original = zero();
    let before = original.clone();
    let enriched = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        original,
        SourceMode::Auto,
        None,
        true,
        false,
        || async { Ok(cli(None, false)) },
    )
    .await;
    assert_eq!(enriched.source_label, "oauth");
    assert_eq!(enriched.usage.primary, before.usage.primary);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
    assert_eq!(enriched.credits.unwrap().remaining, 0.);
    assert!(enriched.usage.primary.is_some());
}

// upstream: CodexOAuthCreditLimitTests.swift:394 auto O auth zero credits keeps partial result when CLI lacks monthly limit
#[tokio::test]
async fn auto_o_auth_zero_credits_keeps_partial_result_when_cli_lacks_monthly_limit() {
    let original = zero();
    let before = original.clone();
    let enriched = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        original,
        SourceMode::Auto,
        None,
        true,
        true,
        || async { Ok(cli(None, false)) },
    )
    .await;
    assert_eq!(enriched.source_label, "oauth");
    assert_eq!(enriched.usage.primary, before.usage.primary);
    assert!(
        enriched
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
}

fn team() -> &'static [u8] {
    br##"{
  "plan_type": "team",
  "rate_limit": {
    "allowed": false,
    "limit_reached": true,
    "primary_window": {
      "used_percent": 100,
      "limit_window_seconds": 604800,
      "reset_after_seconds": 45962,
      "reset_at": 1786161204
    },
    "secondary_window": null
  },
  "credits": {
    "has_credits": true,
    "unlimited": false,
    "balance": null
  },
  "spend_control": {
    "reached": false,
    "individual_limit": {
      "source": "workspace_spend_controls",
      "limit": "1000",
      "used": "36.79748725891113",
      "remaining": "963.2025127410889",
      "used_percent": 4,
      "remaining_percent": 96,
      "reset_after_seconds": 2105558,
      "reset_at": 1788220800
    }
  }
}"##
}
// upstream: CodexOAuthCreditLimitTests.swift:453 decodes monthly credit limit from spend control payload
#[test]
fn decodes_monthly_credit_limit_from_spend_control_payload() {
    let u = CodexOAuthUsageFetcher::decode_usage_response(team()).unwrap();
    assert!(u.individual_limit.is_none());
    assert!(u.rate_limit.unwrap().individual_limit.is_none());
    let v = u.spend_control_individual_limit.unwrap();
    assert_eq!(v.limit, Some(1000.));
    assert_eq!(v.used, Some(36.79748725891113));
    assert_eq!(v.remaining_percent, Some(96.));
    assert_eq!(v.resets_at, Some(1_788_220_800));
}

// upstream: CodexOAuthCreditLimitTests.swift:466 spend control payload surfaces monthly credit limit when balance is null
#[test]
fn spend_control_payload_surfaces_monthly_credit_limit_when_balance_is_null() {
    let r = result(team(), None, true);
    let v = r.credits.unwrap().codex_credit_limit.unwrap();
    assert_eq!(v.limit, 1000.);
    assert_eq!(v.used, 36.79748725891113);
    assert_eq!(v.remaining_percent, 96.);
    assert_eq!(
        v.resets_at,
        Some(Timestamp::from_second(1_788_220_800).unwrap())
    );
    assert_eq!(r.source_label, "oauth");
}

// upstream: CodexOAuthCreditLimitTests.swift:492 root individual limit still wins over spend control
#[test]
fn root_individual_limit_still_wins_over_spend_control() {
    let raw = br##"{
  "plan_type": "team",
  "individual_limit": {
    "limit": 500,
    "used": 100,
    "remaining_percent": 80,
    "resets_at": 1782864000
  },
  "spend_control": {
  "individual_limit": {
    "limit": "1000",
    "used": "36.79748725891113",
    "remaining_percent": 96,
    "reset_at": 1788220800
  }
}
}"##;
    let v = result(raw, None, true)
        .credits
        .unwrap()
        .codex_credit_limit
        .unwrap();
    assert_eq!(v.limit, 500.);
    assert_eq!(
        v.resets_at,
        Some(Timestamp::from_second(1_782_864_000).unwrap())
    );
}

// upstream: CodexOAuthCreditLimitTests.swift:514 rate limit individual limit still wins over spend control
#[test]
fn rate_limit_individual_limit_still_wins_over_spend_control() {
    let raw = br##"{
  "plan_type": "team",
  "rate_limit": {
    "primary_window": null,
    "secondary_window": null,
    "individual_limit": {
      "limit": 100000,
      "used": 7761,
      "remaining_percent": 92.239,
      "resets_at": 1782864000
    }
  },
  "spend_control": {
  "individual_limit": {
    "limit": "1000",
    "used": "36.79748725891113",
    "remaining_percent": 96,
    "reset_at": 1788220800
  }
}
}"##;
    let u = CodexOAuthUsageFetcher::decode_usage_response(raw).unwrap();
    assert_eq!(
        u.rate_limit.unwrap().individual_limit.unwrap().limit,
        Some(100_000.)
    );
    assert_eq!(u.spend_control_individual_limit.unwrap().limit, Some(1000.));
    let v = result(raw, None, true)
        .credits
        .unwrap()
        .codex_credit_limit
        .unwrap();
    assert_eq!(v.limit, 100000.);
    assert_eq!(
        v.resets_at,
        Some(Timestamp::from_second(1_782_864_000).unwrap())
    );
    assert_eq!(v.remaining(), 92239.);
}
