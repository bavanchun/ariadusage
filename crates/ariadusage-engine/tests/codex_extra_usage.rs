// Ported from CodexBar Tests/CodexBarTests/CodexExtraUsageCostTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::cost::ProviderCostSnapshot;
use ariadusage_engine::providers::codex::*;
use jiff::Timestamp;
fn date(offset: i64) -> Timestamp {
    Timestamp::from_second(1_780_000_000 + offset).unwrap()
}
fn credits(
    remaining: f64,
    offset: i64,
    used: f64,
    limit: f64,
    percent: f64,
    cap_offset: i64,
    read: bool,
) -> CreditsSnapshot {
    CreditsSnapshot::with_details(
        remaining,
        vec![],
        date(offset),
        Some(CodexCreditLimitSnapshot::new(
            "Monthly credit limit",
            used,
            limit,
            percent,
            None,
            date(cap_offset),
        )),
        read,
        None,
        false,
    )
}
fn cost(used: f64, limit: f64, balance: Option<f64>, offset: i64) -> ProviderCostSnapshot {
    let mut v = ProviderCostSnapshot::new(
        used,
        limit,
        CodexExtraUsageCost::CURRENCY_CODE,
        date(offset),
    );
    v.balance = balance;
    v
}
fn resolve(live: &CreditsSnapshot, attached: &ProviderCostSnapshot) -> ProviderCostSnapshot {
    CodexExtraUsageCost::resolving(Some(live), Some(attached)).unwrap()
}

// upstream: CodexExtraUsageCostTests.swift:6 monthly credit maps to extra usage used versus limit
#[test]
fn monthly_credit_maps_to_extra_usage_used_versus_limit() {
    let mut c = credits(0., 0, 36.8, 1000., 96.32, 0, true);
    c.codex_credit_limit.as_mut().unwrap().resets_at =
        Some(Timestamp::from_second(1_788_220_800).unwrap());
    let cost = CodexExtraUsageCost::provider_cost(Some(&c)).unwrap();
    assert_eq!(cost.used, 36.8);
    assert_eq!(cost.limit, 1000.);
    assert_eq!(cost.currency_code, CodexExtraUsageCost::CURRENCY_CODE);
    assert_eq!(cost.period.as_deref(), Some("Monthly credit limit"));
    assert!(cost.balance.is_none());
    assert_eq!(
        cost.resets_at,
        Some(Timestamp::from_second(1_788_220_800).unwrap())
    );
}

// upstream: CodexExtraUsageCostTests.swift:29 purchased extra credits are a remaining balance
#[test]
fn purchased_extra_credits_are_a_remaining_balance() {
    let c = CreditsSnapshot::new(14.5, vec![], date(0));
    let cost = CodexExtraUsageCost::provider_cost(Some(&c)).unwrap();
    assert_eq!(cost.used, 0.);
    assert_eq!(cost.limit, 0.);
    assert_eq!(cost.balance, Some(14.5));
    assert_eq!(cost.period.as_deref(), Some("Extra usage"));
}

// upstream: CodexExtraUsageCostTests.swift:41 purchased extra credits sit beside a monthly cap
#[test]
fn purchased_extra_credits_sit_beside_a_monthly_cap() {
    let c = credits(50., 0, 120., 400., 70., 0, true);
    let cost = CodexExtraUsageCost::provider_cost(Some(&c)).unwrap();
    assert_eq!(cost.used, 120.);
    assert_eq!(cost.limit, 400.);
    assert_eq!(cost.balance, Some(50.));
}

// upstream: CodexExtraUsageCostTests.swift:61 monthly remaining is not treated as purchased extra credits
#[test]
fn monthly_remaining_is_not_treated_as_purchased_extra_credits() {
    let c = credits(280., 0, 120., 400., 70., 0, true);
    assert!(
        CodexExtraUsageCost::provider_cost(Some(&c))
            .unwrap()
            .balance
            .is_none()
    );
}

