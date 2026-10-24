//! Test doubles for the Foundation date formatters and AppKit text metrics.

#![allow(dead_code)]

use ariadusage_macos::card_layout::{Font, TextMeasure, TextStyle};
use ariadusage_macos::format::DateTexts;
use ariadusage_macos::geometry::Size;
use jiff::Timestamp;
use jiff::tz::TimeZone;

/// Dates in a fixed zone, formatted like the `en_US_POSIX` locale.
pub struct FixedDates {
    pub tz: TimeZone,
}

impl FixedDates {
    pub fn utc() -> Self {
        Self { tz: TimeZone::UTC }
    }

    pub fn zone(name: &str) -> Self {
        Self {
            tz: TimeZone::get(name).expect("known time zone"),
        }
    }
}

impl DateTexts for FixedDates {
    fn time_zone(&self) -> TimeZone {
        self.tz.clone()
    }

    fn hour_minute(&self, date: Timestamp) -> String {
        date.to_zoned(self.tz.clone())
            .strftime("%-I:%M %p")
            .to_string()
    }

    fn month_day_hour_minute(&self, date: Timestamp) -> String {
        date.to_zoned(self.tz.clone())
            .strftime("%b %-d at %-I:%M %p")
            .to_string()
    }

    fn relative_abbreviated(&self, date: Timestamp, now: Timestamp) -> String {
        let seconds = now.duration_since(date).as_secs();
        if seconds >= 3600 {
            format!("{} hr ago", seconds / 3600)
        } else {
            format!("{} min ago", seconds / 60)
        }
    }
}

/// Non-US (`fr_FR`) test double for `DateTexts`: `month_day_year`, `month_day_short`,
/// `detail_month_day_time`, and `format_one_decimal` follow `Locale.current` (`fr_FR`),
/// while `quota_window_range` remains pinned to the `en_US` resource locale.
pub struct FrenchDates {
    pub tz: TimeZone,
}

impl FrenchDates {
    pub fn utc() -> Self {
        Self { tz: TimeZone::UTC }
    }
}

const FR_MONTH_ABBR: [&str; 12] = [
    "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
    "déc.",
];

impl DateTexts for FrenchDates {
    fn time_zone(&self) -> TimeZone {
        self.tz.clone()
    }

    fn hour_minute(&self, date: Timestamp) -> String {
        date.to_zoned(self.tz.clone()).strftime("%H:%M").to_string()
    }

    fn month_day_hour_minute(&self, date: Timestamp) -> String {
        let z = date.to_zoned(self.tz.clone());
        let m = (z.month() - 1).clamp(0, 11) as usize;
        format!(
            "{} {} à {:02}:{:02}",
            z.day(),
            FR_MONTH_ABBR[m],
            z.hour(),
            z.minute()
        )
    }

    fn relative_abbreviated(&self, date: Timestamp, now: Timestamp) -> String {
        let seconds = now.duration_since(date).as_secs();
        format!("il y a {} min", seconds / 60)
    }

    fn month_day_year(&self, date: Timestamp, date_only: bool) -> String {
        let tz = if date_only {
            TimeZone::UTC
        } else {
            self.tz.clone()
        };
        let z = date.to_zoned(tz);
        let m = (z.month() - 1).clamp(0, 11) as usize;
        format!("{} {} {}", z.day(), FR_MONTH_ABBR[m], z.year())
    }

    fn month_day_short(&self, date: Timestamp) -> String {
        let z = date.to_zoned(self.tz.clone());
        let m = (z.month() - 1).clamp(0, 11) as usize;
        format!("{} {}", z.day(), FR_MONTH_ABBR[m])
    }

    fn format_one_decimal(&self, value: f64) -> String {
        ariadusage_macos::format::format_one_decimal_en_us(value)
            .replace(',', "\u{202f}")
            .replace('.', ",")
    }
}

/// Text metrics with a fixed advance per character and a line height per
/// text style.
pub struct FixedMeasure;

pub const CHAR_WIDTH: f64 = 6.0;

pub fn line_height(font: Font) -> f64 {
    match font.style {
        TextStyle::Title2 => 22.0,
        TextStyle::Headline | TextStyle::Body => 16.0,
        TextStyle::Subheadline => 14.0,
        TextStyle::Footnote | TextStyle::Caption | TextStyle::Caption2 => 13.0,
    }
}

pub fn baseline(font: Font) -> f64 {
    match font.style {
        TextStyle::Title2 => 18.0,
        TextStyle::Headline | TextStyle::Body => 13.0,
        TextStyle::Subheadline => 11.0,
        TextStyle::Footnote | TextStyle::Caption | TextStyle::Caption2 => 10.0,
    }
}

impl TextMeasure for FixedMeasure {
    fn measure(&self, text: &str, font: Font, max_lines: usize, max_width: f64) -> Size {
        let natural = text.chars().count() as f64 * CHAR_WIDTH;
        if natural <= max_width {
            return Size::new(natural, line_height(font));
        }
        let wanted = (natural / max_width).ceil() as usize;
        let lines = if max_lines == 0 {
            wanted
        } else {
            wanted.min(max_lines)
        };
        Size::new(max_width, lines as f64 * line_height(font))
    }

    fn first_baseline(&self, font: Font) -> f64 {
        baseline(font)
    }
}

pub fn ts(text: &str) -> Timestamp {
    text.parse().expect("valid timestamp")
}
