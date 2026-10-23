// Ported from CodexBar Sources/CodexBarCore/UsageFormatter.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/UsageFormatterTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

mod common;

use ariadusage_macos::format::{
    ResetTimeDisplayStyle, clean_plan_name, compact_currency_string, credits_number_string,
    credits_string, currency_string, grouped_u64, percent_string, percent_text, plan_display,
    remaining_string, reset_countdown_description, reset_description, reset_line,
    share_stats_compact_count, token_count_string, updated_string, usage_line,
};
use ariadusage_protocol::usage::RateWindow;
use common::{FixedDates, ts};
use jiff::{SignedDuration, Timestamp};

fn after(now: Timestamp, seconds: f64) -> Timestamp {
    now + SignedDuration::from_secs_f64(seconds)
}

fn window(resets_at: Option<Timestamp>, description: Option<&str>) -> RateWindow {
    RateWindow {
        resets_at,
        reset_description: description.map(str::to_string),
        ..RateWindow::new(68.0)
    }
}

#[test]
fn percent_string_clamps_and_marks_tiny_values() {
    assert_eq!(percent_string(0.0), "0%");
    assert_eq!(percent_string(0.4), "<1%");
    assert_eq!(percent_string(0.999), "<1%");
    assert_eq!(percent_string(1.0), "1%");
    assert_eq!(percent_string(58.4), "58%");
    assert_eq!(percent_string(120.0), "100%");
    assert_eq!(percent_string(-3.0), "0%");
}

#[test]
fn percent_string_rounds_ties_to_even_like_printf() {
    assert_eq!(percent_string(2.5), "2%");
    assert_eq!(percent_string(3.5), "4%");
}

#[test]
fn formats_usage_line() {
    assert_eq!(usage_line(25.0, 75.0, false), "25% left");
    assert_eq!(usage_line(25.0, 75.0, true), "75% used");
    assert_eq!(percent_text(0.5, "left"), "<1% left");
}

#[test]
fn positive_sub_percent_usage_stays_visible() {
    assert_eq!(percent_string(-1.0), "0%");
    assert_eq!(percent_string(0.1), "<1%");
    assert_eq!(percent_string(0.96), "<1%");
    assert_eq!(percent_string(101.0), "100%");
    assert_eq!(usage_line(99.9, 0.1, true), "<1% used");
    assert_eq!(usage_line(99.4, 0.6, true), "<1% used");
    assert_eq!(usage_line(99.25, 0.75, true), "<1% used");
    assert_eq!(usage_line(0.75, 99.25, false), "<1% left");
}

#[test]
fn reset_countdown_rounds_and_limits_displayed_units() {
    let cases: [(&str, f64, &str); 9] = [
        ("minutes", 10.0 * 60.0 + 1.0, "in 11m"),
        ("hours and minutes", 3.0 * 3600.0 + 31.0 * 60.0, "in 3h 31m"),
        ("days with hours", 26.0 * 3600.0 + 60.0, "in 1d 2h"),
        ("days and exact hours", 26.0 * 3600.0, "in 1d 2h"),
        ("days and minutes", 24.0 * 3600.0 + 5.0 * 60.0, "in 1d 5m"),
        ("exact days", 2.0 * 24.0 * 3600.0, "in 2d"),
        (
            "last minute rounds into a day",
            24.0 * 3600.0 - 59.0,
            "in 1d",
        ),
        ("exact hour", 3600.0, "in 1h"),
        ("past date", -10.0, "now"),
    ];
    for (scenario, seconds, expected) in cases {
        assert_eq!(reset_countdown_description(seconds), expected, "{scenario}");
    }
}

#[test]
fn reset_countdown_has_a_one_minute_floor() {
    assert_eq!(reset_countdown_description(1.0), "in 1m");
    assert_eq!(reset_countdown_description(0.5), "now");
}

#[test]
fn reset_line_uses_countdown_when_resets_at_is_available() {
    let now = Timestamp::from_second(1_000_000).unwrap();
    let w = window(Some(after(now, 601.0)), Some("Resets soon"));
    let text = reset_line(
        &w,
        ResetTimeDisplayStyle::Countdown,
        now,
        &FixedDates::utc(),
    );
    assert_eq!(text.as_deref(), Some("Resets in 11m"));
}

#[test]
fn reset_line_says_now_for_past_resets() {
    let now = Timestamp::from_second(1_000_000).unwrap();
    let w = window(Some(after(now, -30.0)), None);
    let text = reset_line(
        &w,
        ResetTimeDisplayStyle::Countdown,
        now,
        &FixedDates::utc(),
    );
    assert_eq!(text.as_deref(), Some("Resets now"));
}

