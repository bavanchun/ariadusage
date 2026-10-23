// Ported from CodexBar Sources/CodexBar/StatusItemController+SwitcherViews.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/ProviderSwitcherButtons.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+SwitcherMetrics.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+UserPlugins.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Provider switcher row model, layout, hit testing and monochrome SVG icon paths.
//!
//! All geometry lives in flipped coordinates (`y` grows downwards) so unit tests
//! can verify button frames, quota bars, selection state and hit testing without
//! an AppKit window.

use std::f64::consts::{PI, TAU};

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::{EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::RateWindow;

use crate::card::{self, Accent};
use crate::geometry::{Point, Rect, Size};

const CLAUDE_MONO_SVG: &str = include_str!("../../../brand/svg/providers/claude-mono.svg");
const CODEX_MONO_SVG: &str = include_str!("../../../brand/svg/providers/codex-mono.svg");
const ANTIGRAVITY_MONO_SVG: &str =
    include_str!("../../../brand/svg/providers/antigravity-mono.svg");

/// Preferred horizontal inset matching the card's grid.
pub const PREFERRED_OUTER_PADDING: f64 = 16.0;
pub const REDUCED_OUTER_PADDING: f64 = 10.0;
pub const MINIMAL_OUTER_PADDING: f64 = 6.0;
pub const MINIMUM_GAP: f64 = 1.0;

/// Button heights (`ProviderSwitcherView.switcherButtonHeight`).
pub const INLINE_ROW_HEIGHT: f64 = 30.0;
pub const STACKED_ROW_HEIGHT: f64 = 36.0;
pub const STACKED_DENSE_ROW_HEIGHT: f64 = 39.0;

/// Icon and label metrics (`ProviderSwitcherButtons.swift`).
pub const ICON_SIZE: f64 = 16.0;
pub const INLINE_FONT_SIZE: f64 = 11.0;
pub const STACKED_FONT_SIZE: f64 = 9.0;
pub const INLINE_BUTTON_LEFT_PADDING: f64 = 7.0;
pub const INLINE_BUTTON_RIGHT_PADDING: f64 = 7.0;
pub const INLINE_ICON_TITLE_SPACING: f64 = 1.0;
pub const STACKED_BUTTON_SIDE_PADDING: f64 = 4.0;
pub const STACKED_BUTTON_TOP_PADDING: f64 = 2.0;
pub const BUTTON_CORNER_RADIUS: f64 = 6.0;

/// Weekly quota indicator bar metrics (`ProviderSwitcherView.quotaIndicator*`).
pub const QUOTA_BAR_HEIGHT: f64 = 2.0;
pub const QUOTA_BAR_BOTTOM_INSET: f64 = 2.0;
pub const QUOTA_BAR_HORIZONTAL_INSET: f64 = 8.0;
pub const QUOTA_BAR_CORNER_RADIUS: f64 = 1.0;
pub const QUOTA_TRACK_ALPHA: f64 = 0.22;

/// SVG viewBox side length in `brand/svg/providers/*-mono.svg`.
const SVG_VIEWBOX_SIZE: f64 = 24.0;

/// Which tab is selected in the merged menu switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitcherSelection {
    Overview,
    Provider(String),
}

impl SwitcherSelection {
    pub fn is_overview(&self) -> bool {
        matches!(self, Self::Overview)
    }

    pub fn provider_id(&self) -> Option<&str> {
        match self {
            Self::Overview => None,
            Self::Provider(id) => Some(id.as_str()),
        }
    }
}

/// Which 16 × 16 pt template icon a switcher segment draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitcherIcon {
    /// SF Symbol `"square.grid.2x2"` for the Overview tab.
    OverviewGrid,
    /// `brand/svg/providers/claude-mono.svg`.
    Claude,
    /// `brand/svg/providers/codex-mono.svg`.
    Codex,
    /// `brand/svg/providers/antigravity-mono.svg`.
    Antigravity,
    /// Fallback rounded-square badge for unknown providers.
    Fallback,
}

impl SwitcherIcon {
    pub fn for_provider(provider_id: &str) -> Self {
        match provider_id {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "antigravity" => Self::Antigravity,
            _ => Self::Fallback,
        }
    }
}

