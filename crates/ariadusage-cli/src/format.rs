// Ported from CodexBar Sources/CodexBarCore/UsageFormatter.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexPlanFormatting.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/TextParsing.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Number, percent, currency, countdown, timestamp, and plan-name formatting helpers
//! matching `UsageFormatter`, `CodexPlanFormatting`, and `TextParsing`.

use ariadusage_protocol::cli::CliRateWindow;
use jiff::Timestamp;
use jiff::tz::TimeZone;

/// Style used to render reset times (`ResetTimeDisplayStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResetTimeDisplayStyle {
    #[default]
    Countdown,
    Absolute,
}

/// Formats `percent` with `suffix` (`"left"` or `"used"`), clamping to `0..=100`
/// and rendering `0 < p < 1` as `"<1% <suffix>"` (`UsageFormatter.percentText`).
pub fn percent_text(percent: f64, suffix: &str) -> String {
    let clamped = percent.clamp(0.0, 100.0);
    if clamped > 0.0 && clamped < 1.0 {
        return format!("<1% {suffix}");
    }
    format!("{:.0}% {suffix}", round_ties_to_even(clamped))
}

/// Formats a usage percentage line (`UsageFormatter.usageLine`).
pub fn usage_line(remaining: f64, used: f64, show_used: bool) -> String {
    let percent = if show_used { used } else { remaining };
    let suffix = if show_used { "used" } else { "left" };
    percent_text(percent, suffix)
}

/// Formats a bare percentage (`UsageFormatter.percentString`).
pub fn percent_string(percent: f64) -> String {
    let clamped = percent.clamp(0.0, 100.0);
    if clamped > 0.0 && clamped < 1.0 {
        return "<1%".to_string();
    }
    format!("{:.0}%", round_ties_to_even(clamped))
}

/// Formats a countdown from `now` to `resets_at` (`UsageFormatter.resetCountdownDescription`).
pub fn reset_countdown_description(resets_at: Timestamp, now: Timestamp) -> String {
    let seconds = resets_at.duration_since(now).as_secs_f64();
    reset_countdown_from_seconds(seconds)
}

