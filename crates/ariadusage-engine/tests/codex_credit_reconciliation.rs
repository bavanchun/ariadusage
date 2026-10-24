// Ported from CodexBar Tests/CodexBarTests/CodexWorkspaceBalanceReconciliationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::{cost::ProviderCostSnapshot, snapshot::UsageSnapshot};
use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::Confidence;
use jiff::Timestamp;
fn date(offset: i64) -> Timestamp {
    Timestamp::from_second(1_780_000_000 + offset).unwrap()
}
fn credits(balance: f64, workspace: bool, read: bool) -> CreditsSnapshot {
    CreditsSnapshot::with_details(
        balance,
        vec![],
        date(0),
        Some(CodexCreditLimitSnapshot::new(
            "Monthly credit limit",
            300.,
            400.,
            25.,
            None,
            date(0),
        )),
        read,
        Some(true),
        workspace,
    )
}
fn stale(limit: f64) -> ProviderCostSnapshot {
    let mut c = ProviderCostSnapshot::new(0., limit, CodexExtraUsageCost::CURRENCY_CODE, date(-60));
    c.balance = Some(1234.);
    c.balance_is_workspace = Some(true);
    c
}
fn empty(cost: Option<ProviderCostSnapshot>) -> UsageSnapshot {
    UsageSnapshot::new(
        None,
        None,
        None,
        None,
        cost,
        vec![],
        None,
        None,
        date(0),
        None,
        Confidence::Exact,
    )
    .unwrap()
}

// upstream: CodexWorkspaceBalanceReconciliationTests.swift:24 workspace balances equal to a monthly remainder survive cost reconciliation
#[test]
fn workspace_balances_equal_to_a_monthly_remainder_survive_cost_reconciliation() {
    let workspace = credits(100., true, true);
    let cost = CodexExtraUsageCost::provider_cost(Some(&workspace)).unwrap();
    assert_eq!(cost.balance, Some(100.));
    assert_eq!(cost.balance_is_workspace, Some(true));
    let display =
        CodexExtraUsageCost::credits_for_display(Some(&credits(0., false, false)), Some(&cost))
            .unwrap();
    assert_eq!(display.remaining, 100.);
    assert!(display.has_workspace_balance());
    assert_eq!(display.display_remaining(), Some(100.));
}

// upstream: CodexWorkspaceBalanceReconciliationTests.swift:38 a fresh hidden pool invalidates an older attached workspace balance but preserves its cap
#[test]
fn a_fresh_hidden_pool_invalidates_an_older_attached_workspace_balance_but_preserves_its_cap() {
    let hidden = CreditsSnapshot::with_details(0., vec![], date(0), None, false, Some(true), false);
    for limit in [0., 400.] {
        let stale = stale(limit);
        let cost = CodexExtraUsageCost::resolving(Some(&hidden), Some(&stale)).unwrap();
        assert!(cost.balance.is_none());
        assert_eq!(cost.balance_updated_at, Some(date(0)));
        assert_eq!(cost.balance_is_unavailable, Some(true));
        assert_eq!(cost.limit, limit);
        let attached = CodexExtraUsageCost::attaching(empty(Some(stale.clone())), Some(&hidden));
        assert_eq!(attached.provider_cost, Some(cost));
        let display =
            CodexExtraUsageCost::credits_for_display(Some(&hidden), Some(&stale)).unwrap();
        assert!(!display.balance_read_succeeded);
        assert_eq!(
            display.display_remaining(),
            if limit > 0. { Some(limit) } else { None }
        );
    }
}

// upstream: CodexWorkspaceBalanceReconciliationTests.swift:72 a cap only refresh still preserves a cached workspace balance
#[test]
fn a_cap_only_refresh_still_preserves_a_cached_workspace_balance() {
    let cap_only = CreditsSnapshot::with_details(
        0.,
        vec![],
        date(0),
        credits(0., false, true).codex_credit_limit,
        false,
        None,
        false,
    );
    let display =
        CodexExtraUsageCost::credits_for_display(Some(&cap_only), Some(&stale(0.))).unwrap();
    assert_eq!(display.display_remaining(), Some(1234.));
    assert!(display.has_workspace_balance());
}

