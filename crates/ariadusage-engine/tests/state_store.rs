// Ported from CodexBar Sources/CodexBarCore/ProviderSessionStoreFile.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use ariadusage_core::gates::delegated_cooldown::CooldownState;
use ariadusage_engine::brokers::credential_file::StatFingerprint;
use ariadusage_engine::state_store::{BrokerState, BrokerStateStore, StateWarning};
use jiff::Timestamp;

#[test]
fn test_broker_state_absent_loads_empty() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_file = temp.path().join("broker-state.json");
    let store = BrokerStateStore::new(state_file);

    let state = store.load().expect("loads empty");
    assert_eq!(state, BrokerState::empty());
    assert_eq!(store.warning_count(), 0);
}

#[test]
fn test_broker_state_is_0600_and_contains_no_tokens_or_path_text() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_file = temp.path().join("broker-state.json");
    let store = BrokerStateStore::new(&state_file);

    let profile_digest = "a".repeat(64);
    let mut state = BrokerState::empty();
    state.last_seen.insert(
        profile_digest.clone(),
        StatFingerprint {
            path: PathBuf::from("/home/secretuser/.claude/credentials.json"),
            dev: 10,
            ino: 200,
            mtime_ns: 123456789,
            size: 512,
        },
    );
    state.quarantine.insert(
        profile_digest.clone(),
        StatFingerprint {
            path: PathBuf::from("/home/secretuser/.claude/credentials.json"),
            dev: 10,
            ino: 200,
            mtime_ns: 123456789,
            size: 512,
        },
    );
    state.delegated_cooldowns.insert(
        profile_digest.clone(),
        CooldownState::new(Timestamp::now(), std::time::Duration::from_secs(300)),
    );

    store.save(&state).expect("save state");

    // 1. Enforces 0600 permissions
    let meta = std::fs::metadata(&state_file).expect("metadata");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "broker state must be written with 0600 mode");

    // 2. Contains NO tokens or path text
    let content = std::fs::read_to_string(&state_file).expect("read file");
    assert!(
        !content.contains("secretuser"),
        "Persisted state must not contain username / path text: {}",
        content
    );
    assert!(
        !content.contains("/home"),
        "Persisted state must not contain path prefix: {}",
        content
    );
    assert!(
        !content.contains("credentials.json"),
        "Persisted state must not contain credential file name: {}",
        content
    );
    assert!(
        !content.contains("sk-ant-"),
        "Persisted state must not contain secret tokens: {}",
        content
    );
    assert!(
        content.contains(&profile_digest),
        "Persisted state should be keyed by profile digest"
    );
}

#[test]
fn test_broker_state_0644_file_is_repaired_and_counted() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_file = temp.path().join("broker-state.json");
    let store = BrokerStateStore::new(&state_file);

    store.save(&BrokerState::empty()).expect("save initial");

    // Loosen permissions to 0644
    std::fs::set_permissions(&state_file, std::fs::Permissions::from_mode(0o644))
        .expect("set 0644");
    assert_eq!(
        std::fs::metadata(&state_file).unwrap().permissions().mode() & 0o777,
        0o644
    );

    // Load should repair permissions to 0600 and record a warning
    let loaded = store.load().expect("load repairs");
    assert_eq!(loaded, BrokerState::empty());

    let repaired_mode = std::fs::metadata(&state_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        repaired_mode, 0o600,
        "File permissions must be repaired to 0600"
    );
    assert_eq!(store.warning_count(), 1);
    assert_eq!(store.warnings(), vec![StateWarning::PermissionsRepaired]);
}

#[test]
fn test_corrupt_broker_state_resets_with_counted_warning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_file = temp.path().join("broker-state.json");
    let store = BrokerStateStore::new(&state_file);

    // Write corrupted JSON content
    std::fs::write(&state_file, b"corrupted { not json content").expect("write corrupt");
    std::fs::set_permissions(&state_file, std::fs::Permissions::from_mode(0o600))
        .expect("set 0600");

    let loaded = store.load().expect("load resets corrupt state");
    assert_eq!(loaded, BrokerState::empty());
    assert_eq!(store.warning_count(), 1);
    assert_eq!(store.warnings(), vec![StateWarning::CorruptStateReset]);
}

#[test]
fn test_unknown_schema_version_resets_with_counted_warning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_file = temp.path().join("broker-state.json");
    let store = BrokerStateStore::new(&state_file);

    // Write state with unknown schema version 999
    let bad_version_json =
        r#"{"schema_version": 999, "last_seen": {}, "quarantine": {}, "delegated_cooldowns": {}}"#;
    std::fs::write(&state_file, bad_version_json.as_bytes()).expect("write bad version");
    std::fs::set_permissions(&state_file, std::fs::Permissions::from_mode(0o600))
        .expect("set 0600");

    let loaded = store.load().expect("load resets unknown schema");
    assert_eq!(loaded, BrokerState::empty());
    assert_eq!(store.warning_count(), 1);
    assert_eq!(
        store.warnings(),
        vec![StateWarning::UnknownVersionReset(999)]
    );
}

#[test]
fn test_broker_state_update_persists_changes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_file = temp.path().join("broker-state.json");
    let store = BrokerStateStore::new(&state_file);

    let profile = "b".repeat(64);
    store
        .update(|state| {
            state.last_seen.insert(
                profile.clone(),
                StatFingerprint {
                    path: PathBuf::new(),
                    dev: 1,
                    ino: 2,
                    mtime_ns: 3,
                    size: 4,
                },
            );
        })
        .expect("update");

    let loaded = store.load().expect("load updated state");
    assert!(loaded.last_seen.contains_key(&profile));
    assert_eq!(loaded.last_seen.get(&profile).unwrap().ino, 2);
}
