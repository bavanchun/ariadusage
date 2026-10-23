use ariadusage_protocol::{ProviderId, ProviderPresentation, ProviderSnapshot};
use serde_json::json;

fn provider_json(presentation: Option<serde_json::Value>) -> serde_json::Value {
    let mut value = json!({
        "id": "claude",
        "displayName": "Claude",
        "enabled": true,
        "sourceMode": "cli"
    });
    if let Some(presentation) = presentation {
        value["presentation"] = presentation;
    }
    value
}

#[test]
fn payload_without_presentation_still_parses() {
    let snapshot: ProviderSnapshot =
        serde_json::from_value(provider_json(None)).expect("old payload parses");
    assert_eq!(snapshot.presentation, None);

    let encoded = serde_json::to_value(&snapshot).expect("serialize");
    assert!(encoded.get("presentation").is_none(), "None is skipped");
}

#[test]
fn presentation_round_trips_in_camel_case() {
    let mut snapshot =
        ProviderSnapshot::new(ProviderId::new("claude").unwrap(), "Claude", true, "cli");
    snapshot.presentation = Some(
        ProviderPresentation::new(
            "Claude",
            "Session",
            "Weekly",
            Some("Sonnet".to_string()),
            Some("#CC7C5E".to_string()),
        )
        .unwrap(),
    );

    let encoded = serde_json::to_value(&snapshot).expect("serialize");
    assert_eq!(
        encoded["presentation"],
        json!({
            "shortDisplayName": "Claude",
            "sessionLabel": "Session",
            "weeklyLabel": "Weekly",
            "tertiaryLabel": "Sonnet",
            "accentColor": "#CC7C5E"
        })
    );

    let decoded: ProviderSnapshot = serde_json::from_value(encoded).expect("deserialize");
    assert_eq!(decoded, snapshot);
}

#[test]
fn optional_presentation_fields_are_skipped_when_absent() {
    let presentation = ProviderPresentation::new("Codex", "Session", "Weekly", None, None).unwrap();
    let encoded = serde_json::to_value(&presentation).expect("serialize");
    assert_eq!(
        encoded,
        json!({
            "shortDisplayName": "Codex",
            "sessionLabel": "Session",
            "weeklyLabel": "Weekly"
        })
    );
}

#[test]
fn accent_color_must_be_uppercase_rrggbb() {
    for valid in ["#000000", "#CC7C5E", "#49A3B0", "#FFFFFF"] {
        assert!(
            ProviderPresentation::validate_accent_color(valid).is_ok(),
            "{valid} should be accepted"
        );
    }
    for invalid in [
        "",
        "#",
        "#cc7c5e",
        "#CC7C5",
        "#CC7C5E0",
        "CC7C5E",
        "#GGGGGG",
        "#CC7C5E ",
        " #CC7C5E",
        "rgb(204,124,94)",
        "#ＣＣ7C5E",
    ] {
        assert!(
            ProviderPresentation::validate_accent_color(invalid).is_err(),
            "{invalid:?} should be rejected"
        );
    }
}

#[test]
fn invalid_accent_color_is_rejected_on_construction_and_deserialize() {
    assert!(
        ProviderPresentation::new("Claude", "Session", "Weekly", None, Some("#cc7c5e".into()))
            .is_err()
    );

    let payload = provider_json(Some(json!({
        "shortDisplayName": "Claude",
        "sessionLabel": "Session",
        "weeklyLabel": "Weekly",
        "accentColor": "#cc7c5e"
    })));
    assert!(serde_json::from_value::<ProviderSnapshot>(payload).is_err());
}

#[test]
fn accent_color_from_rgb_formats_uppercase_hex() {
    assert_eq!(
        ProviderPresentation::accent_color_from_rgb(204, 124, 94),
        "#CC7C5E"
    );
    assert_eq!(
        ProviderPresentation::accent_color_from_rgb(73, 163, 176),
        "#49A3B0"
    );
    assert_eq!(
        ProviderPresentation::accent_color_from_rgb(96, 186, 126),
        "#60BA7E"
    );
    assert_eq!(
        ProviderPresentation::accent_color_from_rgb(0, 0, 0),
        "#000000"
    );
}