// upstream: CodexWorkspaceBalanceReconciliationTests.swift:92 hidden workspace observations survive persistence and reject older live balances
#[test]
fn hidden_workspace_observations_survive_persistence_and_reject_older_live_balances() {
    for has_cap in [false, true] {
        for cached in [false, true] {
            let cap = if has_cap {
                credits(0., false, true).codex_credit_limit
            } else {
                None
            };
            let old = CreditsSnapshot::with_details(
                1234.,
                vec![],
                date(-60),
                cap.clone(),
                true,
                None,
                true,
            );
            let initial =
                CodexExtraUsageCost::attaching(empty(None), if cached { Some(&old) } else { None });
            let hidden =
                CreditsSnapshot::with_details(0., vec![], date(0), None, false, Some(true), false);
            let attached = CodexExtraUsageCost::attaching(initial, Some(&hidden));
            let decoded: UsageSnapshot =
                serde_json::from_slice(&serde_json::to_vec(&attached).unwrap()).unwrap();
            let resolved =
                CodexExtraUsageCost::resolving(Some(&old), decoded.provider_cost.as_ref()).unwrap();
            assert!(resolved.balance.is_none());
            assert_eq!(resolved.balance_updated_at, Some(date(0)));
            let display = CodexExtraUsageCost::credits_for_display(
                Some(&old),
                decoded.provider_cost.as_ref(),
            )
            .unwrap();
            assert!(!display.balance_read_succeeded);
            assert!(!display.has_workspace_balance());
            assert_eq!(display.updated_at, date(0));
            assert_eq!(
                display.display_remaining(),
                if has_cap { Some(100.) } else { None }
            );
            let placeholder =
                CreditsSnapshot::with_details(0., vec![], date(30), cap, false, None, false);
            let refreshed = CodexExtraUsageCost::attaching(decoded, Some(&placeholder));
            let still_hidden = CodexExtraUsageCost::credits_for_display(
                Some(&old),
                refreshed.provider_cost.as_ref(),
            )
            .unwrap();
            assert!(!still_hidden.balance_read_succeeded);
            assert_eq!(
                still_hidden.display_remaining(),
                if has_cap { Some(100.) } else { None }
            );
            for balance in [0., 1200.] {
                let recovered =
                    CreditsSnapshot::new(balance, vec![], date(60)).with_balance_is_workspace(true);
                let display = CodexExtraUsageCost::credits_for_display(
                    Some(&recovered),
                    refreshed.provider_cost.as_ref(),
                )
                .unwrap();
                assert_eq!(display.display_remaining(), Some(balance));
                assert!(display.has_workspace_balance());
                let cost = CodexExtraUsageCost::resolving(
                    Some(&recovered),
                    refreshed.provider_cost.as_ref(),
                )
                .unwrap();
                assert_eq!(
                    cost.balance,
                    if balance > 0. { Some(balance) } else { None }
                );
                assert!(cost.balance_is_unavailable.is_none());
            }
        }
    }
}

// upstream: CodexWorkspaceBalanceReconciliationTests.swift:151 only explicit workspace provenance overrides the monthly display
#[test]
fn only_explicit_workspace_provenance_overrides_the_monthly_display() {
    for (balance, workspace, read, expected) in [
        (0., false, true, 100.),
        (14., false, true, 100.),
        (0., true, true, 0.),
        (1234., true, true, 1234.),
        (0., true, false, 100.),
    ] {
        assert_eq!(
            credits(balance, workspace, read).display_remaining(),
            Some(expected)
        );
    }
}

// upstream: CodexWorkspaceBalanceReconciliationTests.swift:160 a newer workspace zero clears a cached positive balance and retains its source
#[test]
fn a_newer_workspace_zero_clears_a_cached_positive_balance_and_retains_its_source() {
    let cost =
        CodexExtraUsageCost::resolving(Some(&credits(0., true, true)), Some(&stale(0.))).unwrap();
    assert!(cost.balance.is_none());
    assert_eq!(cost.balance_is_workspace, Some(true));
    assert_eq!(cost.balance_updated_at, Some(date(0)));
}

// upstream: CodexWorkspaceBalanceReconciliationTests.swift:176 a newer ordinary balance does not inherit cached workspace provenance
#[test]
fn a_newer_ordinary_balance_does_not_inherit_cached_workspace_provenance() {
    let cost =
        CodexExtraUsageCost::resolving(Some(&credits(14., false, true)), Some(&stale(0.))).unwrap();
    assert_eq!(cost.balance, Some(14.));
    assert_ne!(cost.balance_is_workspace, Some(true));
}

// upstream: CodexWorkspaceBalanceReconciliationTests.swift:191 workspace provenance survives persistence and defaults off in legacy snapshots
#[test]
fn workspace_provenance_survives_persistence_and_defaults_off_in_legacy_snapshots() {
    let credits = credits(1234., true, true);
    let encoded = serde_json::to_vec(&credits).unwrap();
    assert_eq!(
        serde_json::from_slice::<CreditsSnapshot>(&encoded).unwrap(),
        credits
    );
    let cost = CodexExtraUsageCost::provider_cost(Some(&credits)).unwrap();
    assert_eq!(
        serde_json::from_slice::<ProviderCostSnapshot>(&serde_json::to_vec(&cost).unwrap())
            .unwrap(),
        cost
    );
    let mut legacy: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    legacy.as_object_mut().unwrap().remove("balanceIsWorkspace");
    legacy.as_object_mut().unwrap().remove("creditsAvailable");
    let decoded: CreditsSnapshot = serde_json::from_value(legacy).unwrap();
    assert!(!decoded.balance_is_workspace);
    assert!(decoded.credits_available.is_none());
    assert_eq!(decoded.display_remaining(), Some(100.));
}
