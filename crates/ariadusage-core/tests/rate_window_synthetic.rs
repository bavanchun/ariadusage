// Ported from CodexBar Tests/CodexBarTests/RateWindowSyntheticPlaceholderTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::RateWindow;
use jiff::Timestamp;

#[test]
fn synthetic_placeholder_flag_round_trips_through_codable() {
    let window = RateWindow::new(0.0, Some(300), None, None, None, true).expect("valid window");

    let json = serde_json::to_string(&window).expect("encode");
    let decoded: RateWindow = serde_json::from_str(&json).expect("decode");

    assert!(decoded.is_synthetic_placeholder);
    assert_eq!(decoded.used_percent, 0.0);
    assert_eq!(decoded.window_minutes, Some(300));
}

#[test]
fn older_payload_without_the_flag_decodes_as_not_a_placeholder() {
    let json = r#"{"usedPercent": 50, "windowMinutes": 300}"#;
    let decoded: RateWindow = serde_json::from_str(json).expect("decode");

    assert!(!decoded.is_synthetic_placeholder);
    assert_eq!(decoded.used_percent, 50.0);
    assert_eq!(decoded.window_minutes, Some(300));
}

#[test]
fn a_real_window_omits_the_placeholder_flag_when_encoded() {
    let window = RateWindow::new(50.0, Some(300), None, None, None, false).expect("valid window");

    let json = serde_json::to_string(&window).expect("encode");
    assert!(!json.contains("isSyntheticPlaceholder"));
}

#[test]
fn backfilling_a_reset_preserves_the_synthetic_placeholder_flag() {
    let now = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let cached_reset = Timestamp::from_second(1_800_000_000 + 3600).expect("timestamp");

    let cached = RateWindow::new(
        12.0,
        Some(300),
        Some(cached_reset),
        Some("Resets in 1h".to_string()),
        None,
        false,
    )
    .expect("cached window");

    let placeholder =
        RateWindow::new(0.0, Some(300), None, None, None, true).expect("placeholder window");

    let result = placeholder.backfilling_reset_time(Some(&cached), now);

    assert_eq!(result.resets_at, Some(cached_reset));
    assert!(result.is_synthetic_placeholder);
}
