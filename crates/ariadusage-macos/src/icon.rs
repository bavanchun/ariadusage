// Ported from CodexBar Sources/CodexBar/IconRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/LoadingPattern.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarDisplayMode.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarDisplayText.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarLayoutRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/ProviderBrandIcon.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SettingsStore+MenuPreferences.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Animation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SessionQuotaNotifications.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+QuotaWarnings.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfig.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Providers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderBranding.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Status-item icon geometry for both the "bars" and "critters" styles,
//! loading-animation waveforms, critter blink/wiggle/tilt state machine,
//! quota-warning thresholds and flash state, and provider tint contrast checks.
//!
//! The icon is an 18 × 18 pt template image rendered at 2×, so every pixel
//! coordinate here is on a 36 × 36 canvas with the origin at the bottom left
//! (AppKit's default, unflipped). This module only decides what to draw; the
//! AppKit bridge turns the result into an `NSImage`.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;
use std::hash::{Hash, Hasher};

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
            "claude" => Self::Claude,
            "gemini" => Self::Gemini,
            "antigravity" => Self::Antigravity,
            "cursor" => Self::Cursor,
            "factory" => Self::Factory,
            "grok" => Self::Grok,
            "copilot" => Self::Copilot,
            "warp" => Self::Warp,
            _ => Self::Combined,
        }
    }

    /// Critter decorations associated with this style (`ProviderUsagePresentation.iconDecorations`).
    pub const fn decorations(self) -> IconDecorations {
        match self {
            Self::Codex => IconDecorations {
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
            Self::Copilot | Self::Cursor | Self::Combined => IconDecorations::empty(),
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

/// Hat rectangle and tilt transform for the `.face` critter (Codex).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HatDecoration {
    pub rect: RectPx,
    pub pivot: PointPt,
    pub tilt_radians: f64,
    pub y_offset_pt: f64,
}

/// Eye cutouts, blink refills, and hat for the `.face` critter (Codex).
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
    /// When no provider is resolved (`provider_id` is `None`), returns [`Self::default()`].
    pub fn effective_for(self, style: IconStyle, merged: bool, provider_id: Option<&str>) -> Self {
        let Some(provider_id) = provider_id else {
            return Self::default();
        };
        if self == Self::Unbraid
            && ((merged && style == IconStyle::Combined) || (!merged && provider_id == "claude"))
        {
            Self::Cylon
        } else {
            self
        }
    }
}

/// Computes the primary and secondary loading percentages at `phase`, clamping each lane
/// to [`LOADING_PERCENT_EPSILON`] so `0.0` never flips the two-bar layout into the
/// weekly-exhausted single-bar layout (`StatusItemController+Animation.swift:419-422`).
pub fn loading_frame_percents(pattern: LoadingPattern, phase: f64) -> (f64, f64) {
    let primary = pattern.value(phase).max(LOADING_PERCENT_EPSILON);
    let secondary = pattern
        .value(phase + pattern.secondary_offset())
        .max(LOADING_PERCENT_EPSILON);
    (primary, secondary)
}

/// Frame rate of the status-item loading animation (`StatusItemController+Animation.swift:129`).
pub const LOADING_ANIMATION_FPS: f64 = 30.0;

/// Phase increment in radians per 30 FPS tick (`StatusItemController+Animation.swift:130`).
pub const LOADING_ANIMATION_PHASE_INCREMENT: f64 = 2.7 / LOADING_ANIMATION_FPS;

/// Maximum continuous duration in seconds before the loading animation caps and falls back
/// to the static icon (`StatusItemController+Animation.swift:131`).
pub const LOADING_ANIMATION_MAX_CONTINUOUS_DURATION_SECS: f64 = 30.0;

/// Active tick interval in milliseconds while a critter blink/wiggle/tilt is animating
/// (`StatusItemController+Animation.swift:7`: `.milliseconds(75)`).
pub const BLINK_ACTIVE_TICK_INTERVAL_MS: u64 = 75;

/// Fallback idle sleep interval in milliseconds when no blink is pending
/// (`StatusItemController+Animation.swift:8`: `.seconds(1)`).
pub const BLINK_IDLE_FALLBACK_INTERVAL_MS: u64 = 1000;

/// Duration in seconds of one critter blink/wiggle/tilt pulse (`StatusItemController+Animation.swift:9`).
pub const BLINK_DURATION_SECS: f64 = 0.36;

/// Probability (`0.18`) that a scheduled blink triggers a double blink (`StatusItemController+Animation.swift:10`).
pub const DOUBLE_BLINK_CHANCE: f64 = 0.18;

/// Minimum delay in seconds before the second pulse of a double blink (`StatusItemController+Animation.swift:11`).
pub const DOUBLE_BLINK_DELAY_MIN_SECS: f64 = 0.22;

/// Maximum delay in seconds before the second pulse of a double blink (`StatusItemController+Animation.swift:11`).
pub const DOUBLE_BLINK_DELAY_MAX_SECS: f64 = 0.34;

/// Minimum random delay in seconds between idle critter blinks (`StatusItemController+Animation.swift:12`).
pub const BLINK_RANDOM_DELAY_MIN_SECS: f64 = 3.0;

/// Maximum random delay in seconds between idle critter blinks (`StatusItemController+Animation.swift:12`).
pub const BLINK_RANDOM_DELAY_MAX_SECS: f64 = 12.0;

/// Whether the loading animation has exceeded its 30 s continuous cap (`StatusItemController+Animation.swift:139-143`).
pub fn loading_animation_has_exceeded_continuous_cap(
    started_at_secs: Option<f64>,
    now_secs: f64,
) -> bool {
    started_at_secs
        .is_some_and(|start| now_secs - start > LOADING_ANIMATION_MAX_CONTINUOUS_DURATION_SECS)
}

/// Loading-phase blink amount (`StatusItemController+Animation.swift:509-513`):
/// Warp pulses its eye blink at `sin(phase * 3)` during loading animations.
pub fn loading_blink_amount(style: IconStyle, phase: Option<f64>, fallback_blink: f64) -> f64 {
    match (style, phase) {
        (IconStyle::Warp, Some(p)) => (((p * 3.0).sin() + 1.0) / 2.0).clamp(0.0, 1.0),
        _ => fallback_blink,
    }
}

/// Quota warning threshold sanitization and resolution (`CodexBarConfig.swift:483-511`).
pub struct QuotaWarningThresholds;

impl QuotaWarningThresholds {
    /// Clamps a threshold to `0..=99` (`QuotaWarningThresholds.clamped`).
    pub fn clamped(value: i32) -> i32 {
        value.clamp(0, 99)
    }

    /// Clamps each threshold to `0..=99`, deduplicates, and sorts descending (`[50, 20]` when `values` is empty).
    pub fn sanitized(values: &[i32]) -> Vec<i32> {
        if values.is_empty() {
            return DEFAULT_QUOTA_WARNING_THRESHOLDS.to_vec();
        }
        let mut out: Vec<i32> = values.iter().copied().map(Self::clamped).collect();
        out.sort_unstable_by(|a, b| b.cmp(a));
        out.dedup();
        if out.is_empty() {
            DEFAULT_QUOTA_WARNING_THRESHOLDS.to_vec()
        } else {
            out
        }
    }

    /// Returns sanitized positive thresholds (`> 0`), or `[50, 20]` when `values` is `None` (`QuotaWarningThresholds.active`).
    pub fn active(values: Option<&[i32]>) -> Vec<i32> {
        let Some(values) = values else {
            return DEFAULT_QUOTA_WARNING_THRESHOLDS.to_vec();
        };
        let mut clean = Self::sanitized(values);
        clean.retain(|&t| t > 0);
        clean
    }

    /// Resolves upper/lower threshold pair (`QuotaWarningThresholds.resolved(upper:lower:)`).
    pub fn resolved_pair(upper: Option<i32>, lower: Option<i32>) -> Vec<i32> {
        if upper.is_none() && lower.is_none() {
            return DEFAULT_QUOTA_WARNING_THRESHOLDS.to_vec();
        }
        let resolved_upper = Self::clamped(upper.unwrap_or(DEFAULT_QUOTA_WARNING_THRESHOLDS[0]));
        let lower_default = if resolved_upper < DEFAULT_QUOTA_WARNING_THRESHOLDS[1] {
            0
        } else {
            DEFAULT_QUOTA_WARNING_THRESHOLDS[1]
        };
        let resolved_lower = Self::clamped(lower.unwrap_or(lower_default));
        Self::sanitized(&[resolved_upper, resolved_lower])
    }

    /// Resolves per-provider thresholds when non-empty, otherwise global thresholds (`[50, 20]` default).
    pub fn resolved(
        provider_thresholds: Option<&[i32]>,
        global_thresholds: Option<&[i32]>,
    ) -> Vec<i32> {
        if let Some(p) = provider_thresholds
            && !p.is_empty()
        {
            return Self::sanitized(p);
        }
        match global_thresholds {
            Some(g) => Self::sanitized(g),
            None => DEFAULT_QUOTA_WARNING_THRESHOLDS.to_vec(),
        }
    }
}

/// Pure threshold crossing and reset logic (`QuotaWarningNotificationLogic` in `SessionQuotaNotifications.swift:372-398`).
pub struct QuotaWarningNotificationLogic;

impl QuotaWarningNotificationLogic {
    /// Returns the lowest crossed threshold when `current_remaining` drops to or below an unfired threshold.
    pub fn crossed_threshold(
        previous_remaining: Option<f64>,
        current_remaining: f64,
        thresholds: &[i32],
        already_fired: &BTreeSet<i32>,
    ) -> Option<i32> {
        let sanitized = QuotaWarningThresholds::active(Some(thresholds));
        let eligible: Vec<i32> = sanitized
            .into_iter()
            .filter(|&t| current_remaining <= f64::from(t) && !already_fired.contains(&t))
            .collect();
        if eligible.is_empty() {
            return None;
        }
        if let Some(prev) = previous_remaining {
            eligible.into_iter().filter(|&t| prev > f64::from(t)).min()
        } else {
            eligible.into_iter().min()
        }
    }

    /// Returns all active thresholds `>= threshold` that should be marked as fired after warning at `threshold`.
    pub fn fired_thresholds_after_warning(threshold: i32, thresholds: &[i32]) -> BTreeSet<i32> {
        QuotaWarningThresholds::active(Some(thresholds))
            .into_iter()
            .filter(|&t| t >= threshold)
            .collect()
    }

    /// Returns fired thresholds that should be cleared because `current_remaining` recovered above them.
    pub fn thresholds_to_clear(
        current_remaining: f64,
        already_fired: &BTreeSet<i32>,
    ) -> BTreeSet<i32> {
        already_fired
            .iter()
            .copied()
            .filter(|&t| current_remaining > f64::from(t))
            .collect()
    }
}

/// Per-window state for quota-warning threshold crossing (`UsageStore+QuotaWarnings.swift:61-106`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuotaWarningWindowState {
    pub previous_remaining: Option<f64>,
    pub fired_thresholds: BTreeSet<i32>,
    pub last_resets_at_secs: Option<i64>,
}

