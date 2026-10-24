// Ported from CodexBar Sources/CodexBar/IconRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/LoadingPattern.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarLayoutRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfig.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Providers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderBranding.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Status-item icon geometry for both the "bars" and "critters" styles,
//! loading-animation waveforms, quota-warning thresholds, and provider tint
//! contrast checks.
//!
//! The icon is an 18 × 18 pt template image rendered at 2×, so every pixel
//! coordinate here is on a 36 × 36 canvas with the origin at the bottom left
//! (AppKit's default, unflipped). This module only decides what to draw; the
//! AppKit bridge turns the result into an `NSImage`.

use std::f64::consts::PI;

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

/// Smallest non-zero percent used during loading animations so the secondary
/// lane never collapses into the exhausted-weekly layout (`StatusItemController+Animation.swift:6`).
pub const LOADING_PERCENT_EPSILON: f64 = 0.0001;

/// Duration in seconds that a quota-warning flash remains active (`StatusItemController.swift:32`).
pub const QUOTA_WARNING_FLASH_DURATION_SECS: f64 = 60.0;

/// Default remaining-percentage warning thresholds (`QuotaWarningThresholds.defaults`).
pub const DEFAULT_QUOTA_WARNING_THRESHOLDS: [i32; 2] = [50, 20];

const TOP_RECT: RectPx = RectPx::new(BAR_X_PX, 19, BAR_WIDTH_PX, 12);
const BOTTOM_RECT: RectPx = RectPx::new(BAR_X_PX, 5, BAR_WIDTH_PX, 8);
const CREDITS_RECT: RectPx = RectPx::new(BAR_X_PX, 14, BAR_WIDTH_PX, 16);
const CREDITS_BOTTOM_RECT: RectPx = RectPx::new(BAR_X_PX, 4, BAR_WIDTH_PX, 6);

/// Width of the track outline in pixels (1 pt at 2×).
pub const STROKE_WIDTH_PX: i32 = 2;

const fn px_to_pt(px: i32) -> f64 {
    (px as f64) / ICON_SCALE
}

fn snap_delta(value: f64) -> f64 {
    (value * ICON_SCALE).round() / ICON_SCALE
}

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

    pub const fn mid_x_px(&self) -> i32 {
        self.x + self.w / 2
    }

    pub const fn mid_y_px(&self) -> i32 {
        self.y + self.h / 2
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

/// A 2D point in points on the 18 × 18 pt icon canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointPt {
    pub x: f64,
    pub y: f64,
}

impl PointPt {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn lerp(self, other: Self, p: f64) -> Self {
        Self {
            x: self.x + (other.x - self.x) * p,
            y: self.y + (other.y - self.y) * p,
        }
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
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

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

/// Provider-specific icon style (`CodexBarCore.IconStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IconStyle {
    Codex,
    Claude,
    Gemini,
    Antigravity,
    Cursor,
    Factory,
    Grok,
    Copilot,
    Warp,
    #[default]
    Combined,
}

impl IconStyle {
    /// Resolves the icon style for a provider ID (`ProviderDefaults.metadata`).
    pub fn from_provider_id(provider_id: &str) -> Self {
        match provider_id {
            "codex" => Self::Codex,
            "claude" | "kiro" => Self::Claude,
            "gemini" => Self::Gemini,
            "antigravity" => Self::Antigravity,
            "cursor" => Self::Cursor,
            "factory" | "droid" => Self::Factory,
            "grok" => Self::Grok,
            "copilot" => Self::Copilot,
            "warp" => Self::Warp,
            _ => Self::Combined,
        }
    }

    /// Critter decorations associated with this style (`ProviderUsagePresentation.iconDecorations`).
    pub const fn decorations(self) -> IconDecorations {
        match self {
            Self::Codex | Self::Copilot => IconDecorations {
                face: true,
                ..IconDecorations::empty()
            },
            Self::Claude => IconDecorations {
                notches: true,
                ..IconDecorations::empty()
            },
            Self::Gemini => IconDecorations {
                gemini: true,
                ..IconDecorations::empty()
            },
            Self::Antigravity => IconDecorations {
                gemini: true,
                antigravity: true,
                ..IconDecorations::empty()
            },
            Self::Factory => IconDecorations {
                factory: true,
                ..IconDecorations::empty()
            },
            Self::Warp => IconDecorations {
                warp: true,
                ..IconDecorations::empty()
            },
            Self::Grok => IconDecorations {
                grok: true,
                ..IconDecorations::empty()
            },
            Self::Cursor | Self::Combined => IconDecorations::empty(),
        }
    }

    /// Default quota lane policy for this style (`IconRenderer.QuotaLayoutPolicy.style`).
    pub const fn quota_layout_policy(self) -> QuotaLayoutPolicy {
        QuotaLayoutPolicy {
            reserves_missing_secondary_lane: matches!(self, Self::Claude),
            treats_exhausted_secondary_as_missing: matches!(self, Self::Warp),
        }
    }
}

/// Active critter decoration twists (`ProviderIconDecorations`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IconDecorations {
    pub face: bool,
    pub notches: bool,
    pub gemini: bool,
    pub antigravity: bool,
    pub factory: bool,
    pub warp: bool,
    pub grok: bool,
}

