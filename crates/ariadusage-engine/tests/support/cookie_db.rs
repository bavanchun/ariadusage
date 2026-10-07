#![cfg(target_os = "linux")]
#![expect(
    dead_code,
    reason = "shared synthetic browser fixtures are used by separate test binaries"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ariadusage_core::cookie as cookie_core;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::browser::{
    Browser, BrowserBroker, BrowserPaths, SafeStorageFuture, SafeStorageKeyProvider,
};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::secret_store::safe_storage::SafeStorageError;
use ariadusage_engine::state_store::BrokerStateStore;
use ariadusage_protocol::ids::ProviderId;
use rusqlite::{Connection, params};
use tempfile::TempDir;
use zeroize::Zeroizing;

pub struct TestHome {
    _home_root: TempDir,
    _runtime_root: TempDir,
    pub home: PathBuf,
    pub process_home: PathBuf,
    pub config: PathBuf,
    pub runtime: PathBuf,
    state_path: PathBuf,
}

impl TestHome {
    pub fn new() -> Self {
        let home_root = tempfile::tempdir().expect("temporary home");
        let home = home_root.path().join("home");
        let process_home = home_root.path().join("process-home");
        let config = home.join(".config");
        std::fs::create_dir_all(&config).expect("config directory");
        std::fs::create_dir_all(&process_home).expect("synthetic process home");

        let runtime_root = tempfile::Builder::new()
            .prefix("ariadusage-browser-test-")
            .tempdir_in("/dev/shm")
            .expect("tmpfs runtime directory");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(runtime_root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private runtime permissions");
        let runtime = runtime_root.path().to_path_buf();

        Self {
            _home_root: home_root,
            _runtime_root: runtime_root,
            state_path: home.join("state/broker-state.json"),
            home,
            process_home,
            config,
            runtime,
        }
    }

    pub fn state_store(&self) -> Arc<BrokerStateStore> {
        Arc::new(BrokerStateStore::new(self.state_path.clone()))
    }

    pub fn broker(&self, keys: Arc<dyn SafeStorageKeyProvider>) -> BrowserBroker {
        BrowserBroker::new(
            self.browser_paths(self.home.clone(), Some(self.runtime.clone())),
            self.state_store(),
            keys,
        )
    }

    pub fn browser_paths(&self, home: PathBuf, runtime: Option<PathBuf>) -> BrowserPaths {
        let process_home = self.process_home.clone();
        let config = self.config.clone();
        BrowserPaths::from_env(
            |key| match key {
                "HOME" => Some(process_home.as_os_str().to_os_string()),
                "XDG_CONFIG_HOME" => Some(config.as_os_str().to_os_string()),
                "XDG_RUNTIME_DIR" => runtime.as_ref().map(|path| path.as_os_str().to_os_string()),
                _ => None,
            },
            home,
        )
        .expect("injected browser paths")
    }

    pub fn chromium_profile(&self, browser: Browser, label: &str) -> PathBuf {
        let root = self.config.join(match browser {
            Browser::Chrome => "google-chrome",
            Browser::Edge => "microsoft-edge",
            Browser::Brave => "BraveSoftware/Brave-Browser",
            Browser::Chromium => "chromium",
            Browser::Vivaldi => "vivaldi",
            Browser::Firefox => panic!("Firefox profile requested as Chromium"),
            Browser::Opera => "opera",
        });
        let profile = root.join("Default");
        std::fs::create_dir_all(&profile).expect("browser profile");
        std::fs::write(
            root.join("Local State"),
            serde_json::json!({
                "profile": {
                    "info_cache": {
                        "Default": { "name": label }
                    }
                }
            })
            .to_string(),
        )
        .expect("profile metadata");
        create_chromium_database(&profile.join("Cookies"), 24);
        profile
    }

    pub fn firefox_profile(&self, profile_name: &str, schema_version: i64) -> PathBuf {
        let root = self.config.join("mozilla/firefox");
        let profile = root.join("test-profile");
        std::fs::create_dir_all(&profile).expect("Firefox profile");
        std::fs::create_dir_all(&root).expect("Firefox profile root");
        std::fs::write(
            root.join("profiles.ini"),
            format!(
                "[General]\nVersion=2\n[Profile0]\nName={profile_name}\nIsRelative=1\nPath=test-profile\nDefault=1\n"
            ),
        )
        .expect("Firefox profile listing");
        std::fs::write(
            profile.join("containers.json"),
            r#"{"identities":[{"userContextId":1,"name":"Work"},{"userContextId":2,"name":"Personal"}]}"#,
        )
        .expect("Firefox containers");
        create_firefox_database(&profile.join("cookies.sqlite"), schema_version);
        profile
    }
}

pub struct StaticKeys {
    pub key: Option<Vec<u8>>,
    pub error: Option<SafeStorageError>,
    pub calls: AtomicUsize,
}

impl StaticKeys {
    pub fn available(key: Vec<u8>) -> Self {
        Self {
            key: Some(key),
            error: None,
            calls: AtomicUsize::new(0),
        }
    }

