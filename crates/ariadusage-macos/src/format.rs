// Ported from CodexBar Sources/CodexBarCore/UsageFormatter.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexPlanFormatting.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/TextParsing.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Text formats shared by the menu and the status item.
//!
//! Strings are the English values of CodexBar's `Localizable.strings`. Where
//! CodexBar formats dates through Foundation, the caller supplies a
//! [`DateTexts`] implementation: the AppKit bridge calls the real Foundation
//! formatters, and tests use a fixed one.

use std::sync::LazyLock;

use ariadusage_protocol::usage::RateWindow;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use regex::Regex;

/// Suffix for remaining percentages (`usage_percent_suffix_left`).
pub const SUFFIX_LEFT: &str = "left";
/// Suffix for used percentages (`usage_percent_suffix_used`).
pub const SUFFIX_USED: &str = "used";

/// Date strings CodexBar takes from Foundation in the user's locale.
pub trait DateTexts {
    /// The time zone of `Calendar.current`, used to compare calendar days.
    fn time_zone(&self) -> TimeZone;
    /// `date.formatted(.dateTime.hour().minute())`, such as `3:05 PM`.
    fn hour_minute(&self, date: Timestamp) -> String;
    /// `date.formatted(.dateTime.month(.abbreviated).day().hour().minute())`,
    /// such as `Oct 25 at 3:05 PM`.
    fn month_day_hour_minute(&self, date: Timestamp) -> String;
    /// `RelativeDateTimeFormatter` with `.abbreviated` units, such as
    /// `5 min ago`.
    fn relative_abbreviated(&self, date: Timestamp, now: Timestamp) -> String;
}

/// How reset times are shown (`ResetTimeDisplayStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResetTimeDisplayStyle {
    #[default]
    Countdown,
    Absolute,
}

/// Percentage without suffix: clamped to 0–100, `<1%` for values strictly
/// between 0 and 1, otherwise rounded to a whole number like `%.0f`.
pub fn percent_string(percent: f64) -> String {
    let clamped = percent.clamp(0.0, 100.0);
    if clamped > 0.0 && clamped < 1.0 {
        "<1%".to_string()
    } else {
        format!("{clamped:.0}%")
    }
}

/// Percentage followed by a suffix, such as `58% left`.
pub fn percent_text(percent: f64, suffix: &str) -> String {
    format!("{} {suffix}", percent_string(percent))
}

/// The usage line for a window: remaining by default, used when the user
/// chose to show used.
pub fn usage_line(remaining: f64, used: f64, show_used: bool) -> String {
    if show_used {
        percent_text(used, SUFFIX_USED)
    } else {
        percent_text(remaining, SUFFIX_LEFT)
    }
}

/// Seconds from `now` until `date`, with sub-second precision.
pub fn seconds_between(now: Timestamp, date: Timestamp) -> f64 {
    date.duration_since(now).as_secs_f64()
}

/// Whole minutes until a reset `seconds` away: rounded up, at least 1, and
/// 0 once less than a second remains. `None` when not representable.
fn reset_countdown_minutes(seconds: f64) -> Option<i64> {
    let minutes = (seconds / 60.0).ceil();
    if !minutes.is_finite() || minutes.abs() >= 9.0e18 {
        return None;
    }
    Some(if seconds < 1.0 {
        0
    } else {
        (minutes as i64).max(1)
    })
}

/// Countdown to a reset `seconds` away: `now`, `in 45m`, `in 2h 13m`,
/// `in 3d 4h`, using at most two units.
pub fn reset_countdown_description(seconds: f64) -> String {
    let Some(total_minutes) = reset_countdown_minutes(seconds) else {
        return "Unknown".to_string();
    };
    if total_minutes == 0 {
        return "now".to_string();
    }
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes / 60) % 24;
    let minutes = total_minutes % 60;
    if days > 0 {
        if hours > 0 {
            return format!("in {days}d {hours}h");
        }
        if minutes > 0 {
            return format!("in {days}d {minutes}m");
        }
        return format!("in {days}d");
    }
    if hours > 0 {
        if minutes > 0 {
            return format!("in {hours}h {minutes}m");
        }
        return format!("in {hours}h");
    }
    format!("in {total_minutes}m")
}

