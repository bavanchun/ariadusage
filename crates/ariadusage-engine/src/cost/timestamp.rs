// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+Timestamp.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+TimestampOrder.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CostUsageModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::cmp::Ordering;
use std::fmt;
use std::ops::Deref;

use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

/// Constructs a UTC `Timestamp` from `(year, month, day, hour, minute, second)`.
#[must_use]
pub fn utc_ymd_hms(year: i16, month: i8, day: i8, hour: i8, minute: i8, second: i8) -> Timestamp {
    DateTime::new(year, month, day, hour, minute, second, 0)
        .expect("valid civil datetime")
        .to_zoned(TimeZone::UTC)
        .expect("valid UTC datetime")
        .timestamp()
}

/// Canonical `YYYY-MM-DD` local day key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DayKey(pub String);

impl DayKey {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        let (y, m, d) = parse_day_key(trimmed)?;
        if !is_valid_calendar_day(y, m, d) {
            return None;
        }
        Some(Self(format_day_key(y, m, d)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn add_days(&self, days: i32) -> Self {
        add_days_to_day_key(&self.0, days)
            .map(Self)
            .unwrap_or_else(|| self.clone())
    }
}

impl Deref for DayKey {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for DayKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for DayKey {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Maps legacy IANA timezone aliases that may be absent on slim Linux hosts without `tzdata-legacy`.
pub fn canonical_iana_timezone(name: &str) -> &str {
    match name {
        "Asia/Calcutta" => "Asia/Kolkata",
        "Asia/Saigon" => "Asia/Ho_Chi_Minh",
        "Asia/Katmandu" => "Asia/Kathmandu",
        "Asia/Rangoon" => "Asia/Yangon",
        "Europe/Kiev" | "Europe/Uzhgorod" | "Europe/Zaporozhye" => "Europe/Kyiv",
        "US/Eastern" => "America/New_York",
        "US/Central" => "America/Chicago",
        "US/Mountain" => "America/Denver",
        "US/Pacific" => "America/Los_Angeles",
        "GMT" | "Etc/GMT" | "Etc/UTC" | "Z" => "UTC",
        other => other,
    }
}

/// Resolves and pins the local time zone used to bucket timestamps into `YYYY-MM-DD` day keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketTimeZone {
    tz: TimeZone,
    identifier: String,
}

impl Default for BucketTimeZone {
    fn default() -> Self {
        Self::system()
    }
}

impl BucketTimeZone {
    /// Returns the system time zone (falling back to UTC if unavailable).
    pub fn system() -> Self {
        let tz = TimeZone::system();
        let identifier = tz.iana_name().unwrap_or("UTC").to_string();
        Self { tz, identifier }
    }

    /// Returns a UTC bucket time zone.
    pub fn utc() -> Self {
        Self {
            tz: TimeZone::UTC,
            identifier: "UTC".to_string(),
        }
    }

    /// Checks whether `identifier` is a recognized IANA time zone identifier.
    pub fn is_valid_identifier(identifier: &str) -> bool {
        let trimmed = identifier.trim();
        if trimmed.is_empty() {
            return false;
        }
        let canonical = canonical_iana_timezone(trimmed);
        TimeZone::get(canonical).is_ok()
    }

    /// Resolves a `BucketTimeZone` from an optional IANA identifier, falling back to `Self::system()`.
    pub fn from_identifier(identifier: Option<&str>) -> Self {
        if let Some(raw) = identifier {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                let canonical = canonical_iana_timezone(trimmed);
                if let Ok(tz) = TimeZone::get(canonical) {
                    return Self {
                        tz,
                        identifier: trimmed.to_string(),
                    };
                }
            }
        }
        Self::system()
    }

    /// Constructs a `BucketTimeZone` from an explicit `jiff::tz::TimeZone`.
    pub fn from_tz(tz: TimeZone) -> Self {
        let identifier = tz.iana_name().unwrap_or("UTC").to_string();
        Self { tz, identifier }
    }

    /// Returns the underlying `jiff::tz::TimeZone`.
    pub fn tz(&self) -> &TimeZone {
        &self.tz
    }

    /// Returns the time zone identifier string.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Converts a `Timestamp` to a `YYYY-MM-DD` day key in this time zone.
    pub fn day_key_from_timestamp(&self, ts: Timestamp) -> String {
        let zdt = ts.to_zoned(self.tz.clone());
        format_day_key(zdt.year() as i32, zdt.month() as i32, zdt.day() as i32)
    }

    /// Converts Unix epoch seconds to a `YYYY-MM-DD` day key in this time zone.
    pub fn day_key_from_epoch_seconds(&self, secs: i64) -> Option<String> {
        let ts = Timestamp::from_second(secs).ok()?;
        Some(self.day_key_from_timestamp(ts))
    }

    /// Converts Unix epoch milliseconds to a `YYYY-MM-DD` day key in this time zone.
    pub fn day_key_from_unix_ms(&self, ms: i64) -> Option<String> {
        let ts = Timestamp::from_millisecond(ms).ok()?;
        Some(self.day_key_from_timestamp(ts))
    }

    /// Returns the Unix epoch second for local midnight (`00:00:00`) at the start of `day_key`.
    pub fn start_of_day_epoch_seconds(&self, day_key: &str) -> Option<i64> {
        let date = parse_day_key_date(day_key)?;
        let zdt = date.at(0, 0, 0, 0).to_zoned(self.tz.clone()).ok()?;
        Some(zdt.timestamp().as_second())
    }

    /// Returns the Unix epoch second for local midnight at the start of the day after `day_key`.
    pub fn start_of_next_day_epoch_seconds(&self, day_key: &str) -> Option<i64> {
        let date = parse_day_key_date(day_key)?;
        let next = date.tomorrow().ok()?;
        let zdt = next.at(0, 0, 0, 0).to_zoned(self.tz.clone()).ok()?;
        Some(zdt.timestamp().as_second())
    }
}

/// Formats `(year, month, day)` as `YYYY-MM-DD`.
pub fn format_day_key(year: i32, month: i32, day: i32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// Parses a `YYYY-MM-DD` day key into `(year, month, day)`.
pub fn parse_day_key(key: &str) -> Option<(i32, i32, i32)> {
    let bytes = key.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let y = read_digits(bytes, 0, 4)?;
    let m = read_digits(bytes, 5, 2)?;
    let d = read_digits(bytes, 8, 2)?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

/// Parses a `YYYY-MM-DD` day key into a civil `Date`.
pub fn parse_day_key_date(key: &str) -> Option<Date> {
    let (y, m, d) = parse_day_key(key)?;
    Date::new(y as i16, m as i8, d as i8).ok()
}

/// Adds `days` to a `YYYY-MM-DD` day key using the Gregorian calendar.
pub fn add_days_to_day_key(key: &str, days: i32) -> Option<String> {
    let date = parse_day_key_date(key)?;
    let shifted = date.checked_add(jiff::Span::new().days(days)).ok()?;
    Some(format_day_key(
        shifted.year() as i32,
        shifted.month() as i32,
        shifted.day() as i32,
    ))
}

/// Alias for `CostUsageDayRange`.
pub type DayRange = CostUsageDayRange;

/// Inclusive local day range used when scanning and filtering cost history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostUsageDayRange {
    pub since_key: String,
    pub until_key: String,
    pub scan_since_key: String,
    pub scan_until_key: String,
    pub tz: BucketTimeZone,
    pub timezone: Option<TimeZone>,
}

impl CostUsageDayRange {
    /// Creates a `CostUsageDayRange` from `since` and `until` timestamps in `timezone` (or system time zone when `None`).
    #[must_use]
    pub fn new(since: Timestamp, until: Timestamp, timezone: Option<TimeZone>) -> Self {
        let tz = match timezone.clone() {
            Some(explicit) => BucketTimeZone::from_tz(explicit),
            None => BucketTimeZone::system(),
        };
        let since_key = tz.day_key_from_timestamp(since);
        let until_key = tz.day_key_from_timestamp(until);
        Self::from_day_keys(&since_key, &until_key, tz)
    }

    /// Creates a `CostUsageDayRange` directly from `YYYY-MM-DD` day keys and `tz`.
    #[must_use]
    pub fn from_day_keys(since_key: &str, until_key: &str, tz: BucketTimeZone) -> Self {
        let scan_since_key =
            add_days_to_day_key(since_key, -1).unwrap_or_else(|| since_key.to_string());
        let scan_until_key =
            add_days_to_day_key(until_key, 1).unwrap_or_else(|| until_key.to_string());
        let timezone = Some(tz.tz().clone());
        Self {
            since_key: since_key.to_string(),
            until_key: until_key.to_string(),
            scan_since_key,
            scan_until_key,
            tz,
            timezone,
        }
    }

    /// Returns the `BucketTimeZone` for this range.
    #[must_use]
    pub fn timezone(&self) -> &BucketTimeZone {
        &self.tz
    }

    /// Formats `ts` as a `YYYY-MM-DD` day key in `tz`.
    #[must_use]
    pub fn day_key(ts: Timestamp, tz: &BucketTimeZone) -> String {
        tz.day_key_from_timestamp(ts)
    }

    /// Returns true when `day_key` is within `[since_key, until_key]`.
    #[must_use]
    pub fn is_in_range(day_key: &str, since_key: &str, until_key: &str) -> bool {
        day_key >= since_key && day_key <= until_key
    }

    /// Returns true when `day_key` is within `[self.since_key, self.until_key]`.
    #[must_use]
    pub fn contains_day(&self, day_key: &str) -> bool {
        Self::is_in_range(day_key, &self.since_key, &self.until_key)
    }

    /// Alias for `contains_day`.
    #[must_use]
    pub fn contains_day_str(&self, day_key: &str) -> bool {
        self.contains_day(day_key)
    }

    /// Returns true when `day_key` is within the expanded scan window `[self.scan_since_key, self.scan_until_key]`.
    #[must_use]
    pub fn contains_scan_day(&self, day_key: &str) -> bool {
        Self::is_in_range(day_key, &self.scan_since_key, &self.scan_until_key)
    }
}

/// Parsed components of an ISO-8601 timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedISOTimestamp {
    pub year: i32,
    pub month: i32,
    pub day: i32,
    pub hour: i32,
    pub minute: i32,
    pub second: i32,
    pub milliseconds: i32,
    pub offset_seconds: Option<i32>,
}

fn read_digits(bytes: &[u8], start: usize, len: usize) -> Option<i32> {
    if start + len > bytes.len() {
        return None;
    }
    let mut value: i32 = 0;
    for &b in &bytes[start..start + len] {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value * 10 + i32::from(b - b'0');
    }
    Some(value)
}

fn is_valid_calendar_day(year: i32, month: i32, day: i32) -> bool {
    if year <= 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    day <= max_day
}

/// Civil date-time to Unix epoch seconds using Howard Hinnant's algorithm.
pub fn seconds_since_epoch(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
) -> i64 {
    let mut y = i64::from(year);
    let m = i64::from(month);
    let d = i64::from(day);
    if m <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 24 + i64::from(hour)) * 60 + i64::from(minute)) * 60 + i64::from(second)
}

