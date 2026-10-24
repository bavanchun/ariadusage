// Ported from CodexBar Tests/CodexBarTests/CodexCombinedMetricHighestUsageTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexWeeklyCapSurfaceTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/UsageStoreHighestUsageTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_macos::icon::IconStyle;
use ariadusage_macos::presentation::{
    MenuBarMetricPreference, antigravity_quota_summary_ranking_window,
    menu_bar_credits_remaining_for_icon, provider_with_highest_usage_with_options,
    resolve_menu_bar_metric_window, resolved_icon_percents_for_style,
};
use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::metric::{Confidence, Metric};
use ariadusage_protocol::snapshot::ProviderSnapshot;
use ariadusage_protocol::usage::{Cost, Credits, Identity, NamedWindow, RateWindow};
use jiff::{SignedDuration, Timestamp};

fn value<T>(value: T) -> Metric<T> {
    Metric::value(value, None, None, Confidence::Exact)
}

fn shifted(now: Timestamp, seconds: i64) -> Timestamp {
    now.checked_add(SignedDuration::from_secs(seconds)).unwrap()
}

fn window(
    used_percent: f64,
    window_minutes: Option<u32>,
    resets_at: Option<Timestamp>,
) -> RateWindow {
    let mut window = RateWindow::new(used_percent);
    window.window_minutes = window_minutes;
    window.resets_at = resets_at;
    window
}

fn provider(
    id: &str,
    primary: Option<RateWindow>,
    secondary: Option<RateWindow>,
    updated_at: Timestamp,
) -> ProviderSnapshot {
    let mut provider = ProviderSnapshot::new(ProviderId::new(id).unwrap(), id, true, "cli");
    provider.windows.primary = primary.map(value);
    provider.windows.secondary = secondary.map(value);
    provider.updated_at = Some(updated_at);
    provider
}

fn spend_limit(used: f64, limit: f64, updated_at: Timestamp) -> Cost {
    Cost {
        used,
        limit: Some(limit),
        currency_code: "USD".to_string(),
        period: Some("Spend limit".to_string()),
        resets_at: None,
        balance: None,
        next_regen_amount: None,
        personal_used: None,
        balance_updated_at: Some(updated_at),
        balance_is_workspace: None,
        balance_is_unavailable: None,
    }
}

// Upstream returns provider + selected percent; Rust returns the provider only.
// Check its selected metric through the production window resolver for the percent assertion.
// Rust's options API accepts one preference for the candidate set, whereas upstream stores it per
// provider. In these two-provider combined cases the other provider has either one lane or the same
// combined preference; passing PrimaryAndSecondary preserves every upstream selected metric.

// upstream: CodexCombinedMetricHighestUsageTests.swift:9 combined codex metric uses weekly lane when ranking highest usage
#[test]
fn combined_codex_metric_uses_weekly_lane_when_ranking_highest_usage() {
    let now = Timestamp::now();
    let codex = provider(
        "codex",
        Some(window(12.0, Some(300), None)),
        Some(window(91.0, Some(7 * 24 * 60), None)),
        now,
    );
    let claude = provider("claude", Some(window(80.0, None, None)), None, now);
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &claude],
        MenuBarMetricPreference::PrimaryAndSecondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(91.0));
}

// upstream: CodexCombinedMetricHighestUsageTests.swift:30 combined codex metric ignores expired weekly lane when ranking highest usage
#[test]
fn combined_codex_metric_ignores_expired_weekly_lane_when_ranking_highest_usage() {
    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let codex = provider(
        "codex",
        Some(window(1.0, Some(300), Some(shifted(now, 3600)))),
        Some(window(100.0, Some(7 * 24 * 60), Some(now))),
        shifted(now, -7200),
    );
    let claude = provider("claude", Some(window(80.0, None, None)), None, now);
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &claude],
        MenuBarMetricPreference::PrimaryAndSecondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("claude"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));
}

// upstream: CodexCombinedMetricHighestUsageTests.swift:61 combined codex metric excludes an actively binding weekly cap
#[test]
fn combined_codex_metric_excludes_an_actively_binding_weekly_cap() {
    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let codex = provider(
        "codex",
        Some(window(1.0, Some(300), Some(shifted(now, 3600)))),
        Some(window(100.0, Some(7 * 24 * 60), Some(shifted(now, 7200)))),
        now,
    );
    let claude = provider("claude", Some(window(80.0, None, None)), None, now);
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &claude],
        MenuBarMetricPreference::PrimaryAndSecondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("claude"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));
}

