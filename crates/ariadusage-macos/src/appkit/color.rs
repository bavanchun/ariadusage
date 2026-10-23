//! Resolves card colors to `NSColor`s.

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSColorSpace};

use crate::card::Accent;
use crate::highlight_style::{Color, ColorRole};

fn role_color(role: ColorRole, accent: Accent) -> Retained<NSColor> {
    match role {
        ColorRole::SelectedMenuItemText => NSColor::selectedMenuItemTextColor(),
        ColorRole::ControlText => NSColor::controlTextColor(),
        ColorRole::Label => NSColor::labelColor(),
        ColorRole::SecondaryLabel => NSColor::secondaryLabelColor(),
        ColorRole::TertiaryLabel => NSColor::tertiaryLabelColor(),
        ColorRole::SystemRed => NSColor::systemRedColor(),
        ColorRole::SystemGreen => NSColor::systemGreenColor(),
        ColorRole::ControlAccent => NSColor::controlAccentColor(),
        ColorRole::SelectedContentBackground => NSColor::selectedContentBackgroundColor(),
        ColorRole::Separator => NSColor::separatorColor(),
        ColorRole::White => NSColor::whiteColor(),
        ColorRole::ProgressTint => match accent {
            Accent::Brand(rgb) => NSColor::colorWithSRGBRed_green_blue_alpha(
                f64::from(rgb.r) / 255.0,
                f64::from(rgb.g) / 255.0,
                f64::from(rgb.b) / 255.0,
                1.0,
            ),
            Accent::System => NSColor::controlAccentColor(),
        },
    }
}

/// The `NSColor` for `color`. Opaque roles stay dynamic, so they follow
/// appearance changes; a partial opacity multiplies the color's own alpha
/// like SwiftUI's `.opacity`, which resolves the color for the current
/// appearance, so call this while drawing.
pub fn ns_color(color: Color, accent: Accent) -> Retained<NSColor> {
    let base = role_color(color.role, accent);
    if color.alpha >= 1.0 {
        return base;
    }
    let resolved = base
        .colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())
        .unwrap_or(base);
    let alpha = resolved.alphaComponent() * color.alpha;
    resolved.colorWithAlphaComponent(alpha)
}