/// Fast ASCII parser for ISO-8601 timestamps (`YYYY-MM-DD` or `YYYY-MM-DD[T ]HH:MM:SS[.fff][Z|±HH:MM]`).
pub fn parse_iso_timestamp(text: &str) -> Option<ParsedISOTimestamp> {
    let bytes = text.trim().as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = read_digits(bytes, 0, 4)?;
    let month = read_digits(bytes, 5, 2)?;
    let day = read_digits(bytes, 8, 2)?;
    if !is_valid_calendar_day(year, month, day) {
        return None;
    }
    if bytes.len() == 10 {
        return Some(ParsedISOTimestamp {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
            offset_seconds: None,
        });
    }

    let sep = bytes[10];
    if sep != b'T' && sep != b't' && sep != b' ' {
        return None;
    }
    if bytes.len() < 19 || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let hour = read_digits(bytes, 11, 2)?;
    let minute = read_digits(bytes, 14, 2)?;
    let second = read_digits(bytes, 17, 2)?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=60).contains(&second) {
        return None;
    }

    let mut index = 19;
    let mut milliseconds = 0;
    if index < bytes.len() && (bytes[index] == b'.' || bytes[index] == b',') {
        index += 1;
        let frac_start = index;
        let mut digits = 0;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            let digit = i32::from(bytes[index] - b'0');
            if digits == 0 {
                milliseconds += digit * 100;
            } else if digits == 1 {
                milliseconds += digit * 10;
            } else if digits == 2 {
                milliseconds += digit;
            }
            digits += 1;
            index += 1;
        }
        if index == frac_start {
            return None;
        }
    }

    if index == bytes.len() {
        return Some(ParsedISOTimestamp {
            year,
            month,
            day,
            hour,
            minute,
            second,
            milliseconds,
            offset_seconds: None,
        });
    }

    let tz_char = bytes[index];
    if tz_char == b'Z' || tz_char == b'z' {
        if index + 1 != bytes.len() {
            return None;
        }
        return Some(ParsedISOTimestamp {
            year,
            month,
            day,
            hour,
            minute,
            second,
            milliseconds,
            offset_seconds: Some(0),
        });
    }

    if tz_char != b'+' && tz_char != b'-' {
        return None;
    }
    let sign = if tz_char == b'-' { -1 } else { 1 };
    index += 1;
    let tz_hours = read_digits(bytes, index, 2)?;
    index += 2;
    let mut tz_minutes = 0;
    if index < bytes.len() {
        if bytes[index] == b':' {
            index += 1;
        }
        tz_minutes = read_digits(bytes, index, 2)?;
        index += 2;
    }
    if index != bytes.len() || !(0..=23).contains(&tz_hours) || !(0..=59).contains(&tz_minutes) {
        return None;
    }
    let offset_seconds = sign * ((tz_hours * 60 + tz_minutes) * 60);
    Some(ParsedISOTimestamp {
        year,
        month,
        day,
        hour,
        minute,
        second,
        milliseconds,
        offset_seconds: Some(offset_seconds),
    })
}

