// Ported from CodexBar Tests/CodexBarTests/ProviderSwitcherViewTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

use ariadusage_macos::geometry::{Point, Size};
use ariadusage_macos::switcher::{
    self, INLINE_ROW_HEIGHT, PREFERRED_OUTER_PADDING, PathCmd, QUOTA_BAR_HEIGHT,
    STACKED_ROW_HEIGHT, SwitcherIcon, SwitcherSelection, SwitcherTextMeasure,
};
use ariadusage_protocol::fixture::generate_scenario_snapshot;
use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::metric::{Confidence, Metric};
use ariadusage_protocol::snapshot::ProviderSnapshot;
use ariadusage_protocol::usage::RateWindow;

struct FixedSwitcherMeasure;

impl SwitcherTextMeasure for FixedSwitcherMeasure {
    fn measure_title(&self, text: &str, _font_size: f64) -> Size {
        Size::new(text.chars().count() as f64 * 5.0, 12.0)
    }
}

#[test]
fn shows_switcher_requires_at_least_two_enabled_providers() {
    let mut snap = generate_scenario_snapshot(0);
    assert!(switcher::shows_switcher(&snap));

    for p in snap.providers.iter_mut().skip(1) {
        p.enabled = false;
    }
    assert!(!switcher::shows_switcher(&snap));
    assert!(switcher::segments(&snap, false).is_empty());
}

#[test]
fn resolve_selection_defaults_to_first_enabled_and_preserves_valid_tabs() {
    let mut snap = generate_scenario_snapshot(0);
    assert_eq!(
        switcher::resolve_selection(&snap, None),
        Some(SwitcherSelection::Provider("claude".to_string()))
    );
    assert_eq!(
        switcher::resolve_selection(&snap, Some(&SwitcherSelection::Overview)),
        Some(SwitcherSelection::Overview)
    );
    assert_eq!(
        switcher::resolve_selection(
            &snap,
            Some(&SwitcherSelection::Provider("codex".to_string()))
        ),
        Some(SwitcherSelection::Provider("codex".to_string()))
    );
    assert_eq!(
        switcher::resolve_selection(
            &snap,
            Some(&SwitcherSelection::Provider("cursor".to_string()))
        ),
        Some(SwitcherSelection::Provider("claude".to_string()))
    );

    // When only one provider remains enabled, Overview falls back to that provider.
    snap.providers[0].enabled = false;
    snap.providers[2].enabled = false;
    assert_eq!(
        switcher::resolve_selection(&snap, Some(&SwitcherSelection::Overview)),
        Some(SwitcherSelection::Provider("codex".to_string()))
    );
}

#[test]
fn weekly_percent_prefers_secondary_and_falls_back_to_primary() {
    let mut p = ProviderSnapshot::new(ProviderId::new("claude").unwrap(), "Claude", true, "cli");
    p.windows.primary = Some(Metric::value(
        RateWindow::new(40.0),
        None,
        None,
        Confidence::Exact,
    ));
    p.windows.secondary = Some(Metric::value(
        RateWindow::new(25.0),
        None,
        None,
        Confidence::Exact,
    ));
    assert_eq!(switcher::weekly_percent(&p, false), Some(75.0));
    assert_eq!(switcher::weekly_percent(&p, true), Some(25.0));

    p.windows.secondary = None;
    assert_eq!(switcher::weekly_percent(&p, false), Some(60.0));
    assert_eq!(switcher::weekly_percent(&p, true), Some(40.0));
}

