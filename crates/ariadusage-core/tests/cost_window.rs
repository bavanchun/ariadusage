// Ported from CodexBar Tests/CodexBarTests/ProviderCostWindowTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::ProviderCostSnapshot;
use jiff::Timestamp;

#[test]
fn spend_budget_meters_clamp_usage_and_retain_reset_metadata() {
    let cases = [
        (-5.0, 0.0),
        (0.0, 0.0),
        (25.0, 25.0),
        (100.0, 100.0),
        (150.0, 100.0),
    ];

    let reset = Timestamp::from_second(1_800_000_000).expect("timestamp");

    for (used, expected) in cases {
        let mut cost = ProviderCostSnapshot::new(used, 100.0, "USD", reset);
        cost.resets_at = Some(reset);

        let window = cost
            .spend_limit_window()
            .unwrap_or_else(|| panic!("spend limit window should exist for used: {used}"));

        assert_eq!(window.used_percent, expected);
        assert_eq!(window.resets_at, Some(reset));
        assert_eq!(window.window_minutes, None);
        assert_eq!(window.reset_description, None);
        assert!(!window.is_synthetic_placeholder);
    }
}

#[test]
fn balance_only_or_invalid_limits_never_become_quota_meters() {
    let now = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let invalid_limits = [0.0, -1.0, f64::NAN];

    for limit in invalid_limits {
        let cost = ProviderCostSnapshot::new(10.0, limit, "USD", now);
        assert!(
            cost.spend_limit_window().is_none(),
            "Expected None for limit {limit}"
        );
    }
}

#[test]
fn only_credit_denominated_costs_are_not_monetary() {
    let updated_at = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let credits = ProviderCostSnapshot::new(
        200.0,
        1000.0,
        ProviderCostSnapshot::CREDITS_CURRENCY_CODE,
        updated_at,
    );
    assert!(!credits.is_monetary());
    // The spend meter itself does not depend on the currency.
    assert_eq!(credits.spend_limit_window().unwrap().used_percent, 20.0);

    for code in ["USD", "EUR", "credits", ""] {
        assert!(ProviderCostSnapshot::new(1.0, 10.0, code, updated_at).is_monetary());
    }
}
