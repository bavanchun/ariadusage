// Ported from CodexBar Tests/CodexBarTests/MenuBarMetricWindowResolverTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeSpendWindowTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/MergedWarpIconRenderingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemAnimationSignatureTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemIconObservationSignatureTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemAnimationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexbarTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

use std::collections::HashSet;

use ariadusage_macos::icon::{IconInput, IconStyle, StatusLevel, icon_render_signature, layout};
use ariadusage_macos::presentation::{
    IconPreferences, MenuBarMetricPreference, UnifiedIconContext, UnifiedIconSource,
    VISIBLE_ZERO_PERCENT, antigravity_quota_summary_families_are_all_blocked,
    antigravity_quota_summary_ranking_window, claude_monthly_spend_window,
    claude_scoped_weekly_window, connection_status, end_of_utc_month, icon_input,
    icon_input_with_override, icon_provider, inferred_monthly_window_minutes,
    primary_provider_for_unified_icon, primary_provider_for_unified_icon_with_overview,
    quota_layout_policy_for_provider, resolve_menu_bar_metric_window,
    resolved_claude_reset_window_for_pace, resolved_icon_percents,
    resolved_icon_percents_for_style, resolved_icon_windows, status_item_icon_input,
    store_icon_observation_signature, store_icon_observation_signature_with_override,
    store_icon_style,
};
use ariadusage_protocol::fixture::generate_scenario_snapshot;
use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::metric::{Confidence, Metric};
use ariadusage_protocol::snapshot::{EngineInfo, EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::{
    Cost, Credits, Identity, NamedWindow, ProviderStatus, RateWindow, StatusIndicator,
};
use jiff::Timestamp;

fn metric_ok<T>(val: T) -> Metric<T> {
    Metric::value(val, None, None, Confidence::Exact)
}

fn make_provider(id: &str) -> ProviderSnapshot {
    ProviderSnapshot::new(ProviderId::new(id).unwrap(), id, true, "cli")
}

fn rate_window(
    used_percent: f64,
    window_minutes: Option<u32>,
    resets_at: Option<Timestamp>,
) -> RateWindow {
    let mut w = RateWindow::new(used_percent);
    w.window_minutes = window_minutes;
    w.resets_at = resets_at;
    w
}

fn extra_window(
    id: &str,
    label: &str,
    used_percent: f64,
    window_minutes: Option<u32>,
    resets_at: Option<Timestamp>,
) -> NamedWindow {
    NamedWindow {
        id: id.to_string(),
        title: label.to_string(),
        window: metric_ok(rate_window(used_percent, window_minutes, resets_at)),
    }
}

fn make_cost(used: f64, limit: Option<f64>, updated_at: Option<Timestamp>) -> Cost {
    Cost {
        used,
        limit,
        currency_code: "USD".to_string(),
        period: Some("Monthly".to_string()),
        resets_at: None,
        balance: None,
        next_regen_amount: None,
        personal_used: None,
        balance_updated_at: updated_at,
        balance_is_workspace: None,
        balance_is_unavailable: None,
    }
}

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

    let used = icon_input(
        codex,
        IconPreferences {
            show_used: true,
            ..IconPreferences::default()
        },
    );
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
fn stale_and_no_data_follow_upstream_usage_store_error_rule() {
    // Upstream `UsageStore.isStale` is `errors[provider] != nil` (`UsageStore.swift:701-703`).
    // Step 1: Claude has a stale window value without `last_error` -> keeps value and `stale: false`.
    let snapshot = generate_scenario_snapshot(1);
    let claude = icon_provider(&snapshot, Some("claude")).unwrap();
    let input = icon_input(claude, IconPreferences::default());
    assert_eq!(input.primary, Some(58.0));
    assert!(!input.stale);

    // Step 2: Codex has `last_error.is_some()` -> `stale: true`.
    let step2 = generate_scenario_snapshot(2);
    let codex_err = icon_provider(&step2, Some("codex")).unwrap();
    let err_input = icon_input(codex_err, IconPreferences::default());
    assert!(err_input.stale);

    // No snapshot -> empty lanes with `stale: false` (0.28 / 0.44 track palette).
    let (no_prov, no_data_input) =
        status_item_icon_input(None, None, true, IconPreferences::default());
    assert!(no_prov.is_none());
    assert!(!no_data_input.stale);
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

// --- Ported from CodexbarTests.swift (IconRemainingResolver tests) ---

#[test]
fn antigravity_icon_ignores_legacy_model_quota_lanes() {
    let mut provider = make_provider("antigravity");
    provider.windows.primary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(55.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(10.0, None, None)));

    let (p_win, s_win) = resolved_icon_windows(&provider, None, None);
    let (p_pct, s_pct) = resolved_icon_percents(&provider, false, None, None);
    let (p_used, s_used) = resolved_icon_percents(&provider, true, None, None);
    assert_eq!((p_win, s_win), (None, None));
    assert_eq!((p_pct, s_pct), (None, None));
    assert_eq!((p_used, s_used), (None, None));
}

#[test]
fn antigravity_quota_summary_icon_shows_session_on_top_and_weekly_on_bottom() {
    let mut provider = make_provider("antigravity");
    provider.windows.primary = Some(metric_ok(rate_window(55.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(10.0, None, None)));
    provider.windows.extra = vec![
        extra_window(
            "antigravity-quota-summary-gemini-pro-session",
            "Gemini Pro 5h",
            20.0,
            Some(300),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-gemini-pro-weekly",
            "Gemini Pro Weekly",
            85.0,
            Some(10080),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-claude-gpt-session",
            "Claude / GPT 5h",
            55.0,
            Some(300),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-gemini-flash-weekly",
            "Gemini Flash Weekly",
            60.0,
            Some(10080),
            None,
        ),
    ];

    let (p_rem, s_rem) = resolved_icon_percents(&provider, false, None, None);
    let (p_used, s_used) = resolved_icon_percents(&provider, true, None, None);
    assert_eq!(p_rem, Some(45.0));
    assert_eq!(s_rem, Some(15.0));
    assert_eq!(p_used, Some(55.0));
    assert_eq!(s_used, Some(85.0));
}

#[test]
fn antigravity_quota_summary_icon_uses_most_constrained_lanes() {
    let mut provider = make_provider("antigravity");
    provider.windows.extra = vec![
        extra_window(
            "antigravity-quota-summary-gemini-pro-session",
            "Gemini Pro 5h",
            100.0,
            Some(300),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-claude-gpt-session",
            "Claude / GPT 5h",
            55.0,
            Some(300),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-gemini-flash-weekly",
            "Gemini Flash Weekly",
            100.0,
            Some(10080),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-claude-gpt-weekly",
            "Claude / GPT Weekly",
            35.0,
            Some(10080),
            None,
        ),
    ];

    let (p_rem, s_rem) = resolved_icon_percents(&provider, false, None, None);
    let (p_used, s_used) = resolved_icon_percents(&provider, true, None, None);
    assert_eq!(p_rem, Some(0.0));
    assert_eq!(s_rem, Some(0.0));
    assert_eq!(p_used, Some(100.0));
    assert_eq!(s_used, Some(100.0));
}

#[test]
fn antigravity_quota_summary_icon_tie_break_is_stable() {
    let mut provider = make_provider("antigravity");
    provider.windows.extra = vec![
        extra_window(
            "antigravity-quota-summary-gemini-pro-session",
            "Gemini Pro 5h",
            55.0,
            Some(300),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-claude-gpt-session",
            "Claude / GPT 5h",
            55.0,
            Some(300),
            None,
        ),
    ];
    let (primary, _) = resolved_icon_windows(&provider, None, None);
    assert_eq!(primary.map(|w| w.used_percent), Some(55.0));
}

#[test]
fn perplexity_icon_falls_back_to_purchased_lane_when_bonus_is_exhausted() {
    let mut provider = make_provider("perplexity");
    provider.windows.primary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(35.0, None, None)));

    let (p_rem, s_rem) = resolved_icon_percents(&provider, false, None, None);
    let (p_used, s_used) = resolved_icon_percents(&provider, true, None, None);
    assert_eq!(p_rem, Some(80.0));
    assert_eq!(s_rem, Some(65.0));
    assert_eq!(p_used, Some(20.0));
    assert_eq!(s_used, Some(35.0));
}

