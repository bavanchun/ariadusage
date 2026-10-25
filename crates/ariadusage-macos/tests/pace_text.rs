// Ported from CodexBar Tests/CodexBarTests/UsagePaceTextTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The upstream tests derive pace from a window with `UsagePace.weekly`;
//! here the engine owns that computation, so each case states the pace the
//! window yields.

mod common;

use ariadusage_macos::pace_text::{
    PaceContext, PaceFacts, detail, session_detail, session_equivalent_detail, session_pace,
    session_summary,
};
use ariadusage_protocol::usage::{PaceStage, RateWindow, SessionEquivalentForecast};
use common::{FixedDates, FrenchDates};
use jiff::Timestamp;

const DAY: f64 = 24.0 * 3600.0;

fn pace(stage: PaceStage, delta: f64, expected: f64) -> PaceFacts {
    PaceFacts {
        stage,
        delta_percent: delta,
        expected_used_percent: expected,
        will_last_to_reset: false,
        eta_seconds: None,
        run_out_probability: None,
        speed_multiplier_to_reset: None,
    }
}

fn rate_window(used: f64, minutes: Option<u32>, resets_in_seconds: Option<i64>) -> RateWindow {
    let now = Timestamp::from_second(0).unwrap();
    RateWindow {
        window_minutes: minutes,
        resets_at: resets_in_seconds.map(|s| now + jiff::SignedDuration::from_secs(s)),
        ..RateWindow::new(used)
    }
}

// upstream: UsagePaceTextTests.swift:31 weekly pace detail provides left right labels
// upstream: UsagePaceTextTests.swift:79 weekly pace summary formats single line text
#[test]
fn weekly_pace_detail_provides_left_right_labels() {
    // 50 % used with 4 of 7 days left: 42.9 % expected, out in 3 days.
    let facts = PaceFacts {
        eta_seconds: Some(3.0 * DAY),
        ..pace(PaceStage::Ahead, 50.0 - 300.0 / 7.0, 300.0 / 7.0)
    };
    let d = detail(&facts, true, PaceContext::Weekly);
    assert_eq!(d.left, "7% in deficit");
    assert_eq!(d.right.as_deref(), Some("Runs out in 3d"));
    assert_eq!(d.summary(), "Pace: 7% in deficit · Runs out in 3d");
}

// upstream: UsagePaceTextTests.swift:47 weekly pace detail treats rounded zero delta as on pace
#[test]
fn weekly_pace_detail_treats_rounded_zero_delta_as_on_pace() {
    let facts = PaceFacts {
        will_last_to_reset: true,
        ..pace(PaceStage::SlightlyBehind, -0.4, 50.4)
    };
    assert_eq!(detail(&facts, true, PaceContext::Weekly).left, "On pace");
}

// upstream: UsagePaceTextTests.swift:63 weekly pace detail reports lasts until reset
// upstream: UsagePaceTextTests.swift:94 weekly pace detail reports capped speed headroom when under pace
// upstream: UsagePaceTextTests.swift:125 weekly pace detail reports remaining headroom late in window
#[test]
fn weekly_pace_detail_reports_lasts_until_reset_with_headroom() {
    // 10 % used with 4 of 7 days left.
    let facts = PaceFacts {
        will_last_to_reset: true,
        speed_multiplier_to_reset: Some(7.0),
        ..pace(PaceStage::FarBehind, 10.0 - 300.0 / 7.0, 300.0 / 7.0)
    };
    let d = detail(&facts, true, PaceContext::Weekly);
    assert_eq!(d.left, "33% in reserve");
    assert_eq!(
        d.right.as_deref(),
        Some("Lasts until reset · 1.5× headroom")
    );
}

// upstream: UsagePaceTextTests.swift:110 weekly pace detail limits headroom hint to Codex
#[test]
fn weekly_pace_detail_limits_headroom_hint_to_providers_that_show_it() {
    let facts = PaceFacts {
        will_last_to_reset: true,
        speed_multiplier_to_reset: Some(4.0),
        ..pace(PaceStage::FarBehind, -37.0, 57.0)
    };
    assert_eq!(
        detail(&facts, false, PaceContext::Weekly).right.as_deref(),
        Some("Lasts until reset")
    );
}

// Rust-added: verifies headroom threshold boundaries (-15% delta and 1.5x multiplier)
#[test]
fn headroom_needs_a_large_reserve_and_multiplier() {
    let base = PaceFacts {
        will_last_to_reset: true,
        ..pace(PaceStage::Behind, -10.0, 40.0)
    };
    let small_reserve = PaceFacts {
        speed_multiplier_to_reset: Some(3.0),
        ..base
    };
    assert_eq!(
        detail(&small_reserve, true, PaceContext::Weekly)
            .right
            .as_deref(),
        Some("Lasts until reset")
    );
    let slow = PaceFacts {
        delta_percent: -20.0,
        speed_multiplier_to_reset: Some(1.4),
        ..base
    };
    assert_eq!(
        detail(&slow, true, PaceContext::Weekly).right.as_deref(),
        Some("Lasts until reset")
    );
}