#[test]
fn reset_line_falls_back_to_provided_description() {
    let now = Timestamp::from_second(1_000_000).unwrap();
    let w = window(None, Some("Resets at 23:30 (UTC)"));
    for style in [
        ResetTimeDisplayStyle::Countdown,
        ResetTimeDisplayStyle::Absolute,
    ] {
        let text = reset_line(&w, style, now, &FixedDates::utc());
        assert_eq!(text.as_deref(), Some("Resets at 23:30 (UTC)"));
    }
}

#[test]
fn reset_description_normalizes_singular_and_plural_labels() {
    let now = Timestamp::from_second(1_000_000).unwrap();
    let cases = [
        (
            "Reset Jul 10 at 2:59am (Europe/Prague)",
            "Resets Jul 10 at 2:59am (Europe/Prague)",
        ),
        (
            "Resets Jul 10 at 2:59am (Europe/Prague)",
            "Resets Jul 10 at 2:59am (Europe/Prague)",
        ),
        ("Reset in 11m", "Resets in 11m"),
        ("Resets in 11m", "Resets in 11m"),
        (" \nReSeT at 23:30 (UTC)\t", "Resets at 23:30 (UTC)"),
        ("  rEsEt In 2h 5m \n", "Resets in 2h 5m"),
        ("Reset demain à 23:30", "Resets demain à 23:30"),
        ("at 23:30 (UTC)", "Resets at 23:30 (UTC)"),
        ("Resetting soon", "Resets Resetting soon"),
    ];
    for (description, expected) in cases {
        let w = window(None, Some(description));
        for style in [
            ResetTimeDisplayStyle::Countdown,
            ResetTimeDisplayStyle::Absolute,
        ] {
            assert_eq!(
                reset_line(&w, style, now, &FixedDates::utc()).as_deref(),
                Some(expected),
                "{description:?}"
            );
        }
    }
}

#[test]
fn blank_reset_description_has_no_line() {
    let now = Timestamp::from_second(1_000_000).unwrap();
    let w = window(None, Some("  \n"));
    assert_eq!(
        reset_line(
            &w,
            ResetTimeDisplayStyle::Countdown,
            now,
            &FixedDates::utc()
        ),
        None
    );
    assert_eq!(
        reset_line(
            &RateWindow::new(1.0),
            ResetTimeDisplayStyle::Countdown,
            now,
            &FixedDates::utc()
        ),
        None
    );
}

#[test]
fn parsed_reset_date_takes_precedence_over_singular_description() {
    let now = Timestamp::from_second(1_000_000).unwrap();
    let reset = after(now, 601.0);
    let w = window(Some(reset), Some("Reset in 99h"));
    let dates = FixedDates::utc();
    assert_eq!(
        reset_line(&w, ResetTimeDisplayStyle::Countdown, now, &dates).as_deref(),
        Some("Resets in 11m")
    );
    assert_eq!(
        reset_line(&w, ResetTimeDisplayStyle::Absolute, now, &dates),
        Some(format!("Resets {}", reset_description(reset, now, &dates)))
    );
}

#[test]
fn absolute_reset_names_today_tomorrow_or_the_date_in_the_local_zone() {
    // 2026-10-23 10:00 in Saigon (UTC+7).
    let dates = FixedDates::zone("Asia/Ho_Chi_Minh");
    let now = ts("2026-10-23T03:00:00Z");
    assert_eq!(
        reset_description(ts("2026-10-23T08:05:00Z"), now, &dates),
        "3:05 PM"
    );
    // 16:59 UTC is already the next local day.
    assert_eq!(
        reset_description(ts("2026-10-23T17:05:00Z"), now, &dates),
        "tomorrow, 12:05 AM"
    );
    assert_eq!(
        reset_description(ts("2026-10-25T08:05:00Z"), now, &dates),
        "Oct 25 at 3:05 PM"
    );
    let w = window(Some(ts("2026-10-24T08:05:00Z")), None);
    assert_eq!(
        reset_line(&w, ResetTimeDisplayStyle::Absolute, now, &dates).as_deref(),
        Some("Resets tomorrow, 3:05 PM")
    );
}

#[test]
fn relative_updated_recent() {
    let now = Timestamp::from_second(1_710_048_000).unwrap();
    let text = updated_string(after(now, -5.0 * 3600.0), now, &FixedDates::utc());
    assert_eq!(text, "Updated 5 hr ago");
}

#[test]
fn updated_just_now_within_a_minute_either_way() {
    let now = Timestamp::from_second(1_710_048_000).unwrap();
    let dates = FixedDates::utc();
    assert_eq!(
        updated_string(after(now, -59.9), now, &dates),
        "Updated just now"
    );
    assert_eq!(
        updated_string(after(now, 30.0), now, &dates),
        "Updated just now"
    );
    assert_eq!(
        updated_string(after(now, -61.0), now, &dates),
        "Updated 1 min ago"
    );
}