#[test]
fn perplexity_icon_skips_exhausted_recurring_lane_when_purchased_credits_remain() {
    let mut provider = make_provider("perplexity");
    provider.windows.primary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.secondary = None;
    provider.windows.tertiary = Some(metric_ok(rate_window(35.0, None, None)));

    let (p_rem, s_rem) = resolved_icon_percents(&provider, false, None, None);
    let (p_used, s_used) = resolved_icon_percents(&provider, true, None, None);
    assert_eq!(p_rem, Some(65.0));
    assert_eq!(s_rem, Some(0.0));
    assert_eq!(p_used, Some(35.0));
    assert_eq!(s_used, Some(100.0));
}

#[test]
fn perplexity_icon_prefers_purchased_lane_before_bonus() {
    let mut provider = make_provider("perplexity");
    provider.windows.primary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(10.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(35.0, None, None)));

    let (p_rem, s_rem) = resolved_icon_percents(&provider, false, None, None);
    let (p_used, s_used) = resolved_icon_percents(&provider, true, None, None);
    assert_eq!(p_rem, Some(80.0));
    assert_eq!(s_rem, Some(65.0));
    assert_eq!(p_used, Some(20.0));
    assert_eq!(s_used, Some(35.0));
}

#[test]
fn copilot_icon_can_use_selected_budget_as_secondary_lane() {
    let mut provider = make_provider("copilot");
    provider.windows.primary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(35.0, None, None)));
    provider.windows.extra = vec![extra_window(
        "copilot-premium-shared-1",
        "Premium shared budget",
        72.0,
        None,
        None,
    )];

    let (p_rem, s_rem) =
        resolved_icon_percents(&provider, false, Some("copilot-premium-shared-1"), None);
    assert_eq!(p_rem, Some(80.0));
    assert_eq!(s_rem, Some(28.0));

    let input = icon_input_with_override(
        &provider,
        IconPreferences::default(),
        Some("copilot-premium-shared-1"),
    );
    assert_eq!(input.primary, Some(80.0));
    assert_eq!(input.secondary, Some(28.0));
}

#[test]
fn copilot_icon_falls_back_to_chat_lane_when_selected_budget_is_unavailable() {
    let mut provider = make_provider("copilot");
    provider.windows.primary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(35.0, None, None)));

    let (p_rem, s_rem) =
        resolved_icon_percents(&provider, false, Some("copilot-missing-budget"), None);
    assert_eq!(p_rem, Some(80.0));
    assert_eq!(s_rem, Some(65.0));
}

#[test]
fn copilot_icon_uses_selected_budget_in_show_used_mode() {
    let mut provider = make_provider("copilot");
    provider.windows.primary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(35.0, None, None)));
    provider.windows.extra = vec![extra_window(
        "copilot-premium-shared-1",
        "Premium shared budget",
        72.0,
        None,
        None,
    )];

    let (p_used, s_used) =
        resolved_icon_percents(&provider, true, Some("copilot-premium-shared-1"), None);
    assert_eq!(p_used, Some(20.0));
    assert_eq!(s_used, Some(72.0));
}

#[test]
fn doubao_icon_falls_back_to_agent_extra_windows() {
    let mut provider = make_provider("doubao");
    provider.windows.extra = vec![
        extra_window(
            "doubao-agent-session",
            "Agent Session",
            30.0,
            Some(300),
            None,
        ),
        extra_window(
            "doubao-agent-weekly",
            "Agent Weekly",
            60.0,
            Some(10080),
            None,
        ),
    ];

    let (p_rem, s_rem) = resolved_icon_percents(&provider, false, None, None);
    assert_eq!(p_rem, Some(70.0));
    assert_eq!(s_rem, Some(40.0));
}

