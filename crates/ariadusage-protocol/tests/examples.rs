#[path = "support/mod.rs"]
mod support;

use std::fs;

use ariadusage_protocol::ipc::{IpcMessage, parse_client_message};
use ariadusage_protocol::snapshot::EngineSnapshot;
use jsonschema::validator_for;

#[test]
fn valid_examples_validate_and_round_trip() {
    let snapshot_schema_str =
        fs::read_to_string(support::snapshot_schema_path()).expect("read snapshot schema file");
    let snapshot_schema: serde_json::Value =
        serde_json::from_str(&snapshot_schema_str).expect("parse snapshot schema");
    let snapshot_validator = validator_for(&snapshot_schema).expect("compile snapshot validator");

    let ipc_schema_str =
        fs::read_to_string(support::ipc_schema_path()).expect("read ipc schema file");
    let ipc_schema: serde_json::Value =
        serde_json::from_str(&ipc_schema_str).expect("parse ipc schema");
    let ipc_validator = validator_for(&ipc_schema).expect("compile ipc validator");

    let dir = support::examples_dir();
    let entries = fs::read_dir(&dir).unwrap_or_else(|e| panic!("read dir {}: {e}", dir.display()));

    let mut count = 0;
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
            let filename = path.file_name().unwrap().to_str().unwrap();
            let content = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read file {}: {e}", path.display()));
            let val: serde_json::Value = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("parse json in {filename}: {e}"));

            if val.get("schemaVersion").is_some() {
                // Must validate against snapshot schema
                if !snapshot_validator.is_valid(&val) {
                    let errors: Vec<_> = snapshot_validator.iter_errors(&val).collect();
                    panic!("valid example {filename} failed snapshot schema: {errors:?}");
                }
                // Must survive serde round trip
                let decoded: EngineSnapshot = serde_json::from_str(&content)
                    .unwrap_or_else(|e| panic!("deserialize {filename}: {e}"));
                let encoded = serde_json::to_string(&decoded)
                    .unwrap_or_else(|e| panic!("re-serialize {filename}: {e}"));
                let re_decoded: EngineSnapshot = serde_json::from_str(&encoded)
                    .unwrap_or_else(|e| panic!("re-deserialize {filename}: {e}"));
                assert_eq!(decoded, re_decoded, "round trip mismatch for {filename}");
                count += 1;
            } else if val.get("type").is_some() {
                // Must validate against ipc schema
                if !ipc_validator.is_valid(&val) {
                    for err in ipc_validator.iter_errors(&val) {
                        eprintln!(
                            "VALIDATION ERROR in {filename}: {err} at {}",
                            err.instance_path()
                        );
                    }
                    panic!("valid example {filename} failed ipc schema");
                }
                // Must survive serde round trip
                let decoded: IpcMessage = serde_json::from_str(&content)
                    .unwrap_or_else(|e| panic!("deserialize {filename}: {e}"));
                let encoded = serde_json::to_string(&decoded)
                    .unwrap_or_else(|e| panic!("re-serialize {filename}: {e}"));
                let re_decoded: IpcMessage = serde_json::from_str(&encoded)
                    .unwrap_or_else(|e| panic!("re-deserialize {filename}: {e}"));
                assert_eq!(decoded, re_decoded, "round trip mismatch for {filename}");
                count += 1;
            } else {
                panic!("example file {filename} has neither schemaVersion nor type");
            }
        }
    }

    assert!(
        count >= 28,
        "expected at least 28 valid examples, found {count}"
    );
}

#[test]
fn invalid_examples_fail_schema_and_decoding() {
    let snapshot_schema_str =
        fs::read_to_string(support::snapshot_schema_path()).expect("read snapshot schema file");
    let snapshot_schema: serde_json::Value =
        serde_json::from_str(&snapshot_schema_str).expect("parse snapshot schema");
    let snapshot_validator = validator_for(&snapshot_schema).expect("compile snapshot validator");

    let ipc_schema_str =
        fs::read_to_string(support::ipc_schema_path()).expect("read ipc schema file");
    let ipc_schema: serde_json::Value =
        serde_json::from_str(&ipc_schema_str).expect("parse ipc schema");
    let ipc_validator = validator_for(&ipc_schema).expect("compile ipc validator");

    let dir = support::invalid_examples_dir();
    let entries = fs::read_dir(&dir).unwrap_or_else(|e| panic!("read dir {}: {e}", dir.display()));

    let mut count = 0;
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
            let filename = path.file_name().unwrap().to_str().unwrap();
            let content = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read file {}: {e}", path.display()));
            let val: serde_json::Value = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("parse json in {filename}: {e}"));

            if val.get("schemaVersion").is_some() {
                assert!(
                    !snapshot_validator.is_valid(&val),
                    "invalid example {filename} unexpectedly PASSED snapshot schema"
                );
                assert!(
                    serde_json::from_str::<EngineSnapshot>(&content).is_err(),
                    "invalid example {filename} unexpectedly PASSED EngineSnapshot deserialization"
                );
                count += 1;
            } else if val.get("type").is_some() {
                assert!(
                    !ipc_validator.is_valid(&val),
                    "invalid example {filename} unexpectedly PASSED ipc schema"
                );
                assert!(
                    parse_client_message(&content).is_err()
                        && serde_json::from_str::<IpcMessage>(&content).is_err(),
                    "invalid example {filename} unexpectedly PASSED IPC decoding"
                );
                count += 1;
            } else {
                panic!("invalid example file {filename} has neither schemaVersion nor type");
            }
        }
    }

    assert!(
        count >= 5,
        "expected at least 5 invalid examples, found {count}"
    );
}