/// Absolute reset time: the time today, `tomorrow, <time>` tomorrow,
/// otherwise the date and time, all in the calendar's time zone.
pub fn reset_description(date: Timestamp, now: Timestamp, dates: &dyn DateTexts) -> String {
    let tz = dates.time_zone();
    let day = date.to_zoned(tz.clone()).date();
    let today = now.to_zoned(tz).date();
    if day == today {
        return dates.hour_minute(date);
    }
    if today.tomorrow().is_ok_and(|tomorrow| tomorrow == day) {
        return format!("tomorrow, {}", dates.hour_minute(date));
    }
    dates.month_day_hour_minute(date)
}

/// The reset line of a window, preferring its reset date over the
/// provider's description.
pub fn reset_line(
    window: &RateWindow,
    style: ResetTimeDisplayStyle,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> Option<String> {
    if let Some(date) = window.resets_at {
        let seconds = seconds_between(now, date);
        if reset_countdown_minutes(seconds).is_some() {
            if style == ResetTimeDisplayStyle::Countdown {
                let countdown = reset_countdown_description(seconds);
                if countdown == "now" {
                    return Some("Resets now".to_string());
                }
                if let Some(rest) = countdown.strip_prefix("in ") {
                    return Some(format!("Resets in {rest}"));
                }
                return Some(format!("Resets {countdown}"));
            }
            return Some(format!("Resets {}", reset_description(date, now, dates)));
        }
    }
    let trimmed = window.reset_description.as_deref()?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lowercased = trimmed.to_lowercase();
    for prefix in ["resets in ", "reset in "] {
        if lowercased.starts_with(prefix) {
            return Some(format!("Resets in {}", drop_chars(trimmed, prefix)));
        }
    }
    for prefix in ["resets ", "reset "] {
        if lowercased.starts_with(prefix) {
            return Some(format!("Resets {}", drop_chars(trimmed, prefix)));
        }
    }
    Some(format!("Resets {trimmed}"))
}

/// `text` without as many leading characters as `prefix` has.
fn drop_chars<'a>(text: &'a str, prefix: &str) -> &'a str {
    let count = prefix.chars().count();
    text.char_indices()
        .nth(count)
        .map_or("", |(index, _)| &text[index..])
}

/// `Updated just now` within a minute, a relative time within a day,
/// otherwise the absolute time.
pub fn updated_string(date: Timestamp, now: Timestamp, dates: &dyn DateTexts) -> String {
    let elapsed = seconds_between(date, now).trunc();
    if elapsed > -60.0 && elapsed < 60.0 {
        return "Updated just now".to_string();
    }
    if (elapsed / 3600.0).trunc() < 24.0 {
        format!("Updated {}", dates.relative_abbreviated(date, now))
    } else {
        format!("Updated {}", dates.hour_minute(date))
    }
}

/// `N left` for a credit balance.
pub fn credits_string(value: f64) -> String {
    remaining_string(&credits_number_string(value))
}

/// `<value> left`.
pub fn remaining_string(formatted: &str) -> String {
    format!("{formatted} left")
}

