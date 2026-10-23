// Ported from CodexBar Sources/CodexBar/MenuHighlightStyle.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Colors of menu-card content in its normal and highlighted states.
//!
//! Colors are described by their AppKit semantic role so they follow light
//! and dark mode; the AppKit bridge resolves each role to an `NSColor`.

/// An AppKit color role, or the card's progress tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorRole {
    /// `NSColor.selectedMenuItemTextColor`.
    SelectedMenuItemText,
    /// `NSColor.controlTextColor`.
    ControlText,
    /// `NSColor.labelColor` (SwiftUI `.primary`).
    Label,
    /// `NSColor.secondaryLabelColor`.
    SecondaryLabel,
    /// `NSColor.tertiaryLabelColor`.
    TertiaryLabel,
    /// `NSColor.systemRedColor` (SwiftUI `.red`).
    SystemRed,
    /// `NSColor.systemGreenColor` (SwiftUI `.green`).
    SystemGreen,
    /// `NSColor.controlAccentColor`.
    ControlAccent,
    /// `NSColor.selectedContentBackgroundColor`.
    SelectedContentBackground,
    /// `NSColor.separatorColor`, used for dividers.
    Separator,
    /// `NSColor.whiteColor`.
    White,
    /// The provider's accent, or the system accent when it has none.
    ProgressTint,
}

/// A color role with an opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub role: ColorRole,
    pub alpha: f64,
}

impl Color {
    pub const fn opaque(role: ColorRole) -> Self {
        Self { role, alpha: 1.0 }
    }

    pub const fn with_alpha(role: ColorRole, alpha: f64) -> Self {
        Self { role, alpha }
    }
}

const SELECTION_TEXT: ColorRole = ColorRole::SelectedMenuItemText;

/// Primary text: control text, or the selection text color when highlighted.
pub fn primary(highlighted: bool) -> Color {
    Color::opaque(if highlighted {
        SELECTION_TEXT
    } else {
        ColorRole::ControlText
    })
}

/// Secondary text.
pub fn secondary(highlighted: bool) -> Color {
    Color::opaque(if highlighted {
        SELECTION_TEXT
    } else {
        ColorRole::SecondaryLabel
    })
}

/// Error text.
pub fn error(highlighted: bool) -> Color {
    Color::opaque(if highlighted {
        SELECTION_TEXT
    } else {
        ColorRole::SystemRed
    })
}

/// Emphasized status labels such as an active plan.
pub fn accent(highlighted: bool) -> Color {
    Color::opaque(if highlighted {
        SELECTION_TEXT
    } else {
        ColorRole::ControlAccent
    })
}

/// The progress bar track.
pub fn progress_track(highlighted: bool) -> Color {
    if highlighted {
        Color::with_alpha(SELECTION_TEXT, 0.22)
    } else {
        Color::with_alpha(ColorRole::TertiaryLabel, 0.22)
    }
}

/// The progress bar fill.
pub fn progress_tint(highlighted: bool) -> Color {
    Color::opaque(if highlighted {
        SELECTION_TEXT
    } else {
        ColorRole::ProgressTint
    })
}

/// The row background behind highlighted content, if any.
pub fn selection_background(highlighted: bool) -> Option<Color> {
    highlighted.then_some(Color::opaque(ColorRole::SelectedContentBackground))
}
