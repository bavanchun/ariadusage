// Ported from CodexBar Tests/CodexBarTests/CredentialFileWriterTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/CredentialFileWriterTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use ariadusage_core::config::Config;
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::error::StoreError;
use ariadusage_engine::private_file::{WriteHooks, repair_permissions, write_private};

fn file_mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn dir_entries(path: &Path) -> Vec<String> {
    let mut entries: Vec<String> = fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();
    entries
}

fn test_staging_private_impl(existing: bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("case");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();

    let url = root.join("auth.json");
    let original = b"original-synthetic";
    let replacement = b"replacement-synthetic";

    if existing {
        fs::write(&url, original).unwrap();
    }

    let mut reader = if existing {
        Some(fs::File::open(&url).unwrap())
    } else {
        None
    };

    let root_clone = root.clone();
    let before_write = Arc::new(move |staged: &Path| -> Result<(), StoreError> {
        let directory = staged.parent().unwrap();
        assert_ne!(directory, root_clone);
        assert_eq!(directory.parent().unwrap(), root_clone);
        assert_eq!(file_mode(directory), 0o700);
        assert_eq!(file_mode(staged), 0o600);
        assert!(fs::read(staged).unwrap().is_empty());
        Ok(())
    });

    let url_clone2 = url.clone();
    let before_publish = Arc::new(move |staged: &Path| -> Result<(), StoreError> {
        assert_eq!(fs::read(staged).unwrap(), replacement);
        if existing {
            assert_eq!(fs::read(&url_clone2).unwrap(), original);
        } else {
            assert!(!url_clone2.exists());
        }
        Ok(())
    });

    let hooks = WriteHooks {
        before_write: Some(before_write),
        before_publish: Some(before_publish),
    };

    write_private(&url, replacement, &hooks).unwrap();

    assert_eq!(fs::read(&url).unwrap(), replacement);
    assert_eq!(file_mode(&url), 0o600);
    assert_eq!(file_mode(&root), 0o755);

    if let Some(mut r) = reader.take() {
        let mut buf = Vec::new();
        r.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, original);
    }

    assert_eq!(dir_entries(&root), vec!["auth.json"]);
}

#[test]
fn staging_is_private_before_writing_and_replacement_preserves_open_readers_nonexisting() {
    test_staging_private_impl(false);
}

#[test]
fn staging_is_private_before_writing_and_replacement_preserves_open_readers_existing() {
    test_staging_private_impl(true);
}

fn test_failed_write_or_publication_impl(fail_before_write: bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("case");
    fs::create_dir_all(&root).unwrap();

    let url = root.join("auth.json");
    let original = b"original-synthetic";
    fs::write(&url, original).unwrap();

    let fail_hook = Arc::new(|_staged: &Path| -> Result<(), StoreError> {
        Err(StoreError::Io(std::io::Error::other("simulated failure")))
    });

    let hooks = if fail_before_write {
        WriteHooks {
            before_write: Some(fail_hook),
            before_publish: None,
        }
    } else {
        WriteHooks {
            before_write: None,
            before_publish: Some(fail_hook),
        }
    };

    let result = write_private(&url, b"replacement", &hooks);
    assert!(result.is_err());

    assert_eq!(fs::read(&url).unwrap(), original);
    assert_eq!(dir_entries(&root), vec!["auth.json"]);
}

#[test]
fn failed_write_preserves_destination_and_cleans_staging() {
    test_failed_write_or_publication_impl(true);
}

#[test]
fn failed_publication_preserves_destination_and_cleans_staging() {
    test_failed_write_or_publication_impl(false);
}

#[test]
fn credential_stores_use_private_staging_before_publication_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("case");
    fs::create_dir_all(&root).unwrap();

    let url = root.join("auth.json");
    let original = b"original-synthetic";
    fs::write(&url, original).unwrap();

    let url_clone = url.clone();
    let inspect_staged = Arc::new(move |staged: &Path| -> Result<(), StoreError> {
        assert_eq!(file_mode(staged.parent().unwrap()), 0o700);
        assert_eq!(file_mode(staged), 0o600);
        assert_eq!(fs::read(&url_clone).unwrap(), original);
        Err(StoreError::Io(std::io::Error::other("cancellation")))
    });

    let hooks = WriteHooks {
        before_write: None,
        before_publish: Some(inspect_staged),
    };

    let store = ConfigStore::with_write_hooks(url.clone(), hooks);
    let config = Config::new(Config::CURRENT_VERSION, Vec::new());
    let res = store.save(&config);
    assert!(res.is_err());

    assert_eq!(fs::read(&url).unwrap(), original);
    assert_eq!(dir_entries(&root), vec!["auth.json", "auth.json.lock"]);
}

#[test]
fn writes_credential_file_owner_only() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("cfw");
    fs::create_dir_all(&dir).unwrap();
    let url = dir.join("factory-session.json");

    write_private(&url, br#"{"bearerToken":"secret"}"#, &WriteHooks::default()).unwrap();

    assert_eq!(file_mode(&url), 0o600, "credential file must be 0600");
    let staged_dirs: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().to_string();
            if name.contains("ariadusage-staged") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    assert!(
        staged_dirs.is_empty(),
        "no staged temp directory should remain after publish"
    );
}

#[test]
fn overwrite_replaces_atomically_and_stays_private() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("cfw");
    fs::create_dir_all(&dir).unwrap();
    let url = dir.join("cursor-session.json");

    write_private(&url, b"v1", &WriteHooks::default()).unwrap();
    write_private(&url, b"v2", &WriteHooks::default()).unwrap();

    assert_eq!(fs::read_to_string(&url).unwrap(), "v2");
    assert_eq!(file_mode(&url), 0o600);
}

#[test]
fn repairs_legacy_world_readable_file() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("cfw");
    fs::create_dir_all(&dir).unwrap();
    let url = dir.join("legacy-session.json");

    fs::write(&url, b"legacy").unwrap();
    fs::set_permissions(&url, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(file_mode(&url), 0o644);

    repair_permissions(&url);
    assert_eq!(
        file_mode(&url),
        0o600,
        "existing 0644 file must be repaired to 0600"
    );
}

#[test]
fn repair_permissions_on_symlink_leaves_target_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("real-file.json");
    fs::write(&target, b"target").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(file_mode(&target), 0o644);

    let symlink = temp.path().join("symlink.json");
    std::os::unix::fs::symlink(&target, &symlink).unwrap();

    repair_permissions(&symlink);
    assert_eq!(
        file_mode(&target),
        0o644,
        "target permissions must remain untouched when repair is called on symlink"
    );
}
