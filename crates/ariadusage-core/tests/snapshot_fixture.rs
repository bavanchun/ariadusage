// Ported from CodexBar Tests/CodexBarTests/ProviderDetailSectionTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::UsageSnapshot;
use ariadusage_protocol::Confidence;
use serde_json::Value;

#[test]
fn usage_snapshot_current_fixture_round_trip() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/codexbar/usage-snapshot-current.json"
    ));

    let snapshot: UsageSnapshot = serde_json::from_str(raw).expect("decode snapshot fixture");

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
    assert_eq!(
        snapshot
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("fixture@example.com")
    );
    assert_eq!(
        snapshot
            .identity
            .as_ref()
            .and_then(|i| i.account_organization.as_deref()),
        Some("Fixture Org")
    );
    assert_eq!(
        snapshot
            .identity
            .as_ref()
            .and_then(|i| i.login_method.as_deref()),
        Some("API key")
    );
    assert_eq!(snapshot.data_confidence, Confidence::Exact);
    assert!(snapshot.details.is_empty());

    let encoded_str = serde_json::to_string(&snapshot).expect("encode snapshot");
    assert!(!encoded_str.contains("\"details\""));

    let mut original_val: Value = serde_json::from_str(raw).expect("parse original json");
    let mut reencoded_val: Value =
        serde_json::from_str(&encoded_str).expect("parse re-encoded json");

    // Remove known-omitted keys ("resetDescription": null is omitted on re-encode by Swift and Rust)
    if let Some(primary) = original_val
        .get_mut("primary")
        .and_then(Value::as_object_mut)
        && primary.get("resetDescription") == Some(&Value::Null)
    {
        primary.remove("resetDescription");
    }

    normalize_numbers(&mut original_val);
    normalize_numbers(&mut reencoded_val);

    assert_eq!(original_val, reencoded_val);
}

fn normalize_numbers(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64()
                && let Some(normalized) = serde_json::Number::from_f64(f)
            {
                *n = normalized;
            }
        }
        Value::Array(arr) => {
            for item in arr {
                normalize_numbers(item);
            }
        }
        Value::Object(map) => {
            for val in map.values_mut() {
                normalize_numbers(val);
            }
        }
        _ => {}
    }
}