impl QuotaWarningWindowState {
    /// Evaluates a window update (`remaining_percent` and optional `resets_at_secs`) against `thresholds`.
    /// Returns `Some(crossed_threshold)` when a warning should fire.
    pub fn update(
        &mut self,
        remaining_percent: Option<f64>,
        resets_at_secs: Option<i64>,
        thresholds: &[i32],
        warnings_enabled: bool,
    ) -> Option<i32> {
        let Some(remaining) = remaining_percent else {
            *self = Self::default();
            return None;
        };
        if let (Some(prev_reset), Some(curr_reset)) = (self.last_resets_at_secs, resets_at_secs)
            && (prev_reset - curr_reset).abs() > 1
        {
            self.fired_thresholds.clear();
            self.previous_remaining = None;
        }
        self.last_resets_at_secs = resets_at_secs;

        let active_set: BTreeSet<i32> = QuotaWarningThresholds::active(Some(thresholds))
            .into_iter()
            .collect();
        self.fired_thresholds.retain(|t| active_set.contains(t));

        let cleared =
            QuotaWarningNotificationLogic::thresholds_to_clear(remaining, &self.fired_thresholds);
        for t in cleared {
            self.fired_thresholds.remove(&t);
        }

        if !warnings_enabled {
            self.previous_remaining = Some(remaining);
            self.fired_thresholds.clear();
            return None;
        }

        let crossed = QuotaWarningNotificationLogic::crossed_threshold(
            self.previous_remaining,
            remaining,
            thresholds,
            &self.fired_thresholds,
        );
        if let Some(threshold) = crossed {
            let mark = QuotaWarningNotificationLogic::fired_thresholds_after_warning(
                threshold, thresholds,
            );
            self.fired_thresholds.extend(mark);
        }
        self.previous_remaining = Some(remaining);
        crossed
    }
}

