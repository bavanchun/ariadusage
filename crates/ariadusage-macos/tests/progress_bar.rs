// Ported from CodexBar Tests/CodexBarTests/MenuCardQuotaWarningMarkerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_macos::geometry::Size;
use ariadusage_macos::highlight_style::ColorRole;
use ariadusage_macos::progress_bar::{
    BAR_HEIGHT, BarSpec, Marker, MarkerKind, Paint, WorkdayTickAppearance, accessibility_value,
    rendered_fill_percent, resolved_markers, shapes, warning_marker_percents, warning_marker_rect,
    warning_marker_stripe_rect, work_day_marker_percents, workday_marker_rect,
};

const BAR: Size = Size::new(100.0, 6.0);

fn marker(percent: f64, kind: MarkerKind) -> Marker {
    Marker { percent, kind }
}

#[test]
fn progress_fill_matches_rounded_edge_labels() {
    assert_eq!(rendered_fill_percent(0.4), 0.0);
    assert_eq!(rendered_fill_percent(0.6), 0.6);
    assert_eq!(rendered_fill_percent(99.4), 99.4);
    assert_eq!(rendered_fill_percent(99.6), 100.0);
}

#[test]
fn quota_warning_marker_geometry_matches_pace_stripe_edges() {
    let rect = warning_marker_rect(50.0, BAR, 2.0);
    let stripe = warning_marker_stripe_rect(rect, 2.0);
    assert_eq!(rect.w, 5.0);
    assert_eq!(rect.h, 6.0);
    assert_eq!(rect.min_y(), 0.0);
    assert_eq!(rect.max_y(), 6.0);
    assert!((rect.mid_x() - 50.0).abs() <= 0.5);
    assert_eq!(stripe.w, 1.0);
    assert_eq!(stripe.h, rect.h);
    assert!((stripe.mid_x() - rect.mid_x()).abs() <= 0.001);
    assert!(stripe.min_x() > rect.min_x());
    assert!(stripe.max_x() < rect.max_x());
}

#[test]
fn quota_warning_marker_geometry_stays_centered_across_display_scales() {
    for scale in [1.0, 2.0, 3.0] {
        let rect = warning_marker_rect(33.0, BAR, scale);
        let stripe = warning_marker_stripe_rect(rect, scale);
        assert_eq!(rect.min_y(), 0.0);
        assert_eq!(rect.h, 6.0);
        assert_eq!(rect.w, 5.0);
        assert_eq!(stripe.w, 1.0);
        assert_eq!(stripe.h, rect.h);
        assert!((stripe.mid_x() - rect.mid_x()).abs() <= 1.0 / scale);
        assert!(stripe.min_x() > rect.min_x());
        assert!(stripe.max_x() < rect.max_x());
    }
}

#[test]
fn workday_boundary_is_a_subtle_lower_tick() {
    let rect = workday_marker_rect(50.0, BAR, 2.0, WorkdayTickAppearance::Subtle);
    assert_eq!(rect.w, 0.5);
    assert_eq!(rect.h, 3.0);
    assert_eq!(rect.min_y(), 3.0);
    assert!((rect.mid_x() - 50.0).abs() <= 0.5);
}

#[test]
fn high_contrast_workday_boundary_is_full_height_and_ninety_six_percent_wider() {
    for scale in [1.0, 2.0, 3.0] {
        let rect = workday_marker_rect(50.0, BAR, scale, WorkdayTickAppearance::HighContrast);
        assert_eq!(rect.w, 1.96);
        assert_eq!(rect.h, 6.0);
        assert_eq!(rect.min_y(), 0.0);
        assert!((rect.mid_x() - 50.0).abs() <= 1.0 / scale);
    }
}

#[test]
fn hidden_workday_appearance_removes_boundary_markers() {
    let markers = resolved_markers(
        &[80.0],
        &[20.0, 40.0, 60.0, 80.0],
        WorkdayTickAppearance::Hidden,
    );
    assert_eq!(markers, [marker(80.0, MarkerKind::QuotaWarning)]);
}

#[test]
fn quota_warning_wins_when_marker_kinds_overlap() {
    let markers = resolved_markers(
        &[50.0, 80.0],
        &[20.0, 50.0, 60.0],
        WorkdayTickAppearance::Subtle,
    );
    assert_eq!(
        markers,
        [
            marker(20.0, MarkerKind::WorkdayBoundary),
            marker(50.0, MarkerKind::QuotaWarning),
            marker(60.0, MarkerKind::WorkdayBoundary),
            marker(80.0, MarkerKind::QuotaWarning),
        ]
    );
}

#[test]
fn marker_resolver_removes_edges_duplicates_and_invalid_values() {
    let markers = resolved_markers(
        &[-10.0, 0.0, 50.0, 50.0, 100.0, 120.0],
        &[f64::NAN, 25.0, 25.0],
        WorkdayTickAppearance::Subtle,
    );
    assert_eq!(
        markers,
        [
            marker(25.0, MarkerKind::WorkdayBoundary),
            marker(50.0, MarkerKind::QuotaWarning),
        ]
    );
}

#[test]
fn warning_markers_follow_enabled_thresholds_and_flip_for_used_bars() {
    assert!(warning_marker_percents(&[], false).is_empty());
    assert_eq!(warning_marker_percents(&[50], false), [50.0]);
    assert_eq!(
        warning_marker_percents(&[20, 50, 50, 0], false),
        [50.0, 20.0]
    );
    assert_eq!(warning_marker_percents(&[20, 50], true), [50.0, 80.0]);
    assert_eq!(warning_marker_percents(&[150], false), [99.0]);
}

#[test]
fn work_day_marker_percents_for_5_day_week() {
    assert_eq!(
        work_day_marker_percents(Some(5), Some(10080)),
        [20.0, 40.0, 60.0, 80.0]
    );
}

