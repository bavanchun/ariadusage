// Ported from CodexBar Tests/CodexBarTests/UsagePaceTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ProviderPaceCapabilityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::RateWindow;
use ariadusage_core::pace::{
    DEFAULT_WEEKLY_WINDOW_MINUTES, MONTHLY_WINDOW_SENTINEL_MINUTES, ProviderPaceCapability,
    ProviderPaceDurationRule, ProviderPaceKind, ProviderPaceSlot, ProviderPaceWindowRule,
    ProviderStandardPaceLane, UsagePace,
};
use ariadusage_core::providers::first_party_order;
use ariadusage_protocol::PaceStage;
use jiff::Timestamp;
use jiff::tz::TimeZone;

fn utc_date(year: i16, month: i8, day: i8, hour: i8, minute: i8) -> Timestamp {
    jiff::civil::date(year, month, day)
        .at(hour, minute, 0, 0)
        .to_zoned(TimeZone::UTC)
        .expect("valid utc date")
        .timestamp()
}

fn la_date(year: i16, month: i8, day: i8, hour: i8, minute: i8) -> Timestamp {
    let tz = TimeZone::get("America/Los_Angeles").expect("America/Los_Angeles timezone");
    jiff::civil::date(year, month, day)
        .at(hour, minute, 0, 0)
        .to_zoned(tz)
        .expect("valid la date")
        .timestamp()
}

fn add_seconds(ts: Timestamp, seconds: i64) -> Timestamp {
    ts.checked_add(jiff::Span::new().seconds(seconds))
        .expect("valid add")
}

fn sub_seconds(ts: Timestamp, seconds: i64) -> Timestamp {
    ts.checked_sub(jiff::Span::new().seconds(seconds))
        .expect("valid sub")
}

// upstream: UsagePaceTests.swift:6 weekly pace computes delta and eta
#[test]
fn weekly_pace_computes_delta_and_eta() {
    let now = Timestamp::from_second(0).unwrap();
    let window = RateWindow::new(
        50.0,
        Some(10080),
        Some(add_seconds(now, 4 * 24 * 3600)),
        None,
        None,
        false,
    )
    .unwrap();

    let pace = UsagePace::weekly(&window, now, None, None, &TimeZone::UTC);

    assert!(pace.is_some());
    let pace = pace.unwrap();
    assert!((pace.expected_used_percent - 42.857).abs() < 0.01);
    assert!((pace.delta_percent - 7.143).abs() < 0.01);
    assert_eq!(pace.stage, PaceStage::Ahead);
    assert!(!pace.will_last_to_reset);
    assert!(pace.eta_seconds.is_some());
    assert_eq!(pace.run_out_probability, None);
    assert!((pace.eta_seconds.unwrap() - (3.0 * 24.0 * 3600.0)).abs() < 1.0);
}

// upstream: UsagePaceTests.swift:28 weekly pace marks lasts to reset when usage is low
#[test]
fn weekly_pace_marks_lasts_to_reset_when_usage_is_low() {
    let now = Timestamp::from_second(0).unwrap();
    let window = RateWindow::new(
        5.0,
        Some(10080),
        Some(add_seconds(now, 4 * 24 * 3600)),
        None,
        None,
        false,
    )
    .unwrap();

    let pace = UsagePace::weekly(&window, now, None, None, &TimeZone::UTC);

    assert!(pace.is_some());
    let pace = pace.unwrap();
    assert!(pace.will_last_to_reset);
    assert_eq!(pace.eta_seconds, None);
    assert_eq!(pace.run_out_probability, None);
    assert_eq!(pace.stage, PaceStage::FarBehind);
    assert!((pace.speed_multiplier_to_reset.unwrap() - 14.25).abs() < 0.01);
}

