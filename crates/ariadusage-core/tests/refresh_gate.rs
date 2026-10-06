// Ported from CodexBar Tests/CodexBarTests/ClaudeResilienceTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/UsageStoreCoverageTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::refresh::FailureGate;

#[test]
fn suppresses_single_flake_when_prior_data_exists() {
    let mut gate = FailureGate::new();
    let first_failure = gate.should_surface(true);
    let second_failure = gate.should_surface(true);
    assert!(!first_failure);
    assert!(second_failure);
}

#[test]
fn surfaces_failure_without_prior_data() {
    let mut gate = FailureGate::new();
    let should_surface = gate.should_surface(false);
    assert!(should_surface);
}

#[test]
fn resets_after_success() {
    let mut gate = FailureGate::new();
    let _ = gate.should_surface(true);
    gate.record_success();
    let should_surface = gate.should_surface(true);
    assert!(!should_surface);
}

#[test]
fn failure_gate_coverage_and_reset() {
    let mut gate = FailureGate::new();
    let first = gate.should_surface(true);
    assert!(!first);
    let second = gate.should_surface(true);
    assert!(second);
    gate.record_success();
    let third = gate.should_surface(false);
    assert!(third);
    gate.reset();
    assert_eq!(gate.streak, 0);
    assert_eq!(gate.streak(), 0);
}