/// Converts a `ParsedISOTimestamp` into Unix epoch milliseconds.
pub fn parsed_iso_to_unix_ms(parsed: &ParsedISOTimestamp, tz: &BucketTimeZone) -> Option<i64> {
    let clamped_sec = parsed.second.min(59);
    if let Some(offset_seconds) = parsed.offset_seconds {
        let utc_seconds = seconds_since_epoch(
            parsed.year,
            parsed.month,
            parsed.day,
            parsed.hour,
            parsed.minute,
            clamped_sec,
        ) - i64::from(offset_seconds);
        return Some(
            utc_seconds
                .saturating_mul(1_000)
                .saturating_add(i64::from(parsed.milliseconds)),
        );
    }

    let dt = DateTime::new(
        parsed.year as i16,
        parsed.month as i8,
        parsed.day as i8,
        parsed.hour as i8,
        parsed.minute as i8,
        clamped_sec as i8,
        parsed.milliseconds * 1_000_000,
    )
    .ok()?;
    let zdt = dt.to_zoned(tz.tz().clone()).ok()?;
    Some(zdt.timestamp().as_millisecond())
}

/// Parses an ISO-8601 timestamp string into Unix epoch milliseconds.
pub fn parse_timestamp_unix_ms(text: &str, tz: &BucketTimeZone) -> Option<i64> {
    let parsed = parse_iso_timestamp(text)?;
    parsed_iso_to_unix_ms(&parsed, tz)
}