#[test]
fn warp_icon_preserves_exhausted_bonus_layout_in_show_used_mode() {
    let mut provider = make_provider("warp");
    provider.windows.primary = Some(metric_ok(rate_window(25.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(100.0, None, None)));

    let (p_rem, s_rem) =
        resolved_icon_percents_for_style(&provider, IconStyle::Warp, false, None, None);
    let (p_used, s_used) =
        resolved_icon_percents_for_style(&provider, IconStyle::Warp, true, None, None);

    assert_eq!(p_rem, Some(75.0));
    assert_eq!(s_rem, Some(0.0));
    assert_eq!(p_used, Some(25.0));
    assert_eq!(s_used, Some(0.0));

    // Verify IconLayout dims the exhausted secondary lane on BOTTOM_RECT in both modes
    let rem_layout = layout(&IconInput {
        primary: p_rem,
        secondary: s_rem,
        treats_exhausted_secondary_as_missing: true,
        style: IconStyle::Warp,
        ..IconInput::default()
    });
    let used_layout = layout(&IconInput {
        primary: p_used,
        secondary: s_used,
        treats_exhausted_secondary_as_missing: true,
        style: IconStyle::Warp,
        ..IconInput::default()
    });
    assert_eq!(rem_layout.bars.len(), 2);
    assert_eq!(rem_layout.bars[1].percent, None);
    assert!((rem_layout.bars[1].alpha - 0.45).abs() < 1e-6);
    assert_eq!(used_layout.bars.len(), 2);
    assert_eq!(used_layout.bars[1].percent, None);
    assert!((used_layout.bars[1].alpha - 0.45).abs() < 1e-6);
}

#[test]
fn warp_icon_keeps_unused_bonus_lane_visible_in_show_used_mode() {
    let mut provider = make_provider("warp");
    provider.windows.primary = Some(metric_ok(rate_window(25.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(0.0, None, None)));

    let (p_rem, s_rem) =
        resolved_icon_percents_for_style(&provider, IconStyle::Warp, false, None, None);
    let (p_used, s_used) =
        resolved_icon_percents_for_style(&provider, IconStyle::Warp, true, None, None);

    assert_eq!(p_rem, Some(75.0));
    assert_eq!(s_rem, Some(100.0));
    assert_eq!(p_used, Some(25.0));
    assert_eq!(s_used, Some(VISIBLE_ZERO_PERCENT));

    let used_layout = layout(&IconInput {
        primary: p_used,
        secondary: s_used,
        treats_exhausted_secondary_as_missing: true,
        style: IconStyle::Warp,
        ..IconInput::default()
    });
    assert_eq!(used_layout.bars.len(), 2);
    assert_eq!(used_layout.bars[1].percent, Some(VISIBLE_ZERO_PERCENT));
    assert!((used_layout.bars[1].alpha - 1.0).abs() < 1e-6);
}

#[test]
fn codex_icon_promotes_weekly_only_window_into_primary_display_lane() {
    let mut provider = make_provider("codex");
    provider.windows.secondary = Some(metric_ok(rate_window(20.0, Some(10080), None)));

    let (p_rem, s_rem) = resolved_icon_percents(&provider, false, None, None);
    assert_eq!(p_rem, Some(80.0));
    assert_eq!(s_rem, None);
}

#[test]
fn codex_icon_caps_session_only_until_exhausted_weekly_lane_resets() {
    let now: Timestamp = "2026-04-06T12:00:00Z".parse().unwrap();
    let future_reset: Timestamp = "2026-04-06T12:10:00Z".parse().unwrap();
    let expired_reset: Timestamp = "2026-04-06T11:50:00Z".parse().unwrap();

    let mut active = make_provider("codex");
    active.windows.primary = Some(metric_ok(rate_window(25.0, Some(300), None)));
    active.windows.secondary = Some(metric_ok(rate_window(
        100.0,
        Some(10080),
        Some(future_reset),
    )));

    let (active_rem_p, active_rem_s) = resolved_icon_percents(&active, false, None, Some(now));
    let (active_used_p, active_used_s) = resolved_icon_percents(&active, true, None, Some(now));
    assert_eq!(active_rem_p, Some(0.0));
    assert_eq!(active_rem_s, Some(0.0));
    assert_eq!(active_used_p, Some(100.0));
    assert_eq!(active_used_s, Some(100.0));

    let mut expired = make_provider("codex");
    expired.windows.primary = Some(metric_ok(rate_window(25.0, Some(300), None)));
    expired.windows.secondary = Some(metric_ok(rate_window(
        100.0,
        Some(10080),
        Some(expired_reset),
    )));

    let (exp_rem_p, exp_rem_s) = resolved_icon_percents(&expired, false, None, Some(now));
    let (exp_used_p, exp_used_s) = resolved_icon_percents(&expired, true, None, Some(now));
    assert_eq!(exp_rem_p, Some(75.0));
    assert_eq!(exp_rem_s, None);
    assert_eq!(exp_used_p, Some(25.0));
    assert_eq!(exp_used_s, None);
}

// --- Ported from ClaudeSpendWindowTests.swift ---

#[test]
fn claude_spend_only_account_has_meter_pace_and_first_of_month_rollover() {
    let march_18: Timestamp = "2026-03-18T14:22:09Z".parse().unwrap();
    let april_1: Timestamp = "2026-04-01T00:00:00Z".parse().unwrap();

    let mut provider = make_provider("claude");
    provider.identity = Some(metric_ok(Identity {
        provider_id: ProviderId::new("claude").unwrap(),
        account_email: None,
        account_organization: None,
        login_method: Some("Claude API".to_string()),
        account_id: None,
        plan: None,
    }));
    provider.cost = Some(metric_ok(make_cost(3100.0, Some(5000.0), Some(march_18))));

    let spend_win = claude_monthly_spend_window(&provider).expect("spend window");
    assert!((spend_win.used_percent - 62.0).abs() < 0.001);
    assert_eq!(spend_win.window_minutes, Some(43_200));
    assert_eq!(spend_win.resets_at, Some(april_1));

    let (icon_p, icon_s) = resolved_icon_windows(&provider, None, Some(march_18));
    assert_eq!(icon_p.as_ref().map(|w| w.used_percent), Some(62.0));
    assert_eq!(icon_s, None);

    // Spend-only Claude account does NOT reserve missing secondary lane
    let policy = quota_layout_policy_for_provider(&provider, None, Some(march_18));
    assert!(!policy.reserves_missing_secondary_lane);

    // MenuBarMetricWindowResolver resolves the spend window for Automatic, PrimaryAndSecondary, ExtraUsage
    for pref in [
        MenuBarMetricPreference::Automatic,
        MenuBarMetricPreference::PrimaryAndSecondary,
        MenuBarMetricPreference::ExtraUsage,
    ] {
        let resolved =
            resolve_menu_bar_metric_window(pref, &provider, false, false, Some(march_18));
        assert_eq!(
            resolved.as_ref().map(|w| w.used_percent),
            Some(62.0),
            "pref {pref:?}"
        );
    }

    // Pace window resolves 31-day March (44640 minutes)
    let pace_win = resolved_claude_reset_window_for_pace(&spend_win);
    assert_eq!(pace_win.window_minutes, Some(31 * 24 * 60));

    // Admin API login method suppresses claude_monthly_spend_window
    provider.identity = Some(metric_ok(Identity {
        provider_id: ProviderId::new("claude").unwrap(),
        account_email: None,
        account_organization: None,
        login_method: Some("Admin API".to_string()),
        account_id: None,
        plan: None,
    }));
    assert!(claude_monthly_spend_window(&provider).is_none());
}

#[test]
fn utc_month_rollover_and_pace_follow_leap_february_across_the_boundary() {
    let feb_20_2024: Timestamp = "2024-02-20T12:00:00Z".parse().unwrap();
    let mar_1_2024: Timestamp = "2024-03-01T00:00:00Z".parse().unwrap();
    assert_eq!(end_of_utc_month(feb_20_2024), Some(mar_1_2024));
    assert_eq!(
        inferred_monthly_window_minutes(mar_1_2024),
        Some(29 * 24 * 60)
    );

    let dec_15_2025: Timestamp = "2025-12-15T08:00:00Z".parse().unwrap();
    let jan_1_2026: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
    assert_eq!(end_of_utc_month(dec_15_2025), Some(jan_1_2026));
    assert_eq!(
        inferred_monthly_window_minutes(jan_1_2026),
        Some(31 * 24 * 60)
    );
}

#[test]
fn real_quota_lanes_keep_their_icon_and_reserve_missing_secondary_lane() {
    let mut provider = make_provider("claude");
    provider.windows.primary = Some(metric_ok(rate_window(40.0, Some(300), None)));
    provider.cost = Some(metric_ok(make_cost(3100.0, Some(5000.0), None)));

    let policy = quota_layout_policy_for_provider(&provider, None, None);
    assert!(policy.reserves_missing_secondary_lane);

    // ExtraUsage still returns the spend window even when primary quota exists
    let extra = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::ExtraUsage,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(extra.map(|w| w.used_percent), Some(62.0));
}

#[test]
fn claude_scoped_weekly_fallback_wins_when_top_level_lanes_are_absent() {
    let mut provider = make_provider("claude");
    provider.windows.extra = vec![
        extra_window(
            "claude-weekly-scoped-sonnet",
            "Sonnet Weekly",
            45.0,
            Some(10080),
            None,
        ),
        extra_window(
            "claude-weekly-scoped-opus",
            "Opus Weekly",
            80.0,
            Some(10080),
            None,
        ),
    ];

    assert_eq!(
        claude_scoped_weekly_window(&provider).map(|w| w.used_percent),
        Some(80.0)
    );
    let (icon_p, icon_s) = resolved_icon_windows(&provider, None, None);
    assert_eq!(icon_p.map(|w| w.used_percent), Some(80.0));
    assert_eq!(icon_s, None);
}

// --- Ported from MenuBarMetricWindowResolverTests.swift ---

#[test]
fn gemini_metrics_fall_back_to_flash_when_pro_is_unavailable() {
    let mut provider = make_provider("gemini");
    provider.windows.secondary = Some(metric_ok(rate_window(37.0, None, None)));

    for pref in [
        MenuBarMetricPreference::Automatic,
        MenuBarMetricPreference::Primary,
        MenuBarMetricPreference::Secondary,
        MenuBarMetricPreference::Average,
    ] {
        let win = resolve_menu_bar_metric_window(pref, &provider, true, false, None);
        assert_eq!(win.map(|w| w.used_percent), Some(37.0), "pref {pref:?}");
    }
}

#[test]
fn combined_primary_and_secondary_metric_uses_most_constrained_lane() {
    let mut provider = make_provider("codex");
    provider.windows.primary = Some(metric_ok(rate_window(25.0, Some(300), None)));
    provider.windows.secondary = Some(metric_ok(rate_window(75.0, Some(10080), None)));

    let win = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(win.map(|w| w.used_percent), Some(75.0));

    let avg = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Average,
        &provider,
        true,
        false,
        None,
    );
    assert_eq!(avg.map(|w| w.used_percent), Some(50.0));
}

#[test]
fn cursor_automatic_metric_skips_exhausted_subquota_when_total_remains_usable() {
    let mut provider = make_provider("cursor");
    provider.windows.primary = Some(metric_ok(rate_window(28.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(45.0, None, None)));

    let win = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(win.map(|w| w.used_percent), Some(45.0));
}

#[test]
fn cursor_automatic_metric_keeps_exhausted_total_when_subquota_remains_usable() {
    let mut provider = make_provider("cursor");
    provider.windows.primary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(40.0, None, None)));

    let win = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(win.map(|w| w.used_percent), Some(100.0));
}

#[test]
fn cursor_automatic_metric_reports_exhausted_when_all_subquotas_exhausted() {
    let mut provider = make_provider("cursor");
    provider.windows.secondary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.extra = vec![extra_window(
        "cursor-grok-bot",
        "Grok Bot",
        100.0,
        None,
        None,
    )];

    let win = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(win.map(|w| w.used_percent), Some(100.0));
}

