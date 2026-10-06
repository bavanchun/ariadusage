// Ported from CodexBar Tests/CodexBarTests/ProviderDetailSectionTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::UsageSnapshot;

#[test]
fn decoding_rejects_too_many_sections() {
    let section = r#"{"rows":[]}"#;
    let details = (0..9).map(|_| section).collect::<Vec<_>>().join(",");
    let json = format!(
        r#"{{
          "primary": null,
          "secondary": null,
          "tertiary": null,
          "details": [{details}],
          "updatedAt": "2026-08-02T12:00:00Z"
        }}"#
    );

    let decoded = serde_json::from_str::<UsageSnapshot>(&json);
    assert!(decoded.is_err());
}

#[test]
fn current_snapshot_fixture_decodes_with_empty_details() {
    let fixture = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/codexbar/usage-snapshot-current.json"
    ));

    let snapshot: UsageSnapshot = serde_json::from_str(fixture).expect("decode snapshot fixture");

    assert_eq!(
        snapshot.primary.as_ref().map(|p| p.used_percent),
        Some(42.0)
    );
    assert_eq!(
        snapshot
            .identity
            .as_ref()
            .and_then(|i| i.provider_id.as_ref())
            .map(|id| id.as_str()),
        Some("synthetic")
    );
    assert!(snapshot.details.is_empty());

    let encoded = serde_json::to_string(&snapshot).expect("encode snapshot");
    assert!(!encoded.contains("\"details\""));
}