impl IconDecorations {
    pub const fn empty() -> Self {
        Self {
            face: false,
            notches: false,
            gemini: false,
            antigravity: false,
            factory: false,
            warp: false,
            grok: false,
        }
    }

    pub const fn is_empty(&self) -> bool {
        !self.face
            && !self.notches
            && !self.gemini
            && !self.antigravity
            && !self.factory
            && !self.warp
            && !self.grok
    }
}

/// Policy flags that adjust how missing or exhausted secondary lanes are laid out
/// (`IconRenderer.QuotaLayoutPolicy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QuotaLayoutPolicy {
    pub reserves_missing_secondary_lane: bool,
    pub treats_exhausted_secondary_as_missing: bool,
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
        Self::with_decorations(rect, percent, alpha, IconDecorations::empty())
    }

    fn with_decorations(
        rect: RectPx,
        percent: Option<f64>,
        alpha: f64,
        decorations: IconDecorations,
    ) -> Self {
        // `IconRenderer.swift:186`: Claude (`notches`) uses 0 corner radius,
        // Warp (`warp`) uses 3 px, and all other styles use a capsule (`rect.h / 2`).
        let corner_radius_px = if decorations.notches {
            0
        } else if decorations.warp {
            3
        } else {
            rect.h / 2
        };
        Self {
            rect,
            percent,
            alpha,
            corner_radius_px,
        }
    }

    /// Width of the straight-edged fill in pixels.
    pub fn fill_width_px(&self) -> i32 {
        self.percent
            .map(|p| fill_width_pixels(p, self.rect.w))
            .unwrap_or(0)
    }
}

/// Hat rectangle and tilt transform for the `.face` critter (Codex / Copilot).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HatDecoration {
    pub rect: RectPx,
    pub pivot: PointPt,
    pub tilt_radians: f64,
    pub y_offset_pt: f64,
}

/// Eye cutouts, blink refills, and hat for the `.face` critter (Codex / Copilot).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceDecoration {
    pub left_eye: RectPx,
    pub right_eye: RectPx,
    pub left_blink: Option<RectPx>,
    pub right_blink: Option<RectPx>,
    pub hat: HatDecoration,
}

/// Blocky crab arms, 4 legs, vertical eye cutouts, and blink refills for `.notches` (Claude).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotchesDecoration {
    pub left_arm: RectPx,
    pub right_arm: RectPx,
    pub legs: [RectPx; 4],
    pub left_eye: RectPx,
    pub right_eye: RectPx,
    pub left_blink: Option<RectPx>,
    pub right_blink: Option<RectPx>,
}

/// 4-pointed star eye cutouts, 4 protruding sparkle points, and blink stars for `.gemini`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeminiDecoration {
    pub left_star: [PointPt; 8],
    pub right_star: [PointPt; 8],
    pub top_point: [PointPt; 3],
    pub bottom_point: [PointPt; 3],
    pub left_point: [PointPt; 3],
    pub right_point: [PointPt; 3],
    pub left_blink_star: Option<[PointPt; 8]>,
    pub right_blink_star: Option<[PointPt; 8]>,
}

/// 8-pointed asterisk eye cutouts, 4 gear teeth, and blink asterisks for `.factory`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FactoryDecoration {
    pub left_asterisk: [PointPt; 16],
    pub right_asterisk: [PointPt; 16],
    pub teeth: [RectPx; 4],
    pub left_blink_asterisk: Option<[PointPt; 16]>,
    pub right_blink_asterisk: Option<[PointPt; 16]>,
}

/// One tilted ellipse eye for `.warp`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TiltedEllipse {
    pub center: PointPt,
    pub width_pt: f64,
    pub height_pt: f64,
    pub tilt_radians: f64,
}

/// Two tilted ellipse eye cutouts for `.warp`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpDecoration {
    pub left_eye: TiltedEllipse,
    pub right_eye: TiltedEllipse,
    pub eyes_filled: bool,
}

/// Twin top horns and a rounded-rect visor cutout for `.grok`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrokDecoration {
    pub left_horn: RectPx,
    pub right_horn: RectPx,
    pub visor: RectPx,
    pub visor_corner_radius_pt: f64,
}

/// Resolved critter decorations attached to the top / prominent bar.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CritterDecorations {
    pub alpha: f64,
    pub face: Option<FaceDecoration>,
    pub notches: Option<NotchesDecoration>,
    pub gemini: Option<GeminiDecoration>,
    pub antigravity_dot: Option<RectPx>,
    pub factory: Option<FactoryDecoration>,
    pub warp: Option<WarpDecoration>,
    pub grok: Option<GrokDecoration>,
}

