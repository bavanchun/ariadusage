// Ported from CodexBar Sources/CodexBarCore/UsageFormatter.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_macos::format::{percent_string, percent_text, usage_line};

#[test]
fn percent_string_clamps_and_marks_tiny_values() {
    assert_eq!(percent_string(0.0), "0%");
    assert_eq!(percent_string(0.4), "<1%");
    assert_eq!(percent_string(0.999), "<1%");
    assert_eq!(percent_string(1.0), "1%");
    assert_eq!(percent_string(58.4), "58%");
    assert_eq!(percent_string(120.0), "100%");
    assert_eq!(percent_string(-3.0), "0%");
}

#[test]
fn percent_string_rounds_ties_to_even_like_printf() {
    assert_eq!(percent_string(2.5), "2%");
    assert_eq!(percent_string(3.5), "4%");
}

#[test]
fn usage_line_picks_remaining_or_used() {
    assert_eq!(usage_line(58.0, 42.0, false), "58% left");
    assert_eq!(usage_line(58.0, 42.0, true), "42% used");
    assert_eq!(percent_text(0.5, "left"), "<1% left");
}
