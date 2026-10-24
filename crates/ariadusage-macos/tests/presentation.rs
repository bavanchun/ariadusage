// Ported from CodexBar Tests/CodexBarTests/MenuBarMetricWindowResolverTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeSpendWindowTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexbarTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

use std::collections::HashSet;

use ariadusage_macos::icon::{IconInput, IconStyle, layout};
use ariadusage_macos::presentation::{
    IconPreferences, MenuBarMetricPreference, UnifiedIconContext, UnifiedIconSource,
    VISIBLE_ZERO_PERCENT, antigravity_quota_summary_families_are_all_blocked,
    antigravity_quota_summary_ranking_window, claude_monthly_spend_window,
    claude_scoped_weekly_window, connection_status, end_of_utc_month, icon_input,
    icon_input_with_override, icon_provider, inferred_monthly_window_minutes,
    primary_provider_for_unified_icon, quota_layout_policy_for_provider,
    resolve_menu_bar_metric_window, resolved_claude_reset_window_for_pace, resolved_icon_percents,
    resolved_icon_percents_for_style, resolved_icon_windows,
};
use ariadusage_protocol::fixture::generate_scenario_snapshot;
use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::metric::{Confidence, Metric};
use ariadusage_protocol::snapshot::{EngineInfo, EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::{Cost, Identity, NamedWindow, RateWindow};
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
