// Ported from CodexBar TestsLinux/CodexBarConfigStoreEmptyTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use ariadusage_core::config::{Config, ProviderConfig, normalize};
use ariadusage_engine::config_store::{ConfigStore, UpdateResult};
use ariadusage_engine::error::StoreError;

fn file_mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

struct Fixture {
    directory: PathBuf,
    store: ConfigStore,
}

impl Fixture {
    fn new(contents: Option<&str>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("store_test");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

        let config_path = dir.join("config.json");
        if let Some(c) = contents {
            fs::write(&config_path, c).unwrap();
            fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
        }

        let store = ConfigStore::new(config_path);
        // Leak the tempdir handle so it is kept alive until Fixture is dropped manually
        std::mem::forget(temp);

        Self {
            directory: dir,
            store,
        }
    }

    fn contents(&self) -> Option<Vec<u8>> {
        if self.store.path().exists() {
            Some(fs::read(self.store.path()).unwrap())
        } else {
            None
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn missing_and_blank_configs_load_as_absent_without_writing() {
    let cases = [None, Some(""), Some(" \t\r\n ")];
    let expected_default = normalize(Config::new(Config::CURRENT_VERSION, Vec::new()));

    for contents in cases {
        let fixture = Fixture::new(contents);
        assert_eq!(fixture.store.load().unwrap(), None);

        let effective = fixture.store.load_effective().unwrap();
        assert_eq!(effective, expected_default);

        assert_eq!(fixture.contents(), contents.map(|s| s.as_bytes().to_vec()));
    }
}

#[test]
fn default_creation_and_subsequent_settings_save_produce_private_valid_json() {
    let cases = [None, Some(""), Some(" \t\r\n ")];
    let expected_default = normalize(Config::new(Config::CURRENT_VERSION, Vec::new()));

    for contents in cases {
        let fixture = Fixture::new(contents);

        let mut config = fixture.store.load_or_default().unwrap();
        assert_eq!(config, expected_default);
        assert_eq!(file_mode(fixture.store.path()), 0o600);

        // Modify config
        let mut custom =
            ProviderConfig::new(ariadusage_protocol::ProviderId::new("custom-test-prov").unwrap());
        custom.enabled = Some(true);
        config.set_provider_config(custom);

        fixture.store.save(&config).unwrap();

        let loaded = fixture.store.load().unwrap().unwrap();
        assert!(
            loaded
                .providers
                .iter()
                .any(|p| p.id() == "custom-test-prov" && p.is_enabled())
        );
        assert_eq!(file_mode(fixture.store.path()), 0o600);
    }
}

#[test]
fn nonempty_malformed_configs_still_fail_closed_and_retain_their_bytes() {
    let cases = ["{", " \n{\"providers\":", "null", "garbage", "\u{0000}"];

    for contents in cases {
        let fixture = Fixture::new(Some(contents));

        assert!(matches!(fixture.store.load(), Err(StoreError::Decode(_))));
        assert!(matches!(
            fixture.store.load_or_default(),
            Err(StoreError::Decode(_))
        ));
        assert!(matches!(
            fixture.store.load_effective(),
            Err(StoreError::Decode(_))
        ));

        assert_eq!(fixture.contents().unwrap(), contents.as_bytes());
    }
}

#[test]
fn symlink_config_file_and_lock_file_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("symlink_test");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

    // 1. Config file is a symlink
    let target = dir.join("real_config.json");
    fs::write(&target, b"{}").unwrap();
    let symlink_config = dir.join("config.json");
    std::os::unix::fs::symlink(&target, &symlink_config).unwrap();

    let store = ConfigStore::new(symlink_config);
    assert!(matches!(store.load(), Err(StoreError::UntrustedFile(_))));

    // 2. Lock file is a symlink
    let lock_target = dir.join("real.lock");
    fs::write(&lock_target, b"").unwrap();
    let lock_symlink = dir.join("valid_config.json.lock");
    std::os::unix::fs::symlink(&lock_target, &lock_symlink).unwrap();

    let store2 = ConfigStore::new(dir.join("valid_config.json"));
    let dummy_cfg = Config::new(Config::CURRENT_VERSION, Vec::new());
    assert!(matches!(
        store2.save(&dummy_cfg),
        Err(StoreError::LockRejected(_))
    ));
}

#[test]
fn group_writable_config_file_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("group_writable_file");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

    let config_path = dir.join("config.json");
    fs::write(&config_path, b"{}").unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o664)).unwrap();

    let store = ConfigStore::new(config_path);
    assert!(matches!(store.load(), Err(StoreError::UntrustedFile(_))));
}