/// Tracks quota-warning state across providers and their primary/secondary lanes.
#[derive(Debug, Clone, Default)]
pub struct QuotaWarningTracker {
    pub primary: BTreeMap<String, QuotaWarningWindowState>,
    pub secondary: BTreeMap<String, QuotaWarningWindowState>,
}

impl QuotaWarningTracker {
    /// Updates both primary and secondary windows for `provider_id` and returns `true`
    /// if either window crossed a warning threshold.
    #[allow(clippy::too_many_arguments)]
    pub fn update_provider(
        &mut self,
        provider_id: &str,
        primary_remaining: Option<f64>,
        primary_resets_at_secs: Option<i64>,
        secondary_remaining: Option<f64>,
        secondary_resets_at_secs: Option<i64>,
        thresholds: &[i32],
        warnings_enabled: bool,
    ) -> bool {
        let p_fired = self
            .primary
            .entry(provider_id.to_string())
            .or_default()
            .update(
                primary_remaining,
                primary_resets_at_secs,
                thresholds,
                warnings_enabled,
            )
            .is_some();
        let s_fired = self
            .secondary
            .entry(provider_id.to_string())
            .or_default()
            .update(
                secondary_remaining,
                secondary_resets_at_secs,
                thresholds,
                warnings_enabled,
            )
            .is_some();
        p_fired || s_fired
    }
}

/// Whether a quota-warning flash posted at `posted_at_secs` is still active at `now_secs`
/// (`StatusItemController+Animation.swift:584-593`).
pub fn quota_warning_flash_active(posted_at_secs: Option<f64>, now_secs: f64) -> bool {
    posted_at_secs.is_some_and(|posted| posted + QUOTA_WARNING_FLASH_DURATION_SECS > now_secs)
}

/// Critter motion effect selected for a blink pulse (`StatusItemController.MotionEffect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MotionEffect {
    #[default]
    Blink,
    Wiggle,
    Tilt,
}

/// Per-provider critter blink schedule and active pulse state (`StatusItemController.BlinkState`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlinkState {
    pub next_blink_secs: f64,
    pub blink_start_secs: Option<f64>,
    pub pending_second_start_secs: Option<f64>,
    pub effect: MotionEffect,
}

impl BlinkState {
    /// Creates a new `BlinkState` using a deterministic seed for initial delay (`3.0..=12.0` s).
    pub fn seeded(now_secs: f64, seed: u64) -> Self {
        let delay = seeded_range(
            seed ^ 0x9E37_79B9_7F4A_7C15,
            BLINK_RANDOM_DELAY_MIN_SECS,
            BLINK_RANDOM_DELAY_MAX_SECS,
        );
        Self {
            next_blink_secs: now_secs + delay,
            blink_start_secs: None,
            pending_second_start_secs: None,
            effect: MotionEffect::Blink,
        }
    }
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn seeded_unit(seed: u64) -> f64 {
    let bits = splitmix64(seed) >> 11;
    (bits as f64) * (1.0 / ((1u64 << 53) as f64))
}

fn seeded_range(seed: u64, min: f64, max: f64) -> f64 {
    min + (max - min) * seeded_unit(seed)
}

/// Whether a provider's icon style supports critter blinking (`StatusItemController.shouldBlink`).
pub fn style_supports_blink(style: IconStyle, hide_critters: bool) -> bool {
    if hide_critters {
        return false;
    }
    let dec = style.decorations();
    dec.face || dec.notches || dec.gemini || dec.factory || dec.warp
}

/// Selects the motion effect for `style` using a deterministic random roll (`StatusItemController.motionEffect`).
pub fn select_motion_effect(style: IconStyle, roll: f64) -> MotionEffect {
    let dec = style.decorations();
    if dec.notches {
        if roll < 0.35 {
            MotionEffect::Wiggle
        } else {
            MotionEffect::Blink
        }
    } else if dec.face {
        if roll < 0.25 {
            MotionEffect::Tilt
        } else {
            MotionEffect::Blink
        }
    } else {
        MotionEffect::Blink
    }
}

/// Evaluates whether critter blinking is currently allowed (`StatusItemController.isBlinkingAllowed`).
pub fn is_blinking_allowed(
    random_blink_enabled: bool,
    blink_force_until_secs: Option<f64>,
    now_secs: f64,
) -> bool {
    if random_blink_enabled {
        return true;
    }
    blink_force_until_secs.is_some_and(|until| until > now_secs)
}

/// Evaluates whether the critter blink task should run (`StatusItemController.shouldRunBlinkTask`).
pub fn should_run_blink_task(
    blinking_allowed: bool,
    needs_menu_bar_icon_animation: bool,
    hide_critters: bool,
    has_blinking_providers: bool,
) -> bool {
    blinking_allowed && !needs_menu_bar_icon_animation && !hide_critters && has_blinking_providers
}

/// Computes the sleep duration in milliseconds before the next blink tick
/// (`StatusItemController.blinkTickSleepDuration`, `StatusItemController+Animation.swift:164-184`).
pub fn blink_tick_sleep_duration_ms(
    states: &BTreeMap<String, BlinkState>,
    providers: &[String],
    now_secs: f64,
) -> u64 {
    let mut earliest_wake: Option<f64> = None;
    for provider in providers {
        let Some(state) = states.get(provider) else {
            return BLINK_ACTIVE_TICK_INTERVAL_MS;
        };
        if state.blink_start_secs.is_some() {
            return BLINK_ACTIVE_TICK_INTERVAL_MS;
        }
        let candidate = state
            .pending_second_start_secs
            .map_or(state.next_blink_secs, |pending| {
                pending.min(state.next_blink_secs)
            });
        earliest_wake = Some(earliest_wake.map_or(candidate, |cur| cur.min(candidate)));
    }
    let Some(earliest) = earliest_wake else {
        return BLINK_IDLE_FALLBACK_INTERVAL_MS;
    };
    let delay_secs = (earliest - now_secs).max(0.0);
    if delay_secs <= (BLINK_ACTIVE_TICK_INTERVAL_MS as f64) / 1000.0 {
        BLINK_ACTIVE_TICK_INTERVAL_MS
    } else {
        (delay_secs * 1000.0).ceil() as u64
    }
}

/// Pure state machine for status-item loading animation, critter blink/wiggle/tilt,
/// quota-warning flash windows, and render deduplication signatures
/// (`StatusItemController+Animation.swift`).
#[derive(Debug, Clone, Default)]
pub struct StatusItemAnimationState {
    pub animation_phase: f64,
    pub loading_started_at_secs: Option<f64>,
    pub loading_capped: bool,
    pub debug_force_animation: bool,
    pub blink_states: BTreeMap<String, BlinkState>,
    pub blink_amounts: BTreeMap<String, f64>,
    pub wiggle_amounts: BTreeMap<String, f64>,
    pub tilt_amounts: BTreeMap<String, f64>,
    pub blink_force_until_secs: Option<f64>,
    pub blink_rng_counter: u64,
    pub quota_warning_flash_until_secs: BTreeMap<String, f64>,
    pub quota_warning_tracker: QuotaWarningTracker,
    pub last_merged_render_signature: Option<u64>,
    pub last_rendered_signatures: BTreeMap<String, u64>,
    pub last_store_observation_signature: Option<u64>,
}

impl StatusItemAnimationState {
    /// Synchronizes the loading animation start/cap state at `now_secs`.
    /// Returns `true` when the 30 FPS loading timer should be running.
    pub fn sync_loading_animation(&mut self, needs_animation: bool, now_secs: f64) -> bool {
        self.sync_loading_animation_with_any_loading(needs_animation, needs_animation, now_secs)
    }