    pub fn without_key() -> Self {
        Self {
            key: None,
            error: None,
            calls: AtomicUsize::new(0),
        }
    }

    pub fn failing(error: SafeStorageError) -> Self {
        Self {
            key: None,
            error: Some(error),
            calls: AtomicUsize::new(0),
        }
    }
}

impl SafeStorageKeyProvider for StaticKeys {
    fn chromium_safe_storage<'a>(
        &'a self,
        _app_attribute: &'a str,
        _call: &'a BrokerCall,
    ) -> SafeStorageFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.error {
                Err(error)
            } else {
                Ok(self.key.clone().map(Zeroizing::new))
            }
        })
    }
}

pub fn call(interaction: FetchInteraction) -> BrokerCall {
    BrokerCall {
        interaction,
        cancel: tokio_util::sync::CancellationToken::new(),
        request_id: "browser-test".to_owned(),
    }
}

pub fn authorization() -> cookie_core::ImportAuthorized {
    match cookie_core::resolve_cookie_source(
        Some(ariadusage_core::config::CookieSource::Auto),
        false,
        None,
        false,
    ) {
        cookie_core::CookieResolution::Import(authorization) => authorization,
        _ => unreachable!("explicit automatic import is authorized"),
    }
}

pub fn query<'a>(
    domains: &'a cookie_core::DeclaredDomains,
    browsers: &'a [Browser],
) -> ariadusage_engine::brokers::browser::CookieQuery<'a> {
    ariadusage_engine::brokers::browser::CookieQuery {
        provider: ProviderId::new("claude").expect("synthetic provider id"),
        domains,
        names: None,
        browsers,
    }
}

pub fn create_chromium_database(path: &Path, version: i64) {
    let connection = Connection::open(path).expect("create Chromium database");
    connection
        .execute_batch(
            "CREATE TABLE meta(version INTEGER);\
             INSERT INTO meta(version) VALUES (24);\
             CREATE TABLE cookies(host_key TEXT, name TEXT, path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER, value TEXT, encrypted_value BLOB, top_frame_site_key TEXT);",
        )
        .expect("Chromium schema");
    connection
        .execute("UPDATE meta SET version=?1", [version])
        .expect("Chromium version");
}

pub struct ChromiumRow<'a> {
    pub host: &'a str,
    pub name: &'a str,
    pub expires_utc: i64,
    pub value: &'a str,
    pub encrypted: &'a [u8],
    pub partition_key: &'a str,
}

pub fn insert_chromium(path: &Path, row: ChromiumRow<'_>) {
    let connection = Connection::open(path).expect("open Chromium database");
    connection
        .execute(
            "INSERT INTO cookies(host_key,name,path,expires_utc,is_secure,is_httponly,value,encrypted_value,top_frame_site_key) VALUES (?1,?2,'/',?3,1,1,?4,?5,?6)",
            params![row.host, row.name, row.expires_utc, row.value, row.encrypted, row.partition_key],
        )
        .expect("insert Chromium cookie row");
}

pub fn create_firefox_database(path: &Path, schema_version: i64) {
    let connection = Connection::open(path).expect("create Firefox database");
    connection
        .execute_batch(
            "CREATE TABLE moz_cookies(host TEXT, name TEXT, path TEXT, expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER, value TEXT, originAttributes TEXT);",
        )
        .expect("Firefox schema");
    connection
        .pragma_update(None, "user_version", schema_version)
        .expect("Firefox schema version");
}

pub fn insert_firefox(
    path: &Path,
    host: &str,
    name: &str,
    expires: i64,
    value: &str,
    origin_attributes: &str,
) {
    let connection = Connection::open(path).expect("open Firefox database");
    connection
        .execute(
            "INSERT INTO moz_cookies(host,name,path,expiry,isSecure,isHttpOnly,value,originAttributes) VALUES (?1,?2,'/',?3,1,1,?4,?5)",
            params![host, name, expires, value, origin_attributes],
        )
        .expect("insert Firefox cookie row");
}