#[test]
fn litellm_automatic_metric_uses_team_budget_and_prioritizes_exhausted() {
    let mut provider = make_provider("litellm");
    provider.windows.primary = Some(metric_ok(rate_window(60.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(35.0, None, None)));

    let win = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    // Secondary (team budget) is preferred when neither is exhausted
    assert_eq!(win.map(|w| w.used_percent), Some(35.0));

    // Exhausted primary wins over non-exhausted secondary
    provider.windows.primary = Some(metric_ok(rate_window(100.0, None, None)));
    let exhausted = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(exhausted.map(|w| w.used_percent), Some(100.0));
}

#[test]
fn default_automatic_metric_prioritizes_exhausted_secondary_or_tertiary_and_highest_usage() {
    let mut provider = make_provider("codex");
    provider.windows.primary = Some(metric_ok(rate_window(30.0, Some(300), None)));
    provider.windows.secondary = Some(metric_ok(rate_window(85.0, Some(10080), None)));

    let win = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(win.map(|w| w.used_percent), Some(85.0));

    provider.windows.tertiary = Some(metric_ok(rate_window(100.0, None, None)));
    let exhausted = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(exhausted.map(|w| w.used_percent), Some(100.0));
}

#[test]
fn antigravity_automatic_metric_and_exhausted_first_ranking_and_family_blocking() {
    let now: Timestamp = "2026-04-06T12:00:00Z".parse().unwrap();
    let sooner: Timestamp = "2026-04-06T13:00:00Z".parse().unwrap();
    let later: Timestamp = "2026-04-06T15:00:00Z".parse().unwrap();

    let mut provider = make_provider("antigravity");
    provider.windows.extra = vec![
        extra_window(
            "antigravity-quota-summary-gemini-pro-session",
            "Gemini Pro 5h",
            100.0,
            Some(300),
            Some(later),
        ),
        extra_window(
            "antigravity-quota-summary-gemini-pro-weekly",
            "Gemini Pro Weekly",
            40.0,
            Some(10080),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-claude-gpt-session",
            "Claude / GPT 5h",
            65.0,
            Some(300),
            Some(sooner),
        ),
        extra_window(
            "antigravity-quota-summary-claude-gpt-weekly",
            "Claude / GPT Weekly",
            30.0,
            Some(10080),
            None,
        ),
    ];

    // Not all families are blocked because claude-gpt is still at 65% / 30%
    assert!(!antigravity_quota_summary_families_are_all_blocked(
        &provider
    ));

    // Standard Automatic skips the 100% exhausted family when a usable family exists
    let standard = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        Some(now),
    );
    assert_eq!(standard.map(|w| w.used_percent), Some(65.0));

    // Opt-in exhausted-first ranking picks the 100% lane
    let opt_in = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        true,
        Some(now),
    );
    assert_eq!(opt_in.map(|w| w.used_percent), Some(100.0));
    assert_eq!(
        antigravity_quota_summary_ranking_window(&provider, Some(now)).map(|w| w.used_percent),
        Some(100.0)
    );

    // Exhausting claude-gpt as well makes all families blocked
    provider.windows.extra[2] = extra_window(
        "antigravity-quota-summary-claude-gpt-session",
        "Claude / GPT 5h",
        100.0,
        Some(300),
        Some(sooner),
    );
    assert!(antigravity_quota_summary_families_are_all_blocked(
        &provider
    ));
}

#[test]
fn mistral_monthly_plan_and_extra_usage_metric_windows() {
    let mut provider = make_provider("mistral");
    provider.windows.extra = vec![extra_window(
        "mistral-monthly-plan",
        "Monthly plan",
        48.0,
        None,
        None,
    )];
    provider.cost = Some(metric_ok(make_cost(15.0, Some(50.0), None)));

    assert_eq!(
        resolve_menu_bar_metric_window(
            MenuBarMetricPreference::Automatic,
            &provider,
            false,
            false,
            None
        ),
        None
    );
    assert_eq!(
        resolve_menu_bar_metric_window(
            MenuBarMetricPreference::MonthlyPlan,
            &provider,
            false,
            false,
            None
        )
        .map(|w| w.used_percent),
        Some(48.0)
    );
    assert_eq!(
        resolve_menu_bar_metric_window(
            MenuBarMetricPreference::ExtraUsage,
            &provider,
            false,
            false,
            None
        )
        .map(|w| w.used_percent),
        Some(30.0)
    );

    // icon_input for Mistral is (None, None) in Automatic mode, and uses MonthlyPlan when selected
    let auto_input = icon_input(&provider, IconPreferences::default());
    assert_eq!((auto_input.primary, auto_input.secondary), (None, None));
    let plan_input = icon_input(
        &provider,
        IconPreferences {
            metric_preference: MenuBarMetricPreference::MonthlyPlan,
            ..IconPreferences::default()
        },
    );
    assert_eq!(plan_input.primary, Some(52.0));
}

#[test]
fn unified_icon_source_resolves_current_selection_highest_usage_and_frontmost_app() {
    let mut claude = make_provider("claude");
    claude.windows.primary = Some(metric_ok(rate_window(35.0, Some(300), None)));

    let mut codex = make_provider("codex");
    codex.windows.primary = Some(metric_ok(rate_window(90.0, Some(300), None)));

    let mut gemini = make_provider("gemini");
    gemini.windows.primary = Some(metric_ok(rate_window(50.0, Some(300), None)));

    let snapshot = EngineSnapshot::new(
        "2026-04-06T12:00:00Z".parse().unwrap(),
        900,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        vec![claude, codex, gemini],
    );

    // CurrentSelection uses the selected provider
    let sel = primary_provider_for_unified_icon(
        &snapshot,
        Some("gemini"),
        UnifiedIconSource::CurrentSelection,
        Some("claude"),
        false,
        false,
        None,
    );
    assert_eq!(sel.map(|p| p.id.as_str()), Some("gemini"));

    // HighestUsage picks codex (90% used)
    let highest = primary_provider_for_unified_icon(
        &snapshot,
        Some("claude"),
        UnifiedIconSource::HighestUsage,
        None,
        false,
        false,
        None,
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));

    // FrontmostApp uses focused_provider when merged menu is closed and not stacked
    let frontmost = primary_provider_for_unified_icon(
        &snapshot,
        Some("claude"),
        UnifiedIconSource::FrontmostApp,
        Some("gemini"),
        false,
        false,
        None,
    );
    assert_eq!(frontmost.map(|p| p.id.as_str()), Some("gemini"));

    // FrontmostApp freezes to fallback while merged menu is open
    let frozen = primary_provider_for_unified_icon(
        &snapshot,
        Some("claude"),
        UnifiedIconSource::FrontmostApp,
        Some("gemini"),
        true,
        false,
        None,
    );
    assert_eq!(frozen.map(|p| p.id.as_str()), Some("claude"));

    let enabled: HashSet<&str> = ["claude", "codex", "gemini"].into_iter().collect();
    let ctx = UnifiedIconContext {
        source: UnifiedIconSource::FrontmostApp,
        focused_provider: Some("codex"),
        is_merged_menu_open: false,
        is_stacked: false,
    };
    assert_eq!(ctx.resolve("claude", true, &enabled), "codex");
    assert_eq!(ctx.resolve("claude", false, &enabled), "claude");
}

#[test]
fn synthetic_placeholder_unknown_window_is_treated_as_missing_in_claude_and_layout_policy() {
    let march_18: Timestamp = "2026-03-18T14:22:09Z".parse().unwrap();
    let mut provider = make_provider("claude");
    // Synthetic placeholder (`MetricState::Unknown` with a fallback `RateWindow(0.0)`) on primary
    provider.windows.primary = Some(Metric {
        state: ariadusage_protocol::metric::MetricState::Unknown,
        value: Some(rate_window(0.0, Some(300), None)),
        as_of: None,
        source: None,
        confidence: Confidence::Exact,
        error: None,
    });
    provider.identity = Some(metric_ok(Identity {
        provider_id: ProviderId::new("claude").unwrap(),
        account_email: None,
        account_organization: None,
        login_method: Some("Claude API".to_string()),
        account_id: None,
        plan: None,
    }));
    provider.cost = Some(metric_ok(make_cost(3100.0, Some(5000.0), Some(march_18))));

    // Claude icon windows treat the synthetic placeholder primary as missing and fall back to monthly spend (62%).
    let (icon_p, icon_s) = resolved_icon_windows(&provider, None, Some(march_18));
    assert_eq!(icon_p.as_ref().map(|w| w.used_percent), Some(62.0));
    assert_eq!(icon_s, None);

    // `quota_layout_policy_for_provider` detects `raw_primary.is_none() && resolved_primary.is_some() && resolved_secondary.is_none()`
    // and sets `reserves_missing_secondary_lane = false`.
    let policy = quota_layout_policy_for_provider(&provider, None, Some(march_18));
    assert!(!policy.reserves_missing_secondary_lane);

    // `resolve_menu_bar_metric_window` also ignores the synthetic placeholder and resolves the spend window.
    let metric = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        Some(march_18),
    );
    assert_eq!(metric.map(|w| w.used_percent), Some(62.0));
}