/// Converts a `ParsedISOTimestamp` into a local `YYYY-MM-DD` day key in `tz`.
pub fn day_key_from_parsed_iso(parsed: &ParsedISOTimestamp, tz: &BucketTimeZone) -> Option<String> {
    if let Some(offset_seconds) = parsed.offset_seconds {
        let clamped_sec = parsed.second.min(59);
        let unix_seconds = seconds_since_epoch(
            parsed.year,
            parsed.month,
            parsed.day,
            parsed.hour,
            parsed.minute,
            clamped_sec,
        ) - i64::from(offset_seconds);
        return tz.day_key_from_epoch_seconds(unix_seconds);
    }
    Some(format_day_key(parsed.year, parsed.month, parsed.day))
}

/// Converts a UTC `Timestamp` to a `DayKey` in `timezone` (or system time zone when `None`).
#[must_use]
pub fn day_key_from_utc(ts: Timestamp, timezone: Option<TimeZone>) -> DayKey {
    let btz = match timezone {
        Some(tz) => BucketTimeZone::from_tz(tz),
        None => BucketTimeZone::system(),
    };
    DayKey(btz.day_key_from_timestamp(ts))
}

/// Parses an ISO-8601 timestamp string and returns `(DayKey, unix_ms)` in `timezone` (or system time zone when `None`).
#[must_use]
pub fn day_key_from_timestamp(text: &str, timezone: Option<TimeZone>) -> Option<(DayKey, i64)> {
    let btz = match timezone {
        Some(tz) => BucketTimeZone::from_tz(tz),
        None => BucketTimeZone::system(),
    };
    let parsed = parse_iso_timestamp(text)?;
    let day = day_key_from_parsed_iso(&parsed, &btz)?;
    let ms = parsed_iso_to_unix_ms(&parsed, &btz)?;
    Some((DayKey(day), ms))
}