// upstream: CodexExtraUsageCostTests.swift:79 zero remaining without a monthly cap retains the successful reading
#[test]
fn zero_remaining_without_a_monthly_cap_retains_the_successful_reading() {
    let c = CreditsSnapshot::new(0., vec![], date(0));
    let cost = CodexExtraUsageCost::provider_cost(Some(&c)).unwrap();
    assert_eq!(cost.balance_updated_at, Some(c.updated_at));
    assert!(cost.balance.is_none());
    assert!(CodexExtraUsageCost::provider_cost(None).is_none());
}

// upstream: CodexExtraUsageCostTests.swift:87 attached monthly cap wins over balance-only live credits
#[test]
fn attached_monthly_cap_wins_over_balance_only_live_credits() {
    let c = CreditsSnapshot::new(14.5, vec![], date(0));
    let mut attached = cost(120., 400., None, 0);
    attached.period = Some("Monthly credit limit".into());
    attached.resets_at = Some(Timestamp::from_second(1_788_220_800).unwrap());
    let r = resolve(&c, &attached);
    assert_eq!(r.used, 120.);
    assert_eq!(r.limit, 400.);
    assert_eq!(
        r.resets_at,
        Some(Timestamp::from_second(1_788_220_800).unwrap())
    );
    assert_eq!(r.balance, Some(14.5));
}

// upstream: CodexExtraUsageCostTests.swift:106 the fresher cap wins when both sides carry one
#[test]
fn the_fresher_cap_wins_when_both_sides_carry_one() {
    let c = credits(50., 0, 300., 400., 25., 0, true);
    assert_eq!(resolve(&c, &cost(120., 400., None, -3600)).used, 300.);
    let newer = resolve(&c, &cost(380., 400., None, 3600));
    assert_eq!(newer.used, 380.);
    assert_eq!(newer.balance, Some(50.));
}

// upstream: CodexExtraUsageCostTests.swift:140 the monthly cap ages by its own timestamp, not the balance refresh
#[test]
fn the_monthly_cap_ages_by_its_own_timestamp_not_the_balance_refresh() {
    let c = credits(50., 7200, 300., 400., 25., 0, true);
    assert_eq!(
        CodexExtraUsageCost::provider_cost(Some(&c))
            .unwrap()
            .updated_at,
        date(0)
    );
    assert_eq!(resolve(&c, &cost(380., 400., None, 3600)).used, 380.);
}

// upstream: CodexExtraUsageCostTests.swift:168 the purchased balance comes from the fresher side, not from whichever cap wins
#[test]
fn the_purchased_balance_comes_from_the_fresher_side_not_from_whichever_cap_wins() {
    let preserved = credits(50., 7200, 300., 400., 25., 0, true);
    let dashboard = cost(380., 400., Some(30.), 3600);
    let stale = resolve(&preserved, &dashboard);
    assert_eq!(stale.used, 380.);
    assert_eq!(stale.balance, Some(50.));
    let live = CreditsSnapshot::new(14.5, vec![], date(0));
    assert_eq!(resolve(&live, &dashboard).balance, Some(30.));
}

// upstream: CodexExtraUsageCostTests.swift:203 a winning live cap still takes the attached balance when its own credits fetch failed
#[test]
fn a_winning_live_cap_still_takes_the_attached_balance_when_its_own_credits_fetch_failed() {
    let c = credits(0., 0, 300., 400., 25., 0, false);
    let r = resolve(&c, &cost(120., 400., Some(30.), -3600));
    assert_eq!(r.used, 300.);
    assert_eq!(r.balance, Some(30.));
}

// upstream: CodexExtraUsageCostTests.swift:231 a newer successful zero purchased-credit balance clears an older attached balance
#[test]
fn a_newer_successful_zero_purchased_credit_balance_clears_an_older_attached_balance() {
    let c = credits(0., 0, 300., 400., 25., 0, true);
    let r = resolve(&c, &cost(120., 400., Some(30.), -3600));
    assert_eq!(r.used, 300.);
    assert!(r.balance.is_none());
}