// upstream: UsagePaceTextTests.swift:141 reported weekly state renders deficit and run out headline
#[test]
fn reported_weekly_state_renders_deficit_and_run_out_headline() {
    // 88 % used with 2 days 19 hours left: out in 13h 47m.
    let facts = PaceFacts {
        eta_seconds: Some(13.0 * 3600.0 + 46.5 * 60.0),
        ..pace(PaceStage::FarAhead, 28.4, 59.6)
    };
    let d = detail(&facts, true, PaceContext::Weekly);
    assert_eq!(d.left, "28% in deficit");
    assert_eq!(d.right.as_deref(), Some("Runs out in 13h 47m"));
}

// upstream: UsagePaceTextTests.swift:158 weekly pace detail formats rounded risk when available
#[test]
fn weekly_pace_detail_formats_rounded_risk_when_available() {
    let facts = PaceFacts {
        eta_seconds: Some(2.0 * DAY),
        run_out_probability: Some(0.683),
        ..pace(PaceStage::Ahead, 8.0, 42.0)
    };
    assert_eq!(
        detail(&facts, true, PaceContext::Weekly).right.as_deref(),
        Some("Runs out in 2d (70% risk)")
    );
}

// upstream: UsagePaceTextTests.swift:175 weekly pace detail combines reserve outcome with risk
#[test]
fn weekly_pace_detail_combines_reserve_outcome_with_risk() {
    let facts = PaceFacts {
        will_last_to_reset: true,
        run_out_probability: Some(0.45),
        ..pace(PaceStage::SlightlyBehind, -9.0, 21.0)
    };
    let d = detail(&facts, true, PaceContext::Weekly);
    assert_eq!(d.left, "9% in reserve");
    assert_eq!(d.right.as_deref(), Some("Lasts until reset (45% risk)"));
}

// upstream: UsagePaceTextTests.swift:193 weekly pace detail keeps lasts until reset only when rounded risk is zero
#[test]
fn weekly_pace_detail_keeps_lasts_until_reset_when_rounded_risk_is_zero() {
    let facts = PaceFacts {
        will_last_to_reset: true,
        run_out_probability: Some(0.02),
        speed_multiplier_to_reset: Some(4.0),
        ..pace(PaceStage::FarBehind, -30.0, 40.0)
    };
    assert_eq!(
        detail(&facts, true, PaceContext::Weekly).right.as_deref(),
        Some("Lasts until reset · 1.5× headroom (0% risk)")
    );
}

// upstream: UsagePaceTextTests.swift:211 weekly pace detail keeps outcome beside material risk
#[test]
fn weekly_pace_detail_keeps_outcome_beside_material_risk() {
    let facts = PaceFacts {
        will_last_to_reset: true,
        run_out_probability: Some(0.03),
        ..pace(PaceStage::SlightlyBehind, -9.0, 21.0)
    };
    assert_eq!(
        detail(&facts, true, PaceContext::Weekly).right.as_deref(),
        Some("Lasts until reset (5% risk)")
    );
}

// Rust-added: risk suffix without an ETA/lasts outcome
#[test]
fn risk_alone_when_there_is_no_outcome() {
    let facts = PaceFacts {
        run_out_probability: Some(0.5),
        ..pace(PaceStage::Ahead, 8.0, 42.0)
    };
    assert_eq!(
        detail(&facts, true, PaceContext::Weekly).right.as_deref(),
        Some("(50% risk)")
    );
}

// upstream: UsagePaceTextTests.swift:230 session pace detail provides left right labels
#[test]
fn session_pace_detail_provides_left_right_labels() {
    // 80 % used, 3 of 5 hours elapsed: 60 % expected, empty in 45 minutes.
    let facts = PaceFacts {
        eta_seconds: Some(45.0 * 60.0),
        ..pace(PaceStage::FarAhead, 20.0, 60.0)
    };
    let d = detail(&facts, false, PaceContext::Session);
    assert_eq!(d.left, "20% in deficit");
    assert_eq!(d.right.as_deref(), Some("Projected empty in 45m"));
    assert_eq!(d.summary(), "Pace: 20% in deficit · Projected empty in 45m");
}

// Rust-added: verifies zero-minute ETA wording for both Session and Weekly contexts
#[test]
fn session_and_weekly_wording_for_running_out_now() {
    let facts = PaceFacts {
        eta_seconds: Some(0.0),
        ..pace(PaceStage::FarAhead, 40.0, 60.0)
    };
    assert_eq!(
        detail(&facts, false, PaceContext::Session).right.as_deref(),
        Some("Projected empty now")
    );
    assert_eq!(
        detail(&facts, false, PaceContext::Weekly).right.as_deref(),
        Some("Runs out now")
    );
}

// upstream: UsagePaceTextTests.swift:249 Claude session pace does not show Codex headroom
#[test]
fn claude_session_pace_does_not_show_codex_headroom() {
    let facts = PaceFacts {
        will_last_to_reset: true,
        speed_multiplier_to_reset: Some(9.0),
        ..pace(PaceStage::FarBehind, -50.0, 60.0)
    };
    let d = detail(&facts, false, PaceContext::Session);
    assert_eq!(d.left, "50% in reserve");
    assert_eq!(d.right.as_deref(), Some("Lasts until reset"));
}

