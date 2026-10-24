// Ported from CodexBar Sources/CodexBarCore/OpenAIWeb/OpenAIDashboardParser.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::VecDeque;
use std::sync::LazyLock;

use ariadusage_core::model::window::RateWindow;
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use regex::Regex;
use serde_json::Value;

use super::credits::CreditEvent;
use super::plan::CodexPlanFormatting;

static CODE_REVIEW_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)Code\s*review[^0-9%]*([0-9]{1,3})%\s*remaining",
        r"(?i)Core\s*review[^0-9%]*([0-9]{1,3})%\s*remaining",
    ]
    .into_iter()
    .map(|p| Regex::new(p).expect("valid code review regex"))
    .collect()
});

static CREDITS_REMAINING_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)credits\s*remaining[^0-9]*([0-9][0-9.,]*)",
        r"(?i)remaining\s*credits[^0-9]*([0-9][0-9.,]*)",
        r"(?i)credit\s*balance[^0-9]*([0-9][0-9.,]*)",
    ]
    .into_iter()
    .map(|p| Regex::new(p).expect("valid credits remaining regex"))
    .collect()
});

static PERCENT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9]{1,3})\s*%").expect("valid percent regex"));

static FIRST_NUMBER_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9][0-9.,\s\u{00A0}\u{202F}]*)").expect("valid token regex"));

static THOUSANDS_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,3}(,\d{3})+$").expect("valid thousands regex"));

static FIVE_H_WORD_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b5\s*h\b").expect("valid 5h regex"));

static SEVEN_D_WORD_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b7\s*d\b").expect("valid 7d regex"));

static RESETS_PREFIX_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^resets?:?\s*").expect("valid resets prefix regex"));

static AT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+at\s+").expect("valid at regex"));

static ON_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+on\s+").expect("valid on regex"));

static TODAY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\btoday\b").expect("valid today regex"));

static TOMORROW_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\btomorrow\b").expect("valid tomorrow regex"));

static WEEKDAY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(mon|tue|tues|wed|wednes|thu|thur|thurs|fri|sat|satur|sun)(day)?\b")
        .expect("valid weekday regex")
});

const CLIENT_BOOTSTRAP_NEEDLE: &str = "id=\"client-bootstrap\"";
const NEXT_DATA_NEEDLE: &str = "id=\"__NEXT_DATA__\"";
const SCRIPT_CLOSE_NEEDLE: &str = "</script>";

pub struct OpenAIDashboardParser;

impl OpenAIDashboardParser {
    pub fn parse_signed_in_email_from_client_bootstrap(html: &str) -> Option<String> {
        let data = Self::client_bootstrap_json_data(html)?;
        let json: Value = serde_json::from_slice(data).ok()?;

        if let Some(dict) = json.as_object() {
            if let Some(email) = dict
                .get("session")
                .and_then(Value::as_object)
                .and_then(|s| s.get("user"))
                .and_then(Value::as_object)
                .and_then(|u| u.get("email"))
                .and_then(Value::as_str)
                && email.contains('@')
            {
                return Some(email.trim().to_owned());
            }

            if let Some(email) = dict
                .get("user")
                .and_then(Value::as_object)
                .and_then(|u| u.get("email"))
                .and_then(Value::as_str)
                && email.contains('@')
            {
                return Some(email.trim().to_owned());
            }
        }

        let mut queue: VecDeque<&Value> = VecDeque::from([&json]);
        let mut seen = 0usize;
        while let Some(cur) = queue.pop_front() {
            if seen >= 4000 {
                break;
            }
            seen += 1;
            match cur {
                Value::Object(dict) => {
                    for (k, v) in dict {
                        if k.eq_ignore_ascii_case("email")
                            && let Some(email) = v.as_str()
                            && email.contains('@')
                        {
                            return Some(email.trim().to_owned());
                        }
                        queue.push_back(v);
                    }
                }
                Value::Array(arr) => {
                    for v in arr {
                        queue.push_back(v);
                    }
                }
                _ => {}
            }
        }
        None
    }