impl CritterDecorations {
    pub const fn is_empty(&self) -> bool {
        self.face.is_none()
            && self.notches.is_none()
            && self.gemini.is_none()
            && self.antigravity_dot.is_none()
            && self.factory.is_none()
            && self.warp.is_none()
            && self.grok.is_none()
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
        // Upstream computes `fillColor = baseFill.withAlphaComponent(stale ? 0.55 : 1.0)`
        // (`IconRenderer.swift:163`), then draws with `fillColor.withAlphaComponent(alpha)`
        // (`:217`). Because `colorWithAlphaComponent:` replaces alpha rather than
        // multiplying it, the fill always uses the lane alpha (`1.0 * bar.alpha`).
        if stale {
            Self {
                track_fill_alpha: 0.18,
                track_stroke_alpha: 0.28,
                fill_alpha: 1.0,
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
    pub critters: CritterDecorations,
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
    /// Treat an exhausted (`<= 0`) secondary window as missing so the bottom
    /// lane dims at `0.45` on `BOTTOM_RECT` (Warp).
    pub treats_exhausted_secondary_as_missing: bool,
    /// Provider icon style (determines critter decorations when `!hide_critters`).
    pub style: IconStyle,
    /// Suppress critter decorations and draw plain rounded meter bars (`MenuBarIconStyle.bars`).
    pub hide_critters: bool,
    /// Eye-blink amount in `0.0..=1.0`.
    pub blink: f64,
    /// Vertical wiggle amount for Claude's crab arms/legs/eyes.
    pub wiggle: f64,
    /// Hat tilt angle in radians for Codex's `.face` cap.
    pub tilt: f64,
}

/// Width in pixels of a fill for `percent` in a lane `rect_width` wide.
pub fn fill_width_pixels(percent: f64, rect_width: i32) -> i32 {
    let clamped = (percent / 100.0).clamp(0.0, 1.0);
    let width = (f64::from(rect_width) * clamped).round() as i32;
    width.clamp(0, rect_width)
}

/// Lays out the icon for `input`, following CodexBar's lane and critter rules.
pub fn layout(input: &IconInput) -> IconLayout {
    let uses_missing_secondary_layout =
        input.treats_exhausted_secondary_as_missing || matches!(input.style, IconStyle::Warp);
    let effective_secondary = match input.secondary {
        Some(v) if uses_missing_secondary_layout && v <= 0.0 => None,
        other => other,
    };

    let top = input.primary;
    let bottom = effective_secondary;
    let credits_ratio = input
        .credits_remaining
        .map(|c| (c / CREDITS_CAP * 100.0).min(100.0));
    let has_weekly = bottom.is_some();
    let weekly_available = bottom.is_some_and(|b| b > 0.0);
    let missing_secondary = uses_missing_secondary_layout && !weekly_available;

    let decorations = if input.hide_critters {
        IconDecorations::empty()
    } else {
        input.style.decorations()
    };

    let mut bars = Vec::with_capacity(2);
    let mut overlay_on_prominent_meter = false;

    if let (Some(bottom), None) = (bottom, top)
        && bottom > 0.0
        && !input.reserves_missing_secondary_lane
        && !uses_missing_secondary_layout
    {
        // The only meaningful quota sits in the secondary slot.
        overlay_on_prominent_meter = true;
        bars.push(Bar::with_decorations(
            CREDITS_RECT,
            Some(bottom),
            1.0,
            decorations,
        ));
    } else if weekly_available {
        bars.push(Bar::with_decorations(TOP_RECT, top, 1.0, decorations));
        bars.push(Bar::lane(BOTTOM_RECT, bottom, 1.0));
    } else if !has_weekly || missing_secondary {
        if uses_missing_secondary_layout {
            // Warp: no bonus or bonus exhausted -> top = monthly credits, bottom = dimmed track.
            let warp_only = IconDecorations {
                warp: decorations.warp,
                ..IconDecorations::empty()
            };
            bars.push(Bar::with_decorations(TOP_RECT, top, 1.0, warp_only));
            bars.push(Bar::lane(BOTTOM_RECT, None, DIMMED_LANE_ALPHA));
        } else {
            match (top, credits_ratio) {
                (None, Some(ratio)) => {
                    bars.push(Bar::with_decorations(
                        CREDITS_RECT,
                        Some(ratio),
                        1.0,
                        decorations,
                    ));
                    bars.push(Bar::lane(CREDITS_BOTTOM_RECT, None, DIMMED_LANE_ALPHA));
                }
                (Some(top), _) if !input.reserves_missing_secondary_lane => {
                    // One quota reads as one meter rather than half an icon.
                    overlay_on_prominent_meter = true;
                    bars.push(Bar::with_decorations(
                        CREDITS_RECT,
                        Some(top),
                        1.0,
                        decorations,
                    ));
                }
                _ => {
                    bars.push(Bar::with_decorations(TOP_RECT, top, 1.0, decorations));
                    bars.push(Bar::lane(BOTTOM_RECT, None, DIMMED_LANE_ALPHA));
                }
            }
        }
    } else {
        // Weekly exhausted: credits on top when known, else the session lane.
        match credits_ratio {
            Some(ratio) => bars.push(Bar::with_decorations(
                CREDITS_RECT,
                Some(ratio),
                1.0,
                decorations,
            )),
            None => bars.push(Bar::with_decorations(TOP_RECT, top, 1.0, decorations)),
        }
        bars.push(Bar::lane(CREDITS_BOTTOM_RECT, bottom, 1.0));
    }

    let critters = bars
        .first()
        .map(|bar| {
            let active_decs = if !has_weekly || missing_secondary {
                if uses_missing_secondary_layout {
                    IconDecorations {
                        warp: decorations.warp,
                        ..IconDecorations::empty()
                    }
                } else {
                    decorations
                }
            } else {
                decorations
            };
            build_critter_decorations(
                bar.rect,
                bar.alpha,
                active_decs,
                input.blink,
                input.wiggle,
                input.tilt,
            )
        })
        .unwrap_or_default();

    IconLayout {
        bars,
        palette: IconPalette::for_stale(input.stale),
        overlay: status_overlay(input.status, overlay_on_prominent_meter),
        critters,
    }
}

fn build_critter_decorations(
    rect: RectPx,
    alpha: f64,
    decorations: IconDecorations,
    blink: f64,
    wiggle: f64,
    tilt: f64,
) -> CritterDecorations {
    if decorations.is_empty() {
        return CritterDecorations::default();
    }

    let center_x_px = rect.mid_x_px();
    let eye_center_y_px = rect.mid_y_px();

    let face = decorations.face.then(|| {
        let eye_size_px = 4;
        let eye_offset_px = 7;
        let left_eye = RectPx::new(
            center_x_px - eye_offset_px - eye_size_px / 2,
            eye_center_y_px - eye_size_px / 2,
            eye_size_px,
            eye_size_px,
        );
        let right_eye = RectPx::new(
            center_x_px + eye_offset_px - eye_size_px / 2,
            eye_center_y_px - eye_size_px / 2,
            eye_size_px,
            eye_size_px,
        );
        let (left_blink, right_blink) = if blink > 0.001 {
            let clamped = blink.clamp(0.0, 1.0);
            let blink_h = (f64::from(eye_size_px) * clamped).round() as i32;
            let blink_y = eye_center_y_px + eye_size_px / 2 - blink_h;
            (
                Some(RectPx::new(left_eye.x, blink_y, eye_size_px, blink_h)),
                Some(RectPx::new(right_eye.x, blink_y, eye_size_px, blink_h)),
            )
        } else {
            (None, None)
        };
        let hat_width_px = 18;
        let hat_height_px = 4;
        let hat = HatDecoration {
            rect: RectPx::new(
                center_x_px - hat_width_px / 2,
                rect.y + rect.h - hat_height_px,
                hat_width_px,
                hat_height_px,
            ),
            pivot: PointPt::new(px_to_pt(center_x_px), px_to_pt(eye_center_y_px)),
            tilt_radians: tilt,
            y_offset_pt: -tilt.abs() * 1.2,
        };
        FaceDecoration {
            left_eye,
            right_eye,
            left_blink,
            right_blink,
            hat,
        }
    });

    let notches = decorations.notches.then(|| {
        let wiggle_offset = snap_delta(wiggle * 0.6);
        let wiggle_px = (wiggle_offset * ICON_SCALE).round() as i32;

        let arm_width_px = 3;
        let arm_height_px = (rect.h - 6).max(0);
        let arm_y_px = rect.y + 3 + wiggle_px / 6;
        let left_arm = RectPx::new(rect.x - arm_width_px, arm_y_px, arm_width_px, arm_height_px);
        let right_arm = RectPx::new(rect.x + rect.w, arm_y_px, arm_width_px, arm_height_px);

        let leg_width_px = 2;
        let leg_height_px = 3;
        let leg_y_px = rect.y - leg_height_px + wiggle_px / 6;
        let step_px = (rect.w / 5).max(1);
        let legs = std::array::from_fn(|idx| {
            let cx = rect.x + step_px * (idx as i32 + 1);
            RectPx::new(cx - leg_width_px / 2, leg_y_px, leg_width_px, leg_height_px)
        });

        let eye_width_px = 2;
        let eye_height_px = 5;
        let eye_offset_px = 6;
        let eye_y_px = rect.y + rect.h - eye_height_px - 2 + wiggle_px / 8;
        let left_eye = RectPx::new(
            center_x_px - eye_offset_px - eye_width_px / 2,
            eye_y_px,
            eye_width_px,
            eye_height_px,
        );
        let right_eye = RectPx::new(
            center_x_px + eye_offset_px - eye_width_px / 2,
            eye_y_px,
            eye_width_px,
            eye_height_px,
        );
        let (left_blink, right_blink) = if blink > 0.001 {
            let clamped = blink.clamp(0.0, 1.0);
            let blink_h = (f64::from(eye_height_px) * clamped).round() as i32;
            let blink_y = eye_y_px + eye_height_px - blink_h;
            (
                Some(RectPx::new(left_eye.x, blink_y, eye_width_px, blink_h)),
                Some(RectPx::new(right_eye.x, blink_y, eye_width_px, blink_h)),
            )
        } else {
            (None, None)
        };

        NotchesDecoration {
            left_arm,
            right_arm,
            legs,
            left_eye,
            right_eye,
            left_blink,
            right_blink,
        }
    });

    let gemini = decorations.gemini.then(|| {
        let star_size_px = 8;
        let eye_offset_px = 8;
        let sr = px_to_pt(star_size_px / 2);
        let inner_r = sr * 0.25;
        let ld_cx = px_to_pt(center_x_px - eye_offset_px);
        let rd_cx = px_to_pt(center_x_px + eye_offset_px);
        let y_cy = px_to_pt(eye_center_y_px);

        let left_star = four_point_star(ld_cx, y_cy, sr, inner_r);
        let right_star = four_point_star(rd_cx, y_cy, sr, inner_r);

        let point_height_px = 4;
        let point_width_px = 4;
        let top_cx = px_to_pt(center_x_px);
        let half_w = px_to_pt(point_width_px / 2);
        let top_base_y = px_to_pt(rect.y + rect.h);
        let top_peak_y = px_to_pt(rect.y + rect.h + point_height_px);
        let top_point = [
            PointPt::new(top_cx - half_w, top_base_y),
            PointPt::new(top_cx, top_peak_y),
            PointPt::new(top_cx + half_w, top_base_y),
        ];

        let bottom_base_y = px_to_pt(rect.y);
        let bottom_peak_y = px_to_pt(rect.y - point_height_px);
        let bottom_point = [
            PointPt::new(top_cx - half_w, bottom_base_y),
            PointPt::new(top_cx, bottom_peak_y),
            PointPt::new(top_cx + half_w, bottom_base_y),
        ];

        let side_point_h = 3;
        let side_point_w = 3;
        // Matches Swift integer division `Self.grid.pt(sidePointW / 2)` (`3 / 2 == 1`).
        let side_half_w = px_to_pt(side_point_w / 2);
        let bar_mid_y = px_to_pt(eye_center_y_px);
        let left_base_x = px_to_pt(rect.x);
        let left_peak_x = px_to_pt(rect.x - side_point_h);
        let left_point = [
            PointPt::new(left_base_x, bar_mid_y - side_half_w),
            PointPt::new(left_peak_x, bar_mid_y),
            PointPt::new(left_base_x, bar_mid_y + side_half_w),
        ];

        let right_base_x = px_to_pt(rect.x + rect.w);
        let right_peak_x = px_to_pt(rect.x + rect.w + side_point_h);
        let right_point = [
            PointPt::new(right_base_x, bar_mid_y - side_half_w),
            PointPt::new(right_peak_x, bar_mid_y),
            PointPt::new(right_base_x, bar_mid_y + side_half_w),
        ];

        let (left_blink_star, right_blink_star) = if blink > 0.001 {
            let clamped = blink.clamp(0.0, 1.0);
            let blink_r = sr * clamped;
            let blink_inner_r = blink_r * 0.25;
            (
                Some(four_point_star(ld_cx, y_cy, blink_r, blink_inner_r)),
                Some(four_point_star(rd_cx, y_cy, blink_r, blink_inner_r)),
            )
        } else {
            (None, None)
        };

        GeminiDecoration {
            left_star,
            right_star,
            top_point,
            bottom_point,
            left_point,
            right_point,
            left_blink_star,
            right_blink_star,
        }
    });

    let antigravity_dot = decorations.antigravity.then(|| {
        let dot_size_px = 3;
        let dot_offset_x_px = rect.x + rect.w + 2;
        let dot_offset_y_px = rect.y + rect.h - 2;
        RectPx::new(
            dot_offset_x_px - dot_size_px / 2,
            dot_offset_y_px - dot_size_px / 2,
            dot_size_px,
            dot_size_px,
        )
    });

    let factory = decorations.factory.then(|| {
        let star_size_px = 7;
        let eye_offset_px = 8;
        // Matches Swift integer division `Self.grid.pt(starSizePx / 2)` (`7 / 2 == 3`).
        let sr = px_to_pt(star_size_px / 2);
        let inner_r = sr * 0.3;
        let ld_cx = px_to_pt(center_x_px - eye_offset_px);
        let rd_cx = px_to_pt(center_x_px + eye_offset_px);
        let y_cy = px_to_pt(eye_center_y_px);

        let left_asterisk = eight_point_asterisk(ld_cx, y_cy, sr, inner_r);
        let right_asterisk = eight_point_asterisk(rd_cx, y_cy, sr, inner_r);

        let tooth_width_px = 3;
        let tooth_height_px = 2;
        let tooth_1_x = center_x_px - 5 - tooth_width_px / 2;
        let tooth_2_x = center_x_px + 5 - tooth_width_px / 2;
        let top_y = rect.y + rect.h;
        let bottom_y = rect.y - tooth_height_px;
        let teeth = [
            RectPx::new(tooth_1_x, top_y, tooth_width_px, tooth_height_px),
            RectPx::new(tooth_2_x, top_y, tooth_width_px, tooth_height_px),
            RectPx::new(tooth_1_x, bottom_y, tooth_width_px, tooth_height_px),
            RectPx::new(tooth_2_x, bottom_y, tooth_width_px, tooth_height_px),
        ];

        let (left_blink_asterisk, right_blink_asterisk) = if blink > 0.001 {
            let clamped = blink.clamp(0.0, 1.0);
            let blink_r = sr * clamped;
            let blink_inner_r = blink_r * 0.3;
            (
                Some(eight_point_asterisk(ld_cx, y_cy, blink_r, blink_inner_r)),
                Some(eight_point_asterisk(rd_cx, y_cy, blink_r, blink_inner_r)),
            )
        } else {
            (None, None)
        };

        FactoryDecoration {
            left_asterisk,
            right_asterisk,
            teeth,
            left_blink_asterisk,
            right_blink_asterisk,
        }
    });

    let warp = decorations.warp.then(|| {
        let eye_width_px = 5;
        let eye_height_px = 8;
        let eye_offset_px = 7;
        let eye_tilt_angle = PI / 3.0;
        let left_cx = px_to_pt(center_x_px) - px_to_pt(eye_offset_px);
        let right_cx = px_to_pt(center_x_px) + px_to_pt(eye_offset_px);
        let eye_cy = px_to_pt(eye_center_y_px);
        let width_pt = px_to_pt(eye_width_px);
        let height_pt = px_to_pt(eye_height_px);
        WarpDecoration {
            left_eye: TiltedEllipse {
                center: PointPt::new(left_cx, eye_cy),
                width_pt,
                height_pt,
                tilt_radians: eye_tilt_angle,
            },
            right_eye: TiltedEllipse {
                center: PointPt::new(right_cx, eye_cy),
                width_pt,
                height_pt,
                tilt_radians: -eye_tilt_angle,
            },
            eyes_filled: false,
        }
    });

    let grok = decorations.grok.then(|| {
        let horn_width_px = 4;
        let horn_height_px = 3;
        let horn_offset_px = 8;
        let left_horn = RectPx::new(
            center_x_px - horn_offset_px - horn_width_px / 2,
            rect.y + rect.h,
            horn_width_px,
            horn_height_px,
        );
        let right_horn = RectPx::new(
            center_x_px + horn_offset_px - horn_width_px / 2,
            rect.y + rect.h,
            horn_width_px,
            horn_height_px,
        );
        let visor_width_px = 14;
        let visor_height_px = 3;
        let visor = RectPx::new(
            center_x_px - visor_width_px / 2,
            rect.y + rect.h / 2 - visor_height_px / 2,
            visor_width_px,
            visor_height_px,
        );
        GrokDecoration {
            left_horn,
            right_horn,
            visor,
            visor_corner_radius_pt: px_to_pt(visor_height_px) / 2.0,
        }
    });

    CritterDecorations {
        alpha,
        face,
        notches,
        gemini,
        antigravity_dot,
        factory,
        warp,
        grok,
    }
}

fn four_point_star(cx: f64, cy: f64, outer_r: f64, inner_r: f64) -> [PointPt; 8] {
    std::array::from_fn(|i| {
        let angle = (i as f64) * PI / 4.0 - PI / 2.0;
        let r = if i % 2 == 0 { outer_r } else { inner_r };
        PointPt::new(cx + angle.cos() * r, cy + angle.sin() * r)
    })
}

fn eight_point_asterisk(cx: f64, cy: f64, outer_r: f64, inner_r: f64) -> [PointPt; 16] {
    std::array::from_fn(|i| {
        let angle = (i as f64) * PI / 8.0 - PI / 2.0;
        let r = if i % 2 == 0 { outer_r } else { inner_r };
        PointPt::new(cx + angle.cos() * r, cy + angle.sin() * r)
    })
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

/// One ribbon segment of the unbraid morph animation (`IconRenderer.drawUnbraidMorph`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MorphRibbon {
    pub center: PointPt,
    pub length: f64,
    pub thickness: f64,
    pub angle_deg: f64,
    pub alpha: f64,
}

/// Complete layout for a frame of `makeMorphIcon(progress:style:hideCritters:)`.
#[derive(Debug, Clone, PartialEq)]
pub struct MorphLayout {
    pub ribbons: [MorphRibbon; 3],
    pub crossfade_bars: Option<(IconLayout, f64)>,
}

/// Computes the unbraid morph geometry at `progress` (`0.0..=1.0`) (`IconRenderer.swift:894-1037`).
pub fn morph_layout(progress: f64, style: IconStyle, hide_critters: bool) -> MorphLayout {
    let t = progress.clamp(0.0, 1.0);
    let center = PointPt::new(ICON_SIZE_PT / 2.0, ICON_SIZE_PT / 2.0);

    #[derive(Clone, Copy)]
    struct SegmentSpec {
        start_center: PointPt,
        end_center: PointPt,
        start_angle: f64,
        end_angle: f64,
        start_length: f64,
        end_length: f64,
        start_thickness: f64,
        end_thickness: f64,
        fade_out: bool,
    }

    let specs = [
        // Upper ribbon -> top bar
        SegmentSpec {
            start_center: PointPt::new(center.x, center.y + 2.0),
            end_center: PointPt::new(center.x, 9.0),
            start_angle: -30.0,
            end_angle: 0.0,
            start_length: 16.0,
            end_length: 14.0,
            start_thickness: 3.4,
            end_thickness: 3.0,
            fade_out: false,
        },
        // Lower ribbon -> bottom bar
        SegmentSpec {
            start_center: PointPt::new(center.x, center.y - 2.0),
            end_center: PointPt::new(center.x, 4.0),
            start_angle: 210.0,
            end_angle: 0.0,
            start_length: 16.0,
            end_length: 12.0,
            start_thickness: 3.4,
            end_thickness: 2.4,
            fade_out: false,
        },
        // Side ribbon fades away
        SegmentSpec {
            start_center: center,
            end_center: PointPt::new(center.x, center.y + 6.0),
            start_angle: 90.0,
            end_angle: 0.0,
            start_length: 16.0,
            end_length: 8.0,
            start_thickness: 3.4,
            end_thickness: 1.8,
            fade_out: true,
        },
    ];

    let lerp = |a: f64, b: f64, p: f64| a + (b - a) * p;
    let ribbons = std::array::from_fn(|idx| {
        let seg = specs[idx];
        let p = if seg.fade_out { t * 1.1 } else { t };
        MorphRibbon {
            center: seg.start_center.lerp(seg.end_center, p),
            length: lerp(seg.start_length, seg.end_length, p),
            thickness: lerp(seg.start_thickness, seg.end_thickness, p),
            angle_deg: lerp(seg.start_angle, seg.end_angle, p),
            alpha: if seg.fade_out {
                (1.0 - p).clamp(0.0, 1.0)
            } else {
                1.0
            },
        }
    });

    let crossfade_bars = (t > 0.55).then(|| {
        let bar_t = ((t - 0.55) / 0.45).clamp(0.0, 1.0);
        let policy = style.quota_layout_policy();
        let bars_layout = layout(&IconInput {
            primary: Some(100.0),
            secondary: Some(100.0),
            credits_remaining: None,
            stale: false,
            status: StatusLevel::None,
            reserves_missing_secondary_lane: policy.reserves_missing_secondary_lane,
            treats_exhausted_secondary_as_missing: policy.treats_exhausted_secondary_as_missing,
            style,
            hide_critters,
            blink: 0.0,
            wiggle: 0.0,
            tilt: 0.0,
        });
        (bars_layout, bar_t)
    });

    MorphLayout {
        ribbons,
        crossfade_bars,
    }
}

/// Loading animation waveform patterns (`LoadingPattern.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LoadingPattern {
    #[default]
    KnightRider,
    Cylon,
    OutsideIn,
    Race,
    Pulse,
    Unbraid,
}

impl LoadingPattern {
    pub const ALL: [Self; 6] = [
        Self::KnightRider,
        Self::Cylon,
        Self::OutsideIn,
        Self::Race,
        Self::Pulse,
        Self::Unbraid,
    ];

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::KnightRider => "Knight Rider",
            Self::Cylon => "Cylon",
            Self::OutsideIn => "Outside-In",
            Self::Race => "Race",
            Self::Pulse => "Pulse",
            Self::Unbraid => "Unbraid (logo → bars)",
        }
    }

