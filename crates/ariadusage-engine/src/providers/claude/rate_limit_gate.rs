// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthUsageRateLimitGate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::sync::Mutex;

use ariadusage_core::digest::sha256_hex;
use jiff::{SignedDuration, Timestamp};

/// Per-token rate-limit cooldown gate for Claude OAuth usage fetches.
///
/// Keyed by a domain-separated SHA-256 digest of the normalized OAuth access token so raw
/// tokens are never retained in memory.
#[derive(Debug, Default)]
pub struct ClaudeOAuthUsageRateLimitGate {
    cooldown_until_by_token_key: Mutex<HashMap<String, Timestamp>>,
}

impl ClaudeOAuthUsageRateLimitGate {
    pub const DEFAULT_COOLDOWN_SECS: i64 = 5 * 60;

    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the active cooldown expiration timestamp if `token` is currently rate-limited at `now`.
    pub fn active_cooldown_until(&self, token: &str, now: Timestamp) -> Option<Timestamp> {
        let key = Self::token_key(token)?;
        let mut map = self
            .cooldown_until_by_token_key
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.retain(|_, exp| *exp > now);
        map.get(&key).copied().filter(|exp| *exp > now)
    }

    /// Records a rate-limit cooldown for `token` at `now` and returns the effective `cooldown_until` timestamp.
    pub fn record_rate_limit(
        &self,
        token: &str,
        retry_after: Option<Timestamp>,
        now: Timestamp,
    ) -> Timestamp {
        let fallback = now
            .checked_add(SignedDuration::from_secs(Self::DEFAULT_COOLDOWN_SECS))
            .unwrap_or(Timestamp::MAX);
        let cooldown_until = match retry_after {
            Some(ts) if ts > now => ts,
            _ => fallback,
        };
        if let Some(key) = Self::token_key(token) {
            let mut map = self
                .cooldown_until_by_token_key
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            map.retain(|_, exp| *exp > now);
            let entry = map
                .entry(key)
                .and_modify(|e| *e = (*e).max(cooldown_until))
                .or_insert(cooldown_until);
            return *entry;
        }
        cooldown_until
    }

    /// Clears any recorded rate-limit cooldown for `token`.
    pub fn clear(&self, token: &str) {
        let Some(key) = Self::token_key(token) else {
            return;
        };
        let mut map = self
            .cooldown_until_by_token_key
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.remove(&key);
    }

    /// Clears all recorded rate-limit cooldown entries.
    pub fn reset(&self) {
        let mut map = self
            .cooldown_until_by_token_key
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.clear();
    }

    fn token_key(token: &str) -> Option<String> {
        let normalized = token.trim();
        if normalized.is_empty() {
            return None;
        }
        Some(sha256_hex(
            "ariadusage:claude-oauth-rate-limit-gate:v1",
            &[normalized.as_bytes()],
        ))
    }
}

/// Parses an HTTP `Retry-After` header value (delta-seconds or HTTP-date) relative to `now`.
pub fn parse_retry_after(raw: Option<&str>, now: Timestamp) -> Option<Timestamp> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(seconds) = trimmed.parse::<i64>() {
        if seconds < 0 {
            return None;
        }
        return now.checked_add(SignedDuration::from_secs(seconds)).ok();
    }

    parse_http_date(trimmed)
}

/// Parses RFC 9110 HTTP-date formats:
/// - IMF-fixdate: `Sun, 06 Nov 1994 08:49:37 GMT`
/// - obsolete RFC 850: `Sunday, 06-Nov-94 08:49:37 GMT`
/// - ANSI C `asctime()`: `Sun Nov  6 08:49:37 1994`
fn parse_http_date(raw: &str) -> Option<Timestamp> {
    let trimmed = raw.trim();

    if let Some((_weekday, rest)) = trimmed.split_once(',') {
        let rest = rest.trim();
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() == 5 && parts[4].eq_ignore_ascii_case("GMT") {
            // IMF-fixdate: 06 Nov 1994 08:49:37 GMT
            let day = parts[0].parse::<i8>().ok()?;
            let month = parse_month(parts[1])?;
            let year = parts[2].parse::<i16>().ok()?;
            let (hour, minute, second) = parse_hms(parts[3])?;
            return build_utc_timestamp(year, month, day, hour, minute, second);
        }
        if parts.len() == 3 && parts[2].eq_ignore_ascii_case("GMT") {
            // RFC 850: 06-Nov-94 08:49:37 GMT
            let mut dmy = parts[0].split('-');
            let day = dmy.next()?.parse::<i8>().ok()?;
            let month = parse_month(dmy.next()?)?;
            let two_digit_year = dmy.next()?.parse::<i16>().ok()?;
            if dmy.next().is_some() {
                return None;
            }
            let year = if two_digit_year >= 70 {
                1900 + two_digit_year
            } else {
                2000 + two_digit_year
            };
            let (hour, minute, second) = parse_hms(parts[1])?;
            return build_utc_timestamp(year, month, day, hour, minute, second);
        }
        return None;
    }

    // ANSI C asctime(): Sun Nov  6 08:49:37 1994
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() == 5 {
        let month = parse_month(parts[1])?;
        let day = parts[2].parse::<i8>().ok()?;
        let (hour, minute, second) = parse_hms(parts[3])?;
        let year = parts[4].parse::<i16>().ok()?;
        return build_utc_timestamp(year, month, day, hour, minute, second);
    }

    None
}

fn parse_month(raw: &str) -> Option<i8> {
    match raw {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

fn parse_hms(raw: &str) -> Option<(i8, i8, i8)> {
    let mut parts = raw.split(':');
    let hour = parts.next()?.parse::<i8>().ok()?;
    let minute = parts.next()?.parse::<i8>().ok()?;
    let second = parts.next()?.parse::<i8>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((hour, minute, second))
}

fn build_utc_timestamp(
    year: i16,
    month: i8,
    day: i8,
    hour: i8,
    minute: i8,
    second: i8,
) -> Option<Timestamp> {
    let date = jiff::civil::Date::new(year, month, day).ok()?;
    let time = jiff::civil::Time::new(hour, minute, second, 0).ok()?;
    date.to_datetime(time)
        .to_zoned(jiff::tz::TimeZone::UTC)
        .ok()
        .map(|z| z.timestamp())
}
