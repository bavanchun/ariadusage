#![cfg(unix)]

use ariadusage_macos::icon::IconInput;
use ariadusage_macos::presentation::{IconPreferences, icon_input, icon_provider, provider_lines};
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
fn menu_lines_describe_each_enabled_provider() {
    let snapshot = generate_scenario_snapshot(0);
    let lines = provider_lines(Some(&snapshot), true, IconPreferences::default());
    let texts: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Claude: 58% left",
            "Codex: 58% left",
            "Antigravity: 58% left"
        ]
    );
}

#[test]
fn menu_lines_report_the_connection_state_without_data() {
    let waiting = provider_lines(None, false, IconPreferences::default());
    assert_eq!(waiting[0].text, "Waiting for the AriadUsage engine…");
    let refreshing = provider_lines(None, true, IconPreferences::default());
    assert_eq!(refreshing[0].text, "Refreshing…");
}

#[test]
fn default_input_has_no_values() {
    let input = IconInput::default();
    assert_eq!((input.primary, input.secondary), (None, None));
}
