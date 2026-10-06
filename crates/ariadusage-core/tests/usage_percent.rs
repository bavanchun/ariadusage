// Ported from CodexBar Tests/CodexBarTests/UsagePercentTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::UsagePercent;

#[test]
fn display_normalization_preserves_boundaries_and_small_percentages() {
    assert_eq!(UsagePercent::new(0.0).display_clamped(), 0.0);
    assert_eq!(UsagePercent::new(0.25).display_clamped(), 0.25);
    assert_eq!(UsagePercent::new(100.0).display_clamped(), 100.0);
}

#[test]
fn display_normalization_clamps_overage_while_preserving_the_raw_percentage() {
    let percent = UsagePercent::from_ratio_unclamped(150.0, 100.0);

    assert_eq!(percent.raw, 150.0);
    assert_eq!(percent.display_clamped(), 100.0);
}

#[test]
fn display_normalization_guards_negative_usage() {
    let percent = UsagePercent::from_ratio_unclamped(-1.0, 100.0);

    assert_eq!(percent.raw, -1.0);
    assert_eq!(percent.display_clamped(), 0.0);
}