    /// Phase offset applied to the lower bar (`LoadingPattern.secondaryOffset`).
    pub const fn secondary_offset(self) -> f64 {
        match self {
            Self::KnightRider | Self::OutsideIn => PI,
            Self::Cylon | Self::Pulse | Self::Unbraid => PI / 2.0,
            Self::Race => PI / 3.0,
        }
    }

    /// Evaluates the waveform in `0.0..=100.0` at `phase` radians (`LoadingPattern.value(phase:)`).
    pub fn value(self, phase: f64) -> f64 {
        let two_pi = PI * 2.0;
        let v = match self {
            Self::KnightRider | Self::Unbraid => 0.5 + 0.5 * phase.sin(),
            Self::Cylon => (phase % two_pi) / two_pi,
            Self::OutsideIn => phase.cos().abs(),
            Self::Race => ((phase * 1.2) % two_pi) / two_pi,
            Self::Pulse => 0.4 + 0.6 * (0.5 + 0.5 * phase.sin()),
        };
        (v * 100.0).clamp(0.0, 100.0)
    }

    /// Effective loading pattern for a provider (`StatusItemController+Animation.swift:408-410`):
    /// Claude's glyph and merged combined icons fall back from `.unbraid` to `.cylon`.
    pub fn effective_for(self, style: IconStyle, merged: bool, provider_id: &str) -> Self {
        if self == Self::Unbraid
            && ((merged && style == IconStyle::Combined) || (!merged && provider_id == "claude"))
        {
            Self::Cylon
        } else {
            self
        }
    }
}

