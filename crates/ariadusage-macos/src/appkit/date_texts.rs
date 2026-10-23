//! Date strings from Foundation's formatters in the user's locale.
//!
//! CodexBar formats these with `Date.FormatStyle` and
//! `RelativeDateTimeFormatter` in the app locale, which is the system locale
//! unless the user picked an app language. `NSDateFormatter` templates
//! `jmm` and `MMMdjmm` produce the same strings as the `.dateTime.hour()
//! .minute()` and `.dateTime.month(.abbreviated).day().hour().minute()`
//! styles.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use objc2::rc::Retained;
use objc2_foundation::{
    NSDate, NSDateFormatter, NSRelativeDateTimeFormatter, NSRelativeDateTimeFormatterUnitsStyle,
    NSString,
};

use crate::format::DateTexts;

/// Foundation-backed [`DateTexts`].
pub struct FoundationDateTexts {
    hour_minute: Retained<NSDateFormatter>,
    month_day_hour_minute: Retained<NSDateFormatter>,
    relative: Retained<NSRelativeDateTimeFormatter>,
}

impl Default for FoundationDateTexts {
    fn default() -> Self {
        Self::new()
    }
}

impl FoundationDateTexts {
    pub fn new() -> Self {
        let template = |t: &str| {
            let formatter = NSDateFormatter::new();
            formatter.setLocalizedDateFormatFromTemplate(&NSString::from_str(t));
            formatter
        };
        let relative = NSRelativeDateTimeFormatter::new();
        relative.setUnitsStyle(NSRelativeDateTimeFormatterUnitsStyle::Abbreviated);
        Self {
            hour_minute: template("jmm"),
            month_day_hour_minute: template("MMMdjmm"),
            relative,
        }
    }
}

fn ns_date(date: Timestamp) -> Retained<NSDate> {
    NSDate::dateWithTimeIntervalSince1970(date.as_duration().as_secs_f64())
}

impl DateTexts for FoundationDateTexts {
    fn time_zone(&self) -> TimeZone {
        TimeZone::system()
    }

    fn hour_minute(&self, date: Timestamp) -> String {
        self.hour_minute.stringFromDate(&ns_date(date)).to_string()
    }

    fn month_day_hour_minute(&self, date: Timestamp) -> String {
        self.month_day_hour_minute
            .stringFromDate(&ns_date(date))
            .to_string()
    }

    fn relative_abbreviated(&self, date: Timestamp, now: Timestamp) -> String {
        self.relative
            .localizedStringForDate_relativeToDate(&ns_date(date), &ns_date(now))
            .to_string()
    }
}
