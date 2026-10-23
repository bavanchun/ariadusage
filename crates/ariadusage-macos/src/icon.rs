// Ported from CodexBar Sources/CodexBar/IconRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Status-item icon geometry for the "bars" style.
//!
//! The icon is an 18 × 18 pt template image rendered at 2×, so every
//! coordinate here is in pixels on a 36 × 36 canvas with the origin at the
//! bottom left (AppKit's default, unflipped). This module only decides what
//! to draw; the AppKit bridge turns the result into an `NSImage`.

/// Point size of the rendered icon.
pub const ICON_SIZE_PT: f64 = 18.0;
/// Pixels per point for the rendered bitmap.
pub const ICON_SCALE: f64 = 2.0;
/// Side length of the bitmap in pixels.
pub const CANVAS_PX: i32 = 36;

const BAR_WIDTH_PX: i32 = 30;
const BAR_X_PX: i32 = (CANVAS_PX - BAR_WIDTH_PX) / 2;
const CREDITS_CAP: f64 = 1000.0;
const DIMMED_LANE_ALPHA: f64 = 0.45;

const TOP_RECT: RectPx = RectPx::new(BAR_X_PX, 19, BAR_WIDTH_PX, 12);
const BOTTOM_RECT: RectPx = RectPx::new(BAR_X_PX, 5, BAR_WIDTH_PX, 8);
const CREDITS_RECT: RectPx = RectPx::new(BAR_X_PX, 14, BAR_WIDTH_PX, 16);
const CREDITS_BOTTOM_RECT: RectPx = RectPx::new(BAR_X_PX, 4, BAR_WIDTH_PX, 6);

/// Width of the track outline in pixels (1 pt at 2×).
pub const STROKE_WIDTH_PX: i32 = 2;

/// An integer pixel rectangle on the icon canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectPx {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectPx {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// The rectangle the outline is stroked in, inset by half the stroke so
    /// the stroke stays within pixel bounds.
    pub fn stroke_rect(&self) -> RectPx {
        let inset = STROKE_WIDTH_PX / 2;
        RectPx::new(
            self.x + inset,
            self.y + inset,
            (self.w - inset * 2).max(0),
            (self.h - inset * 2).max(0),
        )
    }
}

/// A rectangle in points, used for overlays placed off the pixel grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectPt {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectPt {
    fn inset(&self, d: f64) -> RectPt {
        RectPt {
            x: self.x + d,
            y: self.y + d,
            w: self.w - 2.0 * d,
            h: self.h - 2.0 * d,
        }
    }

    fn union(&self, other: &RectPt) -> RectPt {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let max_x = (self.x + self.w).max(other.x + other.w);
        let max_y = (self.y + self.h).max(other.y + other.h);
        RectPt {
            x,
            y,
            w: max_x - x,
            h: max_y - y,
        }
    }
}

/// One meter lane: a rounded track, its outline and an optional fill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub rect: RectPx,
    /// Percentage that drives the fill, or `None` for an empty track.
    pub percent: Option<f64>,
    /// Multiplier applied to every alpha of this lane.
    pub alpha: f64,
    pub corner_radius_px: i32,
}

impl Bar {
    fn lane(rect: RectPx, percent: Option<f64>, alpha: f64) -> Self {
        Self {
            rect,
            percent,
            alpha,
            corner_radius_px: rect.h / 2,
        }
    }

    /// Width of the straight-edged fill in pixels.
    pub fn fill_width_px(&self) -> i32 {
        self.percent
            .map(|p| fill_width_pixels(p, self.rect.w))
            .unwrap_or(0)
    }
}

/// Provider status as shown on the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusLevel {
    #[default]
    None,
    Minor,
    Maintenance,
    Major,
    Critical,
    Unknown,
}

/// The status mark drawn in the icon's lower right corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StatusOverlay {
    /// Minor issues and maintenance: a 4 pt dot.
    Dot { dot: RectPt, halo: RectPt },
    /// Major, critical and unknown issues: an exclamation mark.
    Exclamation {
        line: RectPt,
        dot: RectPt,
        halo: RectPt,
    },
}

/// Alphas for the track, outline and fill, which depend on staleness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconPalette {
    pub track_fill_alpha: f64,
    pub track_stroke_alpha: f64,
    pub fill_alpha: f64,
}

impl IconPalette {
    pub fn for_stale(stale: bool) -> Self {
        if stale {
            Self {
                track_fill_alpha: 0.18,
                track_stroke_alpha: 0.28,
                fill_alpha: 0.55,
            }
        } else {
            Self {
                track_fill_alpha: 0.28,
                track_stroke_alpha: 0.44,
                fill_alpha: 1.0,
            }
        }
    }
}

