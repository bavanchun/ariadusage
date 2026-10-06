// Ported from CodexBar Tests/CodexBarTests/AntigravityStatusProbeTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::NamedRateWindow;

#[test]
fn named_rate_windows_default_legacy_payloads_to_known_usage() {
    let json = r#"{
      "id": "legacy-window",
      "title": "Legacy Window",
      "window": {
        "usedPercent": 42,
        "windowMinutes": null,
        "resetsAt": null,
        "resetDescription": null,
        "nextRegenPercent": null
      }
    }"#;

    let decoded: NamedRateWindow = serde_json::from_str(json).expect("decode");
    assert!(decoded.usage_known);
}