/// Loading-phase blink amount (`StatusItemController+Animation.swift:509-513`):
/// Warp pulses its eye blink at `sin(phase * 3)` during loading animations.
pub fn loading_blink_amount(style: IconStyle, phase: Option<f64>, fallback_blink: f64) -> f64 {
    match (style, phase) {
        (IconStyle::Warp, Some(p)) => (((p * 3.0).sin() + 1.0) / 2.0).clamp(0.0, 1.0),
        _ => fallback_blink,
    }
}

/// Quota warning threshold sanitization and resolution (`CodexBarConfig.swift:222-265`).
pub struct QuotaWarningThresholds;

impl QuotaWarningThresholds {
    pub fn clamped(value: i32) -> i32 {
        value.clamp(1, 99)
    }

    /// Clamps each threshold to `1..=99`, deduplicates, and sorts descending (`[50, 20]`).
    pub fn sanitized(values: &[i32]) -> Vec<i32> {
        let mut out: Vec<i32> = values.iter().copied().map(Self::clamped).collect();
        out.sort_unstable_by(|a, b| b.cmp(a));
        out.dedup();
        out
    }

    /// Returns sanitized thresholds, or `[50, 20]` when `values` is `None` or empty.
    pub fn active(values: Option<&[i32]>) -> Vec<i32> {
        let Some(values) = values else {
            return DEFAULT_QUOTA_WARNING_THRESHOLDS.to_vec();
        };
        let clean = Self::sanitized(values);
        if clean.is_empty() {
            DEFAULT_QUOTA_WARNING_THRESHOLDS.to_vec()
        } else {
            clean
        }
    }

