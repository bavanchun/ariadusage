//! Date and number strings from Foundation's formatters in the user's locale
//! (and the `en_US` resource locale where CodexBar pins it).
//!
//! CodexBar formats user-locale dates with `Date.FormatStyle`,
//! `DateFormatter`, `NumberFormatter`, and `RelativeDateTimeFormatter` in
//! `Locale.current`, while `InlineUsageDashboardContent.formatDateRange` pins
//! its `DateFormatter` to `codexBarLocalizedResourceLocale()` (`en_US`).

use jiff::Timestamp;
use jiff::tz::TimeZone;
use objc2::rc::Retained;
use objc2_foundation::{
    NSDate, NSDateFormatter, NSLocale, NSNumber, NSNumberFormatter, NSNumberFormatterStyle,
    NSRelativeDateTimeFormatter, NSRelativeDateTimeFormatterUnitsStyle, NSString, NSTimeZone,
};

use crate::format::DateTexts;

/// Foundation-backed [`DateTexts`].
pub struct FoundationDateTexts {
    tz: TimeZone,
    hour_minute: Retained<NSDateFormatter>,
    month_day_hour_minute: Retained<NSDateFormatter>,
    month_day_year_local: Retained<NSDateFormatter>,
    month_day_year_utc: Retained<NSDateFormatter>,
    month_day_short: Retained<NSDateFormatter>,
    detail_month_day_time: Retained<NSDateFormatter>,
    en_us_hour_minute: Retained<NSDateFormatter>,
    en_us_month_day_hour_minute: Retained<NSDateFormatter>,
    en_us_year_month_day_hour_minute: Retained<NSDateFormatter>,
    one_decimal: Retained<NSNumberFormatter>,
    relative: Retained<NSRelativeDateTimeFormatter>,
}

impl Default for FoundationDateTexts {
    fn default() -> Self {
        Self::new()
    }
}

impl FoundationDateTexts {
    pub fn new() -> Self {
        let locale = NSLocale::currentLocale();
        Self::from_locale_and_tz(&locale, TimeZone::system())
    }

    /// Creates a `FoundationDateTexts` pinned to a specific locale identifier
    /// (such as `"fr_FR"`, `"en_GB"`, or `"en_US"`) and time zone.
    pub fn with_locale_and_tz(locale_id: &str, tz: TimeZone) -> Self {
        let locale = NSLocale::localeWithLocaleIdentifier(&NSString::from_str(locale_id));
        Self::from_locale_and_tz(&locale, tz)
    }

    fn from_locale_and_tz(locale: &NSLocale, tz: TimeZone) -> Self {
        let ns_tz = ns_time_zone(&tz);
        let utc_tz = NSTimeZone::timeZoneForSecondsFromGMT(0);
        let en_us = NSLocale::localeWithLocaleIdentifier(&NSString::from_str("en_US"));

        let make_formatter = |loc: &NSLocale, zone: &NSTimeZone, tmpl: &str| {
            let formatter = NSDateFormatter::new();
            formatter.setLocale(Some(loc));
            formatter.setTimeZone(Some(zone));
            formatter.setLocalizedDateFormatFromTemplate(&NSString::from_str(tmpl));
            formatter
        };

        let detail_month_day_time = make_formatter(locale, &ns_tz, "MMM d, h:mm a");
        let am_lower = detail_month_day_time.AMSymbol().to_string().to_lowercase();
        let pm_lower = detail_month_day_time.PMSymbol().to_string().to_lowercase();
        detail_month_day_time.setAMSymbol(Some(&NSString::from_str(&am_lower)));
        detail_month_day_time.setPMSymbol(Some(&NSString::from_str(&pm_lower)));

        let one_decimal = NSNumberFormatter::new();
        one_decimal.setLocale(Some(locale));
        one_decimal.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
        one_decimal.setMinimumFractionDigits(0);
        one_decimal.setMaximumFractionDigits(1);
        one_decimal.setUsesGroupingSeparator(true);

        let relative = NSRelativeDateTimeFormatter::new();
        relative.setLocale(Some(locale));
        relative.setUnitsStyle(NSRelativeDateTimeFormatterUnitsStyle::Abbreviated);

        Self {
            tz,
            hour_minute: make_formatter(locale, &ns_tz, "jmm"),
            month_day_hour_minute: make_formatter(locale, &ns_tz, "MMMdjmm"),
            month_day_year_local: make_formatter(locale, &ns_tz, "MMM d, yyyy"),
            month_day_year_utc: make_formatter(locale, &utc_tz, "MMM d, yyyy"),
            month_day_short: make_formatter(locale, &ns_tz, "MMMd"),
            detail_month_day_time,
            en_us_hour_minute: make_formatter(&en_us, &ns_tz, "jmm"),
            en_us_month_day_hour_minute: make_formatter(&en_us, &ns_tz, "MMMdjmm"),
            en_us_year_month_day_hour_minute: make_formatter(&en_us, &ns_tz, "yMMMdjmm"),
            one_decimal,
            relative,
        }
    }
}