/// One segment in the provider switcher before layout.
#[derive(Debug, Clone, PartialEq)]
pub struct SwitcherSegment {
    pub selection: SwitcherSelection,
    pub title: String,
    pub icon: SwitcherIcon,
    pub accent: Accent,
    pub weekly_percent: Option<f64>,
}

/// The weekly quota mini-bar at the bottom of a provider switcher button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuotaBarLayout {
    pub track: Rect,
    pub fill: Option<Rect>,
    pub ratio: f64,
}

/// One laid-out toggle button in the switcher row.
#[derive(Debug, Clone, PartialEq)]
pub struct SwitcherSegmentLayout {
    pub index: usize,
    pub selection: SwitcherSelection,
    pub title: String,
    pub icon: SwitcherIcon,
    pub accent: Accent,
    pub selected: bool,
    pub hovered: bool,
    pub frame: Rect,
    pub icon_frame: Rect,
    pub title_frame: Rect,
    pub font_size: f64,
    pub quota_bar: Option<QuotaBarLayout>,
}

/// The complete laid-out switcher view.
#[derive(Debug, Clone, PartialEq)]
pub struct SwitcherLayout {
    pub size: Size,
    pub stacked_icons: bool,
    pub row_height: f64,
    pub segments: Vec<SwitcherSegmentLayout>,
}

/// Measures single-line switcher button titles.
pub trait SwitcherTextMeasure {
    fn measure_title(&self, text: &str, font_size: f64) -> Size;
}

/// Whether the snapshot has more than one enabled provider, which enables both
/// the top switcher row and the `Overview` tab (`includesOverviewTab`).
pub fn shows_switcher(snapshot: &EngineSnapshot) -> bool {
    snapshot.providers.iter().filter(|p| p.enabled).count() > 1
}

/// Resolves the active switcher tab against `snapshot`'s enabled providers
/// (`StatusItemController.resolvedSwitcherSelection`).
///
/// When `current` is `None` (initial launch, matching `mergedMenuLastSelectedWasOverview == false`
/// and `selectedMenuProvider == nil`), or when the previously selected provider was disabled,
/// falls back to the first enabled provider.
pub fn resolve_selection(
    snapshot: &EngineSnapshot,
    current: Option<&SwitcherSelection>,
) -> Option<SwitcherSelection> {
    let enabled: Vec<&ProviderSnapshot> = snapshot.providers.iter().filter(|p| p.enabled).collect();
    let first = enabled.first()?;
    let has_overview = enabled.len() > 1;
    match current {
        Some(SwitcherSelection::Overview) if has_overview => Some(SwitcherSelection::Overview),
        Some(SwitcherSelection::Provider(id))
            if enabled.iter().any(|p| p.id.as_str() == id.as_str()) =>
        {
            Some(SwitcherSelection::Provider(id.clone()))
        }
        _ => Some(SwitcherSelection::Provider(first.id.as_str().to_string())),
    }
}

/// Resolves the quota percentage shown on a provider's switcher button
/// (`StatusItemController.switcherWeeklyMetricPercent`).
///
/// Prefers the secondary (weekly) rate window and falls back to the primary
/// (session) rate window when weekly is absent.
pub fn weekly_percent(provider: &ProviderSnapshot, show_used: bool) -> Option<f64> {
    let window = displayable_window(provider.windows.secondary.as_ref())
        .or_else(|| displayable_window(provider.windows.primary.as_ref()))?;
    let used = window.used_percent.clamp(0.0, 100.0);
    Some(if show_used {
        used
    } else {
        (100.0 - used).max(0.0)
    })
}

fn displayable_window(metric: Option<&Metric<RateWindow>>) -> Option<&RateWindow> {
    let metric = metric?;
    match metric.state {
        MetricState::Value | MetricState::Stale => metric.value.as_ref(),
        _ => None,
    }
}

