// Ported from CodexBar Tests/CodexBarTests/ProviderDiagnosticExportTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::{RateWindow, UsageSnapshot};
use ariadusage_protocol::Confidence;
use jiff::Timestamp;

#[test]
fn usage_snapshot_defaults_legacy_payloads_to_unknown_confidence_without_reencoding_unknown() {
    let json = r#"{
      "primary": {
        "usedPercent": 42,
        "windowMinutes": 300
      },
      "secondary": null,
      "tertiary": null,
      "updatedAt": "2023-11-14T22:13:20Z"
    }"#;

    let snapshot: UsageSnapshot = serde_json::from_str(json).expect("decode");
    assert_eq!(snapshot.data_confidence, Confidence::Unknown);

    let encoded = serde_json::to_string(&snapshot).expect("encode");
    assert!(!encoded.contains("dataConfidence"));
}

#[test]
fn usage_snapshot_preserves_explicit_confidence_through_codable() {
    let now = Timestamp::from_second(1_700_000_000).expect("timestamp");
    let reset = Timestamp::from_second(1_700_000_000 + 18000).expect("timestamp");
    let snapshot = UsageSnapshot::new(
        Some(RateWindow::new(12.0, Some(300), Some(reset), None, None, false).unwrap()),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();

    let encoded = serde_json::to_string(&snapshot).expect("encode");
    assert!(encoded.contains(r#""dataConfidence":"exact""#));

    let decoded: UsageSnapshot = serde_json::from_str(&encoded).expect("decode");
    assert_eq!(decoded.data_confidence, Confidence::Exact);
}

#[test]
fn usage_snapshot_treats_future_confidence_values_as_unknown() {
    let json = r#"{
      "primary": null,
      "secondary": null,
      "tertiary": null,
      "updatedAt": "2023-11-14T22:13:20Z",
      "dataConfidence": "future"
    }"#;

    let snapshot: UsageSnapshot = serde_json::from_str(json).expect("decode");
    assert_eq!(snapshot.data_confidence, Confidence::Unknown);

    let encoded = serde_json::to_string(&snapshot).expect("encode");
    assert!(!encoded.contains("dataConfidence"));
}