// upstream: CodexExtraUsageCostTests.swift:283 a cap-only oauth response leaves the purchased balance unread
#[test]
fn a_cap_only_oauth_response_leaves_the_purchased_balance_unread() {
    for balance in [None, Some("0")] {
        let mut obj = serde_json::json!({"rate_limit":{"primary_window":null,"secondary_window":null,"individual_limit":{"limit":100000,"used":7761,"remaining_percent":92.239,"resets_at":1782864000}},"credits":{"has_credits":true,"unlimited":false}});
        if let Some(b) = balance {
            obj["credits"]["balance"] = serde_json::json!(b);
        }
        let raw = serde_json::to_vec(&obj).unwrap();
        let creds = CodexOAuthCredentials::new(
            "access",
            "refresh",
            None,
            None,
            Some(date(0)),
            None,
            CodexOAuthCredentialSource::CodexHome,
            false,
        );
        let r = CodexOAuthFetchStrategy::map_result_for_testing(
            &raw,
            &creds,
            None,
            false,
            true,
            date(0),
        )
        .unwrap();
        let c = r.credits.unwrap();
        assert_eq!(c.remaining, 0.);
        assert_eq!(c.balance_read_succeeded, balance.is_some());
    }
}

// upstream: CodexExtraUsageCostTests.swift:344 legacy credits snapshots treat a missing balance-read flag as succeeded
#[test]
fn legacy_credits_snapshots_treat_a_missing_balance_read_flag_as_succeeded() {
    let original = CreditsSnapshot::new(14.5, vec![], date(0));
    let mut obj = serde_json::to_value(original).unwrap();
    obj.as_object_mut().unwrap().remove("balanceReadSucceeded");
    let decoded: CreditsSnapshot = serde_json::from_value(obj).unwrap();
    assert_eq!(decoded.remaining, 14.5);
    assert!(decoded.balance_read_succeeded);
}

// upstream: CodexExtraUsageCostTests.swift:360 resolving keeps each side when the other is missing or foreign
#[test]
fn resolving_keeps_each_side_when_the_other_is_missing_or_foreign() {
    let live = CreditsSnapshot::new(14.5, vec![], date(0));
    let projected = CodexExtraUsageCost::provider_cost(Some(&live)).unwrap();
    let foreign = ProviderCostSnapshot::new(4., 50., "USD", date(0));
    assert_eq!(
        CodexExtraUsageCost::resolving(None, Some(&foreign)),
        Some(foreign.clone())
    );
    assert!(CodexExtraUsageCost::resolving(None, None).is_none());
    assert_eq!(
        CodexExtraUsageCost::resolving(Some(&live), None),
        Some(projected.clone())
    );
    assert_eq!(
        CodexExtraUsageCost::resolving(Some(&live), Some(&foreign)),
        Some(projected)
    );
}

// upstream: CodexExtraUsageCostTests.swift:374 oauth credits-only result attaches extra usage
#[test]
fn oauth_credits_only_result_attaches_extra_usage() {
    let raw=br#"{"rate_limit":{"primary_window":null,"secondary_window":null},"credits":{"has_credits":true,"unlimited":false,"balance":"14.5"}}"#;
    let creds = CodexOAuthCredentials::new(
        "access",
        "refresh",
        None,
        None,
        Some(date(0)),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let r =
        CodexOAuthFetchStrategy::map_result_for_testing(raw, &creds, None, false, true, date(0))
            .unwrap();
    assert_eq!(r.credits.unwrap().remaining, 14.5);
    let cost = r.usage.provider_cost.unwrap();
    assert_eq!(cost.balance, Some(14.5));
    assert_eq!(cost.currency_code, CodexExtraUsageCost::CURRENCY_CODE);
}