/// Formats a countdown from a delta in seconds (`UsageFormatter.resetCountdownDescription`).
pub fn reset_countdown_from_seconds(seconds: f64) -> String {
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

fn reset_countdown_minutes(seconds: f64) -> Option<i64> {
    if !seconds.is_finite() {
        return None;
    }
    let ceil_min = (seconds / 60.0).ceil();
    if ceil_min < (i64::MIN as f64) || ceil_min > (i64::MAX as f64) {
        return None;
    }
    let minutes = ceil_min as i64;
    Some(if seconds < 1.0 { 0 } else { minutes.max(1) })
}

/// Formats an absolute reset description in `tz` (`UsageFormatter.resetDescription`).
pub fn reset_description_in_tz(resets_at: Timestamp, now: Timestamp, tz: &TimeZone) -> String {
    let target = resets_at.to_zoned(tz.clone());
    let current = now.to_zoned(tz.clone());
    let time_str = format_time_12h(target.hour(), target.minute());
    if target.date() == current.date() {
        return time_str;
    }
    if let Ok(tomorrow) = current.date().tomorrow()
        && target.date() == tomorrow
    {
        return format!("tomorrow, {time_str}");
    }
    format!(
        "{} {}, {time_str}",
        month_abbrev(target.month()),
        target.day()
    )
}

/// Formats an absolute reset description in the system local time zone.
pub fn reset_description(resets_at: Timestamp, now: Timestamp) -> String {
    reset_description_in_tz(resets_at, now, &TimeZone::system())
}

/// Alias for [`reset_description`] (`UsageFormatter.resetDescription`).
pub fn format_reset_description(resets_at: Timestamp, now: Timestamp) -> String {
    reset_description(resets_at, now)
}

/// Formats `Updated ...` relative/absolute string (`UsageFormatter.updatedString`).
pub fn format_updated_string(date: Timestamp, now: Timestamp) -> String {
    let delta = now.duration_since(date).as_secs_f64();
    if !delta.is_finite() {
        return "Updated Unknown".to_string();
    }
    let elapsed_seconds = delta.trunc() as i64;
    if elapsed_seconds > -60 && elapsed_seconds < 60 {
        return "Updated just now".to_string();
    }
    let abs_secs = elapsed_seconds.abs();
    if abs_secs < 24 * 3600 {
        let secs = elapsed_seconds.max(0);
        if secs < 3600 {
            let minutes = (secs / 60).max(1);
            return format!("Updated {minutes}m ago");
        }
        let whole_hours = (secs / 3600).max(1);
        return format!("Updated {whole_hours}h ago");
    }
    let z = date.to_zoned(TimeZone::system());
    format!("Updated {}", format_time_12h(z.hour(), z.minute()))
}

/// Formats `Resets ...` for a [`CliRateWindow`] (`UsageFormatter.resetLine`).
pub fn reset_line_in_tz(
    window: &CliRateWindow,
    style: ResetTimeDisplayStyle,
    now: Timestamp,
    tz: &TimeZone,
) -> Option<String> {
    if let Some(raw_date) = &window.resets_at
        && let Ok(date) = raw_date.parse::<Timestamp>()
    {
        let seconds = date.duration_since(now).as_secs_f64();
        if reset_countdown_minutes(seconds).is_some() {
            if style == ResetTimeDisplayStyle::Countdown {
                let countdown = reset_countdown_from_seconds(seconds);
                if countdown == "now" {
                    return Some("Resets now".to_string());
                }
                if let Some(rest) = countdown.strip_prefix("in ") {
                    return Some(format!("Resets in {rest}"));
                }
                return Some(format!("Resets {countdown}"));
            }
            let text = reset_description_in_tz(date, now, tz);
            return Some(format!("Resets {text}"));
        }
    }

    if let Some(desc) = &window.reset_description {
        let trimmed = desc.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lower = trimmed.to_ascii_lowercase();
        for prefix in ["resets in ", "reset in "] {
            if lower.starts_with(prefix) {
                return Some(format!("Resets in {}", &trimmed[prefix.len()..]));
            }
        }
        for prefix in ["resets ", "reset "] {
            if lower.starts_with(prefix) {
                return Some(format!("Resets {}", &trimmed[prefix.len()..]));
            }
        }
        return Some(format!("Resets {trimmed}"));
    }

    None
}

pub fn reset_line(
    window: &CliRateWindow,
    style: ResetTimeDisplayStyle,
    now: Timestamp,
) -> Option<String> {
    reset_line_in_tz(window, style, now, &TimeZone::system())
}

/// Formats a UTC date-only string (`"MMM d, yyyy"` in `en_US_POSIX` UTC).
pub fn format_utc_date_only(date: Timestamp) -> String {
    let z = date.to_zoned(TimeZone::UTC);
    format!("{} {}, {:04}", month_abbrev(z.month()), z.day(), z.year())
}

/// Formats `<number> left` for a credit balance (`UsageFormatter.creditsString`).
pub fn credits_string(value: f64) -> String {
    format!("{} left", credits_number_string(value))
}

/// Formats a decimal credit number with US grouping and up to 2 fraction digits
/// (`UsageFormatter.creditsNumberString`).
pub fn credits_number_string(value: f64) -> String {
    format_us_decimal(value, 0, 2)
}

/// Formats a currency value in `en_US` locale (`UsageFormatter.currencyString`).
pub fn currency_string(value: f64, currency_code: &str) -> String {
    let sign = if value < 0.0 { "-" } else { "" };
    let symbol = currency_symbol(currency_code);
    let abs_val = value.abs();
    let number = format_us_decimal(abs_val, 2, 2);
    format!("{sign}{symbol}{number}")
}

fn currency_symbol(code: &str) -> String {
    match code.trim().to_ascii_uppercase().as_str() {
        "CREDITS" => String::new(),
        "USD" => "$".to_string(),
        "EUR" => "€".to_string(),
        "GBP" => "£".to_string(),
        "JPY" => "¥".to_string(),
        "CNY" => "CN¥".to_string(),
        "CAD" => "CA$".to_string(),
        "AUD" => "A$".to_string(),
        "INR" => "₹".to_string(),
        "KRW" => "₩".to_string(),
        "BRL" => "R$".to_string(),
        "" => "$".to_string(),
        other => format!("{other}\u{00A0}"),
    }
}

/// Formats a number with US thousand-grouping commas and `min_frac..=max_frac` decimal digits.
pub fn format_us_decimal(value: f64, min_frac: usize, max_frac: usize) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let scale = 10_f64.powi(max_frac as i32);
    let rounded = round_ties_to_even(value * scale) / scale;
    let sign = if rounded < 0.0 { "-" } else { "" };
    let abs_rounded = rounded.abs();
    let formatted = format!("{abs_rounded:.max_frac$}");
    let (int_part, frac_part) = match formatted.split_once('.') {
        Some((i, f)) => (i, f),
        None => (formatted.as_str(), ""),
    };
    let grouped_int = group_thousands(int_part);
    if max_frac == 0 {
        return format!("{sign}{grouped_int}");
    }
    let mut trimmed_frac = frac_part.trim_end_matches('0').to_string();
    while trimmed_frac.len() < min_frac {
        trimmed_frac.push('0');
    }
    if trimmed_frac.is_empty() {
        format!("{sign}{grouped_int}")
    } else {
        format!("{sign}{grouped_int}.{trimmed_frac}")
    }
}

