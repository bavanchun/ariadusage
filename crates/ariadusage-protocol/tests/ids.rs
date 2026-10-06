// Ported from CodexBar Tests/CodexBarTests/ProviderInstanceIDTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;

use ariadusage_protocol::ProviderId;

#[test]
fn accepts_ids_from_the_declared_ascii_grammar() {
    let valid_cases = ["a", "codex", "acme-gateway", &"a".repeat(64), "0-9"];

    for raw in valid_cases {
        let id = ProviderId::new(raw).expect("expected valid provider ID");
        assert_eq!(id.as_str(), raw);
    }
}

#[test]
fn rejects_ids_outside_the_declared_ascii_grammar() {
    let invalid_cases = [
        "",
        "UPPERCASE",
        "under_score",
        "contains space",
        "café",
        "slash/value",
        &"a".repeat(65),
    ];

    for raw in invalid_cases {
        assert!(
            ProviderId::new(raw).is_err(),
            "expected '{raw}' to be rejected"
        );
    }
}

#[test]
fn codable_representation_is_a_validated_bare_string() {
    let id = ProviderId::new("acme-gateway").expect("valid id");
    let json = serde_json::to_string(&id).expect("serializes");
    assert_eq!(json, "\"acme-gateway\"");

    let decoded: ProviderId = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(decoded, id);

    let invalid_json = "\"Acme\"";
    assert!(serde_json::from_str::<ProviderId>(invalid_json).is_err());
}

#[test]
fn distinct_instance_ids_isolate_runtime_state() {
    let first = ProviderId::new("acme-primary").expect("valid id");
    let second = ProviderId::new("acme-secondary").expect("valid id");

    let mut snapshots = HashMap::new();
    let mut errors = HashMap::new();

    snapshots.insert(first.clone(), "first snapshot");
    snapshots.insert(second.clone(), "second snapshot");
    errors.insert(first.clone(), "first error");

    assert_eq!(snapshots.get(&first), Some(&"first snapshot"));
    assert_eq!(snapshots.get(&second), Some(&"second snapshot"));
    assert_eq!(errors.get(&first), Some(&"first error"));
    assert_eq!(errors.get(&second), None);
}