/// A credit amount as `NumberFormatter` prints it in `en_US_POSIX` with the
/// decimal style and at most two fraction digits: no grouping, and ties
/// rounded to even on the shortest decimal form of the value.
pub fn credits_number_string(value: f64) -> String {
    if !value.is_finite() {
        return format!("{value:.2}");
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let text = format!("{}", value.abs());
    let (int_part, frac_part) = text.split_once('.').unwrap_or((&text, ""));
    let mut digits: Vec<u8> = int_part
        .bytes()
        .chain(frac_part.bytes().chain(std::iter::repeat(b'0')).take(2))
        .map(|b| b - b'0')
        .collect();
    let rest = frac_part.get(2..).unwrap_or("");
    let round_up = match rest.bytes().next() {
        Some(b'6'..=b'9') => true,
        Some(b'5') => {
            rest.bytes().skip(1).any(|b| b != b'0') || digits.last().is_some_and(|d| d % 2 == 1)
        }
        _ => false,
    };
    if round_up {
        let mut index = digits.len();
        loop {
            if index == 0 {
                digits.insert(0, 1);
                break;
            }
            index -= 1;
            if digits[index] == 9 {
                digits[index] = 0;
            } else {
                digits[index] += 1;
                break;
            }
        }
    }
    let split = digits.len() - 2;
    let int_digits: String = digits[..split]
        .iter()
        .map(|d| char::from(b'0' + d))
        .collect();
    let frac_digits: String = digits[split..]
        .iter()
        .map(|d| char::from(b'0' + d))
        .collect();
    let frac_digits = frac_digits.trim_end_matches('0');
    if frac_digits.is_empty() {
        format!("{sign}{int_digits}")
    } else {
        format!("{sign}{int_digits}.{frac_digits}")
    }
}

/// Compact token counts: `987`, `1.5K`, `12K`, `1M`, `3B`.
pub fn token_count_string(value: i64) -> String {
    let abs_value = value.unsigned_abs();
    let sign = if value < 0 { "-" } else { "" };
    // Promote at the point where whole lower units would round to 1000.
    let units: [(u64, f64, &str); 3] = [
        (999_500_000, 1_000_000_000.0, "B"),
        (999_500, 1_000_000.0, "M"),
        (1000, 1000.0, "K"),
    ];
    for (threshold, divisor, suffix) in units {
        if abs_value >= threshold {
            let scaled = abs_value as f64 / divisor;
            let formatted = if scaled >= 10.0 {
                format!("{scaled:.0}")
            } else {
                let s = format!("{scaled:.1}");
                s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
            };
            return format!("{sign}{formatted}{suffix}");
        }
    }
    value.to_string()
}

static ANSI_CSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1B\[[0-?]*[ -/]*[@-~]").expect("valid ANSI pattern"));
static LEADING_CODES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:\[\d{1,3}m\s*)+").expect("valid code pattern"));
static BOILERPLATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(claude|codex|account|plan)\b").expect("valid boilerplate pattern")
});
static WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("valid whitespace pattern"));

/// Removes ANSI escape sequences (`TextParsing.stripANSICodes`).
pub fn strip_ansi_codes(text: &str) -> String {
    ANSI_CSI.replace_all(text, "").into_owned()
}

/// Cleans a provider plan string: strip ANSI and bracket noise, drop
/// boilerplate words, collapse whitespace, and capitalize a lowercase
/// first letter.
pub fn clean_plan_name(text: &str) -> String {
    let stripped = strip_ansi_codes(text);
    let without_codes = LEADING_CODES.replace(&stripped, "");
    let without_boilerplate = BOILERPLATE.replace_all(&without_codes, "");
    let mut cleaned = WHITESPACE
        .replace_all(&without_boilerplate, " ")
        .trim()
        .to_string();
    if cleaned.is_empty() {
        cleaned = stripped.trim().to_string();
    }
    if cleaned.to_lowercase() == "oauth" {
        return "Ollama".to_string();
    }
    capitalize_lowercase_first(&cleaned)
}

fn capitalize_lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
        _ => text.to_string(),
    }
}

/// Codex plan names: known tiers get their marketing names, other words are
/// title-cased with a few acronyms kept upper case.
pub fn codex_plan_display_name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(exact) = codex_exact_plan_name(&raw.to_lowercase()) {
        return Some(exact.to_string());
    }
    let cleaned = clean_plan_name(raw);
    let candidate = cleaned.trim();
    if candidate.is_empty() {
        return Some(raw.to_string());
    }
    if let Some(exact) = codex_exact_plan_name(&candidate.to_lowercase()) {
        return Some(exact.to_string());
    }
    let words: Vec<String> = candidate
        .split(|c: char| c == '_' || c == '-' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(codex_word_display_name)
        .collect();
    if words.is_empty() {
        return Some(candidate.to_string());
    }
    Some(words.join(" "))
}

fn codex_exact_plan_name(lower: &str) -> Option<&'static str> {
    match lower {
        "pro" => Some("Pro 20x"),
        "prolite" | "pro_lite" | "pro-lite" | "pro lite" => Some("Pro 5x"),
        _ => None,
    }
}

fn codex_word_display_name(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower == "cbp" || lower == "k12" {
        return lower.to_uppercase();
    }
    if raw == raw.to_uppercase() && raw.chars().any(char::is_alphabetic) {
        return raw.to_string();
    }
    capitalize_lowercase_first(raw)
}

/// The plan label shown in the card header (`planDisplay` in
/// `MenuCardView.swift`).
pub fn plan_display(provider_id: &str, text: &str) -> String {
    let cleaned = if provider_id == "codex" {
        codex_plan_display_name(text).unwrap_or_else(|| clean_plan_name(text))
    } else {
        clean_plan_name(text)
    };
    if cleaned.is_empty() {
        text.to_string()
    } else {
        cleaned
    }
}