/// Builds the switcher segments for `snapshot` (`Overview` + enabled providers in snapshot order).
/// Returns an empty list when fewer than 2 providers are enabled.
pub fn segments(snapshot: &EngineSnapshot, show_used: bool) -> Vec<SwitcherSegment> {
    let enabled: Vec<&ProviderSnapshot> = snapshot.providers.iter().filter(|p| p.enabled).collect();
    if enabled.len() <= 1 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(enabled.len() + 1);
    out.push(SwitcherSegment {
        selection: SwitcherSelection::Overview,
        title: "Overview".to_string(),
        icon: SwitcherIcon::OverviewGrid,
        accent: Accent::System,
        weekly_percent: None,
    });
    for provider in enabled {
        out.push(SwitcherSegment {
            selection: SwitcherSelection::Provider(provider.id.as_str().to_string()),
            title: provider.display_name.clone(),
            icon: SwitcherIcon::for_provider(provider.id.as_str()),
            accent: card::card_presentation(provider).accent,
            weekly_percent: weekly_percent(provider, show_used),
        });
    }
    out
}

/// Lays out `segments` across `width` points (`ProviderSwitcherView`).
pub fn layout(
    segments: &[SwitcherSegment],
    selected: &SwitcherSelection,
    hovered_index: Option<usize>,
    width: f64,
    measure: &dyn SwitcherTextMeasure,
) -> SwitcherLayout {
    if segments.is_empty() {
        return SwitcherLayout {
            size: Size::new(width, 0.0),
            stacked_icons: false,
            row_height: 0.0,
            segments: Vec::new(),
        };
    }

    let stacked_icons = segments.len() > 3;
    let font_size = if stacked_icons {
        STACKED_FONT_SIZE
    } else {
        INLINE_FONT_SIZE
    };
    let row_height = if stacked_icons {
        STACKED_ROW_HEIGHT
    } else {
        INLINE_ROW_HEIGHT
    };

    let title_sizes: Vec<Size> = segments
        .iter()
        .map(|seg| measure.measure_title(&seg.title, font_size))
        .collect();

    let desired_widths: Vec<f64> = title_sizes
        .iter()
        .map(|ts| {
            if stacked_icons {
                let content_w = ts.w.ceil().max(ICON_SIZE);
                (content_w + 2.0 * STACKED_BUTTON_SIDE_PADDING).ceil()
            } else {
                (INLINE_BUTTON_LEFT_PADDING
                    + ICON_SIZE
                    + INLINE_ICON_TITLE_SPACING
                    + ts.w.ceil()
                    + INLINE_BUTTON_RIGHT_PADDING)
                    .ceil()
            }
        })
        .collect();

    let required_uniform =
        (!stacked_icons).then(|| desired_widths.iter().copied().fold(0.0_f64, f64::max));
    let outer_padding =
        switcher_outer_padding(width, segments.len(), MINIMUM_GAP, required_uniform);

    let button_frames = if stacked_icons {
        let widths = non_uniform_segment_widths(width, outer_padding, MINIMUM_GAP, &desired_widths);
        let available = (width - 2.0 * outer_padding).max(0.0);
        let gaps = (segments.len().saturating_sub(1)).max(1) as f64;
        let sum_w: f64 = widths.iter().sum();
        let computed_gap = ((available - sum_w) / gaps).max(MINIMUM_GAP);
        let mut x_offset = 0.0;
        widths
            .into_iter()
            .map(|w| {
                let x = (outer_padding + x_offset).round();
                x_offset += w + computed_gap;
                Rect::new(x, 0.0, w, row_height)
            })
            .collect::<Vec<_>>()
    } else {
        let max_allowed =
            max_allowed_uniform_width(width, segments.len(), outer_padding, MINIMUM_GAP);
        let max_desired = desired_widths.iter().copied().fold(0.0_f64, f64::max);
        let even_desired = even_ceil(max_desired);
        let even_allowed = even_floor(max_allowed);
        let uniform_w = if even_allowed > 0.0 {
            even_desired.min(even_allowed)
        } else {
            even_desired
        };
        inline_button_frames(width, outer_padding, row_height, uniform_w, segments.len())
    };

    let mut laid_out = Vec::with_capacity(segments.len());
    for (index, ((seg, frame), title_size)) in segments
        .iter()
        .zip(button_frames)
        .zip(title_sizes)
        .enumerate()
    {
        let (icon_frame, title_frame) = if stacked_icons {
            let title_h = title_size.h.ceil();
            let stack_h = ICON_SIZE + title_h;
            let icon_y = frame.y
                + ((frame.h - stack_h) / 2.0)
                    .floor()
                    .max(STACKED_BUTTON_TOP_PADDING);
            let icon_x = frame.x + ((frame.w - ICON_SIZE) / 2.0).round();
            let icon_rect = Rect::new(icon_x, icon_y, ICON_SIZE, ICON_SIZE);
            let title_w = (frame.w - 2.0 * STACKED_BUTTON_SIDE_PADDING).max(0.0);
            let title_rect = Rect::new(
                frame.x + STACKED_BUTTON_SIDE_PADDING,
                icon_rect.max_y(),
                title_w,
                title_h,
            );
            (icon_rect, title_rect)
        } else {
            let title_h = title_size.h.ceil();
            let max_title_w = (frame.w
                - INLINE_BUTTON_LEFT_PADDING
                - INLINE_BUTTON_RIGHT_PADDING
                - ICON_SIZE
                - INLINE_ICON_TITLE_SPACING)
                .max(0.0);
            let title_w = title_size.w.ceil().min(max_title_w);
            let stack_w = ICON_SIZE + INLINE_ICON_TITLE_SPACING + title_w;
            let start_x = frame.x
                + ((frame.w - stack_w) / 2.0)
                    .round()
                    .max(INLINE_BUTTON_LEFT_PADDING);
            let icon_y = frame.y + ((frame.h - ICON_SIZE) / 2.0).round();
            let icon_rect = Rect::new(start_x, icon_y, ICON_SIZE, ICON_SIZE);
            let title_y = frame.y + ((frame.h - title_h) / 2.0).round();
            let title_rect = Rect::new(
                icon_rect.max_x() + INLINE_ICON_TITLE_SPACING,
                title_y,
                title_w,
                title_h,
            );
            (icon_rect, title_rect)
        };

        let quota_bar = seg.weekly_percent.map(|percent| {
            let ratio = (percent / 100.0).clamp(0.0, 1.0);
            let track = Rect::new(
                frame.x + QUOTA_BAR_HORIZONTAL_INSET,
                frame.max_y() - QUOTA_BAR_BOTTOM_INSET - QUOTA_BAR_HEIGHT,
                (frame.w - 2.0 * QUOTA_BAR_HORIZONTAL_INSET).max(0.0),
                QUOTA_BAR_HEIGHT,
            );
            let fill_w = (track.w * ratio).round();
            let fill = (ratio > 0.0 && fill_w > 0.0)
                .then(|| Rect::new(track.x, track.y, fill_w, QUOTA_BAR_HEIGHT));
            QuotaBarLayout { track, fill, ratio }
        });

        laid_out.push(SwitcherSegmentLayout {
            index,
            selection: seg.selection.clone(),
            title: seg.title.clone(),
            icon: seg.icon,
            accent: seg.accent,
            selected: seg.selection == *selected,
            hovered: hovered_index == Some(index),
            frame,
            icon_frame,
            title_frame,
            font_size,
            quota_bar,
        });
    }

    SwitcherLayout {
        size: Size::new(width, row_height),
        stacked_icons,
        row_height,
        segments: laid_out,
    }
}