    /// Synchronizes the loading animation start/cap state at `now_secs`, retaining the
    /// 30 s continuous cap marker when `needs_animation` is `false` (for example, after
    /// switching the selected provider in merged mode) while `any_enabled_loading` is still `true`
    /// (`StatusItemController.updateAnimationState`, `StatusItemController+Animation.swift:1256-1272`).
    pub fn sync_loading_animation_with_any_loading(
        &mut self,
        needs_animation: bool,
        any_enabled_loading: bool,
        now_secs: f64,
    ) -> bool {
        if !needs_animation {
            let keep_cap_marker = any_enabled_loading && self.loading_capped;
            self.loading_started_at_secs = None;
            self.loading_capped = keep_cap_marker;
            self.animation_phase = 0.0;
            return false;
        }
        if self.loading_capped {
            return false;
        }
        let start = *self.loading_started_at_secs.get_or_insert(now_secs);
        if loading_animation_has_exceeded_continuous_cap(Some(start), now_secs) {
            self.loading_started_at_secs = None;
            self.loading_capped = true;
            self.animation_phase = 0.0;
            return false;
        }
        true
    }

    /// Returns `true` when a merged-icon render should be skipped because `signature` matches
    /// the last applied merged signature (`StatusItemController.shouldSkipMergedIconRender`,
    /// `StatusItemController+Animation.swift:306-316`).
    pub fn should_skip_merged_icon_render(&mut self, signature: u64, merge_icons: bool) -> bool {
        if !merge_icons {
            self.last_merged_render_signature = Some(signature);
            return false;
        }
        if self.last_merged_render_signature == Some(signature) {
            return true;
        }
        self.last_merged_render_signature = Some(signature);
        false
    }

    /// Returns `true` when a split per-provider icon render should be skipped because `signature`
    /// matches the last applied signature for `provider_id`
    /// (`StatusItemController.shouldSkipProviderIconRender`, `StatusItemController+Animation.swift:318-324`).
    pub fn should_skip_provider_icon_render(&mut self, provider_id: &str, signature: u64) -> bool {
        if self.last_rendered_signatures.get(provider_id).copied() == Some(signature) {
            return true;
        }
        self.last_rendered_signatures
            .insert(provider_id.to_string(), signature);
        false
    }

    /// Advances the loading animation by one 30 FPS frame at `now_secs`.
    /// Returns `true` if still animating, or `false` if the 30 s continuous cap was reached.
    pub fn advance_loading_frame(&mut self, needs_animation: bool, now_secs: f64) -> bool {
        if !self.sync_loading_animation(needs_animation, now_secs) {
            return false;
        }
        self.animation_phase += LOADING_ANIMATION_PHASE_INCREMENT;
        true
    }

    /// Current loading phase if the loading animation is active and uncapped.
    pub fn active_loading_phase(&self, needs_animation: bool) -> Option<f64> {
        (needs_animation && !self.loading_capped && self.loading_started_at_secs.is_some())
            .then_some(self.animation_phase)
    }

    /// Starts or refreshes a 60 s quota-warning flash for `provider_id` at `now_secs`
    /// (`StatusItemController+Animation.swift:595-611`).
    pub fn start_quota_warning_flash(&mut self, provider_id: &str, now_secs: f64) -> f64 {
        let until = now_secs + QUOTA_WARNING_FLASH_DURATION_SECS;
        self.quota_warning_flash_until_secs
            .insert(provider_id.to_string(), until);
        until
    }

    /// Clears expired quota-warning flashes at `now_secs` and returns the provider IDs whose flash expired.
    pub fn clear_expired_quota_warning_flashes(&mut self, now_secs: f64) -> Vec<String> {
        let expired: Vec<String> = self
            .quota_warning_flash_until_secs
            .iter()
            .filter(|&(_, &until)| until <= now_secs)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.quota_warning_flash_until_secs.remove(id);
        }
        expired
    }

    /// Whether a quota-warning flash is currently active for `provider_id` at `now_secs`.
    pub fn is_quota_warning_flash_active(&self, provider_id: &str, now_secs: f64) -> bool {
        self.quota_warning_flash_until_secs
            .get(provider_id)
            .is_some_and(|&until| until > now_secs)
    }

