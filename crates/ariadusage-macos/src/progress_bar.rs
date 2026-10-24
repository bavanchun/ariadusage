// Ported from CodexBar Sources/CodexBar/UsageProgressBar.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardQuotaWarningMarkers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Geometry of the menu card's usage bar.
//!
//! The bar is a 6 pt capsule: a track, the fill, quota-warning and work-day
//! markers, and an optional pace tip. Markers and the pace tip punch through
//! the track and fill with a `destinationOut` blend before drawing their
//! stripe. Everything is a rectangle in a flipped coordinate space (origin at
//! the top left), so the AppKit view draws exactly what [`shapes`] returns.

use crate::geometry::{Rect, Size};
use crate::highlight_style::{self, Color, ColorRole};

/// Height of the bar in points (`.frame(height: 6)`).
pub const BAR_HEIGHT: f64 = 6.0;

const PACE_STRIPE_COUNT: f64 = 3.0;
const PACE_STRIPE_WIDTH: f64 = 2.0;
const STRIPE_PUNCH_OPACITY: f64 = 0.9;
const WARNING_MARKER_PUNCH_WIDTH: f64 = 5.0;
const WARNING_MARKER_STRIPE_WIDTH: f64 = 1.0;
/// Minimum pace tip width, and its width relative to the bar height.
const PACE_TIP_MIN_WIDTH: f64 = 25.0;
const PACE_TIP_HEIGHT_FACTOR: f64 = 6.5;
const MARKER_DEDUP_EPSILON: f64 = 0.001;

/// How work-day boundaries show on weekly bars (`WorkdayTickAppearance` in
/// `SettingsStore.swift`; the default is `subtle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkdayTickAppearance {
    Hidden,
    #[default]
    Subtle,
    HighContrast,
}

/// What a marker stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    QuotaWarning,
    WorkdayBoundary,
}

/// A marker at a percentage of the bar width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    pub percent: f64,
    pub kind: MarkerKind,
}

/// Everything the bar needs to draw.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BarSpec {
    /// Fill percentage, 0–100.
    pub percent: f64,
    /// Where the pace tip sits, when pace is shown.
    pub pace_percent: Option<f64>,
    /// False when usage runs ahead of pace; the tip stripe turns red.
    pub pace_on_top: bool,
    pub warning_marker_percents: Vec<f64>,
    pub workday_marker_percents: Vec<f64>,
    pub workday_appearance: WorkdayTickAppearance,
}

/// What a shape is painted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    Track,
    Fill,
    /// Cleared with a `destinationOut` blend at 0.9 opacity.
    Punch,
    WarningStripe,
    WorkdayTick(WorkdayTickAppearance),
    /// The pace tip's center stripe; `deficit` when usage is ahead of pace.
    PaceStripe {
        deficit: bool,
    },
}

impl Paint {
    /// True for shapes drawn with the `destinationOut` blend mode.
    pub fn is_punch(self) -> bool {
        self == Paint::Punch
    }

    /// The color this paint uses in the normal or highlighted state.
    pub fn color(self, highlighted: bool) -> Color {
        match self {
            Paint::Track => highlight_style::progress_track(highlighted),
            Paint::Fill => highlight_style::progress_tint(highlighted),
            Paint::Punch => Color::with_alpha(ColorRole::White, STRIPE_PUNCH_OPACITY),
            Paint::WarningStripe => warning_marker_color(highlighted),
            Paint::WorkdayTick(appearance) => workday_marker_color(highlighted, appearance),
            Paint::PaceStripe { deficit } => {
                if highlighted {
                    Color::opaque(ColorRole::White)
                } else if deficit {
                    Color::opaque(ColorRole::SystemRed)
                } else {
                    Color::opaque(ColorRole::SystemGreen)
                }
            }
        }
    }
}

/// One rectangle to draw, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub rect: Rect,
    pub corner_radius: f64,
    pub paint: Paint,
}