    pub fn parse_auth_status_from_client_bootstrap(html: &str) -> Option<String> {
        let data = Self::client_bootstrap_json_data(html)?;
        let json: Value = serde_json::from_slice(data).ok()?;
        let dict = json.as_object()?;
        let auth_status = dict.get("authStatus")?.as_str()?.trim();
        if auth_status.is_empty() {
            None
        } else {
            Some(auth_status.to_owned())
        }
    }

    pub fn parse_code_review_remaining_percent(body_text: &str) -> Option<f64> {
        let cleaned = body_text.replace('\r', "\n");
        for regex in CODE_REVIEW_REGEXES.iter() {
            let Some(caps) = regex.captures(&cleaned) else {
                continue;
            };
            let Some(m) = caps.get(1) else {
                continue;
            };
            if let Ok(val) = m.as_str().parse::<f64>() {
                return Some(val.clamp(0.0, 100.0));
            }
        }
        None
    }

    pub fn parse_credits_remaining(body_text: &str) -> Option<f64> {
        let cleaned = body_text.replace('\r', "\n");
        for regex in CREDITS_REMAINING_REGEXES.iter() {
            if let Some(caps) = regex.captures(&cleaned)
                && let Some(m) = caps.get(1)
                && let Some(val) = Self::parse_first_number_token(m.as_str())
            {
                return Some(val);
            }
        }
        None
    }

    pub fn parse_rate_limits(
        body_text: &str,
        now: Timestamp,
    ) -> (Option<RateWindow>, Option<RateWindow>) {
        let cleaned = body_text.replace('\r', "\n");
        let lines: Vec<&str> = cleaned
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();

        let primary =
            Self::parse_rate_window(&lines, Self::is_five_hour_limit_line, Some(5 * 60), now);
        let secondary =
            Self::parse_rate_window(&lines, Self::is_weekly_limit_line, Some(7 * 24 * 60), now);
        (primary, secondary)
    }

    pub fn parse_code_review_limit(body_text: &str, now: Timestamp) -> Option<RateWindow> {
        let cleaned = body_text.replace('\r', "\n");
        let lines: Vec<&str> = cleaned
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();

        Self::parse_rate_window(&lines, Self::is_code_review_limit_line, None, now)
    }

    pub fn parse_plan_from_html(html: &str) -> Option<String> {
        if let Some(data) = Self::client_bootstrap_json_data(html)
            && let Some(plan) = Self::find_plan_in_slice(data)
        {
            return Some(plan);
        }
        if let Some(data) = Self::next_data_json_data(html)
            && let Some(plan) = Self::find_plan_in_slice(data)
        {
            return Some(plan);
        }
        None
    }

    pub fn parse_credit_events<Row, S>(rows: &[Row]) -> Vec<CreditEvent>
    where
        Row: AsRef<[S]>,
        S: AsRef<str>,
    {
        let mut events: Vec<CreditEvent> = rows
            .iter()
            .filter_map(|row| {
                let slice = row.as_ref();
                if slice.len() < 3 {
                    return None;
                }
                let date_str = slice[0].as_ref();
                let service = slice[1].as_ref().trim().to_owned();
                let amount_str = slice[2].as_ref();
                let date = Self::parse_credit_date(date_str)?;
                let credits_used = Self::parse_credits_used(amount_str);
                Some(CreditEvent::new(date, service, credits_used))
            })
            .collect();
        events.sort_by(|a, b| b.date.cmp(&a.date));
        events
    }

    fn parse_credit_date(raw: &str) -> Option<Timestamp> {
        let trimmed = raw.trim();
        let (month_str, rest) = trimmed.split_once(' ')?;
        let month = Self::parse_month_abbr(month_str)?;
        let (day_str, year_str) = rest.split_once(',')?;
        let day: i8 = day_str.trim().parse().ok()?;
        let year: i16 = year_str.trim().parse().ok()?;
        let date = Date::new(year, month, day).ok()?;
        let tz = TimeZone::system();
        date.at(0, 0, 0, 0)
            .to_zoned(tz)
            .ok()
            .map(|z| z.timestamp())
            .or_else(|| {
                date.at(0, 0, 0, 0)
                    .to_zoned(TimeZone::UTC)
                    .ok()
                    .map(|z| z.timestamp())
            })
    }