/// Returns the segment containing `point`, if any (`ProviderSwitcherView.button(at:)`).
pub fn hit_test(layout: &SwitcherLayout, point: Point) -> Option<&SwitcherSegmentLayout> {
    layout.segments.iter().find(|seg| seg.frame.contains(point))
}

fn even_floor(value: f64) -> f64 {
    let mut v = value.floor();
    if (v as i64) % 2 != 0 {
        v -= 1.0;
    }
    v
}

fn even_ceil(value: f64) -> f64 {
    let mut v = value.ceil();
    if (v as i64) % 2 != 0 {
        v += 1.0;
    }
    v
}

fn max_allowed_uniform_width(
    total_width: f64,
    count: usize,
    outer_padding: f64,
    minimum_gap: f64,
) -> f64 {
    if count == 0 {
        return 0.0;
    }
    let available =
        total_width - outer_padding * 2.0 - minimum_gap * (count.saturating_sub(1) as f64);
    if available <= 0.0 {
        return 0.0;
    }
    (available / count as f64).floor()
}

fn switcher_outer_padding(
    width: f64,
    count: usize,
    minimum_gap: f64,
    required_segment_width: Option<f64>,
) -> f64 {
    let minimum_comfortable_average = if count >= 5 { 50.0 } else { 54.0 };
    let fits = |outer_padding: f64| -> bool {
        if let Some(required) = required_segment_width {
            let allowed = max_allowed_uniform_width(width, count, outer_padding, minimum_gap);
            even_floor(allowed) >= even_ceil(required)
        } else {
            let available =
                width - outer_padding * 2.0 - minimum_gap * (count.saturating_sub(1) as f64);
            count > 0 && (available / count as f64) >= minimum_comfortable_average
        }
    };
    if fits(PREFERRED_OUTER_PADDING) {
        PREFERRED_OUTER_PADDING
    } else if fits(REDUCED_OUTER_PADDING) {
        REDUCED_OUTER_PADDING
    } else {
        MINIMAL_OUTER_PADDING
    }
}