// upstream: CodexCombinedMetricHighestUsageTests.swift:92 combined codex metric stays eligible when only one lane is exhausted
#[test]
fn combined_codex_metric_stays_eligible_when_only_one_lane_is_exhausted() {
    let now = Timestamp::now();
    let codex = provider(
        "codex",
        Some(window(100.0, Some(300), None)),
        Some(window(30.0, Some(7 * 24 * 60), None)),
        now,
    );
    let claude = provider("claude", Some(window(80.0, None, None)), None, now);
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &claude],
        MenuBarMetricPreference::PrimaryAndSecondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(100.0));
}

// upstream: CodexCombinedMetricHighestUsageTests.swift:118 combined codex metric is excluded when both lanes are exhausted
#[test]
fn combined_codex_metric_is_excluded_when_both_lanes_are_exhausted() {
    let now = Timestamp::now();
    let codex = provider(
        "codex",
        Some(window(100.0, Some(300), None)),
        Some(window(100.0, Some(7 * 24 * 60), None)),
        now,
    );
    let claude = provider("claude", Some(window(80.0, None, None)), None, now);
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &claude],
        MenuBarMetricPreference::PrimaryAndSecondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("claude"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));
}

// upstream: CodexCombinedMetricHighestUsageTests.swift:144 combined claude metric stays eligible when only one lane is exhausted
#[test]
fn combined_claude_metric_stays_eligible_when_only_one_lane_is_exhausted() {
    let now = Timestamp::now();
    let codex = provider(
        "codex",
        Some(window(50.0, Some(300), None)),
        Some(window(50.0, Some(7 * 24 * 60), None)),
        now,
    );
    let claude = provider(
        "claude",
        Some(window(100.0, Some(300), None)),
        Some(window(30.0, Some(7 * 24 * 60), None)),
        now,
    );
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &claude],
        MenuBarMetricPreference::PrimaryAndSecondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("claude"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(100.0));
}

// upstream: CodexCombinedMetricHighestUsageTests.swift:178 combined claude metric is excluded when both lanes are exhausted
#[test]
fn combined_claude_metric_is_excluded_when_both_lanes_are_exhausted() {
    let now = Timestamp::now();
    let codex = provider(
        "codex",
        Some(window(80.0, Some(300), None)),
        Some(window(80.0, Some(7 * 24 * 60), None)),
        now,
    );
    let claude = provider(
        "claude",
        Some(window(100.0, Some(300), None)),
        Some(window(100.0, Some(7 * 24 * 60), None)),
        now,
    );
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &claude],
        MenuBarMetricPreference::PrimaryAndSecondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));
}

// upstream: CodexCombinedMetricHighestUsageTests.swift:212 combined claude metric excludes an exhausted weekly-only account with a synthetic placeholder
#[test]
fn combined_claude_metric_excludes_an_exhausted_weekly_only_account_with_a_synthetic_placeholder() {
    let now = Timestamp::now();
    let codex = provider(
        "codex",
        Some(window(80.0, Some(300), None)),
        Some(window(80.0, Some(7 * 24 * 60), None)),
        now,
    );
    let mut claude = provider(
        "claude",
        Some(window(0.0, Some(300), None)),
        Some(window(100.0, Some(7 * 24 * 60), None)),
        now,
    );
    // Snapshot v1 projects the upstream synthetic 0%/300-minute placeholder to Unknown (no value).
    claude.windows.primary = Some(Metric::unknown());
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &claude],
        MenuBarMetricPreference::PrimaryAndSecondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));
}

// upstream: CodexCombinedMetricHighestUsageTests.swift:252 combined claude metric excludes an exhausted spend-limit-only account
#[test]
fn combined_claude_metric_excludes_an_exhausted_spend_limit_only_account() {
    let now = Timestamp::now();
    let codex = provider(
        "codex",
        Some(window(80.0, Some(300), None)),
        Some(window(80.0, Some(7 * 24 * 60), None)),
        now,
    );
    let mut claude = provider("claude", Some(window(0.0, Some(300), None)), None, now);
    // Snapshot v1 projects the upstream synthetic 0%/300-minute placeholder to Unknown (no value).
    claude.windows.primary = Some(Metric::unknown());
    claude.cost = Some(value(spend_limit(100.0, 100.0, now)));
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &claude],
        MenuBarMetricPreference::PrimaryAndSecondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));
}

