// Ported from CodexBar Sources/CodexBar/IconRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_macos::icon::{
    Bar, IconInput, IconPalette, RectPt, RectPx, StatusLevel, StatusOverlay, fill_width_pixels,
    layout,
};

const TOP: RectPx = RectPx::new(3, 19, 30, 12);
const BOTTOM: RectPx = RectPx::new(3, 5, 30, 8);
const CREDITS: RectPx = RectPx::new(3, 14, 30, 16);
const CREDITS_BOTTOM: RectPx = RectPx::new(3, 4, 30, 6);

fn lane(rect: RectPx, percent: Option<f64>, alpha: f64) -> Bar {
    Bar {
        rect,
        percent,
        alpha,
        corner_radius_px: rect.h / 2,
    }
}

#[test]
fn fill_width_rounds_and_clamps() {
    assert_eq!(fill_width_pixels(0.0, 30), 0);
    assert_eq!(fill_width_pixels(100.0, 30), 30);
    assert_eq!(fill_width_pixels(150.0, 30), 30);
    assert_eq!(fill_width_pixels(-5.0, 30), 0);
    assert_eq!(fill_width_pixels(50.0, 30), 15);
    // 30 × 0.1% rounds to zero: a visible-zero marker draws no fill.
    assert_eq!(fill_width_pixels(0.1, 30), 0);
    // 30 × 5% = 1.5 rounds away from zero, like Swift's `.rounded()`.
    assert_eq!(fill_width_pixels(5.0, 30), 2);
}

#[test]
fn two_windows_draw_top_and_bottom_lanes() {
    let out = layout(&IconInput {
        primary: Some(60.0),
        secondary: Some(40.0),
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![lane(TOP, Some(60.0), 1.0), lane(BOTTOM, Some(40.0), 1.0)]
    );
    assert_eq!(out.palette, IconPalette::for_stale(false));
    assert_eq!(out.overlay, None);
}

#[test]
fn single_primary_window_reads_as_one_meter() {
    let out = layout(&IconInput {
        primary: Some(46.0),
        ..IconInput::default()
    });
    assert_eq!(out.bars, vec![lane(CREDITS, Some(46.0), 1.0)]);
}

#[test]
fn claude_keeps_a_dimmed_lane_for_a_missing_weekly_window() {
    let out = layout(&IconInput {
        primary: Some(46.0),
        reserves_missing_secondary_lane: true,
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![lane(TOP, Some(46.0), 1.0), lane(BOTTOM, None, 0.45)]
    );
}

#[test]
fn secondary_only_quota_uses_the_prominent_meter() {
    let out = layout(&IconInput {
        secondary: Some(70.0),
        ..IconInput::default()
    });
    assert_eq!(out.bars, vec![lane(CREDITS, Some(70.0), 1.0)]);
}

#[test]
fn credits_only_shows_credits_with_a_dimmed_lane() {
    let out = layout(&IconInput {
        credits_remaining: Some(250.0),
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![
            lane(CREDITS, Some(25.0), 1.0),
            lane(CREDITS_BOTTOM, None, 0.45)
        ]
    );
}

#[test]
fn credits_are_capped_at_one_thousand() {
    let out = layout(&IconInput {
        credits_remaining: Some(5000.0),
        ..IconInput::default()
    });
    assert_eq!(out.bars[0].percent, Some(100.0));
}

#[test]
fn exhausted_weekly_shows_credits_on_top() {
    let out = layout(&IconInput {
        primary: Some(80.0),
        secondary: Some(0.0),
        credits_remaining: Some(500.0),
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![
            lane(CREDITS, Some(50.0), 1.0),
            lane(CREDITS_BOTTOM, Some(0.0), 1.0)
        ]
    );
}

#[test]
fn exhausted_weekly_without_credits_falls_back_to_the_session_lane() {
    let out = layout(&IconInput {
        primary: Some(80.0),
        secondary: Some(0.0),
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![
            lane(TOP, Some(80.0), 1.0),
            lane(CREDITS_BOTTOM, Some(0.0), 1.0)
        ]
    );
}

#[test]
fn no_data_draws_empty_lanes() {
    let out = layout(&IconInput::default());
    assert_eq!(
        out.bars,
        vec![lane(TOP, None, 1.0), lane(BOTTOM, None, 0.45)]
    );
    assert!(out.bars.iter().all(|b| b.fill_width_px() == 0));
}

#[test]
fn stale_data_dims_every_alpha() {
    let out = layout(&IconInput {
        primary: Some(50.0),
        secondary: Some(50.0),
        stale: true,
        ..IconInput::default()
    });
    assert_eq!(
        out.palette,
        IconPalette {
            track_fill_alpha: 0.18,
            track_stroke_alpha: 0.28,
            fill_alpha: 0.55,
        }
    );
}

#[test]
fn minor_status_dot_rises_onto_a_single_meter() {
    let two_lanes = layout(&IconInput {
        primary: Some(50.0),
        secondary: Some(50.0),
        status: StatusLevel::Minor,
        ..IconInput::default()
    });
    let one_lane = layout(&IconInput {
        primary: Some(50.0),
        status: StatusLevel::Maintenance,
        ..IconInput::default()
    });
    let dot_y = |overlay| match overlay {
        Some(StatusOverlay::Dot { dot, .. }) => dot.y,
        other => panic!("expected a dot, got {other:?}"),
    };
    assert_eq!(dot_y(two_lanes.overlay), 2.0);
    assert_eq!(dot_y(one_lane.overlay), 5.0);
}

#[test]
fn major_status_draws_an_exclamation_with_a_halo() {
    let out = layout(&IconInput {
        primary: Some(50.0),
        secondary: Some(50.0),
        status: StatusLevel::Major,
        ..IconInput::default()
    });
    assert_eq!(
        out.overlay,
        Some(StatusOverlay::Exclamation {
            line: RectPt {
                x: 12.0,
                y: 4.0,
                w: 2.0,
                h: 6.0
            },
            dot: RectPt {
                x: 12.0,
                y: 2.0,
                w: 2.0,
                h: 2.0
            },
            halo: RectPt {
                x: 11.0,
                y: 1.0,
                w: 4.0,
                h: 10.0
            },
        })
    );
}

#[test]
fn stroke_rect_insets_by_half_the_stroke() {
    assert_eq!(TOP.stroke_rect(), RectPx::new(4, 20, 28, 10));
}