    /// Forces an immediate critter blink across `providers` for 0.6 s (`StatusItemController.forceBlinkNow`).
    pub fn force_blink_now(
        &mut self,
        providers: &[String],
        hide_critters: bool,
        now_secs: f64,
    ) -> Vec<String> {
        if hide_critters {
            return Vec::new();
        }
        self.blink_force_until_secs = Some(now_secs + 0.6);
        let mut updated = Vec::new();
        for provider in providers {
            let style = IconStyle::from_provider_id(provider);
            if !style_supports_blink(style, hide_critters) {
                continue;
            }
            self.blink_rng_counter = self.blink_rng_counter.wrapping_add(1);
            let seed = self.blink_rng_counter;
            let roll = seeded_unit(seed);
            let mut state = self
                .blink_states
                .get(provider)
                .copied()
                .unwrap_or_else(|| BlinkState::seeded(now_secs, seed));
            state.blink_start_secs = Some(now_secs);
            state.pending_second_start_secs = None;
            state.effect = select_motion_effect(style, roll);
            state.next_blink_secs = now_secs
                + seeded_range(
                    seed ^ 0x517C_C1B7_2722_0A95,
                    BLINK_RANDOM_DELAY_MIN_SECS,
                    BLINK_RANDOM_DELAY_MAX_SECS,
                );
            self.blink_states.insert(provider.clone(), state);
            self.assign_motion(0.0, provider, state.effect);
            updated.push(provider.clone());
        }
        updated
    }

    /// Ticks the critter blink state machine at `now_secs` (`StatusItemController.tickBlink`).
    /// Returns the provider IDs whose icon state changed and needs redrawing.
    pub fn tick_blink(
        &mut self,
        providers: &[String],
        random_blink_enabled: bool,
        hide_critters: bool,
        now_secs: f64,
    ) -> Vec<String> {
        let allowed =
            is_blinking_allowed(random_blink_enabled, self.blink_force_until_secs, now_secs);
        if !allowed || hide_critters {
            let had_motion = self.has_any_motion();
            self.clear_motion();
            return if had_motion {
                providers.to_vec()
            } else {
                Vec::new()
            };
        }

        let mut redraw = Vec::new();
        for provider in providers {
            let style = IconStyle::from_provider_id(provider);
            if !style_supports_blink(style, hide_critters) {
                self.clear_motion_for(provider);
                continue;
            }

            self.blink_rng_counter = self.blink_rng_counter.wrapping_add(1);
            let seed = self.blink_rng_counter;
            let mut state = self
                .blink_states
                .get(provider)
                .copied()
                .unwrap_or_else(|| BlinkState::seeded(now_secs, seed));

            if let Some(pending_second) = state.pending_second_start_secs
                && now_secs >= pending_second
            {
                state.blink_start_secs = Some(now_secs);
                state.pending_second_start_secs = None;
            }

            if let Some(start) = state.blink_start_secs {
                let elapsed = now_secs - start;
                if elapsed >= BLINK_DURATION_SECS {
                    state.blink_start_secs = None;
                    if state.pending_second_start_secs.is_none() {
                        state.next_blink_secs = now_secs
                            + seeded_range(
                                seed ^ 0xA5A5_5A5A_F0F0_0F0F,
                                BLINK_RANDOM_DELAY_MIN_SECS,
                                BLINK_RANDOM_DELAY_MAX_SECS,
                            );
                    }
                    self.assign_motion(0.0, provider, state.effect);
                    redraw.push(provider.clone());
                } else {
                    let progress = (elapsed / BLINK_DURATION_SECS).clamp(0.0, 1.0);
                    let symmetric = if progress < 0.5 {
                        progress * 2.0
                    } else {
                        (1.0 - progress) * 2.0
                    };
                    let eased = symmetric.powf(2.2);
                    self.assign_motion(eased, provider, state.effect);
                    redraw.push(provider.clone());
                }
            } else if now_secs >= state.next_blink_secs {
                state.blink_start_secs = Some(now_secs);
                state.effect = select_motion_effect(style, seeded_unit(seed ^ 1));
                if state.effect == MotionEffect::Blink
                    && seeded_unit(seed ^ 2) < DOUBLE_BLINK_CHANCE
                {
                    state.pending_second_start_secs = Some(
                        now_secs
                            + seeded_range(
                                seed ^ 3,
                                DOUBLE_BLINK_DELAY_MIN_SECS,
                                DOUBLE_BLINK_DELAY_MAX_SECS,
                            ),
                    );
                }
                self.assign_motion(0.0, provider, state.effect);
                redraw.push(provider.clone());
            } else {
                self.clear_motion_for(provider);
            }

            self.blink_states.insert(provider.clone(), state);
        }
        redraw
    }

    pub fn clear_motion(&mut self) {
        self.blink_amounts.clear();
        self.wiggle_amounts.clear();
        self.tilt_amounts.clear();
    }

    pub fn clear_motion_for(&mut self, provider_id: &str) {
        self.blink_amounts.remove(provider_id);
        self.wiggle_amounts.remove(provider_id);
        self.tilt_amounts.remove(provider_id);
    }

    pub fn has_any_motion(&self) -> bool {
        self.blink_amounts.values().any(|&v| v > 0.0)
            || self.wiggle_amounts.values().any(|&v| v > 0.0)
            || self.tilt_amounts.values().any(|&v| v.abs() > 0.0)
    }

    fn assign_motion(&mut self, amount: f64, provider_id: &str, effect: MotionEffect) {
        let clamped = amount.clamp(0.0, 1.0);
        match effect {
            MotionEffect::Blink => {
                self.blink_amounts.insert(provider_id.to_string(), clamped);
                self.wiggle_amounts.remove(provider_id);
                self.tilt_amounts.remove(provider_id);
            }
            MotionEffect::Wiggle => {
                self.wiggle_amounts.insert(provider_id.to_string(), clamped);
                self.blink_amounts.remove(provider_id);
                self.tilt_amounts.remove(provider_id);
            }
            MotionEffect::Tilt => {
                let limit = PI / 28.0;
                self.tilt_amounts
                    .insert(provider_id.to_string(), clamped * limit);
                self.blink_amounts.remove(provider_id);
                self.wiggle_amounts.remove(provider_id);
            }
        }
    }

    /// Returns `(blink, wiggle, tilt)` for `provider_id`.
    pub fn motion_for(&self, provider_id: &str) -> (f64, f64, f64) {
        (
            self.blink_amounts.get(provider_id).copied().unwrap_or(0.0),
            self.wiggle_amounts.get(provider_id).copied().unwrap_or(0.0),
            self.tilt_amounts.get(provider_id).copied().unwrap_or(0.0),
        )
    }
}

