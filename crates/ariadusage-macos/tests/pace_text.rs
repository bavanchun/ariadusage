// Ported from CodexBar Tests/CodexBarTests/UsagePaceTextTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The upstream tests derive pace from a window with `UsagePace.weekly`;
//! here the engine owns that computation, so each case states the pace the
//! window yields.

mod common;

use ariadusage_macos::pace_text::{PaceContext, PaceFacts, detail, session_equivalent_detail};
use ariadusage_protocol::usage::{PaceStage, SessionEquivalentForecast};
use common::{FixedDates, FrenchDates};

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

#[test]
fn weekly_pace_detail_treats_rounded_zero_delta_as_on_pace() {
    let facts = PaceFacts {
        will_last_to_reset: true,
        ..pace(PaceStage::SlightlyBehind, -0.4, 50.4)
    };
    assert_eq!(detail(&facts, true, PaceContext::Weekly).left, "On pace");
}

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
