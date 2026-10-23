#![cfg(unix)]

use ariadusage_macos::icon::IconInput;
use ariadusage_macos::presentation::{
    IconPreferences, connection_status, icon_input, icon_provider,
};
use ariadusage_protocol::fixture::generate_scenario_snapshot;

#[test]
fn icon_follows_the_selected_provider_or_the_first_enabled_one() {
    let snapshot = generate_scenario_snapshot(0);
    assert_eq!(
        icon_provider(&snapshot, None).map(|p| p.id.as_str()),
        Some("claude")
    );
    assert_eq!(
        icon_provider(&snapshot, Some("codex")).map(|p| p.id.as_str()),
        Some("codex")
    );
    assert_eq!(
        icon_provider(&snapshot, Some("missing")).map(|p| p.id.as_str()),
        Some("claude")
    );
}

#[test]
fn value_windows_become_remaining_percentages() {
    let snapshot = generate_scenario_snapshot(0);
    let codex = icon_provider(&snapshot, Some("codex")).unwrap();
    let input = icon_input(codex, IconPreferences::default());
    assert_eq!(input.primary, Some(58.0));
    assert!(!input.stale);
    assert!(!input.reserves_missing_secondary_lane);

    let used = icon_input(codex, IconPreferences { show_used: true });
    assert_eq!(used.primary, Some(42.0));
}

#[test]
fn claude_reserves_the_missing_weekly_lane() {
    let snapshot = generate_scenario_snapshot(0);
    let claude = icon_provider(&snapshot, Some("claude")).unwrap();
    assert!(icon_input(claude, IconPreferences::default()).reserves_missing_secondary_lane);
}

#[test]
fn loading_unknown_and_failed_windows_never_render_a_number() {
    // Step 2: Claude loading, Codex failed. Step 4: Claude unknown.
    for (step, id) in [(2, "claude"), (2, "codex"), (4, "claude")] {
        let snapshot = generate_scenario_snapshot(step);
        let provider = icon_provider(&snapshot, Some(id)).unwrap();
        let input = icon_input(provider, IconPreferences::default());
        assert_eq!(input.primary, None, "step {step} provider {id}");
    }
}

#[test]
fn stale_windows_keep_their_value_and_dim_the_icon() {
    // Step 1: Claude stale.
    let snapshot = generate_scenario_snapshot(1);
    let claude = icon_provider(&snapshot, Some("claude")).unwrap();
    let input = icon_input(claude, IconPreferences::default());
    assert_eq!(input.primary, Some(58.0));
    assert!(input.stale);
}

#[test]
fn connection_status_reports_the_engine_state_without_data() {
    assert_eq!(
        connection_status(None, false).as_deref(),
        Some("Waiting for the AriadUsage engine…")
    );
    assert_eq!(
        connection_status(None, true).as_deref(),
        Some("Refreshing…")
    );
    let snapshot = generate_scenario_snapshot(0);
    assert_eq!(connection_status(Some(&snapshot), true), None);
    assert_eq!(
        connection_status(Some(&snapshot), false).as_deref(),
        Some("Waiting for the AriadUsage engine…")
    );
}

#[test]
fn default_input_has_no_values() {
    let input = IconInput::default();
    assert_eq!((input.primary, input.secondary), (None, None));
}
