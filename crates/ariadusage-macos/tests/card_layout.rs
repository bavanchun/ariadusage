mod common;

use ariadusage_macos::card::{
    Accent, CardModel, CreditsSection, MetricRow, PercentStyle, Subtitle, SubtitleStyle,
};
use ariadusage_macos::card_layout::{
    Align, CardLayout, Item, MIN_CARD_WIDTH, TextColor, Truncation, layout, overview_layout,
    rendered_width,
};
use ariadusage_macos::geometry::Rect;
use ariadusage_macos::progress_bar::WorkdayTickAppearance;
use common::FixedMeasure;

fn model(subtitle: &str, style: SubtitleStyle) -> CardModel {
    CardModel {
        provider_name: "Claude".to_string(),
        email: String::new(),
        subtitle: Subtitle {
            text: subtitle.to_string(),
            style,
        },
        plan: None,
        metrics: Vec::new(),
        placeholder: None,
        credits: None,
        accent: Accent::System,
    }
}

fn row(title: &str, percent: f64, reset: Option<&str>) -> MetricRow {
    MetricRow {
        id: "primary".to_string(),
        title: title.to_string(),
        percent,
        percent_style: PercentStyle::Left,
        status_text: None,
        reset_text: reset.map(str::to_string),
        detail_text: None,
        detail_left_text: None,
        detail_right_text: None,
        pace_percent: None,
        detail_is_pace_derived: false,
        pace_on_top: true,
        warning_marker_percents: Vec::new(),
        workday_marker_percents: Vec::new(),
        workday_tick_appearance: WorkdayTickAppearance::Subtle,
    }
}