#[test]
fn four_segments_use_stacked_icons_and_non_uniform_even_widths() {
    let snap = generate_scenario_snapshot(0);
    let segs = switcher::segments(&snap, false);
    assert_eq!(segs.len(), 4);
    assert_eq!(segs[0].selection, SwitcherSelection::Overview);
    assert_eq!(segs[0].title, "Overview");
    assert_eq!(segs[0].weekly_percent, None);
    assert_eq!(segs[1].title, "Claude");
    assert_eq!(segs[1].weekly_percent, Some(36.0));
    assert_eq!(segs[2].title, "Codex");
    assert_eq!(segs[2].weekly_percent, Some(49.0));
    assert_eq!(segs[3].title, "Antigravity");
    assert_eq!(segs[3].weekly_percent, Some(72.0));

    let laid = switcher::layout(
        &segs,
        &SwitcherSelection::Overview,
        Some(2),
        310.0,
        &FixedSwitcherMeasure,
    );
    assert!(laid.stacked_icons);
    assert_eq!(laid.row_height, STACKED_ROW_HEIGHT);
    assert_eq!(laid.size, Size::new(310.0, STACKED_ROW_HEIGHT));
    assert_eq!(laid.segments.len(), 4);

    assert!(laid.segments[0].selected);
    assert!(!laid.segments[0].hovered);
    assert!(laid.segments[0].quota_bar.is_none());

    assert!(!laid.segments[2].selected);
    assert!(laid.segments[2].hovered);

    // Every button starts at or after 16 pt outer padding and ends at or before 310 - 16 = 294 pt.
    assert_eq!(laid.segments[0].frame.x, PREFERRED_OUTER_PADDING);
    assert!((laid.segments[3].frame.max_x() - (310.0 - PREFERRED_OUTER_PADDING)).abs() <= 1.0);

    for seg in &laid.segments {
        assert_eq!(seg.frame.w as i64 % 2, 0, "button width must be even");
        assert_eq!(seg.icon_frame.w, 16.0);
        assert_eq!(seg.icon_frame.h, 16.0);
        assert!(seg.title_frame.y >= seg.icon_frame.max_y());
    }

    let claude_bar = laid.segments[1].quota_bar.expect("claude quota bar");
    assert_eq!(claude_bar.track.h, QUOTA_BAR_HEIGHT);
    assert_eq!(claude_bar.track.x, laid.segments[1].frame.x + 8.0);
    assert_eq!(
        claude_bar.track.max_y(),
        laid.segments[1].frame.max_y() - 2.0
    );
    let fill = claude_bar.fill.expect("claude quota fill");
    assert_eq!(fill.w, (claude_bar.track.w * 0.36).round());
}

#[test]
fn three_segments_use_inline_icons_and_uniform_widths() {
    let mut snap = generate_scenario_snapshot(0);
    snap.providers[2].enabled = false;
    let segs = switcher::segments(&snap, false);
    assert_eq!(segs.len(), 3);

    let laid = switcher::layout(
        &segs,
        &SwitcherSelection::Provider("codex".to_string()),
        None,
        310.0,
        &FixedSwitcherMeasure,
    );
    assert!(!laid.stacked_icons);
    assert_eq!(laid.row_height, INLINE_ROW_HEIGHT);
    assert_eq!(laid.segments[0].frame.w, laid.segments[1].frame.w);
    assert_eq!(laid.segments[1].frame.w, laid.segments[2].frame.w);
    assert!(laid.segments[2].selected);
    assert!(laid.segments[0].title_frame.x > laid.segments[0].icon_frame.max_x());
}

#[test]
fn hit_test_resolves_buttons_and_ignores_padding_and_gaps() {
    let snap = generate_scenario_snapshot(0);
    let segs = switcher::segments(&snap, false);
    let laid = switcher::layout(
        &segs,
        &SwitcherSelection::Overview,
        None,
        310.0,
        &FixedSwitcherMeasure,
    );

    assert!(switcher::hit_test(&laid, Point::new(4.0, 18.0)).is_none());
    for (idx, seg) in laid.segments.iter().enumerate() {
        let center = Point::new(
            seg.frame.x + seg.frame.w / 2.0,
            seg.frame.y + seg.frame.h / 2.0,
        );
        let hit = switcher::hit_test(&laid, center).expect("hit button center");
        assert_eq!(hit.index, idx);
        assert_eq!(hit.selection, seg.selection);

        if let Some(bar) = seg.quota_bar {
            let bar_pt = Point::new(bar.track.x + bar.track.w / 2.0, bar.track.y + 1.0);
            let bar_hit = switcher::hit_test(&laid, bar_pt).expect("hit quota bar");
            assert_eq!(bar_hit.index, idx);
        }
    }
}

#[test]
fn monochrome_svg_icons_parse_into_bounded_16pt_paths() {
    for icon in [
        SwitcherIcon::Claude,
        SwitcherIcon::Codex,
        SwitcherIcon::Antigravity,
    ] {
        let cmds = switcher::icon_path(icon, 16.0);
        assert!(cmds.len() >= 4, "{icon:?} should produce path commands");
        assert!(matches!(cmds.first(), Some(PathCmd::MoveTo(_))));
        assert!(cmds.iter().any(|c| matches!(c, PathCmd::CubicTo(..))));
        for cmd in cmds {
            let pts: &[Point] = match &cmd {
                PathCmd::MoveTo(p) | PathCmd::LineTo(p) => std::slice::from_ref(p),
                PathCmd::CubicTo(c1, c2, p) => &[*c1, *c2, *p],
                PathCmd::Close => &[],
            };
            for pt in pts {
                assert!(
                    pt.x >= -1.0 && pt.x <= 17.0 && pt.y >= -1.0 && pt.y <= 17.0,
                    "{icon:?} point out of 16x16 bounds: {pt:?}"
                );
            }
        }
    }
}