fn named_window(id: &str, title: &str, used_percent: f64, minutes: Option<u32>) -> NamedWindow {
    NamedWindow {
        id: id.to_string(),
        title: title.to_string(),
        window: value(window(used_percent, minutes, None)),
    }
}

// Upstream antigravityQuotaSummarySnapshot, including its exact IDs, titles and cadences.
fn antigravity_summary(
    gemini_session: f64,
    gemini_weekly: f64,
    other_session: f64,
    other_weekly: f64,
    now: Timestamp,
) -> ProviderSnapshot {
    let mut antigravity = provider("antigravity", None, None, now);
    antigravity.windows.extra = vec![
        named_window(
            "antigravity-quota-summary-gemini-5h",
            "Gemini Session",
            gemini_session,
            Some(5 * 60),
        ),
        named_window(
            "antigravity-quota-summary-gemini-weekly",
            "Gemini Weekly",
            gemini_weekly,
            Some(7 * 24 * 60),
        ),
        named_window(
            "antigravity-quota-summary-3p-5h",
            "Claude + GPT Session",
            other_session,
            Some(5 * 60),
        ),
        named_window(
            "antigravity-quota-summary-3p-weekly",
            "Claude + GPT Weekly",
            other_weekly,
            Some(7 * 24 * 60),
        ),
    ];
    antigravity
}

// upstream: UsageStoreHighestUsageTests.swift:250 automatic metric ignores antigravity tertiary when compact icon has no quota summary
#[test]
fn automatic_metric_ignores_antigravity_tertiary_when_compact_icon_has_no_quota_summary() {
    let now = Timestamp::now();
    let codex = provider("codex", Some(window(70.0, None, None)), None, now);
    let mut antigravity = provider("antigravity", None, None, now);
    antigravity.windows.tertiary = Some(value(window(85.0, None, None)));
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(70.0));
}

// upstream: UsageStoreHighestUsageTests.swift:289 automatic metric ignores unclassified antigravity compact fallback until exhausted priority is enabled
#[test]
fn automatic_metric_ignores_unclassified_antigravity_compact_fallback_until_exhausted_priority_is_enabled()
 {
    let now = Timestamp::now();
    let codex = provider("codex", Some(window(50.0, None, None)), None, now);
    let mut antigravity = provider("antigravity", None, None, now);
    // Snapshot v1 has no raw AntigravityStatusSnapshot modelQuotas. This is the exact wire result of
    // AntigravityStatusProbe.swift:125-179, 691-703 for the upstream local Experimental Model,
    // MODEL_PLACEHOLDER_NEW, remainingFraction=0.36, with no reset or account identity.
    let remaining_fraction = 0.36;
    antigravity.windows.extra = vec![named_window(
        "antigravity-compact-fallback-MODEL_PLACEHOLDER_NEW",
        "Experimental Model",
        100.0 - remaining_fraction * 100.0,
        None,
    )];
    antigravity.identity = Some(value(Identity {
        provider_id: ProviderId::new("antigravity").unwrap(),
        account_email: None,
        account_organization: None,
        login_method: None,
        account_id: None,
        plan: None,
    }));
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(50.0));
    let opt_in_highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        true,
        Some(now),
    );
    assert_eq!(opt_in_highest.map(|p| p.id.as_str()), Some("antigravity"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        opt_in_highest.unwrap(),
        false,
        true,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(64.0));
}

// upstream: UsageStoreHighestUsageTests.swift:344 automatic metric ignores legacy antigravity family lanes without quota summary
#[test]
fn automatic_metric_ignores_legacy_antigravity_family_lanes_without_quota_summary() {
    let now = Timestamp::now();
    let codex = provider("codex", Some(window(70.0, None, None)), None, now);
    let mut primary = window(0.0, None, None);
    primary.reset_description = Some("Claude".to_string());
    let mut secondary = window(100.0, None, None);
    secondary.reset_description = Some("Gemini Pro".to_string());
    let mut tertiary = window(40.0, None, None);
    tertiary.reset_description = Some("Gemini Flash".to_string());
    let mut antigravity = provider("antigravity", Some(primary), Some(secondary), now);
    antigravity.windows.tertiary = Some(value(tertiary));
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(70.0));
}