/// Computes the render deduplication signature for a status item icon
/// (`StatusItemController.iconRenderSignature`, `StatusItemController+Animation.swift:415-456`).
pub fn icon_render_signature(
    input: &IconInput,
    show_used: bool,
    color_by_provider: bool,
    phase: Option<f64>,
    loading_pattern: LoadingPattern,
    quota_warning_flash_active: bool,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.style.hash(&mut hasher);
    quantize_opt(input.primary, 10_000.0).hash(&mut hasher);
    quantize_opt(input.secondary, 10_000.0).hash(&mut hasher);
    quantize_opt(input.credits_remaining, 100.0).hash(&mut hasher);
    input.stale.hash(&mut hasher);
    (input.status as u8).hash(&mut hasher);
    quantize(input.blink, 10_000.0).hash(&mut hasher);
    quantize(input.wiggle, 10_000.0).hash(&mut hasher);
    quantize(input.tilt, 10_000.0).hash(&mut hasher);
    show_used.hash(&mut hasher);
    input.hide_critters.hash(&mut hasher);
    color_by_provider.hash(&mut hasher);
    phase.map(|p| (p * 240.0).round() as i64).hash(&mut hasher);
    loading_pattern.hash(&mut hasher);
    quota_warning_flash_active.hash(&mut hasher);
    input.reserves_missing_secondary_lane.hash(&mut hasher);
    input
        .treats_exhausted_secondary_as_missing
        .hash(&mut hasher);
    hasher.finish()
}

fn quantize(value: f64, scale: f64) -> i64 {
    (value * scale).round() as i64
}

fn quantize_opt(value: Option<f64>, scale: f64) -> Option<i64> {
    value.map(|v| quantize(v, scale))
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

/// Menu-bar status item icon style (`MenuBarIconStyle`, `SettingsStore+MenuPreferences.swift:4-16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MenuBarIconStyle {
    #[default]
    Critters,
    Bars,
    IconAndPercent,
}

impl MenuBarIconStyle {
    pub const fn raw_value(self) -> &'static str {
        match self {
            Self::Critters => "critters",
            Self::Bars => "bars",
            Self::IconAndPercent => "iconAndPercent",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        match raw {
            "critters" => Some(Self::Critters),
            "bars" => Some(Self::Bars),
            "iconAndPercent" => Some(Self::IconAndPercent),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Critters => "Critters",
            Self::Bars => "Bars",
            Self::IconAndPercent => "Icon & percent",
        }
    }

    /// Whether the status item renders the provider brand icon paired with a display text label
    /// (`SettingsStore.showsBrandIconWithPercent`, `SettingsStore+MenuPreferences.swift:158-160`).
    pub const fn shows_brand_icon_with_percent(self) -> bool {
        matches!(self, Self::IconAndPercent)
    }

    /// Whether critter decorations should be suppressed on meter bars
    /// (`SettingsStore.hidesCritters`, `SettingsStore+MenuPreferences.swift:162-164`).
    pub const fn hides_critters(self) -> bool {
        matches!(self, Self::Bars)
    }
}

/// Text display mode when [`MenuBarIconStyle::IconAndPercent`] is active
/// (`MenuBarDisplayMode`, `MenuBarDisplayMode.swift:4-31`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MenuBarDisplayMode {
    #[default]
    Percent,
    Pace,
    Both,
    ResetTime,
}

impl MenuBarDisplayMode {
    pub const fn raw_value(self) -> &'static str {
        match self {
            Self::Percent => "percent",
            Self::Pace => "pace",
            Self::Both => "both",
            Self::ResetTime => "resetTime",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        match raw {
            "percent" => Some(Self::Percent),
            "pace" => Some(Self::Pace),
            "both" => Some(Self::Both),
            "resetTime" => Some(Self::ResetTime),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Percent => "Percent",
            Self::Pace => "Pace",
            Self::Both => "Both",
            Self::ResetTime => "Reset time",
        }
    }
}

/// Pure formatting rules for the status item's text label in [`MenuBarIconStyle::IconAndPercent`]
/// mode (`MenuBarDisplayText`, `MenuBarDisplayText.swift:4-220`).
pub struct MenuBarDisplayText;

impl MenuBarDisplayText {
    /// Prepends an optional prefix and strips an optional trailing suffix from `text`
    /// (`MenuBarDisplayText.prefixedValue`, `MenuBarDisplayText.swift:5-22`).
    pub fn prefixed_value(
        text: Option<&str>,
        prefix: Option<&str>,
        removing_suffix: Option<&str>,
    ) -> Option<String> {
        let trimmed = text?.trim();
        if trimmed.is_empty() {
            return None;
        }
        let mut normalized = trimmed.to_string();
        if let Some(removing_suffix) = removing_suffix
            && !removing_suffix.is_empty()
            && normalized.ends_with(removing_suffix)
        {
            normalized.truncate(normalized.len() - removing_suffix.len());
            normalized = normalized.trim().to_string();
        }
        if normalized.is_empty() {
            return None;
        }
        let Some(prefix) = prefix.map(str::trim).filter(|p| !p.is_empty()) else {
            return Some(normalized);
        };
        Some(format!("{prefix} {normalized}"))
    }

    /// Formats a DeepSeek balance cost snapshot as a currency string
    /// (`MenuBarDisplayText.deepSeekBalanceText`, `MenuBarDisplayText.swift:24-36`).
    pub fn deep_seek_balance_text(
        cost: Option<&ariadusage_protocol::usage::Cost>,
    ) -> Option<String> {
        let cost = cost?;
        if !cost.used.is_finite() {
            return None;
        }
        Some(crate::format::currency_string(
            cost.used,
            &cost.currency_code,
        ))
    }

    /// Formats a rate window as a used or remaining percentage string
    /// (`MenuBarDisplayText.percentText(window:showUsed:)`, `MenuBarDisplayText.swift:38-45`).
    pub fn percent_text(
        window: Option<&ariadusage_protocol::usage::RateWindow>,
        show_used: bool,
    ) -> Option<String> {
        Self::percent_text_estimated(window, show_used, false)
    }

    /// Formats a rate window as a used or remaining percentage string, optionally prefixed with `~`
    /// (`MenuBarDisplayText.percentText(window:showUsed:estimated:)`, `MenuBarDisplayText.swift:38-45`).
    pub fn percent_text_estimated(
        window: Option<&ariadusage_protocol::usage::RateWindow>,
        show_used: bool,
        estimated: bool,
    ) -> Option<String> {
        let window = window?;
        let remaining = (100.0 - window.used_percent).clamp(0.0, 100.0);
        let percent = if show_used {
            window.used_percent.clamp(0.0, 100.0)
        } else {
            remaining
        };
        let formatted = crate::format::percent_string(percent);
        if estimated {
            Some(format!("~{formatted}"))
        } else {
            Some(formatted)
        }
    }