/// Everything the renderer needs to draw one icon.
#[derive(Debug, Clone, PartialEq)]
pub struct IconLayout {
    pub bars: Vec<Bar>,
    pub palette: IconPalette,
    pub overlay: Option<StatusOverlay>,
}

/// Inputs to the icon layout, already resolved from a provider snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct IconInput {
    /// Session lane percentage (remaining by default, used when the user
    /// chose to show used).
    pub primary: Option<f64>,
    /// Weekly lane percentage.
    pub secondary: Option<f64>,
    /// Remaining credits, capped at 1000 for the meter.
    pub credits_remaining: Option<f64>,
    pub stale: bool,
    pub status: StatusLevel,
    /// Keep a dimmed second lane when only the first one has data, so a
    /// missing weekly window still reads as missing (Claude).
    pub reserves_missing_secondary_lane: bool,
}

/// Width in pixels of a fill for `percent` in a lane `rect_width` wide.
pub fn fill_width_pixels(percent: f64, rect_width: i32) -> i32 {
    let clamped = (percent / 100.0).clamp(0.0, 1.0);
    let width = (f64::from(rect_width) * clamped).round() as i32;
    width.clamp(0, rect_width)
}

/// Lays out the icon for `input`, following CodexBar's lane rules.
pub fn layout(input: &IconInput) -> IconLayout {
    let top = input.primary;
    let bottom = input.secondary;
    let credits_ratio = input
        .credits_remaining
        .map(|c| (c / CREDITS_CAP * 100.0).min(100.0));
    let has_weekly = bottom.is_some();
    let weekly_available = bottom.is_some_and(|b| b > 0.0);

    let mut bars = Vec::with_capacity(2);
    let mut overlay_on_prominent_meter = false;

    if let (Some(bottom), None) = (bottom, top)
        && bottom > 0.0
        && !input.reserves_missing_secondary_lane
    {
        // The only meaningful quota sits in the secondary slot.
        overlay_on_prominent_meter = true;
        bars.push(Bar::lane(CREDITS_RECT, Some(bottom), 1.0));
    } else if weekly_available {
        bars.push(Bar::lane(TOP_RECT, top, 1.0));
        bars.push(Bar::lane(BOTTOM_RECT, bottom, 1.0));
    } else if !has_weekly {
        match (top, credits_ratio) {
            (None, Some(ratio)) => {
                bars.push(Bar::lane(CREDITS_RECT, Some(ratio), 1.0));
                bars.push(Bar::lane(CREDITS_BOTTOM_RECT, None, DIMMED_LANE_ALPHA));
            }
            (Some(top), _) if !input.reserves_missing_secondary_lane => {
                // One quota reads as one meter rather than half an icon.
                overlay_on_prominent_meter = true;
                bars.push(Bar::lane(CREDITS_RECT, Some(top), 1.0));
            }
            _ => {
                bars.push(Bar::lane(TOP_RECT, top, 1.0));
                bars.push(Bar::lane(BOTTOM_RECT, None, DIMMED_LANE_ALPHA));
            }
        }
    } else {
        // Weekly exhausted: credits on top when known, else the session lane.
        match credits_ratio {
            Some(ratio) => bars.push(Bar::lane(CREDITS_RECT, Some(ratio), 1.0)),
            None => bars.push(Bar::lane(TOP_RECT, top, 1.0)),
        }
        bars.push(Bar::lane(CREDITS_BOTTOM_RECT, bottom, 1.0));
    }

    IconLayout {
        bars,
        palette: IconPalette::for_stale(input.stale),
        overlay: status_overlay(input.status, overlay_on_prominent_meter),
    }
}

fn status_overlay(status: StatusLevel, on_prominent_meter: bool) -> Option<StatusOverlay> {
    match status {
        StatusLevel::None => None,
        StatusLevel::Minor | StatusLevel::Maintenance => {
            let size = 4.0;
            let dot = RectPt {
                x: ICON_SIZE_PT - size - 2.0,
                y: if on_prominent_meter { 5.0 } else { 2.0 },
                w: size,
                h: size,
            };
            Some(StatusOverlay::Dot {
                dot,
                halo: dot.inset(-1.0),
            })
        }
        StatusLevel::Major | StatusLevel::Critical | StatusLevel::Unknown => {
            let line = RectPt {
                x: ICON_SIZE_PT - 6.0,
                y: 4.0,
                w: 2.0,
                h: 6.0,
            };
            let dot = RectPt {
                x: ICON_SIZE_PT - 6.0,
                y: 2.0,
                w: 2.0,
                h: 2.0,
            };
            Some(StatusOverlay::Exclamation {
                line,
                dot,
                halo: line.union(&dot).inset(-1.0),
            })
        }
    }
}
