// Ported from CodexBar Tests/CodexBarTests/CodexUnlimitedCreditsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::providers::codex::*;
use jiff::Timestamp;
use serde_json::json;
fn now() -> Timestamp {
    Timestamp::from_second(1_700_000_000).unwrap()
}
fn map(
    pat: bool,
    has: bool,
    unlimited: bool,
    balance: Option<f64>,
    cap: bool,
) -> CodexFetchOutcome {
    let mut value = json!({"plan_type":"business","rate_limit":{"primary_window":{"used_percent":12,"reset_at":1700003600,"limit_window_seconds":18000}},"credits":{"has_credits":has,"unlimited":unlimited}});
    if let Some(b) = balance {
        value["credits"]["balance"] = json!(b);
    }
    if cap {
        value["individual_limit"] = json!({"limit":400,"used":300});
    }
    let raw = serde_json::to_vec(&value).unwrap();
    if pat {
        CodexPATFetchStrategy::map_result_for_testing(
            &raw,
            Some(&CodexPATWhoami {
                account_id: None,
                email: Some("fixture@example.com".into()),
                plan_type: Some("business".into()),
            }),
            now(),
        )
        .unwrap()
    } else {
        CodexOAuthFetchStrategy::map_result_for_testing(
            &raw,
            &CodexOAuthCredentials::new(
                "fixture-access",
                "fixture-refresh",
                None,
                Some("fixture-workspace".into()),
                Some(now()),
                None,
                CodexOAuthCredentialSource::CodexHome,
                false,
            ),
            None,
            false,
            true,
            now(),
        )
        .unwrap()
    }
}

// upstream: CodexUnlimitedCreditsTests.swift:21 unlimited credits without a finite balance or cap do not create unavailable credits
#[test]
fn unlimited_credits_without_a_finite_balance_or_cap_do_not_create_unavailable_credits() {
    for pat in [false, true] {
        for has in [false, true] {
            let r = map(pat, has, true, None, false);
            let p = r.usage.primary.unwrap();
            assert_eq!(p.used_percent, 12.);
            assert_eq!(p.window_minutes, Some(300));
            assert!(r.credits.is_none());
            assert!(r.usage.provider_cost.is_none());
        }
    }
}

// upstream: CodexUnlimitedCreditsTests.swift:36 unlimited flag preserves independently reported finite balances and monthly caps
#[test]
fn unlimited_flag_preserves_independently_reported_finite_balances_and_monthly_caps() {
    for pat in [false, true] {
        for scenario in ["positive balance", "zero balance", "monthly cap"] {
            let balance = match scenario {
                "positive balance" => Some(14.),
                "zero balance" => Some(0.),
                _ => None,
            };
            let cap = scenario == "monthly cap";
            let r = map(pat, true, true, balance, cap);
            assert_eq!(r.usage.primary.unwrap().used_percent, 12.);
            let c = r.credits.unwrap();
            let cost = r.usage.provider_cost.unwrap();
            assert_eq!(c.remaining, balance.unwrap_or(0.));
            assert_eq!(c.balance_read_succeeded, balance.is_some());
            assert_eq!(
                c.display_remaining(),
                if cap { Some(100.) } else { balance }
            );
            assert_eq!(
                c.codex_credit_limit.as_ref().map(|v| v.remaining()),
                if cap { Some(100.) } else { None }
            );
            assert_ne!(cost.balance_is_unavailable, Some(true));
            assert_eq!(
                cost.balance,
                if balance == Some(14.) {
                    Some(14.)
                } else {
                    None
                }
            );
            assert_eq!(
                cost.balance_updated_at,
                if balance.is_some() {
                    Some(c.updated_at)
                } else {
                    None
                }
            );
            assert_eq!(cost.limit, if cap { 400. } else { 0. });
        }
    }
}

// upstream: CodexUnlimitedCreditsTests.swift:64 limited available credits with a missing balance still create an unavailable observation
#[test]
fn limited_available_credits_with_a_missing_balance_still_create_an_unavailable_observation() {
    for pat in [false, true] {
        let r = map(pat, true, false, None, false);
        assert_eq!(r.usage.primary.unwrap().used_percent, 12.);
        let c = r.credits.unwrap();
        let cost = r.usage.provider_cost.unwrap();
        assert!(!c.balance_read_succeeded);
        assert_eq!(c.credits_available, Some(true));
        assert!(c.display_remaining().is_none());
        assert_eq!(cost.balance_is_unavailable, Some(true));
        assert_eq!(cost.balance_updated_at, Some(c.updated_at));
        assert!(cost.balance.is_none());
    }
}