    /// Formats a pace delta percentage with an explicit sign
    /// (`MenuBarDisplayText.paceText(deltaPercent:reservePositive:)`, `MenuBarDisplayText.swift:55-60`).
    pub fn pace_text(delta_percent: f64, reserve_positive: bool) -> String {
        let delta_value = delta_percent.abs().round() as i64;
        if delta_value == 0 {
            return "0%".to_string();
        }
        let is_positive = (delta_percent >= 0.0) != reserve_positive;
        let sign = if is_positive { "+" } else { "-" };
        format!("{sign}{delta_value}%")
    }

    /// Short label for a session rate window (`"5h"`, `"30m"`, or `"S"` when duration is unknown)
    /// (`MenuBarDisplayText.sessionWindowLabel`, `MenuBarDisplayText.swift:97-105`).
    pub fn session_window_label(window: Option<&ariadusage_protocol::usage::RateWindow>) -> String {
        let Some(minutes) = window.and_then(|w| w.window_minutes).filter(|&m| m > 0) else {
            return "S".to_string();
        };
        if minutes.is_multiple_of(60) {
            format!("{}h", minutes / 60)
        } else {
            format!("{minutes}m")
        }
    }

    /// Formats combined session + weekly lanes (`"5h 93% · W 82%"`, `"W 82%"`, etc.)
    /// (`MenuBarDisplayText.combinedSessionWeeklyPercentText`, `MenuBarDisplayText.swift:62-95`).
    #[allow(clippy::too_many_arguments)]
    pub fn combined_session_weekly_percent_text(
        session_window: Option<&ariadusage_protocol::usage::RateWindow>,
        weekly_window: Option<&ariadusage_protocol::usage::RateWindow>,
        show_used: bool,
        reset_time_display_style: crate::format::ResetTimeDisplayStyle,
        shows_reset_time_when_exhausted: bool,
        now: jiff::Timestamp,
        dates: &dyn crate::format::DateTexts,
    ) -> Option<String> {
        let session_value = Self::exhausted_reset_text(
            session_window,
            shows_reset_time_when_exhausted,
            reset_time_display_style,
            now,
            dates,
        )
        .or_else(|| Self::percent_text(session_window, show_used));
        let weekly_value = Self::exhausted_reset_text(
            weekly_window,
            shows_reset_time_when_exhausted,
            reset_time_display_style,
            now,
            dates,
        )
        .or_else(|| Self::percent_text(weekly_window, show_used));

        let session_label = Self::session_window_label(session_window);
        let session_part =
            Self::prefixed_value(session_value.as_deref(), Some(&session_label), None);
        let weekly_part = Self::prefixed_value(weekly_value.as_deref(), Some("W"), None);
        let parts: Vec<String> = [session_part, weekly_part].into_iter().flatten().collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    /// Formats the status item display text for `mode`
    /// (`MenuBarDisplayText.displayText`, `MenuBarDisplayText.swift:107-162`).
    #[allow(clippy::too_many_arguments)]
    pub fn display_text(
        mode: MenuBarDisplayMode,
        percent_window: Option<&ariadusage_protocol::usage::RateWindow>,
        pace_window: Option<&ariadusage_protocol::usage::RateWindow>,
        reset_window: Option<&ariadusage_protocol::usage::RateWindow>,
        show_used: bool,
        percent_is_estimated: bool,
        reset_time_display_style: crate::format::ResetTimeDisplayStyle,
        shows_reset_time_when_exhausted: bool,
        pace_delta_percent: Option<f64>,
        pace_reserve_positive: bool,
        now: jiff::Timestamp,
        dates: &dyn crate::format::DateTexts,
    ) -> Option<String> {
        let effective_reset_window = reset_window.or(percent_window);
        let exhausted_source = match mode {
            MenuBarDisplayMode::Percent
            | MenuBarDisplayMode::Both
            | MenuBarDisplayMode::ResetTime => percent_window,
            MenuBarDisplayMode::Pace => pace_window.or(percent_window),
        };
        let exhausted_reset = Self::exhausted_reset_text(
            exhausted_source,
            shows_reset_time_when_exhausted,
            reset_time_display_style,
            now,
            dates,
        );
        match mode {
            MenuBarDisplayMode::Percent => exhausted_reset.or_else(|| {
                Self::percent_text_estimated(percent_window, show_used, percent_is_estimated)
            }),
            MenuBarDisplayMode::Pace => {
                if let Some(exhausted_reset) = exhausted_reset {
                    return Some(exhausted_reset);
                }
                if let Some(delta) = pace_delta_percent {
                    return Some(Self::pace_text(delta, pace_reserve_positive));
                }
                Self::percent_text_estimated(percent_window, show_used, percent_is_estimated)
            }
            MenuBarDisplayMode::Both => {
                if let Some(exhausted_reset) = exhausted_reset {
                    return Some(exhausted_reset);
                }
                let percent =
                    Self::percent_text_estimated(percent_window, show_used, percent_is_estimated)?;
                let Some(delta) = pace_delta_percent else {
                    return Some(percent);
                };
                let pace = Self::pace_text(delta, pace_reserve_positive);
                Some(format!("{percent} · {pace}"))
            }
            MenuBarDisplayMode::ResetTime => {
                if let Some(reset) = Self::reset_time_text(
                    effective_reset_window,
                    reset_time_display_style,
                    now,
                    dates,
                ) {
                    return Some(reset);
                }
                Self::percent_text_estimated(percent_window, show_used, percent_is_estimated)
            }
        }
    }

    /// Formats a window's reset time with a leading `↻ ` glyph
    /// (`MenuBarDisplayText.resetTimeText`, `MenuBarDisplayText.swift:164-179`).
    pub fn reset_time_text(
        window: Option<&ariadusage_protocol::usage::RateWindow>,
        style: crate::format::ResetTimeDisplayStyle,
        now: jiff::Timestamp,
        dates: &dyn crate::format::DateTexts,
    ) -> Option<String> {
        let window = window?;
        if let Some(resets_at) = window.resets_at {
            let text = match style {
                crate::format::ResetTimeDisplayStyle::Countdown => {
                    crate::format::reset_countdown_description(crate::format::seconds_between(
                        now, resets_at,
                    ))
                }
                crate::format::ResetTimeDisplayStyle::Absolute => {
                    crate::format::reset_description(resets_at, now, dates)
                }
            };
            return Some(format!("↻ {text}"));
        }
        if let Some(text) = Self::reset_metadata_text(window.reset_description.as_deref()) {
            return Some(format!("↻ {text}"));
        }
        None
    }

    /// Returns the `↻ ...` reset text when `enabled` is true, the window has `0%` remaining, and
    /// its `resets_at` timestamp is still in the future
    /// (`MenuBarDisplayText.exhaustedResetText`, `MenuBarDisplayText.swift:181-190`).
    pub fn exhausted_reset_text(
        window: Option<&ariadusage_protocol::usage::RateWindow>,
        enabled: bool,
        style: crate::format::ResetTimeDisplayStyle,
        now: jiff::Timestamp,
        dates: &dyn crate::format::DateTexts,
    ) -> Option<String> {
        if !enabled {
            return None;
        }
        let window = window?;
        let remaining = (100.0 - window.used_percent).clamp(0.0, 100.0);
        if remaining > 0.0 {
            return None;
        }
        let resets_at = window.resets_at?;
        if resets_at <= now {
            return None;
        }
        Self::reset_time_text(Some(window), style, now, dates)
    }

    /// Extracts reset metadata text when `raw` starts with or matches a recognized reset keyword
    /// (`MenuBarDisplayText.resetMetadataText`, `MenuBarDisplayText.swift:192-219`).
    pub fn reset_metadata_text(raw: Option<&str>) -> Option<String> {
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            return None;
        }
        let normalized = trimmed.to_lowercase();
        const RESET_PREFIXES: &[&str] = &[
            "reset ",
            "resets ",
            "in ",
            "today ",
            "today,",
            "tomorrow ",
            "tomorrow,",
            "next ",
            "expire ",
            "expires ",
            "refill ",
            "refills ",
        ];
        const EXACT_MATCHES: &[&str] = &["today", "tomorrow", "expired", "now", "soon"];
        let matches_prefix = RESET_PREFIXES.iter().any(|p| normalized.starts_with(p));
        let matches_exact = EXACT_MATCHES.contains(&normalized.as_str());
        if matches_prefix || matches_exact {
            Some(trimmed.to_string())
        } else {
            None
        }
    }
}