fn ns_time_zone(tz: &TimeZone) -> Retained<NSTimeZone> {
    if *tz == TimeZone::UTC {
        return NSTimeZone::timeZoneForSecondsFromGMT(0);
    }
    if let Some(name) = tz.iana_name()
        && let Some(found) = NSTimeZone::timeZoneWithName(&NSString::from_str(name))
    {
        return found;
    }
    let offset = tz.to_offset(Timestamp::UNIX_EPOCH).seconds();
    NSTimeZone::timeZoneForSecondsFromGMT(offset as isize)
}

fn ns_date(date: Timestamp) -> Retained<NSDate> {
    NSDate::dateWithTimeIntervalSince1970(date.as_duration().as_secs_f64())
}

fn normalize_spaces(text: String) -> String {
    text.replace(['\u{202F}', '\u{00A0}'], " ")
}

impl DateTexts for FoundationDateTexts {
    fn time_zone(&self) -> TimeZone {
        self.tz.clone()
    }

    fn hour_minute(&self, date: Timestamp) -> String {
        normalize_spaces(self.hour_minute.stringFromDate(&ns_date(date)).to_string())
    }

    fn month_day_hour_minute(&self, date: Timestamp) -> String {
        normalize_spaces(
            self.month_day_hour_minute
                .stringFromDate(&ns_date(date))
                .to_string(),
        )
    }

    fn relative_abbreviated(&self, date: Timestamp, now: Timestamp) -> String {
        self.relative
            .localizedStringForDate_relativeToDate(&ns_date(date), &ns_date(now))
            .to_string()
    }

    fn month_day_year(&self, date: Timestamp, date_only: bool) -> String {
        let formatter = if date_only {
            &self.month_day_year_utc
        } else {
            &self.month_day_year_local
        };
        normalize_spaces(formatter.stringFromDate(&ns_date(date)).to_string())
    }

    fn month_day_short(&self, date: Timestamp) -> String {
        normalize_spaces(
            self.month_day_short
                .stringFromDate(&ns_date(date))
                .to_string(),
        )
    }

    fn detail_month_day_time(&self, date: Timestamp) -> String {
        normalize_spaces(
            self.detail_month_day_time
                .stringFromDate(&ns_date(date))
                .to_string(),
        )
    }

    fn quota_window_range(&self, start: Timestamp, end: Timestamp) -> String {
        let s_zoned = start.to_zoned(self.tz.clone());
        let e_zoned = end.to_zoned(self.tz.clone());
        let same_day = s_zoned.date() == e_zoned.date();
        let same_year = s_zoned.year() == e_zoned.year();

        let start_fmt = if same_year {
            &self.en_us_month_day_hour_minute
        } else {
            &self.en_us_year_month_day_hour_minute
        };
        let end_fmt = if same_day {
            &self.en_us_hour_minute
        } else if same_year {
            &self.en_us_month_day_hour_minute
        } else {
            &self.en_us_year_month_day_hour_minute
        };

        let start_str = normalize_spaces(start_fmt.stringFromDate(&ns_date(start)).to_string());
        let end_str = normalize_spaces(end_fmt.stringFromDate(&ns_date(end)).to_string());
        format!("{start_str} – {end_str}")
    }

    fn format_one_decimal(&self, value: f64) -> String {
        let rounded = (value * 10.0).round() / 10.0;
        let num = NSNumber::new_f64(rounded);
        self.one_decimal
            .stringFromNumber(&num)
            .map(|s| s.to_string())
            .unwrap_or_else(|| crate::format::format_one_decimal_en_us(rounded))
    }
}