#[test]
fn work_day_marker_percents_for_4_day_week() {
    assert_eq!(
        work_day_marker_percents(Some(4), Some(10080)),
        [25.0, 50.0, 75.0]
    );
}

#[test]
fn work_day_marker_percents_for_7_day_week() {
    let markers = work_day_marker_percents(Some(7), Some(10080));
    assert_eq!(markers.len(), 6);
    assert!((markers[0] - 14.2857).abs() < 0.001);
    assert!((markers[5] - 85.7143).abs() < 0.001);
}

#[test]
fn work_day_marker_percents_without_work_days_or_weekly_window_are_empty() {
    assert!(work_day_marker_percents(None, Some(10080)).is_empty());
    assert!(work_day_marker_percents(Some(5), None).is_empty());
    assert!(work_day_marker_percents(Some(5), Some(300)).is_empty());
    assert!(work_day_marker_percents(Some(5), Some(1440)).is_empty());
}

#[test]
fn work_day_marker_percents_invalid_work_days_returns_empty() {
    for days in [1, 0, 8, -1] {
        assert!(work_day_marker_percents(Some(days), Some(10080)).is_empty());
    }
}

#[test]
fn bar_draws_track_then_fill_with_capsule_corners() {
    let spec = BarSpec {
        percent: 58.0,
        pace_on_top: true,
        ..BarSpec::default()
    };
    let out = shapes(&spec, Size::new(270.0, BAR_HEIGHT), 2.0);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].paint, Paint::Track);
    assert_eq!(out[0].rect.w, 270.0);
    assert_eq!(out[0].corner_radius, 3.0);
    assert_eq!(out[1].paint, Paint::Fill);
    assert!((out[1].rect.w - 270.0 * 0.58).abs() < 1e-9);
    assert_eq!(out[1].corner_radius, 3.0);
}

#[test]
fn empty_bar_has_no_fill_and_tiny_fill_keeps_a_valid_radius() {
    let empty = shapes(&BarSpec::default(), Size::new(100.0, 6.0), 2.0);
    assert_eq!(empty.len(), 1);
    let tiny = shapes(
        &BarSpec {
            percent: 3.0,
            ..BarSpec::default()
        },
        Size::new(100.0, 6.0),
        2.0,
    );
    assert_eq!(tiny[1].rect.w, 3.0);
    assert_eq!(tiny[1].corner_radius, 1.5);
}

#[test]
fn pace_tip_punches_three_stripes_around_the_pace_position() {
    let spec = BarSpec {
        percent: 40.0,
        pace_percent: Some(50.0),
        pace_on_top: false,
        ..BarSpec::default()
    };
    let out = shapes(&spec, Size::new(200.0, 6.0), 2.0);
    let punch = out[out.len() - 2];
    let center = out[out.len() - 1];
    assert_eq!(punch.paint, Paint::Punch);
    assert_eq!(center.paint, Paint::PaceStripe { deficit: true });
    // The punch spans 6 pt centered on 100 pt and extends 12 pt past the bar.
    assert!((punch.rect.min_x() - 97.0).abs() < 1e-9);
    assert_eq!(punch.rect.w, 6.0);
    assert_eq!(punch.rect.min_y(), -12.0);
    assert_eq!(punch.rect.max_y(), 18.0);
    assert!((center.rect.min_x() - 99.0).abs() < 1e-9);
    assert_eq!(center.rect.w, 2.0);
}

#[test]
fn pace_stripe_color_is_green_on_track_red_in_deficit_white_highlighted() {
    let on_top = Paint::PaceStripe { deficit: false };
    let deficit = Paint::PaceStripe { deficit: true };
    assert_eq!(on_top.color(false).role, ColorRole::SystemGreen);
    assert_eq!(deficit.color(false).role, ColorRole::SystemRed);
    assert_eq!(deficit.color(true).role, ColorRole::White);
}

#[test]
fn track_and_fill_switch_to_selection_text_when_highlighted() {
    assert_eq!(Paint::Track.color(false).role, ColorRole::TertiaryLabel);
    assert_eq!(Paint::Track.color(false).alpha, 0.22);
    assert_eq!(
        Paint::Track.color(true).role,
        ColorRole::SelectedMenuItemText
    );
    assert_eq!(Paint::Fill.color(false).role, ColorRole::ProgressTint);
    assert_eq!(
        Paint::Fill.color(true).role,
        ColorRole::SelectedMenuItemText
    );
    assert!(Paint::Punch.is_punch());
    assert_eq!(Paint::Punch.color(false).alpha, 0.9);
}

#[test]
fn warning_marker_punches_then_draws_an_extended_stripe() {
    let spec = BarSpec {
        percent: 70.0,
        warning_marker_percents: vec![50.0],
        workday_marker_percents: vec![20.0],
        ..BarSpec::default()
    };
    let out = shapes(&spec, BAR, 2.0);
    let paints: Vec<Paint> = out.iter().map(|s| s.paint).collect();
    assert_eq!(
        paints,
        [
            Paint::Track,
            Paint::Fill,
            Paint::WorkdayTick(WorkdayTickAppearance::Subtle),
            Paint::Punch,
            Paint::WarningStripe,
        ]
    );
    assert_eq!(out[3].rect.min_y(), -12.0);
    assert_eq!(out[4].rect.w, 1.0);
}

#[test]
fn accessibility_value_lists_percent_and_markers() {
    let spec = BarSpec {
        percent: 57.6,
        warning_marker_percents: vec![50.0],
        workday_marker_percents: vec![20.0, 40.0],
        ..BarSpec::default()
    };
    assert_eq!(
        accessibility_value(&spec),
        "58 percent. Quota warnings: 50%. Work days: 20%, 40%"
    );
}
