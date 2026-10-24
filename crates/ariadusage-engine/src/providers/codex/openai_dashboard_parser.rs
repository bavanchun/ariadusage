// Ported from CodexBar Sources/CodexBarCore/OpenAIWeb/OpenAIDashboardParser.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::VecDeque;

use ariadusage_core::model::window::RateWindow;
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde_json::Value;

use super::credits::CreditEvent;
use super::plan::CodexPlanFormatting;

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
        let lower = cleaned.to_ascii_lowercase();
        for prefix in ["code", "core"] {
            let mut search_from = 0;
            while let Some(rel) = lower[search_from..].find(prefix) {
                let start = search_from + rel + prefix.len();
                search_from = start;
                let rest = &lower[start..];
                let after_ws = rest.trim_start();
                let Some(after_review) = after_ws.strip_prefix("review") else {
                    continue;
                };
                // Skip non-digit, non-'%' characters
                let mut idx = 0;
                let bytes = after_review.as_bytes();
                while idx < bytes.len() && !bytes[idx].is_ascii_digit() && bytes[idx] != b'%' {
                    idx += 1;
                }
                let digits_start = idx;
                while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                    idx += 1;
                }
                let digit_len = idx - digits_start;
                if !(1..=3).contains(&digit_len) || idx >= bytes.len() || bytes[idx] != b'%' {
                    continue;
                }
                let digit_str = &after_review[digits_start..idx];
                let after_pct = after_review[idx + 1..].trim_start();
                if after_pct.starts_with("remaining")
                    && let Ok(val) = digit_str.parse::<f64>()
                {
                    return Some(val.clamp(0.0, 100.0));
                }
            }
        }
        None
    }

    pub fn parse_credits_remaining(body_text: &str) -> Option<f64> {
        let cleaned = body_text.replace('\r', "\n");
        let lower = cleaned.to_ascii_lowercase();
        for (first, second) in [
            ("credits", "remaining"),
            ("remaining", "credits"),
            ("credit", "balance"),
        ] {
            let mut search_from = 0;
            while let Some(rel) = lower[search_from..].find(first) {
                let start = search_from + rel + first.len();
                search_from = start;
                let after_ws = lower[start..].trim_start();
                let Some(after_second) = after_ws.strip_prefix(second) else {
                    continue;
                };
                let mut idx = 0;
                let bytes = after_second.as_bytes();
                while idx < bytes.len() && !bytes[idx].is_ascii_digit() {
                    idx += 1;
                }
                let num_start = idx;
                while idx < bytes.len()
                    && (bytes[idx].is_ascii_digit() || matches!(bytes[idx], b'.' | b','))
                {
                    idx += 1;
                }
                if idx > num_start
                    && let Some(val) = Self::parse_first_number_token(&after_second[num_start..idx])
                {
                    return Some(val);
                }
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
        events.sort_by_key(|a| std::cmp::Reverse(a.date));
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

    fn is_thousands_comma(token: &str) -> bool {
        let parts: Vec<&str> = token.split(',').collect();
        parts.len() >= 2
            && (1..=3).contains(&parts[0].len())
            && parts[0].chars().all(|c| c.is_ascii_digit())
            && parts[1..]
                .iter()
                .all(|p| p.len() == 3 && p.chars().all(|c| c.is_ascii_digit()))
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
            if Self::is_thousands_comma(&token) {
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
        let mut started = false;
        let mut out = String::new();
        for ch in text.chars() {
            if !started {
                if ch.is_ascii_digit() {
                    started = true;
                    out.push(ch);
                }
            } else if ch.is_ascii_digit()
                || matches!(
                    ch,
                    '.' | ',' | ' ' | '\t' | '\n' | '\r' | '\u{00A0}' | '\u{202F}'
                )
            {
                out.push(ch);
            } else {
                break;
            }
        }
        if out.is_empty() { None } else { Some(out) }
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
            if Self::is_thousands_comma(&cleaned) {
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
        let raw_slice = &content_after_gt.as_bytes()[..close_offset];
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
        let mut found_val: Option<f64> = None;
        for (pct_idx, _) in line.match_indices('%') {
            let before = line[..pct_idx].trim_end();
            let digits_len = before
                .bytes()
                .rev()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if (1..=3).contains(&digits_len) {
                let num_str = &before[before.len() - digits_len..];
                if let Ok(v) = num_str.parse::<f64>() {
                    found_val = Some(v);
                    break;
                }
            }
        }
        let val = found_val?;
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

    fn has_spaced_unit_word(lower: &str, digit: char, unit: char) -> bool {
        let chars: Vec<char> = lower.chars().collect();
        for i in 0..chars.len() {
            if chars[i] != digit {
                continue;
            }
            if i > 0 && chars[i - 1].is_alphanumeric() {
                continue;
            }
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && chars[j] == unit {
                let after = j + 1;
                if after == chars.len() || !chars[after].is_alphanumeric() {
                    return true;
                }
            }
        }
        false
    }

    fn is_five_hour_limit_line(line: &str) -> bool {
        let lower = line.to_ascii_lowercase();
        lower.contains("5h")
            || Self::has_spaced_unit_word(&lower, '5', 'h')
            || lower.contains("5-hour")
            || lower.contains("5 hour")
    }

    fn is_weekly_limit_line(line: &str) -> bool {
        let lower = line.to_ascii_lowercase();
        lower.contains("weekly")
            || lower.contains("7-day")
            || lower.contains("7 day")
            || lower.contains("7d")
            || Self::has_spaced_unit_word(&lower, '7', 'd')
    }

    fn is_code_review_limit_line(line: &str) -> bool {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("code review") && !lower.contains("core review") {
            return false;
        }
        !lower.contains("github code review")
    }

    fn strip_resets_prefix(text: &str) -> &str {
        let trimmed = text.trim();
        let lower = trimmed.to_ascii_lowercase();
        let after_word = if lower.starts_with("resets") {
            &trimmed[6..]
        } else if lower.starts_with("reset") {
            &trimmed[5..]
        } else {
            return trimmed;
        };
        let after_colon = after_word.strip_prefix(':').unwrap_or(after_word);
        after_colon.trim_start()
    }

    fn parse_reset_date(line: &str, now: Timestamp) -> Option<Timestamp> {
        let without_prefix = Self::strip_resets_prefix(line);
        let words: Vec<&str> = without_prefix
            .split_whitespace()
            .filter(|w| !w.eq_ignore_ascii_case("at") && !w.eq_ignore_ascii_case("on"))
            .collect();

        let tz = TimeZone::system();
        let now_zoned = now.to_zoned(tz.clone());
        let now_date = now_zoned.date();

        let mut used_relative_day = false;
        let mut had_today = false;
        let mut replaced_words: Vec<String> = Vec::with_capacity(words.len() + 1);
        let mut replaced_weekday = false;

        for word in words {
            let clean_word = word.trim_end_matches(',');
            if clean_word.eq_ignore_ascii_case("today") {
                used_relative_day = true;
                had_today = true;
                replaced_words.push(Self::month_abbr(now_date.month()).to_owned());
                replaced_words.push(now_date.day().to_string());
            } else if clean_word.eq_ignore_ascii_case("tomorrow") {
                used_relative_day = true;
                if let Ok(tomorrow) = now_date.checked_add(jiff::Span::new().days(1)) {
                    replaced_words.push(Self::month_abbr(tomorrow.month()).to_owned());
                    replaced_words.push(tomorrow.day().to_string());
                } else {
                    replaced_words.push(word.to_owned());
                }
            } else if !replaced_weekday
                && let Some(target_weekday_iso) = Self::weekday_iso(clean_word)
            {
                replaced_weekday = true;
                used_relative_day = true;
                let current_iso = now_date.weekday().to_monday_one_offset();
                let mut delta = i64::from(target_weekday_iso) - i64::from(current_iso);
                if delta < 0 {
                    delta += 7;
                }
                if let Ok(target_date) = now_date.checked_add(jiff::Span::new().days(delta)) {
                    replaced_words.push(Self::month_abbr(target_date.month()).to_owned());
                    replaced_words.push(target_date.day().to_string());
                } else {
                    replaced_words.push(word.to_owned());
                }
            } else {
                replaced_words.push(word.to_owned());
            }
        }

        let candidate = replaced_words.join(" ");
        let parsed = Self::parse_candidate_datetime(&candidate, now_date.year(), &tz)?;
        if used_relative_day && parsed < now {
            let days_to_add = if had_today { 1 } else { 7 };
            let bumped = parsed
                .to_zoned(tz)
                .checked_add(jiff::Span::new().days(days_to_add))
                .ok()?;
            return Some(bumped.timestamp());
        }
        Some(parsed)
    }

    fn weekday_iso(word: &str) -> Option<i8> {
        match word.to_ascii_lowercase().as_str() {
            "mon" | "monday" => Some(1),
            "tue" | "tues" | "tuesday" => Some(2),
            "wed" | "wednes" | "wednesday" => Some(3),
            "thu" | "thur" | "thurs" | "thursday" => Some(4),
            "fri" | "friday" => Some(5),
            "sat" | "satur" | "saturday" => Some(6),
            "sun" | "sunday" => Some(7),
            _ => None,
        }
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