#[test]
fn absolute_updated_old() {
    let now = Timestamp::from_second(1_710_048_000).unwrap();
    let text = updated_string(after(now, -26.0 * 3600.0), now, &FixedDates::utc());
    assert!(text.starts_with("Updated "));
    assert!(!text.contains("ago"));
    assert_eq!(text, "Updated 3:20 AM");
}

#[test]
fn credits_string_formats_correctly() {
    assert_eq!(credits_string(42.5), "42.5 left");
    assert_eq!(remaining_string("€24.99"), "€24.99 left");
}

#[test]
fn credits_numbers_match_number_formatter_in_posix_locale() {
    // Values observed from NumberFormatter (decimal, max 2 fraction digits,
    // en_US_POSIX) on macOS.
    let cases: [(f64, &str); 18] = [
        (42.5, "42.5"),
        (1234.567, "1234.57"),
        (1_234_567.0, "1234567"),
        (0.005, "0"),
        (0.015, "0.02"),
        (0.025, "0.02"),
        (2.675, "2.68"),
        (-3.0, "-3"),
        (1e21, "1000000000000000000000"),
        (0.0, "0"),
        (0.125, "0.12"),
        (0.135, "0.14"),
        (-0.001, "-0"),
        (999.995, "1000"),
        (9.995, "10"),
        (0.1 + 0.2, "0.3"),
        (1.005, "1"),
        (123_456_789.125, "123456789.12"),
    ];
    for (value, expected) in cases {
        assert_eq!(credits_number_string(value), expected, "{value}");
    }
}

#[test]
fn token_count_string_formats_small_values_without_grouping() {
    assert_eq!(token_count_string(0), "0");
    assert_eq!(token_count_string(987), "987");
    assert_eq!(token_count_string(-42), "-42");
}

#[test]
fn token_count_string_promotes_rounded_unit_boundaries() {
    assert_eq!(token_count_string(999_499), "999K");
    assert_eq!(token_count_string(999_500), "1M");
    assert_eq!(token_count_string(999_999), "1M");
    assert_eq!(token_count_string(999_499_999), "999M");
    assert_eq!(token_count_string(999_500_000), "1B");
    assert_eq!(token_count_string(999_999_999), "1B");
    assert_eq!(token_count_string(-999_499), "-999K");
    assert_eq!(token_count_string(-999_500), "-1M");
    assert_eq!(token_count_string(-999_999), "-1M");
    assert_eq!(token_count_string(-999_499_999), "-999M");
    assert_eq!(token_count_string(-999_500_000), "-1B");
    assert_eq!(token_count_string(-999_999_999), "-1B");
    assert_eq!(token_count_string(1500), "1.5K");
    assert_eq!(token_count_string(100), "100");
}

#[test]
fn token_count_string_handles_integer_limits() {
    assert_eq!(token_count_string(i64::MAX), "9223372037B");
    assert_eq!(token_count_string(i64::MIN), "-9223372037B");
}

#[test]
fn clean_plan_maps_oauth_to_ollama() {
    assert_eq!(clean_plan_name("oauth"), "Ollama");
}

#[test]
fn clean_plan_drops_boilerplate_and_terminal_noise() {
    assert_eq!(clean_plan_name("Claude Max Account"), "Max");
    assert_eq!(clean_plan_name("\u{1b}[1mpro plan\u{1b}[0m"), "Pro");
    assert_eq!(clean_plan_name("[1m team"), "Team");
    assert_eq!(clean_plan_name("Plan"), "Plan");
    assert_eq!(clean_plan_name("AI  Ultra"), "AI Ultra");
}

#[test]
fn plan_display_uses_codex_tier_names_for_codex_only() {
    assert_eq!(plan_display("codex", "pro"), "Pro 20x");
    assert_eq!(plan_display("codex", "prolite"), "Pro 5x");
    assert_eq!(plan_display("codex", "Plus Plan"), "Plus");
    assert_eq!(plan_display("codex", "k12_edu"), "K12 Edu");
    assert_eq!(plan_display("claude", "pro"), "Pro");
}

#[test]
fn currency_and_share_stats_compact_counts_match_upstream_rules() {
    assert_eq!(grouped_u64(0), "0");
    assert_eq!(grouped_u64(3_570), "3,570");
    assert_eq!(grouped_u64(90_141), "90,141");
    assert_eq!(currency_string(298.8, "USD"), "$298.80");
    assert_eq!(currency_string(2422.13, "USD"), "$2,422.13");
    assert_eq!(compact_currency_string(298.8, "USD"), "$299");
    assert_eq!(share_stats_compact_count(950), "950");
    assert_eq!(share_stats_compact_count(12_600_000), "12.6M");
    assert_eq!(share_stats_compact_count(999_950_000), "1B");
    assert_eq!(share_stats_compact_count(1_235_792_510), "1.24B");
}