fn inline_button_frames(
    width: f64,
    outer_padding: f64,
    row_height: f64,
    uniform_w: f64,
    count: usize,
) -> Vec<Rect> {
    match count {
        0 => Vec::new(),
        1 => vec![Rect::new(
            ((width - uniform_w) / 2.0).round(),
            0.0,
            uniform_w,
            row_height,
        )],
        2 => vec![
            Rect::new(outer_padding, 0.0, uniform_w, row_height),
            Rect::new(
                (width - outer_padding - uniform_w).round(),
                0.0,
                uniform_w,
                row_height,
            ),
        ],
        3 => vec![
            Rect::new(outer_padding, 0.0, uniform_w, row_height),
            Rect::new(
                ((width - uniform_w) / 2.0).round(),
                0.0,
                uniform_w,
                row_height,
            ),
            Rect::new(
                (width - outer_padding - uniform_w).round(),
                0.0,
                uniform_w,
                row_height,
            ),
        ],
        _ => {
            let available = (width - 2.0 * outer_padding).max(0.0);
            let gap =
                ((available - uniform_w * count as f64) / (count - 1) as f64).max(MINIMUM_GAP);
            (0..count)
                .map(|i| {
                    let x = (outer_padding + i as f64 * (uniform_w + gap)).round();
                    Rect::new(x, 0.0, uniform_w, row_height)
                })
                .collect()
        }
    }
}