// upstream: UsageStoreHighestUsageTests.swift:385 antigravity automatic ranking keeps usable first until exhausted priority is enabled
#[test]
fn antigravity_automatic_ranking_keeps_usable_first_until_exhausted_priority_is_enabled() {
    let now = Timestamp::now();
    let codex = provider("codex", Some(window(80.0, None, None)), None, now);
    let mut antigravity = antigravity_summary(10.0, 20.0, 95.0, 90.0, now);
    antigravity.windows.extra.push(named_window(
        "antigravity-quota-summary-future-daily",
        "Future daily lane",
        99.0,
        Some(24 * 60),
    ));
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("antigravity"));
    // Use the production supported-cadence ranking resolver for the returned percent, not the
    // display metric resolver, which also accepts the unknown daily row.
    assert_eq!(
        antigravity_quota_summary_ranking_window(highest.unwrap(), Some(now))
            .map(|w| w.used_percent),
        Some(95.0)
    );

    antigravity = antigravity_summary(95.0, 20.0, 10.0, 10.0, now);
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("antigravity"));
    assert_eq!(
        antigravity_quota_summary_ranking_window(highest.unwrap(), Some(now))
            .map(|w| w.used_percent),
        Some(95.0)
    );

    antigravity = antigravity_summary(100.0, 100.0, 50.0, 50.0, now);
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));

    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        true,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("antigravity"));
    assert_eq!(
        antigravity_quota_summary_ranking_window(highest.unwrap(), Some(now))
            .map(|w| w.used_percent),
        Some(100.0)
    );
}

// upstream: UsageStoreHighestUsageTests.swift:462 opt in automatic metric excludes antigravity only when every summary family is blocked
#[test]
fn opt_in_automatic_metric_excludes_antigravity_only_when_every_summary_family_is_blocked() {
    let now = Timestamp::now();
    let codex = provider("codex", Some(window(80.0, None, None)), None, now);
    let mut antigravity = antigravity_summary(100.0, 40.0, 100.0, 100.0, now);
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        true,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        highest.unwrap(),
        false,
        true,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));

    antigravity.windows.extra.push(named_window(
        "antigravity-quota-summary-future-daily",
        "Future daily lane",
        100.0,
        Some(1440),
    ));
    let fail_open_highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        true,
        Some(now),
    );
    assert_eq!(
        fail_open_highest.map(|p| p.id.as_str()),
        Some("antigravity")
    );
    assert_eq!(
        antigravity_quota_summary_ranking_window(fail_open_highest.unwrap(), Some(now))
            .map(|w| w.used_percent),
        Some(100.0)
    );
}

// upstream: UsageStoreHighestUsageTests.swift:519 automatic metric ignores antigravity legacy detail rows
#[test]
fn automatic_metric_ignores_antigravity_legacy_detail_rows() {
    let now = Timestamp::now();
    let codex = provider("codex", Some(window(80.0, None, None)), None, now);
    let mut antigravity = provider("antigravity", None, None, now);
    antigravity.windows.extra = vec![
        named_window(
            "antigravity-compact-fallback-model-a",
            "Model A",
            100.0,
            None,
        ),
        named_window("model-b", "Model B", 50.0, None),
    ];
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));
}

// upstream: UsageStoreHighestUsageTests.swift:578 automatic metric skips antigravity with no quota lanes
#[test]
fn automatic_metric_skips_antigravity_with_no_quota_lanes() {
    let now = Timestamp::now();
    let codex = provider("codex", Some(window(80.0, None, None)), None, now);
    let mut antigravity = provider("antigravity", None, None, now);
    antigravity.identity = Some(value(Identity {
        provider_id: ProviderId::new("antigravity").unwrap(),
        account_email: Some("user@example.com".to_string()),
        account_organization: None,
        login_method: Some("Pro".to_string()),
        account_id: None,
        plan: None,
    }));
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Automatic,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Automatic,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(80.0));
}

