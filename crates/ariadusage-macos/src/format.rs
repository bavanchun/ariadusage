// Ported from CodexBar Sources/CodexBarCore/UsageFormatter.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Text formats shared by the menu and the status item.

/// Suffix for remaining percentages (`usage_percent_suffix_left`).
pub const SUFFIX_LEFT: &str = "left";
/// Suffix for used percentages (`usage_percent_suffix_used`).
pub const SUFFIX_USED: &str = "used";

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
