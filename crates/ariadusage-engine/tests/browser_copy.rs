#![cfg(target_os = "linux")]

#[path = "support/cookie_db.rs"]
mod cookie_db;

use std::sync::Arc;

use ariadusage_core::cookie as cookie_core;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::browser::{
    Browser, BrowserBroker, BrowserError, CookieQuery, SafeStorageFuture, SafeStorageKeyProvider,
};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::secret_store::safe_storage::SafeStorageError;
use ariadusage_protocol::ids::ProviderId;
use rusqlite::Connection;

use cookie_db::{ChromiumRow, StaticKeys, TestHome, authorization, insert_chromium};

fn run_with_paths(
    home: &TestHome,
    runtime_dir: Option<std::path::PathBuf>,
    keys: Arc<dyn SafeStorageKeyProvider>,
) -> Result<Vec<ariadusage_engine::brokers::browser::CookieCandidate>, BrowserError> {
    let broker = BrowserBroker::new(
        home.browser_paths(home.home.clone(), runtime_dir),
        home.state_store(),
        keys,
    );
    let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
    let browsers = [Browser::Chrome];
    let query = CookieQuery {
        provider: ProviderId::new("claude").expect("provider id"),
        domains: &domains,
        names: None,
        browsers: &browsers,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(broker.candidates(
            &BrokerCall {
                interaction: FetchInteraction::UserInitiated,
                cancel: tokio_util::sync::CancellationToken::new(),
                request_id: "copy-test".to_owned(),
            },
            authorization(),
            query,
        ))
}

#[test]
fn startup_sweep_removes_stale_private_copy_directories() {
    let home = TestHome::new();
    let stale = home.runtime.join("ariadusage-tmp-stale");
    std::fs::create_dir(&stale).expect("stale copy directory");
    let _broker = home.broker(Arc::new(StaticKeys::without_key()));
    assert!(!stale.exists());
}

fn add_plain_chromium_profile(home: &TestHome) -> std::path::PathBuf {
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    insert_chromium(
        &profile.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "sid",
            expires_utc: 0,
            value: "synthetic-session",
            encrypted: b"",
            partition_key: "",
        },
    );
    profile
}

#[test]
fn a_crafted_journal_cannot_remove_its_named_sentinel_and_copies_are_cleaned() {
    let home = TestHome::new();
    let profile = add_plain_chromium_profile(&home);
    let sentinel = home.home.join("sentinel-file");
    std::fs::write(&sentinel, "safe").expect("sentinel");
    std::fs::write(
        profile.join("Cookies-journal"),
        format!("synthetic super-journal target {}", sentinel.display()),
    )
    .expect("crafted journal");
    let candidates = run_with_paths(
        &home,
        Some(home.runtime.clone()),
        Arc::new(StaticKeys::without_key()),
    )
    .expect("import candidate");
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        std::fs::read_to_string(sentinel).expect("sentinel remains"),
        "safe"
    );
    assert!(
        std::fs::read_dir(&home.runtime)
            .expect("runtime directory")
            .next()
            .is_none()
    );
}

#[test]
fn a_symlinked_database_is_rejected_without_following_it() {
    let home = TestHome::new();
    let profile = add_plain_chromium_profile(&home);
    let database = profile.join("Cookies");
    let real_database = profile.join("Cookies.real");
    std::fs::rename(&database, &real_database).expect("move synthetic database");
    std::os::unix::fs::symlink(real_database, &database).expect("database symlink");
    assert_eq!(
        run_with_paths(
            &home,
            Some(home.runtime.clone()),
            Arc::new(StaticKeys::without_key()),
        )
        .expect_err("symlink profile not discovered"),
        BrowserError::NoBrowserSession
    );
    assert!(
        std::fs::read_dir(&home.runtime)
            .expect("runtime directory")
            .next()
            .is_none()
    );
}

#[test]
fn a_database_wal_is_copied_and_remains_readable() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    let database = profile.join("Cookies");
    let writer = Connection::open(&database).expect("open synthetic database");
    writer
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .expect("enable WAL mode");
    writer
        .execute(
            "INSERT INTO cookies(host_key,name,path,expires_utc,is_secure,is_httponly,value,encrypted_value,top_frame_site_key) VALUES (?1,?2,'/',0,1,1,?3,?4,?5)",
            rusqlite::params![".claude.ai", "wal-session", "wal-value", b"" as &[u8], ""],
        )
        .expect("write synthetic WAL row");
    let mut wal_name = database.as_os_str().to_os_string();
    wal_name.push("-wal");
    assert!(
        std::fs::metadata(std::path::PathBuf::from(wal_name))
            .expect("synthetic WAL file")
            .len()
            > 0
    );

    let candidates = run_with_paths(
        &home,
        Some(home.runtime.clone()),
        Arc::new(StaticKeys::without_key()),
    )
    .expect("read copied WAL");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].records[0].value.expose_secret(), "wal-value");
    assert!(
        std::fs::read_dir(&home.runtime)
            .expect("runtime directory")
            .next()
            .is_none()
    );
}

