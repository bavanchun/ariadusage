// Ported from CodexBar Tests/CodexBarTests/ClaudeSubscriptionMetadataTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::{RateWindow, UsageSnapshot};
use ariadusage_protocol::Confidence;
use jiff::Timestamp;

fn empty_snapshot() -> UsageSnapshot {
    UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        Timestamp::from_second(1_793_836_800).expect("timestamp"),
        None,
        Confidence::Unknown,
    )
    .unwrap()
}

#[test]
fn date_precision_survives_snapshot_replacement_serialization_and_older_json() {
    let day = Timestamp::from_second(1_793_836_800).expect("timestamp");
    let snapshot = empty_snapshot().with_subscription_metadata(Some(day), None, true, false);

    let json = serde_json::to_value(&snapshot).expect("serializes");
    assert_eq!(json["subscriptionExpiresAtIsDateOnly"], true);
    assert!(json.get("subscriptionRenewsAtIsDateOnly").is_none());

    let roundtrip: UsageSnapshot = serde_json::from_value(json).expect("deserializes");
    assert!(roundtrip.subscription_expires_at_is_date_only);
    assert!(!roundtrip.subscription_renews_at_is_date_only);
    assert!(
        roundtrip
            .clone()
            .with_rate_windows(None, None)
            .subscription_expires_at_is_date_only
    );
    assert!(
        !roundtrip
            .with_subscription_metadata(None, None, false, false)
            .subscription_expires_at_is_date_only
    );

    let old: UsageSnapshot =
        serde_json::from_str(r#"{"updatedAt":"2026-11-05T00:00:00Z"}"#).expect("decodes");
    assert!(!old.subscription_expires_at_is_date_only && !old.subscription_renews_at_is_date_only);
}

#[test]
fn false_flags_are_omitted_and_renewal_flag_round_trips() {
    let day = Timestamp::from_second(1_793_836_800).expect("timestamp");
    let plain = empty_snapshot().with_rate_windows(
        Some(RateWindow::new(10.0, Some(300), None, None, None, false).unwrap()),
        None,
    );
    let plain_json = serde_json::to_string(&plain).expect("serializes");
    assert!(!plain_json.contains("IsDateOnly"));

    let renewal = plain.with_subscription_metadata(None, Some(day), false, true);
    let json = serde_json::to_value(&renewal).expect("serializes");
    assert_eq!(json["subscriptionRenewsAtIsDateOnly"], true);
    assert!(json.get("subscriptionExpiresAtIsDateOnly").is_none());
    let decoded: UsageSnapshot = serde_json::from_value(json).expect("deserializes");
    assert_eq!(decoded, renewal);

    let explicit_false: UsageSnapshot = serde_json::from_str(
        r#"{"updatedAt":"2026-11-05T00:00:00Z","subscriptionRenewsAtIsDateOnly":false}"#,
    )
    .expect("decodes");
    assert!(!explicit_false.subscription_renews_at_is_date_only);
    assert!(
        !serde_json::to_string(&explicit_false)
            .unwrap()
            .contains("IsDateOnly")
    );
}