/// Parses a Claude ISO-8601 timestamp string and returns `(Timestamp, day_key)` in `tz`.
#[must_use]
pub fn claude_timestamp_and_day_key(
    text: &str,
    tz: &BucketTimeZone,
) -> Option<(Timestamp, String)> {
    let parsed = parse_iso_timestamp(text)?;
    let day = day_key_from_parsed_iso(&parsed, tz)?;
    let ms = parsed_iso_to_unix_ms(&parsed, tz)?;
    let ts = Timestamp::from_millisecond(ms).ok()?;
    Some((ts, day))
}

/// Returns `(slot_key, date, hour)` for `unix_ms` in `tz` (`YYYY-MM-DDTHH`).
#[must_use]
pub fn hour_slot_key_from_unix_ms(
    unix_ms: i64,
    tz: &BucketTimeZone,
) -> Option<(String, String, i32)> {
    let ts = Timestamp::from_millisecond(unix_ms).ok()?;
    let zdt = ts.to_zoned(tz.tz().clone());
    let date = format_day_key(zdt.year() as i32, zdt.month() as i32, zdt.day() as i32);
    let hour = i32::from(zdt.hour());
    let slot = format!("{date}T{hour:02}");
    Some((slot, date, hour))
}

/// Returns `(slice_key, start_date, end_date, start_hour, end_hour)` for 5-hour quota slices.
#[must_use]
pub fn quota_slice_key_from_unix_ms(
    unix_ms: i64,
    tz: &BucketTimeZone,
) -> Option<(String, String, String, i32, i32)> {
    let ts = Timestamp::from_millisecond(unix_ms).ok()?;
    let zdt = ts.to_zoned(tz.tz().clone());
    let start_date = format_day_key(zdt.year() as i32, zdt.month() as i32, zdt.day() as i32);
    let hour = i32::from(zdt.hour());
    let start_hour = (hour / 5) * 5;
    let raw_end_hour = start_hour + 5;
    let (end_date, end_hour) = if raw_end_hour >= 24 {
        let next = add_days_to_day_key(&start_date, 1)?;
        (next, raw_end_hour - 24)
    } else {
        (start_date.clone(), raw_end_hour)
    };
    let slice_key = format!("{start_date}T{start_hour:02}..{end_date}T{end_hour:02}");
    Some((slice_key, start_date, end_date, start_hour, end_hour))
}

/// Compares two Codex timestamp strings chronologically, falling back to lexicographical ordering.
#[must_use]
pub fn compare_codex_timestamps(lhs: &str, rhs: &str) -> Ordering {
    let utc = BucketTimeZone::utc();
    match (
        parse_timestamp_unix_ms(lhs, &utc),
        parse_timestamp_unix_ms(rhs, &utc),
    ) {
        (Some(l), Some(r)) => l.cmp(&r),
        _ => lhs.cmp(rhs),
    }
}