// upstream: UsagePaceTests.swift:48 weekly pace speed headroom uses remaining burn capacity
#[test]
fn weekly_pace_speed_headroom_uses_remaining_burn_capacity() {
    let now = Timestamp::from_second(0).unwrap();
    let window = RateWindow::new(
        70.0,
        Some(10080),
        Some(add_seconds(now, (0.7 * 24.0 * 3600.0) as i64)),
        None,
        None,
        false,
    )
    .unwrap();

    let pace = UsagePace::weekly(&window, now, None, None, &TimeZone::UTC).expect("pace present");

    assert!((pace.expected_used_percent - 90.0).abs() < 0.01);
    assert!(pace.will_last_to_reset);
    assert!((pace.speed_multiplier_to_reset.unwrap() - 3.857).abs() < 0.01);
}

// upstream: UsagePaceTests.swift:64 historical pace speed headroom uses projected remaining usage
#[test]
fn historical_pace_speed_headroom_uses_projected_remaining_usage() {
    let pace = UsagePace::historical(45.0, 20.0, None, true, Some(0.0), Some(20.0));

    assert_eq!(pace.speed_multiplier_to_reset, Some(4.0));
}

// upstream: UsagePaceTests.swift:77 weekly pace hides when reset missing or outside window
#[test]
fn weekly_pace_hides_when_reset_missing_or_outside_window() {
    let now = Timestamp::from_second(0).unwrap();
    let missing = RateWindow::new(10.0, Some(10080), None, None, None, false).unwrap();
    let too_far = RateWindow::new(
        10.0,
        Some(10080),
        Some(add_seconds(now, 9 * 24 * 3600)),
        None,
        None,
        false,
    )
    .unwrap();

    assert!(UsagePace::weekly(&missing, now, None, None, &TimeZone::UTC).is_none());
    assert!(UsagePace::weekly(&too_far, now, None, None, &TimeZone::UTC).is_none());
}

// upstream: UsagePaceTests.swift:95 weekly pace hides when usage exists but no elapsed
#[test]
fn weekly_pace_hides_when_usage_exists_but_no_elapsed() {
    let now = Timestamp::from_second(0).unwrap();
    let window = RateWindow::new(
        12.0,
        Some(10080),
        Some(add_seconds(now, 7 * 24 * 3600)),
        None,
        None,
        false,
    )
    .unwrap();

    let pace = UsagePace::weekly(&window, now, None, None, &TimeZone::UTC);
    assert!(pace.is_none());
}