fn frame_of(layout: &CardLayout, text: &str) -> Rect {
    layout
        .items
        .iter()
        .find_map(|p| match &p.item {
            Item::Text(t) if t.text == text => Some(p.frame),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no label {text:?}"))
}

fn frames_of_kind(layout: &CardLayout, divider: bool) -> Vec<Rect> {
    layout
        .items
        .iter()
        .filter(|p| match p.item {
            Item::Divider => divider,
            Item::Bar { .. } => !divider,
            Item::Text(_) => false,
        })
        .map(|p| p.frame)
        .collect()
}

#[test]
fn card_width_never_drops_below_the_menu_base_width() {
    assert_eq!(rendered_width(0.0), MIN_CARD_WIDTH);
    assert_eq!(rendered_width(310.0), 310.0);
    assert_eq!(rendered_width(400.2), 401.0);
}

#[test]
fn header_only_card_uses_six_point_padding() {
    let m = model("Refreshing…", SubtitleStyle::Loading);
    let l = layout(&m, 310.0, &FixedMeasure);
    assert_eq!(frame_of(&l, "Claude"), Rect::new(20.0, 6.0, 36.0, 16.0));
    assert_eq!(
        frame_of(&l, "Refreshing…"),
        Rect::new(20.0, 26.0, 66.0, 13.0)
    );
    assert_eq!(l.size.h, 45.0);
    assert!(frames_of_kind(&l, true).is_empty());
}

#[test]
fn email_and_plan_sit_on_the_right_aligned_by_baseline() {
    let mut m = model("Updated 5 min ago", SubtitleStyle::Info);
    m.email = "someone at example".to_string();
    m.plan = Some("Max".to_string());
    let l = layout(&m, 310.0, &FixedMeasure);
    // 18 characters at 6 pt, right edge at 310 - 20.
    assert_eq!(
        frame_of(&l, "someone at example"),
        Rect::new(182.0, 8.0, 108.0, 14.0)
    );
    assert_eq!(frame_of(&l, "Max"), Rect::new(272.0, 26.0, 18.0, 13.0));
    let email = l
        .items
        .iter()
        .find_map(|p| match &p.item {
            Item::Text(t) if t.text == "someone at example" => Some(t.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(email.truncation, Truncation::Middle);
    assert_eq!(email.align, Align::Right);
    assert_eq!(email.color, TextColor::Secondary);
}

#[test]
fn metric_row_follows_the_divider_and_section_paddings() {
    let mut m = model("Updated 5 min ago", SubtitleStyle::Info);
    m.metrics = vec![row("Session", 58.0, Some("Resets in 2h 13m"))];
    let l = layout(&m, 310.0, &FixedMeasure);
    // Header ends at 39; 6 pt to the divider; 6 + 10 pt after it.
    assert_eq!(
        frames_of_kind(&l, true),
        [Rect::new(20.0, 45.0, 270.0, 1.0)]
    );
    assert_eq!(
        frame_of(&l, "Session 58% left"),
        Rect::new(20.0, 62.0, 96.0, 16.0)
    );
    // Footnote baseline 10 aligns with body baseline 13.
    assert_eq!(
        frame_of(&l, "Resets in 2h 13m"),
        Rect::new(194.0, 65.0, 96.0, 13.0)
    );
    assert_eq!(
        frames_of_kind(&l, false),
        [Rect::new(20.0, 84.0, 270.0, 6.0)]
    );
    assert_eq!(l.size.h, 96.0);
}

#[test]
fn reset_text_moves_below_the_title_when_both_do_not_fit() {
    let mut m = model("Updated 5 min ago", SubtitleStyle::Info);
    m.metrics = vec![row("Session", 58.0, Some("Resets in 2h 13m"))];
    let l = layout(&m, 200.0, &FixedMeasure);
    let title = frame_of(&l, "Session 58% left");
    let reset = frame_of(&l, "Resets in 2h 13m");
    assert_eq!(title.y, 62.0);
    assert_eq!(reset.y, 62.0 + 16.0 + 2.0);
    assert_eq!(reset.max_x(), 180.0);
    assert_eq!(frames_of_kind(&l, false)[0].y, reset.max_y() + 6.0);
}

#[test]
fn rows_are_twelve_points_apart_and_meta_lines_follow_the_bar() {
    let mut first = row("Session", 58.0, None);
    first.detail_left_text = Some("7% in deficit".to_string());
    first.detail_right_text = Some("Runs out in 3d".to_string());
    let mut m = model("Updated", SubtitleStyle::Info);
    m.metrics = vec![first, row("Weekly", 30.0, None)];
    let l = layout(&m, 310.0, &FixedMeasure);
    let bars = frames_of_kind(&l, false);
    let meta = frame_of(&l, "7% in deficit · Runs out in 3d");
    assert_eq!(meta.y, bars[0].max_y() + 6.0);
    assert_eq!(frame_of(&l, "Weekly 30% left").y, meta.max_y() + 12.0);
}

#[test]
fn status_rows_show_title_and_status_without_a_bar() {
    let mut status = row("Daily routines", 0.0, None);
    status.status_text = Some("Unavailable".to_string());
    let mut m = model("Updated", SubtitleStyle::Info);
    m.metrics = vec![status];
    let l = layout(&m, 310.0, &FixedMeasure);
    let title = frame_of(&l, "Daily routines");
    assert_eq!(frame_of(&l, "Unavailable").y, title.max_y() + 6.0);
    assert!(frames_of_kind(&l, false).is_empty());
}

#[test]
fn credits_follow_a_divider_after_usage_rows() {
    let mut m = model("Updated", SubtitleStyle::Info);
    m.metrics = vec![row("Session", 58.0, None)];
    m.credits = Some(CreditsSection {
        text: "42.5 left".to_string(),
        show_progress: true,
        progress_percent: Some(42.5),
        scale_text: Some("100 tokens".to_string()),
    });
    let l = layout(&m, 310.0, &FixedMeasure);
    let bars = frames_of_kind(&l, false);
    let dividers = frames_of_kind(&l, true);
    assert_eq!(dividers.len(), 2);
    assert_eq!(dividers[1].y, bars[0].max_y() + 12.0);
    let credits_title = frame_of(&l, "Credits");
    assert_eq!(credits_title.y, dividers[1].max_y() + 12.0);
    assert_eq!(bars[1].y, credits_title.max_y() + 6.0);
    let value = frame_of(&l, "42.5 left");
    let scale = frame_of(&l, "100 tokens");
    assert_eq!(value.y, bars[1].max_y() + 6.0);
    assert_eq!(scale.y, value.y);
    assert_eq!(scale.max_x(), 290.0);
    let accessibility: Vec<&str> = l
        .items
        .iter()
        .filter_map(|p| match &p.item {
            Item::Bar {
                accessibility_label,
                ..
            } => Some(accessibility_label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(accessibility, ["Usage remaining", "Credits remaining"]);
}

#[test]
fn credits_only_card_has_no_extra_divider() {
    let mut m = model("Updated", SubtitleStyle::Info);
    m.credits = Some(CreditsSection {
        text: "Credits unavailable".to_string(),
        show_progress: true,
        progress_percent: None,
        scale_text: None,
    });
    let l = layout(&m, 310.0, &FixedMeasure);
    let dividers = frames_of_kind(&l, true);
    assert_eq!(dividers.len(), 1);
    // Without usage rows the divider keeps a 6 pt gap.
    assert_eq!(frame_of(&l, "Credits").y, dividers[0].max_y() + 6.0);
    assert_eq!(
        frame_of(&l, "Credits unavailable").y,
        frame_of(&l, "Credits").max_y() + 6.0
    );
}

#[test]
fn error_subtitle_wraps_to_three_lines_and_reserves_four_points() {
    let long = "x".repeat(200);
    let m = model(&long, SubtitleStyle::Error);
    let l = layout(&m, 310.0, &FixedMeasure);
    let subtitle = frame_of(&l, &long);
    assert_eq!(subtitle.h, 3.0 * 13.0);
    assert_eq!(l.size.h, 26.0 + 39.0 + 4.0 + 6.0);
    let color = l
        .items
        .iter()
        .find_map(|p| match &p.item {
            Item::Text(t) if t.text == long => Some(t.color),
            _ => None,
        })
        .unwrap();
    assert_eq!(color, TextColor::Error);
}

#[test]
fn placeholder_shows_below_the_divider() {
    let mut m = model("Not fetched yet", SubtitleStyle::Info);
    m.placeholder = Some("No usage yet".to_string());
    let l = layout(&m, 310.0, &FixedMeasure);
    let divider = frames_of_kind(&l, true)[0];
    let placeholder = frame_of(&l, "No usage yet");
    assert_eq!(placeholder.y, divider.max_y() + 6.0);
    assert_eq!(l.size.h, placeholder.max_y() + 6.0);
}

#[test]
fn overview_layout_omits_dividers_and_credits() {
    let mut m = model("Updated 5 min ago", SubtitleStyle::Info);
    m.metrics = vec![row("Session", 58.0, Some("Resets in 2h 13m"))];
    m.credits = Some(CreditsSection {
        text: "42.5 left".to_string(),
        show_progress: true,
        progress_percent: Some(42.5),
        scale_text: Some("100 tokens".to_string()),
    });
    let l = overview_layout(&m, 310.0, &FixedMeasure);
    assert!(
        frames_of_kind(&l, true).is_empty(),
        "overview card has no internal dividers"
    );
    assert_eq!(
        frames_of_kind(&l, false).len(),
        1,
        "overview card omits credits bar"
    );
    assert_eq!(frame_of(&l, "Session 58% left").y, 39.0 + 6.0 + 10.0);
}