#[test]
fn a_cookie_symlink_to_a_fifo_is_rejected_without_waiting_for_a_writer() {
    let home = TestHome::new();
    let profile = add_plain_chromium_profile(&home);
    let database = profile.join("Cookies");
    let fifo = profile.join("Cookies.fifo");
    std::fs::remove_file(&database).expect("remove synthetic database");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .expect("create synthetic FIFO");
    std::os::unix::fs::symlink(&fifo, &database).expect("point database path at FIFO");
    assert_eq!(
        run_with_paths(
            &home,
            Some(home.runtime.clone()),
            Arc::new(StaticKeys::without_key()),
        )
        .expect_err("non-regular source rejected"),
        BrowserError::NoBrowserSession
    );
}

#[test]
fn database_size_cap_and_untrusted_runtime_fail_closed_and_cleanup() {
    let home = TestHome::new();
    let profile = add_plain_chromium_profile(&home);
    let database = profile.join("Cookies");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&database)
        .expect("open synthetic database");
    file.set_len(64 * 1024 * 1024 + 1).expect("oversize source");
    assert_eq!(
        run_with_paths(
            &home,
            Some(home.runtime.clone()),
            Arc::new(StaticKeys::without_key()),
        )
        .expect_err("source exceeds cap"),
        BrowserError::Io
    );
    assert!(
        std::fs::read_dir(&home.runtime)
            .expect("runtime directory")
            .next()
            .is_none()
    );

    let path = home.runtime.clone();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("loosen runtime permissions");
    assert_eq!(
        run_with_paths(
            &home,
            Some(path.clone()),
            Arc::new(StaticKeys::without_key()),
        )
        .expect_err("untrusted runtime"),
        BrowserError::Unsupported
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .expect("restore runtime permissions");
}

#[test]
fn missing_runtime_directory_is_typed_unsupported() {
    let home = TestHome::new();
    let _ = add_plain_chromium_profile(&home);
    assert_eq!(
        run_with_paths(&home, None, Arc::new(StaticKeys::without_key()))
            .expect_err("missing runtime directory"),
        BrowserError::Unsupported
    );
}

struct PanicKeys;

impl SafeStorageKeyProvider for PanicKeys {
    fn chromium_safe_storage<'a>(
        &'a self,
        _app_attribute: &'a str,
        _call: &'a BrokerCall,
    ) -> SafeStorageFuture<'a> {
        Box::pin(async { panic!("synthetic key-source panic") })
    }
}

#[test]
fn private_copy_directory_is_removed_when_key_source_panics() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    let encrypted = b"v11synthetic";
    insert_chromium(
        &profile.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "sid",
            expires_utc: 0,
            value: "",
            encrypted,
            partition_key: "",
        },
    );
    let broker = home.broker(Arc::new(PanicKeys));
    let runtime_path = home.runtime.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let join = runtime.block_on(async move {
        tokio::spawn(async move {
            let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
            let browsers = [Browser::Chrome];
            let query = CookieQuery {
                provider: ProviderId::new("claude").expect("provider id"),
                domains: &domains,
                names: None,
                browsers: &browsers,
            };
            broker
                .candidates(
                    &BrokerCall {
                        interaction: FetchInteraction::UserInitiated,
                        cancel: tokio_util::sync::CancellationToken::new(),
                        request_id: "panic-cleanup".to_owned(),
                    },
                    authorization(),
                    query,
                )
                .await
        })
        .await
    });
    assert!(join.expect_err("key source panic captured").is_panic());
    assert!(
        std::fs::read_dir(runtime_path)
            .expect("runtime directory")
            .next()
            .is_none()
    );
}

#[test]
fn corrupt_database_is_retried_once_and_temporary_copies_are_removed() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    std::fs::write(profile.join("Cookies"), b"not a sqlite database").expect("corrupt source");
    assert_eq!(
        run_with_paths(
            &home,
            Some(home.runtime.clone()),
            Arc::new(StaticKeys::without_key()),
        )
        .expect_err("corrupt database persists after one retry"),
        BrowserError::Malformed
    );
    assert!(
        std::fs::read_dir(&home.runtime)
            .expect("runtime directory")
            .next()
            .is_none()
    );
}

#[test]
fn key_error_type_remains_secret_free() {
    assert_eq!(
        SafeStorageError::Locked.to_string(),
        "safe-storage keyring item is locked"
    );
}
