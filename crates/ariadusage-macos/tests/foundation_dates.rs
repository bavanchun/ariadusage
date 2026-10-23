//! The Foundation-backed date strings, in whatever locale the machine uses.

#![cfg(target_os = "macos")]

use ariadusage_macos::appkit::FoundationDateTexts;
use ariadusage_macos::format::{DateTexts, ResetTimeDisplayStyle, reset_line, updated_string};
use ariadusage_protocol::usage::RateWindow;
use jiff::{SignedDuration, Timestamp};

fn now() -> Timestamp {
    Timestamp::from_second(1_790_000_000).unwrap()
}

#[test]
fn relative_updated_string_comes_from_the_relative_formatter() {
    let dates = FoundationDateTexts::new();
    let text = updated_string(now() - SignedDuration::from_hours(5), now(), &dates);
    assert!(text.starts_with("Updated "), "{text}");
    assert!(text.contains('5'), "{text}");
    assert_ne!(text, "Updated just now");
}

#[test]
fn absolute_strings_include_the_time() {
    let dates = FoundationDateTexts::new();
    let time = dates.hour_minute(now());
    assert!(time.chars().any(|c| c.is_ascii_digit()), "{time}");
    let long = dates.month_day_hour_minute(now());
    assert!(long.len() > time.len(), "{long}");
    let window = RateWindow {
        resets_at: Some(now() + SignedDuration::from_hours(72)),
        ..RateWindow::new(10.0)
    };
    let line = reset_line(&window, ResetTimeDisplayStyle::Absolute, now(), &dates).unwrap();
    assert_eq!(
        line,
        format!(
            "Resets {}",
            dates.month_day_hour_minute(now() + SignedDuration::from_hours(72))
        )
    );
}