#[test]
fn codex_duplicate_lanes_are_last_wins_and_expired_capped_session_is_dropped() {
    let now: Timestamp = "2026-04-06T12:00:00Z".parse().unwrap();
    let future_reset: Timestamp = "2026-04-06T12:30:00Z".parse().unwrap();
    let expired_reset: Timestamp = "2026-04-06T11:50:00Z".parse().unwrap();

    // 1. Duplicate session lanes: `secondary` has a 300-min session window after `primary` -> last-wins (65% used).
    let mut dup = make_provider("codex");
    dup.windows.primary = Some(metric_ok(rate_window(20.0, Some(300), Some(future_reset))));
    dup.windows.secondary = Some(metric_ok(rate_window(65.0, Some(300), Some(future_reset))));
    let (dup_p, dup_s) = resolved_icon_windows(&dup, None, Some(now));
    assert_eq!(dup_p.map(|w| w.used_percent), Some(65.0));
    assert_eq!(dup_s, None);

    // 2. Weekly cap (`weekly == 100%` with future reset) when session's own reset has already passed:
    // upstream caps session to 100% first, then drops it because `session.resetsAt <= now`, promoting weekly to primary!
    let mut capped_expired_session = make_provider("codex");
    capped_expired_session.windows.primary =
        Some(metric_ok(rate_window(25.0, Some(300), Some(expired_reset))));
    capped_expired_session.windows.secondary = Some(metric_ok(rate_window(
        100.0,
        Some(10080),
        Some(future_reset),
    )));
    let (cap_p, cap_s) = resolved_icon_windows(&capped_expired_session, None, Some(now));
    assert_eq!(cap_p.as_ref().map(|w| w.used_percent), Some(100.0));
    assert_eq!(cap_p.as_ref().and_then(|w| w.window_minutes), Some(10080));
    assert_eq!(cap_s, None);
}

#[test]
fn merged_status_item_signatures_and_merged_warp_bonus_lane_match_upstream() {
    use ariadusage_macos::icon::LoadingPattern;
    // 1. `StatusItemAnimationSignatureTests`: merged style is `.combined` when >1 provider is enabled
    // and switches to the single provider's style when only 1 provider is enabled.
    let mut codex = make_provider("codex");
    codex.windows.primary = Some(metric_ok(rate_window(50.0, None, None)));
    let mut synthetic = make_provider("synthetic");
    synthetic.enabled = true;

    let two_enabled = EngineSnapshot::new(
        "2026-04-06T12:00:00Z".parse().unwrap(),
        900,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        vec![codex.clone(), synthetic.clone()],
    );
    assert_eq!(store_icon_style(Some(&two_enabled)), IconStyle::Combined);
    let (_, combined_input) = status_item_icon_input(
        Some(&two_enabled),
        Some("codex"),
        true,
        IconPreferences::default(),
    );
    assert_eq!(combined_input.style, IconStyle::Combined);
    let combined_sig = icon_render_signature(
        &combined_input,
        false,
        false,
        None,
        LoadingPattern::KnightRider,
        false,
    );

    synthetic.enabled = false;
    let one_enabled = EngineSnapshot::new(
        "2026-04-06T12:00:00Z".parse().unwrap(),
        900,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        vec![codex, synthetic],
    );
    assert_eq!(store_icon_style(Some(&one_enabled)), IconStyle::Codex);
    let (_, codex_input) = status_item_icon_input(
        Some(&one_enabled),
        Some("codex"),
        true,
        IconPreferences::default(),
    );
    assert_eq!(codex_input.style, IconStyle::Codex);
    let codex_sig = icon_render_signature(
        &codex_input,
        false,
        false,
        None,
        LoadingPattern::KnightRider,
        false,
    );
    assert_ne!(combined_sig, codex_sig);

    // 2. `StatusItemAnimationSignatureTests`: merged Antigravity icon resolves quota summary with provider style
    // while rendering with `style=combined`.
    let mut antigravity = make_provider("antigravity");
    antigravity.windows.primary = Some(metric_ok(rate_window(99.0, Some(10080), None)));
    antigravity.windows.secondary = Some(metric_ok(rate_window(16.0, Some(10080), None)));
    antigravity.windows.extra = vec![
        extra_window(
            "antigravity-quota-summary-gemini-5h",
            "Gemini Session",
            1.0,
            Some(300),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-gemini-weekly",
            "Gemini Weekly",
            99.0,
            Some(10080),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-3p-5h",
            "Claude + GPT Session",
            2.0,
            Some(300),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-3p-weekly",
            "Claude + GPT Weekly",
            16.0,
            Some(10080),
            None,
        ),
    ];
    let mut syn2 = make_provider("synthetic");
    syn2.enabled = true;
    let ag_snap = EngineSnapshot::new(
        "2026-04-06T12:00:00Z".parse().unwrap(),
        900,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        vec![antigravity, syn2],
    );
    let (_, ag_input) = status_item_icon_input(
        Some(&ag_snap),
        Some("antigravity"),
        true,
        IconPreferences::default(),
    );
    assert_eq!(ag_input.style, IconStyle::Combined);
    assert_eq!(ag_input.primary, Some(98.0));
    assert_eq!(ag_input.secondary, Some(1.0));

    // 3. `MergedWarpIconRenderingTests`: merged Warp bonus lane is preserved in show-used mode when bonus is unused (`0.1`)
    // vs exhausted (`0.0`).
    let mut warp_exhausted = make_provider("warp");
    warp_exhausted.windows.primary = Some(metric_ok(rate_window(10.0, None, None)));
    warp_exhausted.windows.secondary = Some(metric_ok(rate_window(100.0, None, None)));
    let snap_ex = EngineSnapshot::new(
        "2026-04-06T12:00:00Z".parse().unwrap(),
        900,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        vec![warp_exhausted, make_provider("codex")],
    );
    let (_, ex_input) = status_item_icon_input(
        Some(&snap_ex),
        Some("warp"),
        true,
        IconPreferences {
            show_used: true,
            ..IconPreferences::default()
        },
    );
    let ex_sig = icon_render_signature(
        &ex_input,
        true,
        false,
        None,
        LoadingPattern::KnightRider,
        false,
    );

    let mut warp_unused = make_provider("warp");
    warp_unused.windows.primary = Some(metric_ok(rate_window(10.0, None, None)));
    warp_unused.windows.secondary = Some(metric_ok(rate_window(0.0, None, None)));
    let snap_un = EngineSnapshot::new(
        "2026-04-06T12:00:00Z".parse().unwrap(),
        900,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        vec![warp_unused, make_provider("codex")],
    );
    let (_, un_input) = status_item_icon_input(
        Some(&snap_un),
        Some("warp"),
        true,
        IconPreferences {
            show_used: true,
            ..IconPreferences::default()
        },
    );
    let un_sig = icon_render_signature(
        &un_input,
        true,
        false,
        None,
        LoadingPattern::KnightRider,
        false,
    );
    assert_ne!(ex_sig, un_sig);
    assert_eq!(ex_input.secondary, Some(0.0));
    assert_eq!(un_input.secondary, Some(0.1));
    assert!(ex_input.treats_exhausted_secondary_as_missing);
    assert!(un_input.treats_exhausted_secondary_as_missing);

    // 4. `StatusItemIconObservationSignatureTests`: observation signature changes when enabled providers,
    // window values, or error states change.
    let prefs = IconPreferences {
        show_used: true,
        ..IconPreferences::default()
    };
    let obs_ex = store_icon_observation_signature(Some(&snap_ex), Some("warp"), true, prefs);
    let obs_un = store_icon_observation_signature(Some(&snap_un), Some("warp"), true, prefs);
    assert_ne!(obs_ex, obs_un);
}

// --- Ported from StatusItemAnimationSignatureTests.swift ---

fn make_snapshot_with_providers(providers: Vec<ProviderSnapshot>) -> EngineSnapshot {
    EngineSnapshot::new(
        "2026-04-06T12:00:00Z".parse().unwrap(),
        900,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        providers,
    )
}

fn render_signature(input: &IconInput) -> u64 {
    icon_render_signature(
        input,
        false,
        false,
        None,
        ariadusage_macos::icon::LoadingPattern::KnightRider,
        false,
    )
}

