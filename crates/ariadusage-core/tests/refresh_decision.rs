// Ported from CodexBar Tests/CodexBarTests/ClaudeResilienceTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::{RateWindow, UsageSnapshot};
use ariadusage_core::projection::{
    ProjectedError, ProjectionInput, RefreshState, provider_snapshot,
};
use ariadusage_core::refresh::{
    FailureGate, FailureInput, SurfacePolicy, failure_decision, on_account_change,
};
use ariadusage_protocol::{
    Confidence, MetricState, ProviderErrorCategory, ProviderErrorKind, ProviderId, SourceKind,
};
use jiff::Timestamp;

fn make_test_snapshot(now: Timestamp) -> UsageSnapshot {
    let window = RateWindow::new(42.0, Some(300), None, None, None, false).unwrap();
    UsageSnapshot::new(
        Some(window),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap()
}

#[test]
fn timeout_keeps_prior_claude_snapshot_without_surfacing_repeated_failure() {
    // Ported from ClaudeResilienceTests.swift:200-283
    // Re-expressed on failure_decision with SurfacePolicy::NeverWhenPreserved
    let mut gate = FailureGate::new();
    let input = FailureInput::new(true, true, SurfacePolicy::NeverWhenPreserved, false);

    // First timeout failure
    let first = failure_decision(input, &mut gate);
    assert!(!first.surface_error);
    assert!(!first.drop_snapshot);
    assert!(!first.publish);
    assert_eq!(gate.streak, 1);

    // Second timeout failure (repeated failure)
    let second = failure_decision(input, &mut gate);
    assert!(!second.surface_error);
    assert!(!second.drop_snapshot);
    assert!(!second.publish);
    assert_eq!(gate.streak, 2);
}

#[test]
fn failure_decision_full_truth_table() {
    // Truth table over had_prior × preservable × policy × restored_history
    let bools = [false, true];
    let policies = [
        SurfacePolicy::Gate,
        SurfacePolicy::AlwaysWithPrior,
        SurfacePolicy::NeverWhenPreserved,
    ];

    for &had_prior in &bools {
        for &preservable in &bools {
            for &policy in &policies {
                for &restored_history in &bools {
                    let mut gate = FailureGate::new();
                    let input = FailureInput::new(had_prior, preservable, policy, restored_history);

                    // First failure evaluation
                    let dec1 = failure_decision(input, &mut gate);
                    assert_eq!(gate.streak, 1);

                    if !had_prior {
                        // Without prior data, ANY failure surfaces immediately and publishes
                        assert!(dec1.surface_error);
                        assert!(dec1.publish);
                        assert_eq!(dec1.drop_snapshot, !preservable);
                    } else {
                        // With prior data, policy overrides determine behavior
                        match policy {
                            SurfacePolicy::NeverWhenPreserved if preservable => {
                                assert!(!dec1.surface_error);
                                assert!(!dec1.drop_snapshot);
                                assert!(!dec1.publish);
                            }
                            SurfacePolicy::AlwaysWithPrior => {
                                assert!(dec1.surface_error);
                                assert!(dec1.publish);
                                assert_eq!(dec1.drop_snapshot, !preservable);
                            }
                            _ if restored_history => {
                                assert!(dec1.surface_error);
                                assert!(dec1.publish);
                                assert_eq!(dec1.drop_snapshot, !preservable);
                            }
                            _ => {
                                // Default gate: first failure is hidden
                                assert!(!dec1.surface_error);
                                assert!(!dec1.drop_snapshot);
                                assert!(!dec1.publish);
                            }
                        }
                    }

                    // Second consecutive failure evaluation
                    let dec2 = failure_decision(input, &mut gate);
                    assert_eq!(gate.streak, 2);

                    if !had_prior {
                        assert!(dec2.surface_error);
                        assert!(dec2.publish);
                        assert_eq!(dec2.drop_snapshot, !preservable);
                    } else {
                        match policy {
                            SurfacePolicy::NeverWhenPreserved if preservable => {
                                // NeverWhenPreserved keeps suppressing repeated failures
                                assert!(!dec2.surface_error);
                                assert!(!dec2.drop_snapshot);
                                assert!(!dec2.publish);
                            }
                            SurfacePolicy::AlwaysWithPrior => {
                                assert!(dec2.surface_error);
                                assert!(dec2.publish);
                                assert_eq!(dec2.drop_snapshot, !preservable);
                            }
                            _ if restored_history => {
                                assert!(dec2.surface_error);
                                assert!(dec2.publish);
                                assert_eq!(dec2.drop_snapshot, !preservable);
                            }
                            _ => {
                                // Default gate: second failure surfaces
                                assert!(dec2.surface_error);
                                assert!(dec2.publish);
                                assert_eq!(dec2.drop_snapshot, !preservable);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn account_change_drops_snapshot_and_resets_gate() {
    let mut gate = FailureGate::new();
    // Simulate previous failures
    let _ = gate.should_surface(true);
    let _ = gate.should_surface(true);
    assert_eq!(gate.streak, 2);

    // Account change drops old snapshot and resets the gate
    let change = on_account_change(&mut gate);
    assert!(change.drop_snapshot);
    assert_eq!(gate.streak, 0);

    // Subsequent preservable failure on the new account has had_prior = false
    // (because the old snapshot was dropped on account change)
    let failure_input = FailureInput::new(false, true, SurfacePolicy::Gate, false);
    let decision = failure_decision(failure_input, &mut gate);

    // Error surfaces immediately because there is no prior snapshot for the new account
    assert!(decision.surface_error);
    assert!(decision.publish);
    assert!(!decision.drop_snapshot);
    assert_eq!(gate.streak, 1);

    let (refresh, kept) = decision.to_projection_refresh(false);
    assert_eq!(refresh, RefreshState::Error);
    assert!(!kept);
}

#[test]
fn decision_to_projection_mapping_surfaced_preservable_failure() {
    let mut gate = FailureGate::new();
    let _ = gate.should_surface(true); // streak 1

    // Second failure: surfaced preservable failure with prior snapshot
    let input = FailureInput::new(true, true, SurfacePolicy::Gate, false);
    let decision = failure_decision(input, &mut gate);
    assert!(decision.surface_error);
    assert!(!decision.drop_snapshot);
    assert!(decision.publish);

    let (refresh, kept) = decision.to_projection_refresh(true);
    assert_eq!(refresh, RefreshState::Error);
    assert!(kept);

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snapshot = make_test_snapshot(now);

    let err = ProjectedError::new(
        ProviderErrorKind::NetworkFailure,
        ProviderErrorCategory::Network,
    );

    let proj_input = ProjectionInput {
        provider: ProviderId::new("claude").unwrap(),
        display_name: "Claude".to_string(),
        enabled: true,
        source_mode: "auto".to_string(),
        refresh,
        kept_snapshot: kept,
        as_of: now,
        source_kind: SourceKind::Cli,
        source_label: "test".to_string(),
        error: Some(err),
        pace: None,
    };

    let projected = provider_snapshot(&snapshot, &proj_input);
    let primary = projected.windows.primary.expect("primary window present");

    // Must yield stale state with retained numeric value and last_error
    assert_eq!(primary.state, MetricState::Stale);
    assert!(primary.value.is_some());
    assert_eq!(primary.value.as_ref().unwrap().used_percent, 42.0);

    let last_err = projected.last_error.expect("last_error present");
    assert_eq!(last_err.category, ProviderErrorCategory::Network);
    assert_eq!(last_err.kind, ProviderErrorKind::NetworkFailure);
    assert_eq!(last_err.message, "Network transport failure");
}

#[test]
fn decision_to_projection_mapping_surfaced_non_preservable_failure() {
    let mut gate = FailureGate::new();
    let _ = gate.should_surface(true); // streak 1

    // Second failure: non-preservable (auth expired) drops snapshot
    let input = FailureInput::new(true, false, SurfacePolicy::Gate, false);
    let decision = failure_decision(input, &mut gate);
    assert!(decision.surface_error);
    assert!(decision.drop_snapshot);
    assert!(decision.publish);

    let (refresh, kept) = decision.to_projection_refresh(true);
    assert_eq!(refresh, RefreshState::Error);
    assert!(!kept);

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snapshot = make_test_snapshot(now);

    let err = ProjectedError::new(
        ProviderErrorKind::AuthenticationExpired,
        ProviderErrorCategory::Auth,
    );

    let proj_input = ProjectionInput {
        provider: ProviderId::new("claude").unwrap(),
        display_name: "Claude".to_string(),
        enabled: true,
        source_mode: "auto".to_string(),
        refresh,
        kept_snapshot: kept,
        as_of: now,
        source_kind: SourceKind::Cli,
        source_label: "test".to_string(),
        error: Some(err),
        pace: None,
    };

    let projected = provider_snapshot(&snapshot, &proj_input);
    let primary = projected.windows.primary.expect("primary window present");

    // Dropped snapshot yields Error state without value
    assert_eq!(primary.state, MetricState::Error);
    assert!(primary.value.is_none());
    assert!(projected.last_error.is_some());
}

#[test]
fn decision_to_projection_mapping_hidden_flake_failure() {
    let mut gate = FailureGate::new();

    // First failure: hidden flake
    let input = FailureInput::new(true, true, SurfacePolicy::Gate, false);
    let decision = failure_decision(input, &mut gate);
    assert!(!decision.surface_error);
    assert!(!decision.drop_snapshot);
    assert!(!decision.publish);

    let (refresh, kept) = decision.to_projection_refresh(true);
    assert_eq!(refresh, RefreshState::Fresh);
    assert!(kept);

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snapshot = make_test_snapshot(now);

    let proj_input = ProjectionInput {
        provider: ProviderId::new("claude").unwrap(),
        display_name: "Claude".to_string(),
        enabled: true,
        source_mode: "auto".to_string(),
        refresh,
        kept_snapshot: kept,
        as_of: now,
        source_kind: SourceKind::Cli,
        source_label: "test".to_string(),
        error: None,
        pace: None,
    };

    let projected = provider_snapshot(&snapshot, &proj_input);
    let primary = projected.windows.primary.expect("primary window present");

    assert_eq!(primary.state, MetricState::Value);
    assert!(primary.value.is_some());
    assert!(projected.last_error.is_none());
}