/// The shapes for a bar of `size` on a display with `display_scale` pixels
/// per point. Callers clip drawing to the bar bounds.
pub fn shapes(spec: &BarSpec, size: Size, display_scale: f64) -> Vec<Shape> {
    let scale = display_scale.max(1.0);
    let fill_width = size.w * rendered_fill_percent(clamped_percent(spec.percent)) / 100.0;
    let pace_width = size.w * spec.pace_percent.map_or(0.0, clamped_percent) / 100.0;
    let tip_width = PACE_TIP_MIN_WIDTH.max(size.h * PACE_TIP_HEIGHT_FACTOR);
    let stripe_inset = 1.0 / scale;
    let tip_offset = pace_width - tip_width + pace_stripe_span() / 2.0 + stripe_inset;
    let show_tip = spec.pace_percent.is_some() && tip_width > 0.5;
    let corner_radius = size.h / 2.0;

    let mut out = vec![Shape {
        rect: Rect::new(0.0, 0.0, size.w, size.h),
        corner_radius: corner_radius.min(size.w / 2.0),
        paint: Paint::Track,
    }];
    if fill_width > 0.0 {
        let width = fill_width.min(size.w);
        out.push(Shape {
            rect: Rect::new(0.0, 0.0, width, size.h),
            corner_radius: corner_radius.min(width / 2.0),
            paint: Paint::Fill,
        });
    }

    let markers = resolved_markers(
        &spec.warning_marker_percents,
        &spec.workday_marker_percents,
        spec.workday_appearance,
    );
    for marker in markers {
        let x = size.w * marker.percent / 100.0;
        match marker.kind {
            MarkerKind::QuotaWarning => {
                let marker_rect = warning_marker_rect(x, size, scale);
                let stripe_rect = warning_marker_stripe_rect(marker_rect, scale);
                out.push(square(
                    extended_marker_rect(marker_rect, size),
                    Paint::Punch,
                ));
                out.push(square(
                    extended_marker_rect(stripe_rect, size),
                    Paint::WarningStripe,
                ));
            }
            MarkerKind::WorkdayBoundary => {
                let rect = workday_marker_rect(x, size, scale, spec.workday_appearance);
                out.push(square(rect, Paint::WorkdayTick(spec.workday_appearance)));
            }
        }
    }

    if show_tip
        && let Some((punched, center)) = pace_stripe_rects(Size::new(tip_width, size.h), scale)
    {
        let shift = |r: Rect| Rect::new(r.x + tip_offset, r.y, r.w, r.h);
        out.push(square(shift(punched), Paint::Punch));
        out.push(square(
            shift(center),
            Paint::PaceStripe {
                deficit: !spec.pace_on_top,
            },
        ));
    }
    out
}

fn square(rect: Rect, paint: Paint) -> Shape {
    Shape {
        rect,
        corner_radius: 0.0,
        paint,
    }
}

fn pace_stripe_span() -> f64 {
    PACE_STRIPE_WIDTH * PACE_STRIPE_COUNT.max(1.0)
}

/// The punched and center stripes of the pace tip, relative to a tip of
/// `size` whose right edge anchors the stripes.
fn pace_stripe_rects(size: Size, scale: f64) -> Option<(Rect, Rect)> {
    let align = |v: f64| (v * scale).round() / scale;
    let extend = size.h * 2.0;
    let stripe_width = PACE_STRIPE_WIDTH;
    let punch_width = stripe_width * 3.0;
    let stripe_inset = 1.0 / scale;
    let anchor_x = align(size.w - stripe_inset);
    let min_y = align(-extend);
    let max_y = align(size.h + extend);
    if anchor_x - punch_width < 0.0 {
        return None;
    }
    let punch_right = align(anchor_x);
    let punch_left = punch_right - punch_width;
    let center_left = align(punch_left + (punch_width - stripe_width) / 2.0);
    Some((
        Rect::new(punch_left, min_y, punch_width, max_y - min_y),
        Rect::new(center_left, min_y, stripe_width, max_y - min_y),
    ))
}

/// Aligns edge rendering with the rounded percent label: below 0.5 % the bar
/// is empty and from 99.5 % it is full.
pub fn rendered_fill_percent(percent: f64) -> f64 {
    let clamped = clamped_percent(percent);
    let display = display_percent(clamped);
    if display <= 0 {
        0.0
    } else if display >= 100 {
        100.0
    } else {
        clamped
    }
}

/// Warning markers and visible work-day markers sorted by position. Values
/// at or beyond the edges and duplicates are dropped, and a warning wins
/// over a work-day boundary at the same position.
pub fn resolved_markers(
    warning_percents: &[f64],
    workday_percents: &[f64],
    appearance: WorkdayTickAppearance,
) -> Vec<Marker> {
    let warnings = normalized_marker_percents(warning_percents);
    let workdays: Vec<f64> = if appearance == WorkdayTickAppearance::Hidden {
        Vec::new()
    } else {
        normalized_marker_percents(workday_percents)
            .into_iter()
            .filter(|w| {
                !warnings
                    .iter()
                    .any(|v| (v - w).abs() < MARKER_DEDUP_EPSILON)
            })
            .collect()
    };
    let mut markers: Vec<Marker> = warnings
        .into_iter()
        .map(|percent| Marker {
            percent,
            kind: MarkerKind::QuotaWarning,
        })
        .chain(workdays.into_iter().map(|percent| Marker {
            percent,
            kind: MarkerKind::WorkdayBoundary,
        }))
        .collect();
    markers.sort_by(|a, b| a.percent.total_cmp(&b.percent));
    markers
}

fn normalized_marker_percents(values: &[f64]) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    for value in values.iter().map(|v| swift_clamped_percent(*v)) {
        if value > 0.0
            && value < 100.0
            && !out.iter().any(|v| (v - value).abs() < MARKER_DEDUP_EPSILON)
        {
            out.push(value);
        }
    }
    out
}

/// The 5 pt punch of a quota-warning marker centered on `x`.
pub fn warning_marker_rect(x: f64, size: Size, raw_scale: f64) -> Rect {
    let scale = raw_scale.max(1.0);
    let align = |v: f64| (v * scale).round() / scale;
    let width = WARNING_MARKER_PUNCH_WIDTH;
    Rect::new(align(x - width / 2.0), 0.0, width, align(size.h))
}

