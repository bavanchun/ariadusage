// Ported from CodexBar Tests/CodexBarTests/ProviderConfigByteStabilityTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::fs;
use std::path::Path;

use ariadusage_core::config::{decode, encode};

#[test]
fn test_current_provider_config_full_fixture_reencodes_byte_for_byte() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parent")
        .parent()
        .expect("workspace root");
    let path = workspace_root.join("fixtures/ariadusage/config/full.json");
    let fixture_bytes = fs::read(&path).expect("fixture file exists");

    let config = decode(&fixture_bytes)
        .expect("decode ok")
        .expect("config present");
    let encoded = encode(&config);

    let expected = fixture_bytes.strip_suffix(b"\n").unwrap_or(&fixture_bytes);
    assert_eq!(
        std::str::from_utf8(&encoded).unwrap(),
        std::str::from_utf8(expected).unwrap()
    );
    assert_eq!(encoded, expected);
}

#[test]
fn test_current_provider_config_sparse_fixture_reencodes_byte_for_byte() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parent")
        .parent()
        .expect("workspace root");
    let path = workspace_root.join("fixtures/ariadusage/config/sparse.json");
    let fixture_bytes = fs::read(&path).expect("fixture file exists");

    let config = decode(&fixture_bytes)
        .expect("decode ok")
        .expect("config present");
    let encoded = encode(&config);

    let expected = fixture_bytes.strip_suffix(b"\n").unwrap_or(&fixture_bytes);
    assert_eq!(
        std::str::from_utf8(&encoded).unwrap(),
        std::str::from_utf8(expected).unwrap()
    );
    assert_eq!(encoded, expected);
}