// upstream: UsageStoreHighestUsageTests.swift:1098 explicit antigravity metric remains authoritative for highest usage
#[test]
fn explicit_antigravity_metric_remains_authoritative_for_highest_usage() {
    let now = Timestamp::now();
    let codex = provider("codex", Some(window(80.0, None, None)), None, now);
    let mut antigravity = antigravity_summary(10.0, 20.0, 95.0, 90.0, now);
    antigravity.windows.primary = Some(value(window(10.0, None, None)));
    antigravity.windows.secondary = Some(value(window(95.0, None, None)));
    // The options API takes one preference. Codex has only a primary lane here, so its Secondary
    // fallback equals its upstream automatic metric; Antigravity retains the explicit Secondary.
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Secondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("antigravity"));
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Secondary,
        highest.unwrap(),
        false,
        false,
        Some(now),
    );
    assert_eq!(selected.map(|w| w.used_percent), Some(95.0));

    antigravity.windows.secondary = Some(value(window(100.0, None, None)));
    let highest = provider_with_highest_usage_with_options(
        &[&codex, &antigravity],
        MenuBarMetricPreference::Secondary,
        false,
        Some(now),
    );
    assert_eq!(highest.map(|p| p.id.as_str()), Some("codex"));
}

// upstream: CodexWeeklyCapSurfaceTests.swift:68 primary menu bar metric and credits follow binding weekly reset
#[test]
fn primary_menu_bar_metric_and_credits_follow_binding_weekly_reset() {
    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let weekly_reset = shifted(now, 3600);
    let session_reset = shifted(now, 1800);
    let mut codex = provider(
        "codex",
        Some(window(1.0, Some(300), Some(session_reset))),
        Some(window(100.0, Some(10080), Some(weekly_reset))),
        shifted(now, -7200),
    );
    let mut credits = Credits::new(80.0);
    credits.updated_at = Some(now);
    codex.credits = Some(value(credits));

    let capped = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Primary,
        &codex,
        false,
        false,
        Some(now),
    );
    let reset = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Primary,
        &codex,
        false,
        false,
        Some(weekly_reset),
    );
    let capped_credits = menu_bar_credits_remaining_for_icon(&codex, Some(now));
    let reset_credits = menu_bar_credits_remaining_for_icon(&codex, Some(weekly_reset));
    assert_eq!(capped.as_ref().map(|w| 100.0 - w.used_percent), Some(0.0));
    assert_eq!(capped.and_then(|w| w.resets_at), Some(weekly_reset));
    assert_eq!(reset.as_ref().map(|w| 100.0 - w.used_percent), Some(99.0));
    assert_eq!(reset.and_then(|w| w.resets_at), Some(session_reset));
    assert_eq!(capped_credits, Some(80.0));
    assert_eq!(reset_credits, None);
}

// upstream: CodexWeeklyCapSurfaceTests.swift:130 combined menu bar modes ignore exhausted weekly lane after its reset
#[test]
fn combined_menu_bar_modes_ignore_exhausted_weekly_lane_after_its_reset() {
    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let session_reset = shifted(now, 3600);
    let codex = provider(
        "codex",
        Some(window(1.0, Some(300), Some(session_reset))),
        Some(window(100.0, Some(10080), Some(now))),
        shifted(now, -7200),
    );
    let selected = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::PrimaryAndSecondary,
        &codex,
        false,
        false,
        Some(now),
    )
    .expect("selected window");
    assert_eq!(100.0 - selected.used_percent, 99.0);
    assert_eq!(selected.resets_at, Some(session_reset));

    // Upstream also asserts percent="5h 99%", pace="99%", both="99%", resetTime="↻ in 1h".
    // The Rust status item has no menuBarDisplayText formatter. These four text assertions remain
    // unported under the documented display-text contract gap; no test-local formatter substitutes it.
    let mut expired_session = provider(
        "codex",
        Some(window(100.0, Some(300), Some(now))),
        Some(window(
            40.0,
            Some(10080),
            Some(shifted(now, 7 * 24 * 60 * 60)),
        )),
        shifted(now, -7200),
    );
    let mut credits = Credits::new(80.0);
    credits.updated_at = Some(now);
    expired_session.credits = Some(value(credits));
    let reset_primary = resolve_menu_bar_metric_window(
        MenuBarMetricPreference::Primary,
        &expired_session,
        false,
        false,
        Some(now),
    )
    .expect("reset primary");
    let (primary, secondary) = resolved_icon_percents_for_style(
        &expired_session,
        IconStyle::Codex,
        false,
        None,
        Some(now),
    );
    assert_eq!(100.0 - reset_primary.used_percent, 60.0);
    // The fifth missing menuBarDisplayText assertion is "60%" for this expired-session snapshot.
    assert_eq!(primary, Some(60.0));
    assert_eq!(secondary, None);
    assert_eq!(
        menu_bar_credits_remaining_for_icon(&expired_session, Some(now)),
        None
    );
}