// upstream: UsagePaceTests.swift:111 workday aware pace shows on track for five day user on friday
#[test]
fn workday_aware_pace_shows_on_track_for_five_day_user_on_friday() {
    let calendar = TimeZone::UTC;
    let resets_at = utc_date(2026, 6, 14, 0, 0);
    let now = sub_seconds(resets_at, 30 * 3600);

    let window = RateWindow::new(100.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let pace7 =
        UsagePace::weekly(&window, now, None, None, &calendar).expect("7-day pace expected");
    let pace5 =
        UsagePace::weekly(&window, now, None, Some(5), &calendar).expect("5-day pace expected");

    assert!(pace7.delta_percent > 15.0);
    assert!((pace5.expected_used_percent - 95.0).abs() < 0.01);
    assert!(pace5.delta_percent.abs() <= 5.0);
}

// upstream: UsagePaceTests.swift:154 six work days includes Saturday and excludes Sunday
#[test]
fn six_work_days_includes_saturday_and_excludes_sunday() {
    let calendar = TimeZone::UTC;
    let reset = utc_date(2026, 6, 15, 0, 0);
    let window = RateWindow::new(90.0, Some(10080), Some(reset), None, None, false).unwrap();

    let saturday = UsagePace::weekly(
        &window,
        sub_seconds(reset, 36 * 3600),
        None,
        Some(6),
        &calendar,
    )
    .expect("saturday pace expected");
    let sunday = UsagePace::weekly(
        &window,
        sub_seconds(reset, 12 * 3600),
        None,
        Some(6),
        &calendar,
    )
    .expect("sunday pace expected");

    assert!((saturday.expected_used_percent - 100.0 * 5.5 / 6.0).abs() < 0.01);
    assert_eq!(sunday.expected_used_percent, 100.0);
}

// upstream: UsagePaceTests.swift:179 workday aware pace shows on track midweek
#[test]
fn workday_aware_pace_shows_on_track_midweek() {
    let calendar = TimeZone::UTC;
    let resets_at = utc_date(2026, 6, 14, 0, 0);
    let now = sub_seconds(resets_at, 72 * 3600);

    let window = RateWindow::new(60.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let pace5 =
        UsagePace::weekly(&window, now, None, Some(5), &calendar).expect("5-day pace expected");

    assert!((pace5.expected_used_percent - 60.0).abs() < 0.01);
    assert!(pace5.delta_percent.abs() < 0.01);
}

// upstream: UsagePaceTests.swift:217 workday aware exhausted quota does not last through weekend
#[test]
fn workday_aware_exhausted_quota_does_not_last_through_weekend() {
    let calendar = TimeZone::UTC;
    let resets_at = utc_date(2026, 6, 14, 0, 0);
    let now = utc_date(2026, 6, 13, 12, 0);

    let window = RateWindow::new(100.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let pace =
        UsagePace::weekly(&window, now, None, Some(5), &calendar).expect("5-day pace expected");

    assert!(!pace.will_last_to_reset);
    assert_eq!(pace.eta_seconds, Some(0.0));
}

// upstream: UsagePaceTests.swift:250 workday aware eta excludes non workday elapsed time
#[test]
fn workday_aware_eta_excludes_non_workday_elapsed_time() {
    let calendar = TimeZone::UTC;
    let resets_at = utc_date(2026, 6, 14, 0, 0);
    let now = utc_date(2026, 6, 8, 12, 0);

    let window = RateWindow::new(20.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let pace =
        UsagePace::weekly(&window, now, None, Some(5), &calendar).expect("5-day pace expected");

    assert!(!pace.will_last_to_reset);
    assert!((pace.eta_seconds.unwrap() - (48.0 * 3600.0)).abs() < 1.0);
}

// upstream: UsagePaceTests.swift:283 workday aware eta maps work time across a weekend
#[test]
fn workday_aware_eta_maps_work_time_across_a_weekend() {
    let calendar = TimeZone::UTC;
    let resets_at = utc_date(2026, 6, 17, 0, 0);
    let now = utc_date(2026, 6, 12, 12, 0);

    let window = RateWindow::new(60.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let pace =
        UsagePace::weekly(&window, now, None, Some(5), &calendar).expect("5-day pace expected");

    assert!(!pace.will_last_to_reset);
    assert!((pace.eta_seconds.unwrap() - (88.0 * 3600.0)).abs() < 1.0);
}

// upstream: UsagePaceTests.swift:315 workday aware pace stays flat on non workdays
#[test]
fn workday_aware_pace_stays_flat_on_non_workdays() {
    let calendar = TimeZone::UTC;
    let resets_at = utc_date(2026, 6, 17, 0, 0);
    let saturday = utc_date(2026, 6, 13, 12, 0);
    let sunday = utc_date(2026, 6, 14, 12, 0);

    let window = RateWindow::new(60.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let saturday_pace = UsagePace::weekly(&window, saturday, None, Some(5), &calendar)
        .expect("saturday pace expected");
    let sunday_pace =
        UsagePace::weekly(&window, sunday, None, Some(5), &calendar).expect("sunday pace expected");

    assert!((saturday_pace.expected_used_percent - 60.0).abs() < 0.01);
    assert_eq!(
        sunday_pace.expected_used_percent,
        saturday_pace.expected_used_percent
    );
}

// upstream: UsagePaceTests.swift:357 zero usage becomes safe only after the first configured workday begins
#[test]
fn zero_usage_becomes_safe_only_after_the_first_configured_workday_begins() {
    let tz = TimeZone::get("America/Los_Angeles").expect("America/Los_Angeles timezone");
    let resets_at = la_date(2026, 6, 14, 0, 0);
    let first_workday = la_date(2026, 6, 8, 0, 0);

    let window = RateWindow::new(0.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let before = UsagePace::weekly(&window, sub_seconds(first_workday, 1), None, Some(5), &tz)
        .expect("before pace expected");
    let boundary = UsagePace::weekly(&window, first_workday, None, Some(5), &tz)
        .expect("boundary pace expected");
    let after = UsagePace::weekly(
        &window,
        add_seconds(first_workday, 3600),
        None,
        Some(5),
        &tz,
    )
    .expect("after pace expected");

    assert_eq!(before.expected_used_percent, 0.0);
    assert!(!before.will_last_to_reset);
    assert_eq!(boundary.expected_used_percent, 0.0);
    assert!(!boundary.will_last_to_reset);
    assert!(after.expected_used_percent > 0.0);
    assert!(after.will_last_to_reset);
}

// upstream: UsagePaceTests.swift:403 workday aware pace does not declare zero usage safe before first workday
#[test]
fn workday_aware_pace_does_not_declare_zero_usage_safe_before_first_workday() {
    let calendar = TimeZone::UTC;
    let resets_at = utc_date(2026, 6, 14, 0, 0);
    let now = utc_date(2026, 6, 7, 12, 0);

    let window = RateWindow::new(0.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let pace =
        UsagePace::weekly(&window, now, None, Some(5), &calendar).expect("5-day pace expected");

    assert_eq!(pace.expected_used_percent, 0.0);
    assert!(!pace.will_last_to_reset);
    assert_eq!(pace.eta_seconds, None);
}

// upstream: UsagePaceTests.swift:437 workday aware exhausted quota stays exhausted before first workday
#[test]
fn workday_aware_exhausted_quota_stays_exhausted_before_first_workday() {
    let calendar = TimeZone::UTC;
    let resets_at = utc_date(2026, 6, 14, 0, 0);
    let now = utc_date(2026, 6, 7, 12, 0);

    let window = RateWindow::new(100.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let pace =
        UsagePace::weekly(&window, now, None, Some(5), &calendar).expect("5-day pace expected");

    assert!(!pace.will_last_to_reset);
    assert_eq!(pace.eta_seconds, Some(0.0));
}

// upstream: UsagePaceTests.swift:470 workday aware pace splits a non midnight reset at local day boundaries
#[test]
fn workday_aware_pace_splits_a_non_midnight_reset_at_local_day_boundaries() {
    let calendar = TimeZone::UTC;
    let resets_at = utc_date(2026, 6, 14, 20, 0);
    let now = utc_date(2026, 6, 8, 12, 0);

    let window = RateWindow::new(10.0, Some(10080), Some(resets_at), None, None, false).unwrap();

    let pace =
        UsagePace::weekly(&window, now, None, Some(5), &calendar).expect("5-day pace expected");

    assert!((pace.expected_used_percent - 10.0).abs() < 0.01);
    assert!(pace.delta_percent.abs() < 0.01);
}

// upstream: UsagePaceTests.swift:506 workday aware pace falls back to linear when workDays is nil or 7
#[test]
fn workday_aware_pace_falls_back_to_linear_when_work_days_is_nil_or_7() {
    let now = Timestamp::from_second(0).unwrap();
    let window = RateWindow::new(
        50.0,
        Some(10080),
        Some(add_seconds(now, 4 * 24 * 3600)),
        None,
        None,
        false,
    )
    .unwrap();

    let pace_nil = UsagePace::weekly(&window, now, None, None, &TimeZone::UTC)
        .expect("pace nil workdays expected");
    let pace_7 = UsagePace::weekly(&window, now, None, Some(7), &TimeZone::UTC)
        .expect("pace 7 workdays expected");
    let pace_default =
        UsagePace::weekly(&window, now, None, None, &TimeZone::UTC).expect("pace default expected");

    assert!((pace_nil.expected_used_percent - pace_default.expected_used_percent).abs() < 0.01);
    assert!((pace_7.expected_used_percent - pace_default.expected_used_percent).abs() < 0.01);
}

// upstream: UsagePaceTests.swift:524 workdays off linear weekly pace keeps deficit sign
#[test]
fn workdays_off_linear_weekly_pace_keeps_deficit_sign() {
    let now = Timestamp::from_second(0).unwrap();
    let window = RateWindow::new(
        88.0,
        Some(10080),
        Some(add_seconds(now, 2 * 24 * 3600 + 19 * 3600)),
        None,
        None,
        false,
    )
    .unwrap();

    let pace = UsagePace::weekly(&window, now, None, None, &TimeZone::UTC).expect("pace expected");

    assert!((pace.expected_used_percent - (101.0 / 168.0 * 100.0)).abs() < 0.01);
    assert!(pace.delta_percent > 25.0);
    assert_eq!(pace.stage, PaceStage::FarAhead);
    assert!(!pace.will_last_to_reset);
}

// upstream: UsagePaceTests.swift:541 workday aware pace ignores non weekly windows
#[test]
fn workday_aware_pace_ignores_non_weekly_windows() {
    let now = Timestamp::from_second(0).unwrap();
    let window = RateWindow::new(
        50.0,
        Some(300),
        Some(add_seconds(now, 2 * 3600)),
        None,
        None,
        false,
    )
    .unwrap();

    let pace_no_work = UsagePace::weekly(&window, now, Some(300), None, &TimeZone::UTC)
        .expect("no workdays pace expected");
    let pace_work_5 = UsagePace::weekly(&window, now, Some(300), Some(5), &TimeZone::UTC)
        .expect("5 workdays pace expected");

    assert!((pace_no_work.expected_used_percent - pace_work_5.expected_used_percent).abs() < 0.01);
}

// upstream: UsagePaceTests.swift:559 session pace computes delta and eta for five hour window
#[test]
fn session_pace_computes_delta_and_eta_for_five_hour_window() {
    let now = Timestamp::from_second(0).unwrap();
    let window = RateWindow::new(
        50.0,
        Some(300),
        Some(add_seconds(now, 2 * 3600)),
        None,
        None,
        false,
    )
    .unwrap();

    let pace = UsagePace::weekly(&window, now, Some(300), None, &TimeZone::UTC);

    assert!(pace.is_some());
    let pace = pace.unwrap();
    assert!((pace.expected_used_percent - 60.0).abs() < 0.01);
    assert!((pace.delta_percent - (-10.0)).abs() < 0.01);
    assert_eq!(pace.stage, PaceStage::Behind);
    assert!(pace.will_last_to_reset);
}

// upstream: UsagePaceTests.swift:578 one work day falls back to linear pace
#[test]
fn one_work_day_falls_back_to_linear_pace() {
    let now = Timestamp::from_second(0).unwrap();
    let window = RateWindow::new(
        50.0,
        Some(10080),
        Some(add_seconds(now, 4 * 24 * 3600)),
        None,
        None,
        false,
    )
    .unwrap();

    let pace_one = UsagePace::weekly(&window, now, None, Some(1), &TimeZone::UTC)
        .expect("pace 1 workday expected");
    let pace_nil =
        UsagePace::weekly(&window, now, None, None, &TimeZone::UTC).expect("pace nil expected");

    assert!((pace_one.expected_used_percent - pace_nil.expected_used_percent).abs() < 0.01);
}

// upstream: ProviderPaceCapabilityTests.swift:56 amp monthly pace is limited to subscription windows
#[test]
fn amp_monthly_pace_is_limited_to_subscription_windows() {
    let now = Timestamp::from_second(1_750_000_000).unwrap();
    let capability = ProviderPaceCapability {
        reset_window_pace: ProviderPaceWindowRule::Custom(|window, _| {
            window.window_minutes.is_some()
                && window
                    .reset_description
                    .as_deref()
                    .is_some_and(|d| d.starts_with("renews in "))
        }),
        ..ProviderPaceCapability::default()
    };
    let free_tier = RateWindow::new(
        50.0,
        Some(24 * 60),
        Some(add_seconds(now, 12 * 60 * 60)),
        None,
        None,
        false,
    )
    .unwrap();
    let subscription = RateWindow::new(
        50.0,
        Some(MONTHLY_WINDOW_SENTINEL_MINUTES),
        Some(add_seconds(now, 20 * 24 * 60 * 60)),
        Some("renews in 20 days".to_string()),
        None,
        false,
    )
    .unwrap();

    assert!(!capability.supports_reset_window_pace(&free_tier, now));
    assert!(!capability.uses_inferred_monthly_duration(&free_tier));
    assert!(capability.supports_reset_window_pace(&subscription, now));
    assert!(!capability.uses_inferred_monthly_duration(&subscription));
}

// upstream: ProviderPaceCapabilityTests.swift:74 calendar month pace resolves the real cycle duration
#[test]
fn calendar_month_pace_resolves_the_real_cycle_duration() {
    let resets_at = utc_date(2026, 3, 1, 0, 0);
    let window = RateWindow::new(
        50.0,
        Some(MONTHLY_WINDOW_SENTINEL_MINUTES),
        Some(resets_at),
        None,
        None,
        false,
    )
    .unwrap();

    let resolved = ProviderPaceCapability::calendar_month_reset_window()
        .resolved_reset_window_for_pace(&window);

    assert_eq!(resolved.window_minutes, Some(28 * 24 * 60));
    assert_eq!(resolved.resets_at, Some(resets_at));
    assert_eq!(resolved.used_percent, window.used_percent);
}

// upstream: ProviderPaceCapabilityTests.swift:95 zai pace maps only verified coding windows
#[test]
fn zai_pace_maps_only_verified_coding_windows() {
    let now = Timestamp::from_second(1_750_000_000).unwrap();
    let capability = ProviderPaceCapability {
        primary: Some(ProviderStandardPaceLane::exact(
            ProviderPaceKind::Session,
            5 * 60,
        )),
        secondary: Some(ProviderStandardPaceLane::exact(
            ProviderPaceKind::Weekly,
            DEFAULT_WEEKLY_WINDOW_MINUTES,
        )),
        tertiary: None,
        shows_headroom_hint: false,
        session_pace_window_rule: ProviderPaceWindowRule::WindowDuration(5 * 60),
        reset_window_pace: ProviderPaceWindowRule::Custom(|window, _| {
            window.window_minutes == Some(MONTHLY_WINDOW_SENTINEL_MINUTES)
                && window.reset_description.as_deref() == Some("MCP")
        }),
        inferred_monthly_duration: ProviderPaceDurationRule::Custom(|window| {
            window.window_minutes == Some(MONTHLY_WINDOW_SENTINEL_MINUTES)
                && window.reset_description.as_deref() == Some("MCP")
        }),
        allows_estimated_usage: false,
    };
    let session = RateWindow::new(
        50.0,
        Some(5 * 60),
        Some(add_seconds(now, 2 * 60 * 60)),
        None,
        None,
        false,
    )
    .unwrap();
    let weekly = RateWindow::new(
        50.0,
        Some(DEFAULT_WEEKLY_WINDOW_MINUTES),
        Some(add_seconds(now, 4 * 24 * 60 * 60)),
        None,
        None,
        false,
    )
    .unwrap();
    let unknown = RateWindow::new(
        50.0,
        None,
        Some(add_seconds(now, 2 * 60 * 60)),
        None,
        None,
        false,
    )
    .unwrap();
    let monthly_mcp = RateWindow::new(
        50.0,
        Some(MONTHLY_WINDOW_SENTINEL_MINUTES),
        Some(add_seconds(now, 20 * 24 * 60 * 60)),
        Some("MCP".to_string()),
        None,
        false,
    )
    .unwrap();
    let rolling_thirty_day = RateWindow::new(
        50.0,
        Some(MONTHLY_WINDOW_SENTINEL_MINUTES),
        Some(add_seconds(now, 20 * 24 * 60 * 60)),
        Some("30 days window".to_string()),
        None,
        false,
    )
    .unwrap();

    assert_eq!(
        capability.resolved_kind(ProviderPaceSlot::Primary, &session, now),
        Some(ProviderPaceKind::Session)
    );
    assert_eq!(
        capability.resolved_kind(ProviderPaceSlot::Secondary, &weekly, now),
        Some(ProviderPaceKind::Weekly)
    );
    assert_eq!(
        capability.resolved_kind(ProviderPaceSlot::Primary, &weekly, now),
        None
    );
    assert_eq!(
        capability.resolved_kind(ProviderPaceSlot::Secondary, &session, now),
        None
    );
    assert!(capability.supports_session_pace(&session, now));
    assert!(!capability.supports_session_pace(&unknown, now));
    assert!(capability.supports_reset_window_pace(&monthly_mcp, now));
    assert!(capability.uses_inferred_monthly_duration(&monthly_mcp));
    assert!(!capability.supports_reset_window_pace(&rolling_thirty_day, now));
    assert!(!capability.uses_inferred_monthly_duration(&rolling_thirty_day));
    assert_eq!(
        capability.resolved_kind(ProviderPaceSlot::Primary, &rolling_thirty_day, now),
        None
    );
    assert_eq!(
        capability.resolved_reset_window_for_pace(&rolling_thirty_day),
        rolling_thirty_day
    );
}

// upstream: ProviderPaceCapabilityTests.swift:9 descriptor pace capabilities match the supported provider mapping
#[test]
fn descriptor_pace_capabilities_match_the_supported_provider_mapping() {
    let now = Timestamp::from_second(1_750_000_000).unwrap();
    let fixtures = vec![
        RateWindow::new(50.0, None, None, None, None, false).unwrap(),
        RateWindow::new(
            50.0,
            None,
            Some(add_seconds(now, 4 * 24 * 60 * 60)),
            None,
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(
            50.0,
            None,
            Some(add_seconds(now, 8 * 24 * 60 * 60)),
            None,
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(
            50.0,
            None,
            Some(add_seconds(now, 30 * 24 * 60 * 60)),
            None,
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(
            50.0,
            Some(60),
            Some(add_seconds(now, 30 * 60)),
            None,
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(
            50.0,
            Some(DEFAULT_WEEKLY_WINDOW_MINUTES),
            None,
            None,
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(
            50.0,
            Some(DEFAULT_WEEKLY_WINDOW_MINUTES),
            Some(add_seconds(now, 4 * 24 * 60 * 60)),
            None,
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(
            50.0,
            Some(DEFAULT_WEEKLY_WINDOW_MINUTES),
            Some(add_seconds(now, 8 * 24 * 60 * 60)),
            None,
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(
            50.0,
            Some(MONTHLY_WINDOW_SENTINEL_MINUTES),
            None,
            None,
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(
            50.0,
            Some(MONTHLY_WINDOW_SENTINEL_MINUTES),
            Some(add_seconds(now, 20 * 24 * 60 * 60)),
            None,
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(
            50.0,
            Some(MONTHLY_WINDOW_SENTINEL_MINUTES),
            Some(add_seconds(now, 20 * 24 * 60 * 60)),
            Some("MCP".to_string()),
            None,
            false,
        )
        .unwrap(),
        RateWindow::new(50.0, Some(0), Some(add_seconds(now, 60)), None, None, false).unwrap(),
        RateWindow::new(
            50.0,
            Some(DEFAULT_WEEKLY_WINDOW_MINUTES),
            Some(sub_seconds(now, 60)),
            None,
            None,
            false,
        )
        .unwrap(),
    ];

    for descriptor in first_party_order() {
        let capability = &descriptor.pace_capability;
        for window in &fixtures {
            // Claude matches the calendar-month sentinel; Codex and Antigravity declare neither rule.
            let expected = descriptor.id.as_str() == "claude"
                && window.window_minutes == Some(MONTHLY_WINDOW_SENTINEL_MINUTES);

            let actual_reset_window_pace = capability.supports_reset_window_pace(window, now);
            assert_eq!(
                actual_reset_window_pace, expected,
                "Reset-window pace changed for {}, window={:?}",
                descriptor.id, window
            );

            let actual_monthly_inference = capability.uses_inferred_monthly_duration(window);
            assert_eq!(
                actual_monthly_inference, expected,
                "Monthly inference changed for {}, window={:?}",
                descriptor.id, window
            );
        }
    }
}