    fn parse_month_abbr(raw: &str) -> Option<i8> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "jan" | "january" => Some(1),
            "feb" | "february" => Some(2),
            "mar" | "march" => Some(3),
            "apr" | "april" => Some(4),
            "may" => Some(5),
            "jun" | "june" => Some(6),
            "jul" | "july" => Some(7),
            "aug" | "august" => Some(8),
            "sep" | "sept" | "september" => Some(9),
            "oct" | "october" => Some(10),
            "nov" | "november" => Some(11),
            "dec" | "december" => Some(12),
            _ => None,
        }
    }

    fn month_abbr(month: i8) -> &'static str {
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

    fn parse_credits_used(text: &str) -> f64 {
        let Some(raw) = Self::first_number_token(text) else {
            return 0.0;
        };
        let token = raw.replace(['\u{00A0}', '\u{202F}', ' '], "");
        let has_comma = token.contains(',');
        let has_dot = token.contains('.');
        if has_comma && has_dot {
            return Self::parse_first_number_token(&token).unwrap_or(0.0);
        }
        if has_comma {
            if Self::uses_localized_decimal_comma_credit_label(text) {
                return token.replace(',', ".").parse::<f64>().unwrap_or(0.0);
            }
            if THOUSANDS_COMMA_REGEX.is_match(&token) {
                return token.replace(',', "").parse::<f64>().unwrap_or(0.0);
            }
            return token.replace(',', ".").parse::<f64>().unwrap_or(0.0);
        }
        token.parse::<f64>().unwrap_or(0.0)
    }

    fn uses_localized_decimal_comma_credit_label(text: &str) -> bool {
        text.to_lowercase().contains("crédit")
    }

    fn first_number_token(text: &str) -> Option<String> {
        FIRST_NUMBER_TOKEN_REGEX
            .captures(text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
    }

    fn parse_first_number_token(raw: &str) -> Option<f64> {
        let mut cleaned = raw.trim().replace(['\u{00A0}', '\u{202F}', ' '], "");
        while cleaned.ends_with(['.', ',']) {
            cleaned.pop();
        }
        if cleaned.is_empty() {
            return None;
        }
        let has_comma = cleaned.contains(',');
        let has_dot = cleaned.contains('.');
        let normalized = if has_comma && has_dot {
            let last_comma = cleaned.rfind(',').unwrap_or(0);
            let last_dot = cleaned.rfind('.').unwrap_or(0);
            if last_comma > last_dot {
                cleaned.replace('.', "").replace(',', ".")
            } else {
                cleaned.replace(',', "")
            }
        } else if has_comma {
            if THOUSANDS_COMMA_REGEX.is_match(&cleaned) {
                cleaned.replace(',', "")
            } else {
                cleaned.replace(',', ".")
            }
        } else {
            cleaned
        };
        normalized.parse::<f64>().ok().filter(|v| v.is_finite())
    }

    fn client_bootstrap_json_data(html: &str) -> Option<&[u8]> {
        Self::extract_script_json_data(html, CLIENT_BOOTSTRAP_NEEDLE)
    }

    fn next_data_json_data(html: &str) -> Option<&[u8]> {
        Self::extract_script_json_data(html, NEXT_DATA_NEEDLE)
    }

    fn extract_script_json_data<'a>(html: &'a str, needle: &str) -> Option<&'a [u8]> {
        let id_pos = html.find(needle)?;
        let after_id = &html[id_pos + needle.len()..];
        let gt_offset = after_id.find('>')?;
        let content_after_gt = &after_id[gt_offset + 1..];
        let close_offset = content_after_gt.find(SCRIPT_CLOSE_NEEDLE)?;
        let raw_slice = content_after_gt[..close_offset].as_bytes();
        let trimmed = Self::trim_ascii_whitespace(raw_slice);
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    fn trim_ascii_whitespace(mut data: &[u8]) -> &[u8] {
        while let Some((first, rest)) = data.split_first() {
            if matches!(first, b'\t' | b'\n' | b'\r' | b' ') {
                data = rest;
            } else {
                break;
            }
        }
        while let Some((last, rest)) = data.split_last() {
            if matches!(last, b'\t' | b'\n' | b'\r' | b' ') {
                data = rest;
            } else {
                break;
            }
        }
        data
    }

    fn parse_rate_window<F>(
        lines: &[&str],
        match_fn: F,
        window_minutes: Option<i64>,
        now: Timestamp,
    ) -> Option<RateWindow>
    where
        F: Fn(&str) -> bool,
    {
        for (idx, line) in lines.iter().enumerate() {
            if !match_fn(line) {
                continue;
            }
            let end = (lines.len() - 1).min(idx + 5);
            let window_lines = &lines[idx..=end];

            let mut parsed_percent: Option<(f64, bool)> = None;
            for wline in window_lines {
                if let Some(p) = Self::parse_percent(wline) {
                    parsed_percent = Some(p);
                    break;
                }
            }
            let Some((percent_value, is_remaining)) = parsed_percent else {
                continue;
            };
            let used_percent = if is_remaining {
                (100.0 - percent_value).clamp(0.0, 100.0)
            } else {
                percent_value.clamp(0.0, 100.0)
            };

            let reset_line = window_lines
                .iter()
                .copied()
                .find(|l| l.to_ascii_lowercase().contains("reset"));
            let reset_description = reset_line.map(|l| l.trim().to_owned());
            let resets_at = reset_line.and_then(|l| Self::parse_reset_date(l, now));

            return RateWindow::new(
                used_percent,
                window_minutes,
                resets_at,
                reset_description,
                None,
                false,
            )
            .ok();
        }
        None
    }

    fn parse_percent(line: &str) -> Option<(f64, bool)> {
        let caps = PERCENT_REGEX.captures(line)?;
        let val = caps.get(1)?.as_str().parse::<f64>().ok()?;
        let lower = line.to_ascii_lowercase();
        let is_remaining = lower.contains("remaining") || lower.contains("left");
        let is_used =
            lower.contains("used") || lower.contains("spent") || lower.contains("consumed");
        if is_used {
            return Some((val, false));
        }
        if is_remaining {
            return Some((val, true));
        }
        Some((val, true))
    }

    fn is_five_hour_limit_line(line: &str) -> bool {
        let lower = line.to_ascii_lowercase();
        lower.contains("5h")
            || FIVE_H_WORD_REGEX.is_match(&lower)
            || lower.contains("5-hour")
            || lower.contains("5 hour")
    }

    fn is_weekly_limit_line(line: &str) -> bool {
        let lower = line.to_ascii_lowercase();
        lower.contains("weekly")
            || lower.contains("7-day")
            || lower.contains("7 day")
            || lower.contains("7d")
            || SEVEN_D_WORD_REGEX.is_match(&lower)
    }

    fn is_code_review_limit_line(line: &str) -> bool {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("code review") && !lower.contains("core review") {
            return false;
        }
        !lower.contains("github code review")
    }

    fn parse_reset_date(line: &str, now: Timestamp) -> Option<Timestamp> {
        let mut raw = RESETS_PREFIX_REGEX.replace(line.trim(), "").into_owned();
        raw = AT_REGEX.replace_all(&raw, " ").into_owned();
        raw = ON_REGEX.replace_all(&raw, " ").into_owned();
        let raw = raw.split_whitespace().collect::<Vec<_>>().join(" ");

        let tz = TimeZone::system();
        let now_zoned = now.to_zoned(tz.clone());
        let now_date = now_zoned.date();

        let mut candidate = raw.clone();
        let lower = candidate.to_ascii_lowercase();
        let mut used_relative_day = false;

        if TODAY_REGEX.is_match(&candidate) {
            used_relative_day = true;
            let date_text = format!("{} {}", Self::month_abbr(now_date.month()), now_date.day());
            candidate = TODAY_REGEX
                .replace_all(&candidate, date_text.as_str())
                .into_owned();
        } else if TOMORROW_REGEX.is_match(&candidate) {
            used_relative_day = true;
            if let Ok(tomorrow) = now_date.checked_add(jiff::Span::new().days(1)) {
                let date_text =
                    format!("{} {}", Self::month_abbr(tomorrow.month()), tomorrow.day());
                candidate = TOMORROW_REGEX
                    .replace_all(&candidate, date_text.as_str())
                    .into_owned();
            }
        }

        if let Some((matched, target_weekday_iso)) = Self::weekday_match(&candidate) {
            used_relative_day = true;
            let current_iso = now_date.weekday().to_monday_one_offset();
            let mut delta = i64::from(target_weekday_iso) - i64::from(current_iso);
            if delta < 0 {
                delta += 7;
            }
            if let Ok(target_date) = now_date.checked_add(jiff::Span::new().days(delta)) {
                let date_text = format!(
                    "{} {}",
                    Self::month_abbr(target_date.month()),
                    target_date.day()
                );
                candidate = candidate.replacen(&matched, &date_text, 1);
            }
        }

        let parsed = Self::parse_candidate_datetime(&candidate, now_date.year(), &tz)?;
        if used_relative_day && parsed < now {
            let days_to_add = if lower.contains("today") { 1 } else { 7 };
            let bumped = parsed
                .to_zoned(tz)
                .checked_add(jiff::Span::new().days(days_to_add))
                .ok()?;
            return Some(bumped.timestamp());
        }
        Some(parsed)
    }

    fn weekday_match(text: &str) -> Option<(String, i8)> {
        let m = WEEKDAY_REGEX.find(text)?;
        let matched = m.as_str().to_owned();
        let lower = matched.to_ascii_lowercase();
        let iso_weekday = match &lower[..3] {
            "mon" => 1,
            "tue" => 2,
            "wed" => 3,
            "thu" => 4,
            "fri" => 5,
            "sat" => 6,
            _ => 7,
        };
        Some((matched, iso_weekday))
    }

    fn parse_candidate_datetime(
        candidate: &str,
        default_year: i16,
        tz: &TimeZone,
    ) -> Option<Timestamp> {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            return None;
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.is_empty() {
            return None;
        }

        // Case 1: Starts with month abbreviation ("MMM d ...")
        if let Some(month) = Self::parse_month_abbr(tokens[0])
            && tokens.len() >= 2
        {
            let day_str = tokens[1].trim_end_matches(',');
            if let Ok(day) = day_str.parse::<i8>() {
                let time_tokens = &tokens[2..];
                let (hour, minute) = if time_tokens.is_empty() {
                    (0, 0)
                } else {
                    Self::parse_time_tokens(time_tokens)?
                };
                let date = Date::new(default_year, month, day).ok()?;
                return Self::date_time_to_timestamp(date, hour, minute, tz);
            }
        }

        // Case 2: Starts with "yyyy-MM-dd"
        if tokens[0].len() == 10
            && tokens[0].as_bytes()[4] == b'-'
            && tokens[0].as_bytes()[7] == b'-'
            && let Ok(date) = Date::strptime("%Y-%m-%d", tokens[0])
        {
            let time_tokens = &tokens[1..];
            let (hour, minute) = if time_tokens.is_empty() {
                (0, 0)
            } else {
                Self::parse_time_tokens(time_tokens)?
            };
            return Self::date_time_to_timestamp(date, hour, minute, tz);
        }

        // Case 3: Starts with "M/d" or "M/d/yyyy" or "M/d/yy"
        if tokens[0].contains('/') {
            let parts: Vec<&str> = tokens[0].split('/').collect();
            if (parts.len() == 2 || parts.len() == 3)
                && let (Ok(month), Ok(day)) = (parts[0].parse::<i8>(), parts[1].parse::<i8>())
            {
                let year = if parts.len() == 3 {
                    let y = parts[2].parse::<i16>().ok()?;
                    if y < 100 { 2000 + y } else { y }
                } else {
                    default_year
                };
                let time_tokens = &tokens[1..];
                let (hour, minute) = if time_tokens.is_empty() {
                    (0, 0)
                } else {
                    Self::parse_time_tokens(time_tokens)?
                };
                let date = Date::new(year, month, day).ok()?;
                return Self::date_time_to_timestamp(date, hour, minute, tz);
            }
        }

        None
    }

    fn parse_time_tokens(tokens: &[&str]) -> Option<(i8, i8)> {
        let joined = tokens.join(" ").to_ascii_uppercase();
        let trimmed = joined.trim();
        let (time_part, is_pm, has_meridiem) = if let Some(s) = trimmed.strip_suffix("AM") {
            (s.trim(), false, true)
        } else if let Some(s) = trimmed.strip_suffix("PM") {
            (s.trim(), true, true)
        } else {
            (trimmed, false, false)
        };
        let (h_str, m_str) = time_part.split_once(':')?;
        let mut hour: i8 = h_str.trim().parse().ok()?;
        let minute: i8 = m_str.trim().parse().ok()?;
        if !(0..=59).contains(&minute) {
            return None;
        }
        if has_meridiem {
            if !(1..=12).contains(&hour) {
                return None;
            }
            if is_pm && hour < 12 {
                hour += 12;
            } else if !is_pm && hour == 12 {
                hour = 0;
            }
        } else if !(0..=23).contains(&hour) {
            return None;
        }
        Some((hour, minute))
    }

    fn date_time_to_timestamp(
        date: Date,
        hour: i8,
        minute: i8,
        tz: &TimeZone,
    ) -> Option<Timestamp> {
        date.at(hour, minute, 0, 0)
            .to_zoned(tz.clone())
            .ok()
            .map(|z| z.timestamp())
            .or_else(|| {
                date.at(hour, minute, 0, 0)
                    .to_zoned(TimeZone::UTC)
                    .ok()
                    .map(|z| z.timestamp())
            })
    }

    fn find_plan_in_slice(data: &[u8]) -> Option<String> {
        let json: Value = serde_json::from_slice(data).ok()?;
        Self::find_plan(&json)
    }

    fn find_plan(json: &Value) -> Option<String> {
        let mut queue: VecDeque<&Value> = VecDeque::from([json]);
        let mut seen = 0usize;
        while let Some(cur) = queue.pop_front() {
            if seen >= 6000 {
                break;
            }
            seen += 1;
            match cur {
                Value::Object(dict) => {
                    for (k, v) in dict {
                        if let Some(plan) = Self::plan_candidate(k, v) {
                            return Some(plan);
                        }
                        queue.push_back(v);
                    }
                }
                Value::Array(arr) => {
                    for v in arr {
                        queue.push_back(v);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn plan_candidate(key: &str, value: &Value) -> Option<String> {
        if !Self::is_plan_key(key) {
            return None;
        }
        if let Some(s) = value.as_str() {
            return Self::normalize_plan_value(s);
        }
        if let Some(dict) = value.as_object() {
            for field in ["name", "displayName", "tier"] {
                if let Some(s) = dict.get(field).and_then(Value::as_str)
                    && let Some(plan) = Self::normalize_plan_value(s)
                {
                    return Some(plan);
                }
            }
        }
        None
    }

    fn is_plan_key(key: &str) -> bool {
        let lower = key.to_ascii_lowercase();
        lower.contains("plan") || lower.contains("tier") || lower.contains("subscription")
    }

    fn normalize_plan_value(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lower = trimmed.to_ascii_lowercase();
        let allowed = [
            "free",
            "plus",
            "pro",
            "team",
            "enterprise",
            "business",
            "edu",
            "education",
            "gov",
            "premium",
            "essential",
        ];
        if !allowed.iter().any(|token| lower.contains(token)) {
            return None;
        }
        CodexPlanFormatting::display_name(trimmed).or_else(|| Some(Self::clean_plan_name(trimmed)))
    }

    fn clean_plan_name(raw: &str) -> String {
        let replaced = raw.replace(['_', '-'], " ");
        let words: Vec<String> = replaced
            .split_whitespace()
            .map(|w| {
                let mut chars = w.chars();
                match chars.next() {
                    Some(first) => {
                        let mut s = first.to_uppercase().to_string();
                        s.push_str(&chars.as_str().to_lowercase());
                        s
                    }
                    None => String::new(),
                }
            })
            .collect();
        words.join(" ")
    }
}