    /// Resolves per-provider thresholds when non-empty, otherwise global thresholds (`[50, 20]` default).
    pub fn resolved(
        provider_thresholds: Option<&[i32]>,
        global_thresholds: Option<&[i32]>,
    ) -> Vec<i32> {
        if let Some(p) = provider_thresholds {
            let clean = Self::sanitized(p);
            if !clean.is_empty() {
                return clean;
            }
        }
        Self::active(global_thresholds)
    }
}

/// Whether a quota-warning flash posted at `posted_at_secs` is still active at `now_secs`
/// (`StatusItemController+Animation.swift:584-593`).
pub fn quota_warning_flash_active(posted_at_secs: Option<f64>, now_secs: f64) -> bool {
    posted_at_secs.is_some_and(|posted| posted + QUOTA_WARNING_FLASH_DURATION_SECS > now_secs)
}

/// Relative luminance of an sRGB color with channels in `0.0..=1.0`
/// (`MenuBarLayoutRenderer.relativeLuminance`).
pub fn relative_luminance_srgb(r: f64, g: f64, b: f64) -> f64 {
    fn linear(value: f64) -> f64 {
        let clamped = value.clamp(0.0, 1.0);
        if clamped <= 0.04045 {
            clamped / 12.92
        } else {
            ((clamped + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// Checks whether a provider accent color meets menu-bar contrast against the given AppKit
/// appearance name (`MenuBarLayoutRenderer.meetsAppearanceContrast`).
pub fn meets_appearance_contrast(r: f64, g: f64, b: f64, appearance_name: &str) -> bool {
    let lower = appearance_name.to_ascii_lowercase();
    let is_dark = lower.contains("dark");
    if !is_dark && !lower.contains("aqua") && !lower.contains("light") {
        return false;
    }
    let gray = if is_dark { 0.25 } else { 0.85 };
    let background = relative_luminance_srgb(gray, gray, gray);
    let luminance = relative_luminance_srgb(r, g, b);
    if is_dark {
        (luminance + 0.05) / (background + 0.05) >= 2.0
    } else {
        (background + 0.05) / (luminance + 0.05) >= 2.0
    }
}

/// Resolves the effective provider tint color (`MenuBarLayoutRenderer.effectiveProviderTintColor`).
/// Returns `None` when monochrome template rendering should own the state (disabled `color_by_provider`,
/// highlighted menu item, stale snapshot, high-contrast mode, accessibility appearance, or insufficient contrast).
pub fn effective_provider_tint_rgb(
    accent_rgb: (u8, u8, u8),
    color_by_provider: bool,
    is_highlighted: bool,
    is_stale: bool,
    high_contrast: bool,
    appearance_name: &str,
) -> Option<(u8, u8, u8)> {
    if !color_by_provider
        || is_highlighted
        || is_stale
        || high_contrast
        || appearance_name
            .to_ascii_lowercase()
            .contains("accessibility")
    {
        return None;
    }
    let (r, g, b) = (
        f64::from(accent_rgb.0) / 255.0,
        f64::from(accent_rgb.1) / 255.0,
        f64::from(accent_rgb.2) / 255.0,
    );
    meets_appearance_contrast(r, g, b, appearance_name).then_some(accent_rgb)
}