// Swift's render signatures are strings; Rust hashes the same typed icon fields into u64.
// Assertions on provider/style/primary/weekly/status therefore inspect those fields directly.
// upstream: StatusItemAnimationSignatureTests.swift:9 merged render signature changes when unified icon style changes
#[test]
fn merged_render_signature_changes_when_unified_icon_style_changes() {
    let mut codex = make_provider("codex");
    codex.windows.primary = Some(metric_ok(rate_window(50.0, None, None)));
    let synthetic = make_provider("synthetic");
    let mut snapshot = make_snapshot_with_providers(vec![codex, synthetic]);
    let enabled = |snapshot: &EngineSnapshot| {
        snapshot
            .providers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.id.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    // The wire snapshot has one enabled list for display and fetching.
    assert_eq!(enabled(&snapshot), ["codex", "synthetic"]);
    assert_eq!(store_icon_style(Some(&snapshot)), IconStyle::Combined);
    let (_, combined) = status_item_icon_input(
        Some(&snapshot),
        Some("codex"),
        true,
        IconPreferences::default(),
    );
    let combined_signature = render_signature(&combined);

    snapshot.providers[1].enabled = false;
    assert_eq!(enabled(&snapshot), ["codex"]);
    assert_eq!(store_icon_style(Some(&snapshot)), IconStyle::Codex);
    let (_, codex) = status_item_icon_input(
        Some(&snapshot),
        Some("codex"),
        true,
        IconPreferences::default(),
    );
    let codex_signature = render_signature(&codex);
    // Rust signatures are u64, not optional strings: assert their typed fields directly.
    assert_ne!(combined_signature, codex_signature);
    assert_eq!(codex.style, IconStyle::Codex);
}

// upstream: StatusItemAnimationSignatureTests.swift:68 merged antigravity icon resolves quota summary with provider style
#[test]
fn merged_antigravity_icon_resolves_quota_summary_with_provider_style() {
    let mut antigravity = make_provider("antigravity");
    antigravity.windows.primary = Some(metric_ok(rate_window(99.0, Some(10080), None)));
    antigravity.windows.secondary = Some(metric_ok(rate_window(16.0, Some(10080), None)));
    antigravity.windows.extra = vec![
        extra_window(
            "antigravity-quota-summary-gemini-5h",
            "Gemini Session",
            1.0,
            Some(300),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-gemini-weekly",
            "Gemini Weekly",
            99.0,
            Some(10080),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-3p-5h",
            "Claude + GPT Session",
            2.0,
            Some(300),
            None,
        ),
        extra_window(
            "antigravity-quota-summary-3p-weekly",
            "Claude + GPT Weekly",
            16.0,
            Some(10080),
            None,
        ),
    ];
    let mut synthetic = make_provider("synthetic");
    synthetic.enabled = true;
    let snap = make_snapshot_with_providers(vec![antigravity, synthetic]);
    let (provider, input) = status_item_icon_input(
        Some(&snap),
        Some("antigravity"),
        true,
        IconPreferences::default(),
    );
    assert_eq!(store_icon_style(Some(&snap)), IconStyle::Combined);
    assert_eq!(provider.map(|p| p.id.as_str()), Some("antigravity"));
    assert_eq!(input.style, IconStyle::Combined);
    assert_eq!(input.primary, Some(98.0));
    assert_eq!(input.secondary, Some(1.0));
}

// upstream: StatusItemAnimationSignatureTests.swift:145 merged mistral icon uses monthly plan metric when selected
#[test]
fn merged_mistral_icon_uses_monthly_plan_metric_when_selected() {
    let mut mistral = make_provider("mistral");
    mistral.windows.extra = vec![extra_window(
        "mistral-monthly-plan",
        "Monthly Plan",
        42.0,
        None,
        None,
    )];
    let snapshot = make_snapshot_with_providers(vec![mistral, make_provider("synthetic")]);
    assert_eq!(store_icon_style(Some(&snapshot)), IconStyle::Combined);
    let (provider, input) = status_item_icon_input(
        Some(&snapshot),
        Some("mistral"),
        true,
        IconPreferences {
            show_used: true,
            metric_preference: MenuBarMetricPreference::MonthlyPlan,
            ..IconPreferences::default()
        },
    );
    assert_eq!(provider.map(|p| p.id.as_str()), Some("mistral"));
    assert_eq!(input.primary, Some(42.0));
    assert_eq!(input.secondary, None);
}

// upstream: StatusItemAnimationSignatureTests.swift:201 mistral pay as you go icon ignores balance primary percent
#[test]
fn mistral_pay_as_you_go_icon_ignores_balance_primary_percent() {
    let mut mistral = make_provider("mistral");
    let mut primary = rate_window(0.0, None, None);
    primary.reset_description = Some("$12.50".to_string());
    mistral.windows.primary = Some(metric_ok(primary));
    let input = icon_input(
        &mistral,
        IconPreferences {
            show_used: true,
            ..IconPreferences::default()
        },
    );
    assert_eq!(input.primary, None);
    assert_eq!(input.secondary, None);
}

// upstream: StatusItemAnimationSignatureTests.swift:540 merged fallback provider follows enabled provider order
#[test]
fn merged_fallback_provider_follows_enabled_provider_order() {
    let mut synthetic = make_provider("synthetic");
    synthetic.windows.primary = Some(metric_ok(rate_window(50.0, None, None)));
    let mut codex = make_provider("codex");
    codex.windows.primary = Some(metric_ok(rate_window(50.0, None, None)));
    let snapshot = make_snapshot_with_providers(vec![synthetic, codex]);
    assert_eq!(
        snapshot
            .providers
            .iter()
            .filter(|p| p.enabled)
            .take(2)
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["synthetic", "codex"]
    );
    let provider = primary_provider_for_unified_icon(
        &snapshot,
        None,
        UnifiedIconSource::CurrentSelection,
        None,
        false,
        false,
        None,
    );
    assert_eq!(provider.map(|p| p.id.as_str()), Some("synthetic"));
}

// upstream: StatusItemAnimationSignatureTests.swift:583 merged icon status indicator follows rendered provider
#[test]
fn merged_icon_status_indicator_follows_rendered_provider() {
    let mut codex = make_provider("codex");
    codex.windows.primary = Some(metric_ok(rate_window(50.0, None, None)));
    let mut claude = make_provider("claude");
    claude.windows.primary = Some(metric_ok(rate_window(50.0, None, None)));
    claude.status = Some(metric_ok(ProviderStatus {
        indicator: StatusIndicator::Major,
        description: Some("Claude status issue".to_string()),
        updated_at: Some(Timestamp::from_second(20).unwrap()),
        page_url: None,
        components: Vec::new(),
    }));
    let snapshot = make_snapshot_with_providers(vec![codex, claude]);
    let (provider, input) = status_item_icon_input(
        Some(&snapshot),
        Some("codex"),
        true,
        IconPreferences::default(),
    );
    assert_eq!(
        primary_provider_for_unified_icon(
            &snapshot,
            Some("codex"),
            UnifiedIconSource::CurrentSelection,
            None,
            false,
            false,
            None
        )
        .map(|p| p.id.as_str()),
        Some("codex")
    );
    assert_eq!(provider.map(|p| p.id.as_str()), Some("codex"));
    assert_eq!(input.status, StatusLevel::None);
    let (provider, input) = status_item_icon_input(
        Some(&snapshot),
        Some("claude"),
        true,
        IconPreferences::default(),
    );
    assert_eq!(
        primary_provider_for_unified_icon(
            &snapshot,
            Some("claude"),
            UnifiedIconSource::CurrentSelection,
            None,
            false,
            false,
            None
        )
        .map(|p| p.id.as_str()),
        Some("claude")
    );
    assert_eq!(provider.map(|p| p.id.as_str()), Some("claude"));
    assert_eq!(input.status, StatusLevel::Major);
}

// upstream: StatusItemAnimationSignatureTests.swift:635 highest usage icon ranks only overview providers
#[test]
fn highest_usage_icon_ranks_only_overview_providers() {
    use ariadusage_macos::presentation::provider_with_highest_usage;
    let mut codex = make_provider("codex");
    codex.windows.primary = Some(metric_ok(rate_window(25.0, None, None)));
    let mut claude = make_provider("claude");
    claude.windows.primary = Some(metric_ok(rate_window(80.0, None, None)));
    let snapshot = make_snapshot_with_providers(vec![codex, claude]);
    assert_eq!(
        provider_with_highest_usage(&snapshot.providers.iter().collect::<Vec<_>>(), None)
            .map(|p| p.id.as_str()),
        Some("claude")
    );
    let provider = primary_provider_for_unified_icon_with_overview(
        &snapshot,
        None,
        Some(&["codex"]),
        false,
        UnifiedIconSource::HighestUsage,
        None,
        false,
        false,
        None,
    );
    assert_eq!(provider.map(|p| p.id.as_str()), Some("codex"));
}

// upstream: StatusItemAnimationSignatureTests.swift:685 highest usage icon keeps nonempty overview authoritative when unrankable
#[test]
fn highest_usage_icon_keeps_nonempty_overview_authoritative_when_unrankable() {
    use ariadusage_macos::presentation::provider_with_highest_usage;
    for overview_used_percent in [None, Some(100.0)] {
        let mut codex = make_provider("codex");
        if let Some(used_percent) = overview_used_percent {
            codex.windows.primary = Some(metric_ok(rate_window(used_percent, None, None)));
        }
        let mut claude = make_provider("claude");
        claude.windows.primary = Some(metric_ok(rate_window(80.0, None, None)));
        let snapshot = make_snapshot_with_providers(vec![codex, claude]);
        assert!(provider_with_highest_usage(&[&snapshot.providers[0]], None).is_none());
        let provider = primary_provider_for_unified_icon_with_overview(
            &snapshot,
            Some("claude"),
            Some(&["codex"]),
            false,
            UnifiedIconSource::HighestUsage,
            None,
            false,
            false,
            None,
        );
        assert_eq!(provider.map(|p| p.id.as_str()), Some("codex"));
    }
}

// upstream: StatusItemAnimationSignatureTests.swift:745 highest usage icon allows broad fallback for explicit empty overview
#[test]
fn highest_usage_icon_allows_broad_fallback_for_explicit_empty_overview() {
    let snapshot =
        make_snapshot_with_providers(vec![make_provider("codex"), make_provider("claude")]);
    let empty_overview: [&str; 0] = [];
    // Snapshot v1 has no persisted overview-selection model; pass upstream's resolved empty list at the resolver boundary.
    let provider = primary_provider_for_unified_icon_with_overview(
        &snapshot,
        Some("claude"),
        Some(&empty_overview),
        false,
        UnifiedIconSource::HighestUsage,
        None,
        false,
        false,
        None,
    );
    assert_eq!(provider.map(|p| p.id.as_str()), Some("claude"));
}

// upstream: StatusItemAnimationSignatureTests.swift:789 merged icon follows overview provider order when first overview provider is loading
#[test]
fn merged_icon_follows_overview_provider_order_when_first_overview_provider_is_loading() {
    let cursor = make_provider("cursor");
    let mut codex = make_provider("codex");
    codex.windows.primary = Some(metric_ok(rate_window(50.0, None, None)));
    let mut claude = make_provider("claude");
    claude.windows.primary = Some(metric_ok(rate_window(50.0, None, None)));
    let snapshot = make_snapshot_with_providers(vec![cursor, codex, claude]);
    let overview = ["cursor", "codex", "claude"];
    let enabled = snapshot
        .providers
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(enabled, overview);
    // Snapshot v1 has no separate persisted overview selection; pass the resolved upstream order.
    let provider = primary_provider_for_unified_icon_with_overview(
        &snapshot,
        Some("codex"),
        Some(&overview),
        true,
        UnifiedIconSource::CurrentSelection,
        None,
        false,
        false,
        None,
    );
    assert_eq!(provider.map(|p| p.id.as_str()), Some("cursor"));
}

// --- Literal observation-signature cases ---

// Port of makeSnapshot: defaults are 10 / 20, 300 / 10080 minutes, epoch 100, and "plus".
fn observation_provider(id: &str, email: &str) -> ProviderSnapshot {
    let mut provider = make_provider(id);
    provider.windows.primary = Some(metric_ok(rate_window(10.0, Some(300), None)));
    provider.windows.secondary = Some(metric_ok(rate_window(20.0, Some(10080), None)));
    provider.updated_at = Some(Timestamp::from_second(100).unwrap());
    provider.identity = Some(metric_ok(Identity {
        provider_id: ProviderId::new(id).unwrap(),
        account_email: Some(email.to_string()),
        account_organization: None,
        login_method: Some("plus".to_string()),
        account_id: None,
        plan: None,
    }));
    provider
}

fn observation_signature(snapshot: &EngineSnapshot) -> u64 {
    store_icon_observation_signature(
        Some(snapshot),
        Some("codex"),
        true,
        IconPreferences::default(),
    )
}

fn observation_status(
    indicator: StatusIndicator,
    description: &str,
    updated_at: i64,
) -> Metric<ProviderStatus> {
    metric_ok(ProviderStatus {
        indicator,
        description: Some(description.to_string()),
        updated_at: Some(Timestamp::from_second(updated_at).unwrap()),
        page_url: None,
        components: Vec::new(),
    })
}

// upstream: StatusItemIconObservationSignatureTests.swift:63 store icon observation signature ignores refresh and status metadata churn
#[test]
fn store_icon_observation_signature_ignores_refresh_and_status_metadata_churn() {
    let mut codex = observation_provider("codex", "icon@example.com");
    codex.status = Some(observation_status(StatusIndicator::None, "initial", 10));
    let mut snapshot = make_snapshot_with_providers(vec![codex]);
    let baseline = observation_signature(&snapshot);
    snapshot.engine.refreshing = true;
    snapshot.providers[0].status = Some(observation_status(
        StatusIndicator::None,
        "same indicator, newer timestamp",
        20,
    ));
    assert_eq!(observation_signature(&snapshot), baseline);
}

// upstream: StatusItemIconObservationSignatureTests.swift:96 store icon observation signature ignores non visual snapshot churn
#[test]
fn store_icon_observation_signature_ignores_non_visual_snapshot_churn() {
    let mut snapshot =
        make_snapshot_with_providers(vec![observation_provider("codex", "icon@example.com")]);
    let baseline = observation_signature(&snapshot);
    // Rust's signature is an opaque u64 and cannot contain account text; there is no string signature to inspect.
    let mut rotated = observation_provider("codex", "rotated-account@example.com");
    rotated.updated_at = Some(Timestamp::from_second(200).unwrap());
    snapshot.providers[0] = rotated;
    let signature = observation_signature(&snapshot);
    assert_eq!(signature, baseline);
}

// upstream: StatusItemIconObservationSignatureTests.swift:163 merged store icon observation signature ignores non primary snapshot churn
#[test]
fn merged_store_icon_observation_signature_ignores_non_primary_snapshot_churn() {
    let mut snapshot = make_snapshot_with_providers(vec![
        observation_provider("codex", "icon@example.com"),
        observation_provider("claude", "claude@example.com"),
    ]);
    let baseline = observation_signature(&snapshot);
    let mut changed = observation_provider("claude", "changed@example.com");
    changed.windows.primary = Some(metric_ok(rate_window(99.0, Some(300), None)));
    changed.windows.secondary = Some(metric_ok(rate_window(88.0, Some(10080), None)));
    changed.updated_at = Some(Timestamp::from_second(300).unwrap());
    snapshot.providers[1] = changed;
    assert_eq!(observation_signature(&snapshot), baseline);
}

// upstream: StatusItemIconObservationSignatureTests.swift:190 store icon observation signature changes when icon percentages change
#[test]
fn store_icon_observation_signature_changes_when_icon_percentages_change() {
    let mut snapshot =
        make_snapshot_with_providers(vec![observation_provider("codex", "icon@example.com")]);
    let baseline = observation_signature(&snapshot);
    snapshot.providers[0].windows.primary = Some(metric_ok(rate_window(42.0, Some(300), None)));
    snapshot.providers[0].windows.secondary = Some(metric_ok(rate_window(63.0, Some(10080), None)));
    assert_ne!(observation_signature(&snapshot), baseline);
}

// upstream: StatusItemIconObservationSignatureTests.swift:209 store icon observation signature tracks selected copilot budget
#[test]
fn store_icon_observation_signature_tracks_selected_copilot_budget() {
    let mut copilot = observation_provider("copilot", "copilot@example.com");
    copilot.windows.primary = Some(metric_ok(rate_window(10.0, None, None)));
    copilot.windows.secondary = Some(metric_ok(rate_window(20.0, None, None)));
    copilot
        .identity
        .as_mut()
        .unwrap()
        .value
        .as_mut()
        .unwrap()
        .login_method = Some("individual".to_string());
    copilot.windows.extra = vec![extra_window(
        "copilot-budget-agent",
        "Budget - Copilot Agent Premium Requests",
        25.0,
        None,
        None,
    )];
    let mut snapshot = make_snapshot_with_providers(vec![copilot]);
    let baseline = store_icon_observation_signature_with_override(
        Some(&snapshot),
        Some("copilot"),
        true,
        IconPreferences::default(),
        Some("copilot-budget-agent"),
    );
    snapshot.providers[0].windows.extra[0].window = metric_ok(rate_window(75.0, None, None));
    assert_ne!(
        store_icon_observation_signature_with_override(
            Some(&snapshot),
            Some("copilot"),
            true,
            IconPreferences::default(),
            Some("copilot-budget-agent")
        ),
        baseline
    );
}

// upstream: StatusItemIconObservationSignatureTests.swift:236 store icon observation signature changes when credit fallback changes
#[test]
fn store_icon_observation_signature_changes_when_credit_fallback_changes() {
    let mut codex = observation_provider("codex", "icon@example.com");
    codex.windows.primary = Some(metric_ok(rate_window(100.0, Some(300), None)));
    let mut credits = Credits::new(80.0);
    credits.updated_at = Some(Timestamp::from_second(100).unwrap());
    codex.credits = Some(metric_ok(credits));
    let mut snapshot = make_snapshot_with_providers(vec![codex]);
    let baseline = observation_signature(&snapshot);
    let mut credits = Credits::new(42.0);
    credits.updated_at = Some(Timestamp::from_second(200).unwrap());
    snapshot.providers[0].credits = Some(metric_ok(credits));
    assert_ne!(observation_signature(&snapshot), baseline);
}

// upstream: StatusItemIconObservationSignatureTests.swift:257 store icon observation signature ignores unused credit balance
#[test]
fn store_icon_observation_signature_ignores_unused_credit_balance() {
    let mut codex = observation_provider("codex", "icon@example.com");
    let mut credits = Credits::new(80.0);
    credits.updated_at = Some(Timestamp::from_second(100).unwrap());
    codex.credits = Some(metric_ok(credits));
    let mut snapshot = make_snapshot_with_providers(vec![codex]);
    let baseline = observation_signature(&snapshot);
    let mut credits = Credits::new(42.0);
    credits.updated_at = Some(Timestamp::from_second(200).unwrap());
    snapshot.providers[0].credits = Some(metric_ok(credits));
    assert_eq!(observation_signature(&snapshot), baseline);
}

// upstream: StatusItemIconObservationSignatureTests.swift:271 merged store icon observation signature ignores non primary status changes
#[test]
fn merged_store_icon_observation_signature_ignores_non_primary_status_changes() {
    let mut snapshot = make_snapshot_with_providers(vec![
        observation_provider("codex", "icon@example.com"),
        make_provider("claude"),
    ]);
    let baseline = observation_signature(&snapshot);
    snapshot.providers[1].status = Some(observation_status(
        StatusIndicator::Major,
        "Claude status issue",
        20,
    ));
    assert_eq!(observation_signature(&snapshot), baseline);
}

// upstream: StatusItemIconObservationSignatureTests.swift:290 store icon observation signature changes when status indicator changes
#[test]
fn store_icon_observation_signature_changes_when_status_indicator_changes() {
    let mut codex = observation_provider("codex", "icon@example.com");
    codex.status = Some(observation_status(StatusIndicator::None, "initial", 10));
    let mut snapshot = make_snapshot_with_providers(vec![codex]);
    let baseline = observation_signature(&snapshot);
    snapshot.providers[0].status = Some(observation_status(
        StatusIndicator::Major,
        "major outage",
        20,
    ));
    assert_ne!(observation_signature(&snapshot), baseline);
}

// upstream: StatusItemIconObservationSignatureTests.swift:310 store icon observation signature changes when hide critters toggles
#[test]
fn store_icon_observation_signature_changes_when_hide_critters_toggles() {
    let snapshot =
        make_snapshot_with_providers(vec![observation_provider("codex", "icon@example.com")]);
    let baseline = store_icon_observation_signature(
        Some(&snapshot),
        Some("codex"),
        true,
        IconPreferences {
            hide_critters: false,
            ..IconPreferences::default()
        },
    );
    assert_ne!(
        store_icon_observation_signature(
            Some(&snapshot),
            Some("codex"),
            true,
            IconPreferences {
                hide_critters: true,
                ..IconPreferences::default()
            }
        ),
        baseline
    );
}

// --- Literal menu-bar metric cases from StatusItemAnimationTests.swift ---

// upstream: StatusItemAnimationTests.swift:415 menu bar percent uses configured metric
#[test]
fn menu_bar_percent_uses_configured_metric() {
    let mut provider = make_provider("codex");
    provider.windows.primary = Some(metric_ok(rate_window(12.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(42.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Secondary,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(42.0));
}

// upstream: StatusItemAnimationTests.swift:455 combined codex menu bar metric window uses most constrained visible lane
#[test]
fn combined_codex_menu_bar_metric_window_uses_most_constrained_visible_lane() {
    let mut provider = make_provider("codex");
    provider.windows.primary = Some(metric_ok(rate_window(12.0, Some(300), None)));
    provider.windows.secondary = Some(metric_ok(rate_window(91.0, Some(7 * 24 * 60), None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(91.0));
    assert_eq!(
        window.as_ref().and_then(|w| w.window_minutes),
        Some(7 * 24 * 60)
    );
}

// upstream: StatusItemAnimationTests.swift:496 menu bar percent automatic prefers rate limit for kimi
#[test]
fn menu_bar_percent_automatic_prefers_rate_limit_for_kimi() {
    let mut provider = make_provider("kimi");
    provider.windows.primary = Some(metric_ok(rate_window(12.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(42.0, Some(300), None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(42.0));
}

// upstream: StatusItemAnimationTests.swift:535 menu bar percent uses average for gemini
#[test]
fn menu_bar_percent_uses_average_for_gemini() {
    let mut provider = make_provider("gemini");
    provider.windows.primary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(60.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Average,
        &provider,
        true,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(40.0));
}

// upstream: StatusItemAnimationTests.swift:575 menu bar percent automatic keeps gemini primary over higher tertiary
#[test]
fn menu_bar_percent_automatic_keeps_gemini_primary_over_higher_tertiary() {
    let mut provider = make_provider("gemini");
    provider.windows.primary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(40.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(95.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(20.0));
}

// upstream: StatusItemAnimationTests.swift:615 menu bar percent automatic picks highest cursor lane including api
#[test]
fn menu_bar_percent_automatic_picks_highest_cursor_lane_including_api() {
    let mut provider = make_provider("cursor");
    provider.windows.primary = Some(metric_ok(rate_window(10.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(25.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(90.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(90.0));
}

// upstream: StatusItemAnimationTests.swift:655 menu bar percent automatic falls back to purchased perplexity lane when bonus is exhausted
#[test]
fn menu_bar_percent_automatic_falls_back_to_purchased_perplexity_lane_when_bonus_is_exhausted() {
    let mut provider = make_provider("perplexity");
    provider.windows.secondary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(20.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(20.0));
}

// upstream: StatusItemAnimationTests.swift:695 menu bar percent automatic falls through after recurring perplexity credits are exhausted
#[test]
fn menu_bar_percent_automatic_falls_through_after_recurring_perplexity_credits_are_exhausted() {
    let mut provider = make_provider("perplexity");
    provider.windows.primary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(32.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(32.0));
}

// upstream: StatusItemAnimationTests.swift:736 menu bar percent automatic prefers purchased perplexity credits before bonus
#[test]
fn menu_bar_percent_automatic_prefers_purchased_perplexity_credits_before_bonus() {
    let mut provider = make_provider("perplexity");
    provider.windows.primary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(45.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(45.0));
}

// upstream: StatusItemAnimationTests.swift:777 menu bar percent primary preference stays on recurring perplexity credits
#[test]
fn menu_bar_percent_primary_preference_stays_on_recurring_perplexity_credits() {
    let mut provider = make_provider("perplexity");
    provider.windows.primary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(32.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Primary,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(100.0));
}

// upstream: StatusItemAnimationTests.swift:818 menu bar percent tertiary preference uses purchased perplexity lane
#[test]
fn menu_bar_percent_tertiary_preference_uses_purchased_perplexity_lane() {
    let mut provider = make_provider("perplexity");
    provider.windows.secondary = Some(metric_ok(rate_window(100.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(28.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Tertiary,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(28.0));
}

// upstream: StatusItemAnimationTests.swift:858 menu bar percent tertiary preference uses api lane for cursor
#[test]
fn menu_bar_percent_tertiary_preference_uses_api_lane_for_cursor() {
    let mut provider = make_provider("cursor");
    provider.windows.primary = Some(metric_ok(rate_window(10.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(20.0, None, None)));
    provider.windows.tertiary = Some(metric_ok(rate_window(72.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Tertiary,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(72.0));
}

// upstream: StatusItemAnimationTests.swift:898 menu bar tertiary preference falls back to automatic when cursor api lane is missing
#[test]
fn menu_bar_tertiary_preference_falls_back_to_automatic_when_cursor_api_lane_is_missing() {
    let mut provider = make_provider("cursor");
    provider.windows.primary = Some(metric_ok(rate_window(10.0, None, None)));
    provider.windows.secondary = Some(metric_ok(rate_window(72.0, None, None)));
    let window = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Tertiary,
        &provider,
        false,
        false,
        None,
    );
    assert_eq!(window.as_ref().map(|w| w.used_percent), Some(72.0));
}