fn group_thousands(digits: &str) -> String {
    let bytes = digits.as_bytes();
    let len = bytes.len();
    if len <= 3 {
        return digits.to_string();
    }
    let mut out = String::with_capacity(len + len / 3);
    for (idx, &b) in bytes.iter().enumerate() {
        if idx > 0 && (len - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char::from(b));
    }
    out
}

/// Truncates a single-line string to `max` characters, appending `…` when truncated
/// (`UsageFormatter.truncatedSingleLine`).
pub fn truncated_single_line(text: &str, max: usize) -> String {
    let single = text.replace('\n', " ");
    let trimmed = single.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let prefix: String = trimmed.chars().take(max).collect();
    format!("{prefix}…")
}

/// Strips ANSI CSI/OSC sequences (`TextParsing.stripANSICodes`).
pub fn strip_ansi_codes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1B {
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                i += 2;
                while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            } else if i + 1 < bytes.len() && bytes[i + 1] == b']' {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Cleans a provider plan name (`UsageFormatter.cleanPlanName`).
pub fn clean_plan_name(text: &str) -> String {
    let stripped = strip_ansi_codes(text);
    let without_codes = strip_leading_bracket_sgr(&stripped);
    let mut words = Vec::new();
    for raw_word in without_codes.split_whitespace() {
        let filtered = strip_boilerplate_word(raw_word);
        if !filtered.is_empty() {
            words.push(filtered);
        }
    }
    let mut cleaned = words.join(" ");
    if cleaned.is_empty() {
        cleaned = stripped.trim().to_string();
    }
    if cleaned.eq_ignore_ascii_case("oauth") {
        return "Ollama".to_string();
    }
    let mut chars = cleaned.chars();
    if let Some(first) = chars.next()
        && first.is_lowercase()
    {
        return format!("{}{}", first.to_uppercase(), chars.as_str());
    }
    cleaned
}

fn strip_leading_bracket_sgr(text: &str) -> &str {
    let mut rest = text.trim_start();
    loop {
        if let Some(after_open) = rest.strip_prefix('[') {
            let digit_len = after_open
                .bytes()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if (1..=3).contains(&digit_len)
                && let Some(after_m) = after_open[digit_len..].strip_prefix('m')
            {
                rest = after_m.trim_start();
                continue;
            }
        }
        break;
    }
    rest
}

fn strip_boilerplate_word(token: &str) -> String {
    let start = token
        .find(|c: char| c.is_alphanumeric())
        .unwrap_or(token.len());
    let end = token
        .rfind(|c: char| c.is_alphanumeric())
        .map_or(start, |idx| {
            idx + token[idx..].chars().next().map_or(0, char::len_utf8)
        });
    let core = &token[start..end];
    if matches!(
        core.to_ascii_lowercase().as_str(),
        "claude" | "codex" | "account" | "plan"
    ) {
        let prefix = &token[..start];
        let suffix = &token[end..];
        format!("{prefix}{suffix}")
    } else {
        token.to_string()
    }
}

/// Formats a Codex plan display name (`CodexPlanFormatting.displayName`).
pub fn codex_plan_display_name(raw: &str) -> Option<String> {
    let cleaned = clean_plan_name(raw);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn format_time_12h(hour24: i8, minute: i8) -> String {
    let is_pm = hour24 >= 12;
    let hour12 = match hour24 % 12 {
        0 => 12,
        h => h,
    };
    let am_pm = if is_pm { "PM" } else { "AM" };
    format!("{hour12}:{minute:02} {am_pm}")
}

fn month_abbrev(month: i8) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "Jan",
    }
}

