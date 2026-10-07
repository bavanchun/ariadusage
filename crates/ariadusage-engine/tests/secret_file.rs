#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::secret_store::file::FileBackend;
use ariadusage_engine::brokers::secret_store::{
    SecretBackend, SecretId, SecretLookup, SecretStoreError,
};
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::secret::SecretString;
use tokio_util::sync::CancellationToken;

fn call() -> BrokerCall {
    BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "secret-file-test".to_owned(),
    }
}

fn id() -> SecretId {
    SecretId::from_setting_id(&SettingId::new("providers.claude.apiKey").expect("valid setting id"))
        .expect("valid secret id")
}

fn mode(path: &std::path::Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn file_backend_refuses_read_and_write_without_consent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ariadusage/secrets.json");
    let backend = FileBackend::new(path.clone(), false);

    assert_eq!(
        backend.lookup_background(&id(), &call()).await.unwrap_err(),
        SecretStoreError::ConsentRequired
    );
    assert_eq!(
        backend
            .set_user(&id(), &SecretString::new("invented-secret"), &call())
            .await
            .unwrap_err(),
        SecretStoreError::ConsentRequired
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn consented_file_backend_writes_private_file_and_reads_secret() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ariadusage/secrets.json");
    let backend = FileBackend::new(path.clone(), true);
    let secret = SecretString::new(["invented", "secret"].join("-"));

    backend.set_user(&id(), &secret, &call()).await.unwrap();
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(mode(&path), 0o600);
    assert!(matches!(
        backend.lookup_background(&id(), &call()).await.unwrap(),
        SecretLookup::Found(found) if found.expose_secret() == secret.expose_secret()
    ));
    assert!(!format!("{backend:?}").contains(secret.expose_secret()));
}

#[tokio::test]
async fn file_backend_rejects_symlinks_and_unsafe_modes() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("ariadusage");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let target = temp.path().join("target.json");
    fs::write(&target, br#"{"providers.claude.apiKey":"unused"}"#).unwrap();

    let symlink_path = parent.join("symlink.json");
    symlink(&target, &symlink_path).unwrap();
    let symlink_backend = FileBackend::new(symlink_path, true);
    assert_eq!(
        symlink_backend
            .lookup_background(&id(), &call())
            .await
            .unwrap_err(),
        SecretStoreError::Storage
    );

    for (name, unsafe_mode) in [("group-write", 0o620), ("world-readable", 0o644)] {
        let path = parent.join(format!("{name}.json"));
        fs::write(&path, br#"{"providers.claude.apiKey":"unused"}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(unsafe_mode)).unwrap();
        let backend = FileBackend::new(path.clone(), true);
        assert_eq!(
            backend.lookup_background(&id(), &call()).await.unwrap_err(),
            SecretStoreError::Storage
        );
        assert_eq!(
            backend
                .set_user(&id(), &SecretString::new("replacement"), &call())
                .await
                .unwrap_err(),
            SecretStoreError::Storage
        );
        assert_eq!(mode(&path), unsafe_mode);
    }
}

#[tokio::test]
async fn file_backend_rejects_untrusted_parent_directory() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("ariadusage");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    let path = parent.join("secrets.json");
    let backend = FileBackend::new(path.clone(), true);

    assert_eq!(
        backend
            .set_user(&id(), &SecretString::new("invented-secret"), &call())
            .await
            .unwrap_err(),
        SecretStoreError::Storage
    );
    assert!(!path.exists());
}