#[test]
fn group_writable_parent_directory_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("group_writable_dir");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o775)).unwrap();

    let config_path = dir.join("config.json");
    fs::write(&config_path, b"{}").unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();

    let store = ConfigStore::new(config_path);
    assert!(matches!(
        store.load(),
        Err(StoreError::UntrustedDirectory(_))
    ));

    let dummy_cfg = Config::new(Config::CURRENT_VERSION, Vec::new());
    assert!(matches!(
        store.save(&dummy_cfg),
        Err(StoreError::UntrustedDirectory(_))
    ));
}

#[test]
fn try_update_returns_skipped_under_lock_contention() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("contention_test");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

    let config_path = dir.join("config.json");
    let store = ConfigStore::new(config_path.clone());
    let initial_cfg = Config::new(Config::CURRENT_VERSION, Vec::new());
    store.save(&initial_cfg).unwrap();

    // Lock the lock file with another handle
    let lock_path = dir.join("config.json.lock");
    let external_lock_file = fs::File::open(&lock_path).unwrap();
    external_lock_file.lock().unwrap();

    let mut update_called = false;
    let res = store
        .try_update(|_| {
            update_called = true;
            true
        })
        .unwrap();

    assert_eq!(res, UpdateResult::Skipped);
    assert!(!update_called);

    drop(external_lock_file);

    // After unlocking, try_update succeeds
    let res2 = store.try_update(|_| true).unwrap();
    assert_eq!(res2, UpdateResult::Updated);
}

#[test]
fn delete_if_present_removes_file_under_lock_and_is_noop_when_absent() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("delete_test");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

    let config_path = dir.join("config.json");
    let store = ConfigStore::new(config_path.clone());
    let cfg = Config::new(Config::CURRENT_VERSION, Vec::new());
    store.save(&cfg).unwrap();
    assert!(config_path.exists());

    store.delete_if_present().unwrap();
    assert!(!config_path.exists());
    assert!(
        dir.join("config.json.lock").exists(),
        "lock file must remain"
    );

    // Deleting again is a no-op
    store.delete_if_present().unwrap();
    assert!(!config_path.exists());
}

#[test]
fn malformed_file_bytes_unchanged_after_failed_load_and_refused_save() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("refused_save");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

    let config_path = dir.join("config.json");
    let malformed_bytes = b"{\n  \"invalid json\": [";
    fs::write(&config_path, malformed_bytes).unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o664)).unwrap(); // group-writable

    let store = ConfigStore::new(config_path.clone());

    // Load refused due to untrusted file
    assert!(matches!(store.load(), Err(StoreError::UntrustedFile(_))));
    assert_eq!(fs::read(&config_path).unwrap(), malformed_bytes);

    // Save refused due to untrusted file
    let cfg = Config::new(Config::CURRENT_VERSION, Vec::new());
    assert!(matches!(
        store.save(&cfg),
        Err(StoreError::UntrustedFile(_))
    ));
    assert_eq!(fs::read(&config_path).unwrap(), malformed_bytes);
}

#[test]
fn debug_of_store_error_never_prints_file_content() {
    let secret_payload = "SECRET-INVENTED-TOKEN-FOR-TEST-PAYLOAD-998877";

    let decode_err = ariadusage_core::config::decode(secret_payload.as_bytes()).unwrap_err();
    let err = StoreError::Decode(decode_err);
    let debug_str = format!("{:?}", err);
    assert!(
        !debug_str.contains(secret_payload),
        "Debug output must not leak file payload bytes"
    );

    let untrusted_err = StoreError::UntrustedFile("config.json is group-writable".to_string());
    let debug_untrusted = format!("{:?}", untrusted_err);
    assert!(!debug_untrusted.contains(secret_payload));
}