/// Formats the status item button title, prepending a leading space when an icon image is
/// present and appending `"D"` when running a debug bundle
/// (`StatusItemController.buttonTitle`, `StatusItemController+Animation.swift:719-735`).
pub fn button_title(title: Option<&str>, has_image: bool, is_debug_app: bool) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(title) = title.filter(|t| !t.is_empty()) {
        parts.push(title);
    }
    if is_debug_app {
        parts.push("D");
    }
    let value = parts.join(" ");
    if value.is_empty() {
        return String::new();
    }
    if has_image {
        format!(" {value}")
    } else {
        value
    }
}

/// Status badge geometry overlaid at the bottom-right of a brand icon when a provider has an
/// active incident (`StatusItemController.drawBrandStatusOverlay`,
/// `StatusItemController+Animation.swift:1333-1373`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BrandStatusOverlay {
    /// Filled `4 × 4` pt dot at `(width - 6, 2)` for `Minor` and `Maintenance` status indicators.
    Dot { dot_rect: RectPt },
    /// Filled exclamation mark (`2 × 6` pt rounded bar at `(width - 6, 4)` + `2 × 2` pt dot at
    /// `(width - 6, 2)`) for `Major`, `Critical`, and `Unknown` status indicators.
    Exclamation {
        line_rect: RectPt,
        line_radius_pt: f64,
        dot_rect: RectPt,
    },
}

impl BrandStatusOverlay {
    /// Evaluates whether the overlay draws opaque pixels (`> 0.0` alpha) on the canvas.
    pub const fn max_alpha(&self) -> f64 {
        match self {
            Self::Dot { .. } | Self::Exclamation { .. } => 1.0,
        }
    }
}

/// Computes the bottom-right status badge overlay for a brand icon of `width_pt` when `status`
/// is active (`StatusItemController.drawBrandStatusOverlay`,
/// `StatusItemController+Animation.swift:1333-1373`).
pub fn brand_status_overlay(status: StatusLevel) -> Option<BrandStatusOverlay> {
    brand_status_overlay_for_width(status, ICON_SIZE_PT)
}

/// Computes the bottom-right status badge overlay for a brand icon with explicit `width_pt`
/// (`StatusItemController.drawBrandStatusOverlay`, `StatusItemController+Animation.swift:1352-1373`).
pub fn brand_status_overlay_for_width(
    status: StatusLevel,
    width_pt: f64,
) -> Option<BrandStatusOverlay> {
    match status {
        StatusLevel::None => None,
        StatusLevel::Minor | StatusLevel::Maintenance => Some(BrandStatusOverlay::Dot {
            dot_rect: RectPt::new(width_pt - 6.0, 2.0, 4.0, 4.0),
        }),
        StatusLevel::Major | StatusLevel::Critical | StatusLevel::Unknown => {
            Some(BrandStatusOverlay::Exclamation {
                line_rect: RectPt::new(width_pt - 6.0, 4.0, 2.0, 6.0),
                line_radius_pt: 1.0,
                dot_rect: RectPt::new(width_pt - 6.0, 2.0, 2.0, 2.0),
            })
        }
    }
}

/// Computes the render deduplication signature for the `iconAndPercent` brand + text status item
/// (`StatusItemController.applyIcon` `mode=brandPercent`, `StatusItemController+Animation.swift:373-381`).
pub fn brand_icon_render_signature(
    provider_id: &str,
    style: IconStyle,
    status: StatusLevel,
    display_text: Option<&str>,
    quota_warning_flash_active: bool,
    high_contrast: bool,
    provider_tint_rgb: Option<(u8, u8, u8)>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "brandPercent".hash(&mut hasher);
    provider_id.hash(&mut hasher);
    style.hash(&mut hasher);
    (status as u8).hash(&mut hasher);
    display_text.hash(&mut hasher);
    quota_warning_flash_active.hash(&mut hasher);
    high_contrast.hash(&mut hasher);
    provider_tint_rgb.hash(&mut hasher);
    hasher.finish()
}