/// IEEE 754 round-half-to-even (`%.0f` in C/Swift `String(format:)`).
fn round_ties_to_even(val: f64) -> f64 {
    let floor = val.floor();
    let diff = val - floor;
    if (diff - 0.5).abs() < 1e-12 {
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        val.round()
    }
}

/// Alias for [`currency_string`] used across CLI renderers.
pub fn format_cost_amount(value: f64, currency_code: &str) -> String {
    currency_string(value, currency_code)
}

/// Alias for [`credits_number_string`] used across CLI renderers.
pub fn format_number(value: f64) -> String {
    credits_number_string(value)
}

/// Formats a token count with `K`/`M`/`B` suffix (`UsageFormatter.tokenCountString`).
pub fn format_token_count(value: i64) -> String {
    let abs_value = value.unsigned_abs();
    let sign = if value < 0 { "-" } else { "" };
    let units: [(u64, f64, &str); 3] = [
        (999_500_000, 1_000_000_000.0, "B"),
        (999_500, 1_000_000.0, "M"),
        (1_000, 1_000.0, "K"),
    ];
    for (threshold, divisor, suffix) in units {
        if abs_value >= threshold {
            let scaled = (abs_value as f64) / divisor;
            let formatted = if scaled >= 10.0 {
                format!("{scaled:.0}")
            } else {
                let s = format!("{scaled:.1}");
                s.strip_suffix(".0").unwrap_or(&s).to_string()
            };
            return format!("{sign}{formatted}{suffix}");
        }
    }
    value.to_string()
}

/// Formats a countdown string from `now` to `resets_at` (`UsageFormatter.resetCountdownDescription`).
pub fn format_reset_countdown(resets_at: Timestamp, now: Timestamp) -> String {
    reset_countdown_description(resets_at, now)
}

/// Formats a countdown duration in seconds without the `"in "` prefix (`CLIRenderer.paceDurationText`).
pub fn format_reset_countdown_no_prefix(seconds: f64, _now: Timestamp) -> String {
    let countdown = reset_countdown_from_seconds(seconds);
    if countdown == "now" {
        return "now".to_string();
    }
    if let Some(rest) = countdown.strip_prefix("in ") {
        return rest.to_string();
    }
    countdown
}

/// Formats `Resets ...` from optional timestamp and description (`UsageFormatter.resetLine`).
pub fn format_reset_line(
    resets_at: Option<Timestamp>,
    reset_description: Option<&str>,
    style: ResetTimeDisplayStyle,
    now: Timestamp,
) -> Option<String> {
    let window = CliRateWindow {
        used_percent: 0.0,
        window_minutes: None,
        resets_at: resets_at.map(|ts| ts.to_string()),
        reset_description: reset_description.map(str::to_string),
        next_regen_percent: None,
        is_synthetic_placeholder: false,
    };
    reset_line(&window, style, now)
}

/// Returns the visible character count of `text` after stripping ANSI escape codes.
pub fn visible_width(text: &str) -> usize {
    strip_ansi_codes(text).chars().count()
}