/// Ported from `ProviderSwitcherView.applyNonUniformSegmentWidths`.
fn non_uniform_segment_widths(
    total_width: f64,
    outer_padding: f64,
    minimum_gap: f64,
    desired: &[f64],
) -> Vec<f64> {
    let count = desired.len();
    if count == 0 {
        return Vec::new();
    }
    let available =
        total_width - outer_padding * 2.0 - minimum_gap * (count.saturating_sub(1) as f64);
    if available <= 0.0 {
        return vec![0.0; count];
    }

    let desired_sum: f64 = desired.iter().sum();
    let avg = (available / count as f64).floor();
    let min_width = avg.clamp(24.0, 40.0);

    let mut widths: Vec<f64> = if desired_sum <= available {
        desired.to_vec()
    } else {
        let total_capacity = (desired_sum - min_width * count as f64).max(0.0);
        if total_capacity <= 0.0 {
            vec![available / count as f64; count]
        } else {
            let overflow = desired_sum - available;
            desired
                .iter()
                .map(|&d| {
                    let capacity = (d - min_width).max(0.0);
                    let shrink = overflow * (capacity / total_capacity);
                    d - shrink
                })
                .collect()
        }
    };

    for w in &mut widths {
        *w = even_floor(*w).max(min_width);
    }
    let mut used: f64 = widths.iter().sum();

    while available - used >= 2.0 {
        let best_deficit = (0..count)
            .filter(|&i| desired[i] - widths[i] >= 2.0)
            .max_by(|&a, &b| {
                (desired[a] - widths[a])
                    .partial_cmp(&(desired[b] - widths[b]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some(best) = best_deficit {
            widths[best] += 2.0;
            used += 2.0;
            continue;
        }
        let Some(best_smallest) = (0..count).min_by(|&a, &b| {
            widths[a]
                .partial_cmp(&widths[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            break;
        };
        widths[best_smallest] += 2.0;
        used += 2.0;
    }

    widths
}

/// A cubic-normalized path command in flipped points (`0..=size_pt`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    MoveTo(Point),
    LineTo(Point),
    CubicTo(Point, Point, Point),
    Close,
}

/// Parses the embedded monochrome SVG for `icon` into cubic-normalized path
/// commands scaled to `size_pt × size_pt` (`16 × 16` pt).
pub fn icon_path(icon: SwitcherIcon, size_pt: f64) -> Vec<PathCmd> {
    let svg = match icon {
        SwitcherIcon::Claude => CLAUDE_MONO_SVG,
        SwitcherIcon::Codex => CODEX_MONO_SVG,
        SwitcherIcon::Antigravity => ANTIGRAVITY_MONO_SVG,
        SwitcherIcon::OverviewGrid | SwitcherIcon::Fallback => return Vec::new(),
    };
    let Some(d) = extract_svg_path_d(svg) else {
        return Vec::new();
    };
    parse_svg_path(d, size_pt / SVG_VIEWBOX_SIZE)
}

fn extract_svg_path_d(svg: &str) -> Option<&str> {
    let path_tag = svg.find("<path ")?;
    let rest = &svg[path_tag..];
    let d_start = rest.find("d=\"")? + 3;
    let after_d = &rest[d_start..];
    let d_end = after_d.find('"')?;
    Some(&after_d[..d_end])
}

/// Parses an SVG `<path d="...">` string into [`PathCmd`]s scaled by `scale`.
fn parse_svg_path(d: &str, scale: f64) -> Vec<PathCmd> {
    let mut cmds = Vec::new();
    let mut tokens = PathTokenizer::new(d);
    let mut cur = Point::new(0.0, 0.0);
    let mut subpath_start = Point::new(0.0, 0.0);
    let mut last_cubic_ctrl: Option<Point> = None;
    let mut last_quad_ctrl: Option<Point> = None;
    let mut current_op = '\0';

    let sp = |p: Point| Point::new(p.x * scale, p.y * scale);

    while let Some(op) = tokens.next_op(current_op) {
        current_op = op;
        match op {
            'M' | 'm' => {
                let Some(mut p) = tokens.next_point() else {
                    break;
                };
                if op == 'm' {
                    p = Point::new(cur.x + p.x, cur.y + p.y);
                }
                cur = p;
                subpath_start = p;
                cmds.push(PathCmd::MoveTo(sp(cur)));
                current_op = if op == 'M' { 'L' } else { 'l' };
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'L' | 'l' => {
                let Some(mut p) = tokens.next_point() else {
                    break;
                };
                if op == 'l' {
                    p = Point::new(cur.x + p.x, cur.y + p.y);
                }
                cur = p;
                cmds.push(PathCmd::LineTo(sp(cur)));
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'H' | 'h' => {
                let Some(mut x) = tokens.next_number() else {
                    break;
                };
                if op == 'h' {
                    x += cur.x;
                }
                cur.x = x;
                cmds.push(PathCmd::LineTo(sp(cur)));
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'V' | 'v' => {
                let Some(mut y) = tokens.next_number() else {
                    break;
                };
                if op == 'v' {
                    y += cur.y;
                }
                cur.y = y;
                cmds.push(PathCmd::LineTo(sp(cur)));
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'C' | 'c' => {
                let (Some(mut c1), Some(mut c2), Some(mut p)) = (
                    tokens.next_point(),
                    tokens.next_point(),
                    tokens.next_point(),
                ) else {
                    break;
                };
                if op == 'c' {
                    c1 = Point::new(cur.x + c1.x, cur.y + c1.y);
                    c2 = Point::new(cur.x + c2.x, cur.y + c2.y);
                    p = Point::new(cur.x + p.x, cur.y + p.y);
                }
                cmds.push(PathCmd::CubicTo(sp(c1), sp(c2), sp(p)));
                last_cubic_ctrl = Some(c2);
                last_quad_ctrl = None;
                cur = p;
            }
            'S' | 's' => {
                let (Some(mut c2), Some(mut p)) = (tokens.next_point(), tokens.next_point()) else {
                    break;
                };
                if op == 's' {
                    c2 = Point::new(cur.x + c2.x, cur.y + c2.y);
                    p = Point::new(cur.x + p.x, cur.y + p.y);
                }
                let c1 = match last_cubic_ctrl {
                    Some(prev) => Point::new(2.0 * cur.x - prev.x, 2.0 * cur.y - prev.y),
                    None => cur,
                };
                cmds.push(PathCmd::CubicTo(sp(c1), sp(c2), sp(p)));
                last_cubic_ctrl = Some(c2);
                last_quad_ctrl = None;
                cur = p;
            }
            'Q' | 'q' => {
                let (Some(mut q), Some(mut p)) = (tokens.next_point(), tokens.next_point()) else {
                    break;
                };
                if op == 'q' {
                    q = Point::new(cur.x + q.x, cur.y + q.y);
                    p = Point::new(cur.x + p.x, cur.y + p.y);
                }
                let c1 = Point::new(
                    cur.x + (2.0 / 3.0) * (q.x - cur.x),
                    cur.y + (2.0 / 3.0) * (q.y - cur.y),
                );
                let c2 = Point::new(
                    p.x + (2.0 / 3.0) * (q.x - p.x),
                    p.y + (2.0 / 3.0) * (q.y - p.y),
                );
                cmds.push(PathCmd::CubicTo(sp(c1), sp(c2), sp(p)));
                last_quad_ctrl = Some(q);
                last_cubic_ctrl = None;
                cur = p;
            }
            'T' | 't' => {
                let Some(mut p) = tokens.next_point() else {
                    break;
                };
                if op == 't' {
                    p = Point::new(cur.x + p.x, cur.y + p.y);
                }
                let q = match last_quad_ctrl {
                    Some(prev) => Point::new(2.0 * cur.x - prev.x, 2.0 * cur.y - prev.y),
                    None => cur,
                };
                let c1 = Point::new(
                    cur.x + (2.0 / 3.0) * (q.x - cur.x),
                    cur.y + (2.0 / 3.0) * (q.y - cur.y),
                );
                let c2 = Point::new(
                    p.x + (2.0 / 3.0) * (q.x - p.x),
                    p.y + (2.0 / 3.0) * (q.y - p.y),
                );
                cmds.push(PathCmd::CubicTo(sp(c1), sp(c2), sp(p)));
                last_quad_ctrl = Some(q);
                last_cubic_ctrl = None;
                cur = p;
            }
            'A' | 'a' => {
                let (Some(rx), Some(ry), Some(x_rot), Some(large_arc), Some(sweep), Some(mut p)) = (
                    tokens.next_number(),
                    tokens.next_number(),
                    tokens.next_number(),
                    tokens.next_flag(),
                    tokens.next_flag(),
                    tokens.next_point(),
                ) else {
                    break;
                };
                if op == 'a' {
                    p = Point::new(cur.x + p.x, cur.y + p.y);
                }
                arc_to_cubics(cur, rx, ry, x_rot, large_arc, sweep, p, scale, &mut cmds);
                cur = p;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'Z' | 'z' => {
                cmds.push(PathCmd::Close);
                cur = subpath_start;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            _ => break,
        }
    }

    cmds
}

/// Converts an SVG elliptical arc from `start` to `end` into cubic Bézier curves
/// (W3C SVG 1.1 Appendix F.6).
#[allow(clippy::too_many_arguments)]
fn arc_to_cubics(
    start: Point,
    rx_in: f64,
    ry_in: f64,
    x_axis_rotation_deg: f64,
    large_arc: bool,
    sweep: bool,
    end: Point,
    scale: f64,
    cmds: &mut Vec<PathCmd>,
) {
    let sp = |p: Point| Point::new(p.x * scale, p.y * scale);
    if (start.x - end.x).abs() < 1e-9 && (start.y - end.y).abs() < 1e-9 {
        return;
    }
    let mut rx = rx_in.abs();
    let mut ry = ry_in.abs();
    if rx < 1e-9 || ry < 1e-9 {
        cmds.push(PathCmd::LineTo(sp(end)));
        return;
    }

    let phi = x_axis_rotation_deg.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();

    let dx2 = (start.x - end.x) / 2.0;
    let dy2 = (start.y - end.y) / 2.0;
    let x1p = cos_phi * dx2 + sin_phi * dy2;
    let y1p = -sin_phi * dx2 + cos_phi * dy2;

    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let x1p2 = x1p * x1p;
    let y1p2 = y1p * y1p;

    let lambda = x1p2 / rx2 + y1p2 / ry2;
    if lambda > 1.0 {
        let root = lambda.sqrt();
        rx *= root;
        ry *= root;
    }

    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let num = (rx2 * ry2 - rx2 * y1p2 - ry2 * x1p2).max(0.0);
    let den = rx2 * y1p2 + ry2 * x1p2;
    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let coef = if den > 0.0 {
        sign * (num / den).sqrt()
    } else {
        0.0
    };

    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * (-ry * x1p / rx);

    let cx = cos_phi * cxp - sin_phi * cyp + (start.x + end.x) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (start.y + end.y) / 2.0;

    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;

    let theta1 = vec_angle(1.0, 0.0, ux, uy);
    let mut dtheta = vec_angle(ux, uy, vx, vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += TAU;
    }

    let segments = ((dtheta.abs() / (PI / 2.0)).ceil() as usize).max(1);
    let delta = dtheta / segments as f64;
    let alpha = (4.0 / 3.0) * (delta / 4.0).tan();

    let map_pt = |x: f64, y: f64| {
        Point::new(
            cos_phi * rx * x - sin_phi * ry * y + cx,
            sin_phi * rx * x + cos_phi * ry * y + cy,
        )
    };

    let mut t1 = theta1;
    for _ in 0..segments {
        let t2 = t1 + delta;
        let (s1, c1_cos) = t1.sin_cos();
        let (s2, c2_cos) = t2.sin_cos();
        let p1 = map_pt(c1_cos - alpha * s1, s1 + alpha * c1_cos);
        let p2 = map_pt(c2_cos + alpha * s2, s2 - alpha * c2_cos);
        let p = map_pt(c2_cos, s2);
        cmds.push(PathCmd::CubicTo(sp(p1), sp(p2), sp(p)));
        t1 = t2;
    }
}

fn vec_angle(ux: f64, uy: f64, vx: f64, vy: f64) -> f64 {
    let dot = ux * vx + uy * vy;
    let len = ((ux * ux + uy * uy) * (vx * vx + vy * vy)).sqrt();
    let cos = if len > 0.0 {
        (dot / len).clamp(-1.0, 1.0)
    } else {
        1.0
    };
    let angle = cos.acos();
    if ux * vy - uy * vx < 0.0 {
        -angle
    } else {
        angle
    }
}

struct PathTokenizer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> PathTokenizer<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            bytes: s.as_bytes(),
            pos: 0,
        }
    }

    fn skip_separators(&mut self) {
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b.is_ascii_whitespace() || b == b',' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn next_op(&mut self, implicit: char) -> Option<char> {
        self.skip_separators();
        let &b = self.bytes.get(self.pos)?;
        if b.is_ascii_alphabetic() {
            self.pos += 1;
            Some(b as char)
        } else if implicit != '\0' {
            Some(implicit)
        } else {
            None
        }
    }

    fn next_flag(&mut self) -> Option<bool> {
        self.skip_separators();
        let &b = self.bytes.get(self.pos)?;
        if b == b'0' || b == b'1' {
            self.pos += 1;
            Some(b == b'1')
        } else {
            None
        }
    }

    fn next_number(&mut self) -> Option<f64> {
        self.skip_separators();
        let start = self.pos;
        if self.pos < self.bytes.len()
            && (self.bytes[self.pos] == b'+' || self.bytes[self.pos] == b'-')
        {
            self.pos += 1;
        }
        let mut has_digits = false;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
            has_digits = true;
        }
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'.' {
            self.pos += 1;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
                has_digits = true;
            }
        }
        if !has_digits {
            return None;
        }
        if self.pos < self.bytes.len()
            && (self.bytes[self.pos] == b'e' || self.bytes[self.pos] == b'E')
        {
            self.pos += 1;
            if self.pos < self.bytes.len()
                && (self.bytes[self.pos] == b'+' || self.bytes[self.pos] == b'-')
            {
                self.pos += 1;
            }
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()?
            .parse::<f64>()
            .ok()
    }

    fn next_point(&mut self) -> Option<Point> {
        let x = self.next_number()?;
        let y = self.next_number()?;
        Some(Point::new(x, y))
    }
}