// upstream: UsagePaceTextTests.swift:267 Codex session pace shows conservative headroom
#[test]
fn codex_session_pace_shows_conservative_headroom() {
    let facts = PaceFacts {
        will_last_to_reset: true,
        speed_multiplier_to_reset: Some(9.0),
        ..pace(PaceStage::FarBehind, -50.0, 60.0)
    };
    assert_eq!(
        detail(&facts, true, PaceContext::Session).right.as_deref(),
        Some("Lasts until reset · 1.5× headroom")
    );
}

// upstream: UsagePaceTextTests.swift:281 Codex session pace suppressed for weekly and monthly durations but kept for fallback shapes
#[test]
fn codex_session_pace_suppressed_for_weekly_and_monthly_durations_but_kept_for_fallback_shapes() {
    let now = Timestamp::from_second(0).unwrap();
    let window = |minutes: Option<u32>| rate_window(50.0, minutes, Some(2 * 3600));

    assert!(session_pace("codex", &window(Some(10_080)), now).is_none());
    assert!(session_pace("codex", &window(Some(43_200)), now).is_none());
    assert!(session_pace("codex", &window(Some(540)), now).is_some());
    assert!(session_pace("codex", &window(None), now).is_some());
    assert!(session_pace("codex", &window(Some(300)), now).is_some());
}

// upstream: UsagePaceTextTests.swift:300 session pace summary formats single line text
#[test]
fn session_pace_summary_formats_single_line_text() {
    let now = Timestamp::from_second(0).unwrap();
    let window = rate_window(80.0, Some(300), Some(2 * 3600));

    let summary = session_summary("claude", &window, now);

    assert_eq!(
        summary.as_deref(),
        Some("Pace: 20% in deficit · Projected empty in 45m")
    );
}

// upstream: UsagePaceTextTests.swift:330 session pace detail supports Antigravity five hour window
#[test]
fn session_pace_detail_supports_antigravity_five_hour_window() {
    let now = Timestamp::from_second(0).unwrap();
    let window = rate_window(80.0, Some(300), Some(2 * 3600));

    let d = session_detail("antigravity", &window, now).expect("session detail expected");

    assert_eq!(d.left, "20% in deficit");
    assert_eq!(d.right.as_deref(), Some("Projected empty in 45m"));
}

// upstream: UsagePaceTextTests.swift:344 session pace detail hides Antigravity weekly window
#[test]
fn session_pace_detail_hides_antigravity_weekly_window() {
    let now = Timestamp::from_second(0).unwrap();
    let window = rate_window(80.0, Some(10_080), Some(2 * 24 * 3600));

    assert!(session_detail("antigravity", &window, now).is_none());
}

// upstream: UsagePaceTextTests.swift:372 session pace detail hides for unsupported provider
#[test]
fn session_pace_detail_hides_for_unsupported_provider() {
    let now = Timestamp::from_second(0).unwrap();
    let window = rate_window(50.0, Some(300), Some(2 * 3600));

    assert!(session_detail("deepseek", &window, now).is_none());
}

// upstream: UsagePaceTextTests.swift:400 session pace detail hides when reset is missing
#[test]
fn session_pace_detail_hides_when_reset_is_missing() {
    let now = Timestamp::from_second(0).unwrap();
    let window = rate_window(50.0, Some(300), None);

    assert!(session_detail("claude", &window, now).is_none());
}

// Rust-added: PaceDetail::summary formats without right label when absent
#[test]
fn summary_without_right_half() {
    let d = detail(
        &pace(PaceStage::Ahead, 5.0, 30.0),
        false,
        PaceContext::Weekly,
    );
    assert_eq!(d.right, None);
    assert_eq!(d.summary(), "Pace: 5% in deficit");
}

// Rust-added: session-equivalent forecast formatting and pluralization
#[test]
fn session_equivalent_detail_respects_locale_and_upstream_pluralization() {
    let forecast = SessionEquivalentForecast::new(2.84, 16, None);
    let en = session_equivalent_detail(&forecast, &FixedDates::utc());
    assert_eq!(en.left, "Est. 2.8 session quotas left");
    assert_eq!(en.right, "16 windows until reset");

    let fr = session_equivalent_detail(&forecast, &FrenchDates::utc());
    assert_eq!(fr.left, "Est. 2,8 session quotas left");
    assert_eq!(fr.right, "16 windows until reset");

    let singular = SessionEquivalentForecast::new(1.0, 1, None);
    let s = session_equivalent_detail(&singular, &FixedDates::utc());
    assert_eq!(s.left, "Est. 1 session quota left");
    assert_eq!(s.right, "1 windows until reset");

    let sub_one = SessionEquivalentForecast::new(0.6, 2, None);
    assert_eq!(
        session_equivalent_detail(&sub_one, &FixedDates::utc()).left,
        "Est. 0.6 session quota left"
    );

    let zero = SessionEquivalentForecast::new(0.0, 3, None);
    assert_eq!(
        session_equivalent_detail(&zero, &FixedDates::utc()).left,
        "Est. 0 session quotas left"
    );
}