/// The 1 pt stripe drawn inside a quota-warning punch.
pub fn warning_marker_stripe_rect(marker_rect: Rect, raw_scale: f64) -> Rect {
    let scale = raw_scale.max(1.0);
    let align = |v: f64| (v * scale).round() / scale;
    let width = marker_rect
        .w
        .min((1.0 / scale).max(WARNING_MARKER_STRIPE_WIDTH));
    Rect::new(
        align(marker_rect.mid_x() - width / 2.0),
        marker_rect.min_y(),
        width,
        marker_rect.h,
    )
}

/// A work-day boundary tick centered on `x`: a hairline on the lower half
/// of the bar, or a full-height 1.96 pt line in high contrast.
pub fn workday_marker_rect(
    x: f64,
    size: Size,
    raw_scale: f64,
    appearance: WorkdayTickAppearance,
) -> Rect {
    let scale = raw_scale.max(1.0);
    let (width, height) = match appearance {
        WorkdayTickAppearance::Hidden => return Rect::default(),
        WorkdayTickAppearance::Subtle => (1.0 / scale, (1.0 / scale).max(size.h * 0.5)),
        WorkdayTickAppearance::HighContrast => ((1.0 / scale).max(1.96), size.h),
    };
    let align = |v: f64| (v * scale).round() / scale;
    Rect::new(
        align(x - width / 2.0),
        align(size.h - height),
        width,
        align(height),
    )
}

fn extended_marker_rect(rect: Rect, size: Size) -> Rect {
    rect.outset_y(size.h * 2.0)
}

/// Quota-warning stripe color.
pub fn warning_marker_color(highlighted: bool) -> Color {
    if highlighted {
        Color::with_alpha(ColorRole::White, 0.96)
    } else {
        Color::with_alpha(ColorRole::Label, 0.68)
    }
}

/// Work-day tick color.
pub fn workday_marker_color(highlighted: bool, appearance: WorkdayTickAppearance) -> Color {
    match (appearance, highlighted) {
        (WorkdayTickAppearance::Hidden, _) => Color::with_alpha(ColorRole::White, 0.0),
        (WorkdayTickAppearance::Subtle, true) => Color::with_alpha(ColorRole::White, 0.55),
        (WorkdayTickAppearance::Subtle, false) => Color::with_alpha(ColorRole::Label, 0.30),
        (WorkdayTickAppearance::HighContrast, true) => Color::with_alpha(ColorRole::White, 0.96),
        (WorkdayTickAppearance::HighContrast, false) => Color::with_alpha(ColorRole::Label, 0.85),
    }
}

/// The accessibility value: `N percent`, then the marker positions.
pub fn accessibility_value(spec: &BarSpec) -> String {
    let mut parts = vec![format!(
        "{} percent",
        display_percent(clamped_percent(spec.percent))
    )];
    let markers = resolved_markers(
        &spec.warning_marker_percents,
        &spec.workday_marker_percents,
        spec.workday_appearance,
    );
    let list = |kind: MarkerKind| {
        markers
            .iter()
            .filter(|m| m.kind == kind)
            .map(|m| format!("{}%", m.percent.round() as i64))
            .collect::<Vec<_>>()
    };
    let warnings = list(MarkerKind::QuotaWarning);
    let workdays = list(MarkerKind::WorkdayBoundary);
    if !warnings.is_empty() {
        parts.push(format!("Quota warnings: {}", warnings.join(", ")));
    }
    if !workdays.is_empty() {
        parts.push(format!("Work days: {}", workdays.join(", ")));
    }
    parts.join(". ")
}

/// Marker percentages for active quota-warning thresholds. Thresholds are
/// remaining percentages, so they flip when the bar shows used.
pub fn warning_marker_percents(thresholds: &[i32], show_used: bool) -> Vec<f64> {
    if thresholds.is_empty() {
        return Vec::new();
    }
    crate::icon::QuotaWarningThresholds::active(Some(thresholds))
        .into_iter()
        .map(|t| {
            if show_used {
                100.0 - f64::from(t)
            } else {
                f64::from(t)
            }
        })
        .filter(|p| *p > 0.0 && *p < 100.0)
        .collect()
}

/// Work-day boundaries for a standard 7-day window, or none when the
/// feature is off, the window is not weekly, or `work_days` is not 2–7.
pub fn work_day_marker_percents(work_days: Option<i32>, window_minutes: Option<u32>) -> Vec<f64> {
    match (work_days, window_minutes) {
        (Some(days), Some(10080)) if (2..=7).contains(&days) => (1..days)
            .map(|d| f64::from(d) * 100.0 / f64::from(days))
            .collect(),
        _ => Vec::new(),
    }
}

fn display_percent(percent: f64) -> i64 {
    clamped_percent(percent).round() as i64
}

fn clamped_percent(value: f64) -> f64 {
    swift_clamped_percent(value)
}

/// `min(100, max(0, value))` with Swift's comparison semantics, which map
/// NaN to 0.
fn swift_clamped_percent(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 100.0)
    }
}
