// Ported from CodexBar Tests/CodexBarTests/BrowserDetectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(any(target_os = "linux", target_os = "macos"))]

#[path = "support/cookie_db.rs"]
mod cookie_db;

use std::path::PathBuf;
use std::sync::Arc;

use ariadusage_core::cookie as cookie_core;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::browser::{
    Browser, BrowserBroker, BrowserError, BrowserPaths, CookieQuery, DEFAULT_IMPORT_ORDER,
    SafeStorageKeyProvider,
};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::secret_store::safe_storage::SafeStorageError;
use ariadusage_protocol::ids::ProviderId;

use cookie_db::{
    ChromiumRow, StaticKeys, TestHome, authorization, insert_chromium, insert_firefox,
};

fn firefox_candidates(
    home: &TestHome,
) -> Result<Vec<ariadusage_engine::brokers::browser::CookieCandidate>, BrowserError> {
    candidates_for(home, &[Browser::Firefox])
}

fn candidates_for(
    home: &TestHome,
    browsers: &[Browser],
) -> Result<Vec<ariadusage_engine::brokers::browser::CookieCandidate>, BrowserError> {
    let broker = home.broker(Arc::new(StaticKeys::without_key()));
    candidates_with_broker(&broker, browsers)
}

fn candidates_with_broker(
    broker: &BrowserBroker,
    browsers: &[Browser],
) -> Result<Vec<ariadusage_engine::brokers::browser::CookieCandidate>, BrowserError> {
    let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
    let query = CookieQuery {
        provider: ProviderId::new("claude").expect("provider id"),
        domains: &domains,
        names: None,
        browsers,
        acknowledged_retry: false,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(broker.candidates(
            &BrokerCall {
                interaction: FetchInteraction::UserInitiated,
                cancel: tokio_util::sync::CancellationToken::new(),
                request_id: "discover-test".to_owned(),
            },
            authorization(),
            query,
        ))
}

#[test]
fn catalog_keeps_the_approved_linux_import_order() {
    #[cfg(target_os = "macos")]
    assert_eq!(
        DEFAULT_IMPORT_ORDER,
        [
            Browser::Chrome,
            Browser::Edge,
            Browser::Brave,
            Browser::Chromium,
            Browser::Vivaldi,
            Browser::Firefox,
            Browser::Opera,
            Browser::Safari,
        ]
    );
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        DEFAULT_IMPORT_ORDER,
        [
            Browser::Chrome,
            Browser::Edge,
            Browser::Brave,
            Browser::Chromium,
            Browser::Vivaldi,
            Browser::Firefox,
            Browser::Opera,
        ]
    );
}

#[test]
// CodexBar: BrowserDetectionTests.swift:57
fn test_hooks_suppress_a_real_home_symlink_alias_before_discovery() {
    let isolated = TestHome::new();
    let home_alias = isolated.home.join("real-home-alias");
    std::os::unix::fs::symlink(&isolated.process_home, &home_alias).expect("home alias");
    let broker = BrowserBroker::with_paths(
        isolated.browser_paths(home_alias, Some(isolated.runtime.clone())),
        isolated.state_store(),
        Arc::new(StaticKeys::without_key()),
    );
    let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
    let query = CookieQuery {
        provider: ProviderId::new("claude").expect("provider id"),
        domains: &domains,
        names: None,
        browsers: &[],
        acknowledged_retry: false,
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(broker.candidates(
            &BrokerCall {
                interaction: FetchInteraction::Background,
                cancel: tokio_util::sync::CancellationToken::new(),
                request_id: "real-home-guard".to_owned(),
            },
            authorization(),
            query,
        ));
    assert_eq!(
        result.expect_err("guarded").to_string(),
        "browser access suppressed"
    );
}

#[test]
// CodexBar: BrowserDetectionTests.swift:79
fn test_hooks_report_typed_suppression_for_the_injected_process_home() {
    let isolated = TestHome::new();
    let broker = BrowserBroker::with_paths(
        isolated.browser_paths(
            isolated.process_home.clone(),
            Some(isolated.runtime.clone()),
        ),
        isolated.state_store(),
        Arc::new(StaticKeys::without_key()),
    );
    let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
    let query = CookieQuery {
        provider: ProviderId::new("claude").expect("provider id"),
        domains: &domains,
        names: None,
        browsers: &[],
        acknowledged_retry: false,
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(broker.candidates(
            &cookie_db::call(FetchInteraction::Background),
            authorization(),
            query,
        ));
    assert!(matches!(result, Err(BrowserError::Suppressed)));
}

#[test]
fn firefox_profile_labels_use_names_and_never_email_or_account_fields() {
    let home = TestHome::new();
    let profile = home.firefox_profile("default-release", 17);
    insert_firefox(
        &profile.join("cookies.sqlite"),
        ".claude.ai",
        "sid",
        0,
        "synthetic",
        "",
    );
    let candidates = firefox_candidates(&home).expect("Firefox profile");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].label, "Firefox default-release");
    assert!(!candidates[0].label.contains('@'));
}

#[test]
// CodexBar: BrowserDetectionTests.swift:93
fn browser_import_requires_explicit_auto_source() {
    let unset = cookie_core::resolve_cookie_source(None, false, None, false);
    assert_eq!(unset, cookie_core::CookieResolution::None);
    let automatic = cookie_core::resolve_cookie_source(
        Some(ariadusage_core::config::CookieSource::Auto),
        false,
        None,
        false,
    );
    assert!(matches!(
        automatic,
        cookie_core::CookieResolution::Import(_)
    ));
}

#[test]
fn chromium_local_state_account_fields_are_ignored_and_email_names_are_redacted() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "synthetic@example.invalid");
    insert_chromium(
        &profile.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "sid",
            expires_utc: 0,
            value: "synthetic",
            encrypted: b"",
            partition_key: "",
        },
    );
    let local_state = profile
        .parent()
        .expect("user data root")
        .join("Local State");
    std::fs::write(
        local_state,
        r#"{"profile":{"info_cache":{"Default":{"name":"synthetic@example.invalid","user_name":"account@example.invalid","gaia_name":"person@example.invalid"}}}}"#,
    )
    .expect("synthetic local state");
    let candidates = candidates_for(&home, &[Browser::Chrome]).expect("Chrome candidate");
    assert_eq!(candidates.len(), 1);
    assert!(!candidates[0].label.contains('@'));
}

#[test]
fn relative_escape_and_symlinked_firefox_profiles_are_refused() {
    let home = TestHome::new();
    let root = home.firefox_root();
    let outside = home.home.join("outside-profile");
    std::fs::create_dir_all(&outside).expect("outside profile");
    let outside_db = outside.join("cookies.sqlite");
    cookie_db::create_firefox_database(&outside_db, 17);
    insert_firefox(&outside_db, ".claude.ai", "sid", 0, "synthetic", "");
    std::fs::create_dir_all(&root).expect("Firefox root");
    std::os::unix::fs::symlink(&outside, root.join("escape-link")).expect("profile symlink");
    std::fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=escape-relative\nIsRelative=1\nPath=../../../../outside-profile\n[Profile1]\nName=escape-link\nIsRelative=1\nPath=escape-link\n",
    )
    .expect("profile listing");
    assert_eq!(
        firefox_candidates(&home).expect_err("no contained profiles"),
        BrowserError::NoBrowserSession
    );
}

#[cfg(target_os = "linux")]
#[test]
fn absolute_firefox_profile_under_flatpak_root_is_refused() {
    let home = TestHome::new();
    let root = home
        .home
        .join(".var/app/org.mozilla.firefox/config/mozilla/firefox");
    let profile = root.join("default-release");
    std::fs::create_dir_all(&profile).expect("Flatpak profile directory");
    let database = profile.join("cookies.sqlite");
    cookie_db::create_firefox_database(&database, 17);
    insert_firefox(&database, ".claude.ai", "sid", 0, "synthetic", "");
    std::fs::write(
        root.join("profiles.ini"),
        format!(
            "[Profile0]\nName=flatpak\nIsRelative=0\nPath={}\nDefault=1\n",
            profile.display()
        ),
    )
    .expect("Flatpak profile listing");
    assert_eq!(
        firefox_candidates(&home).expect_err("sandbox absolute path is refused"),
        BrowserError::NoBrowserSession
    );
}

#[test]
fn injected_xdg_paths_are_used_without_process_environment_changes() {
    use std::ffi::OsString;

    let home = PathBuf::from("/tmp/synthetic-browser-home");
    let config = PathBuf::from("/tmp/synthetic-browser-config");
    let runtime = PathBuf::from("/dev/shm/synthetic-browser-runtime");
    let paths = BrowserPaths::from_env(
        |key| match key {
            "XDG_CONFIG_HOME" => Some(OsString::from(config.as_os_str())),
            "XDG_RUNTIME_DIR" => Some(OsString::from(runtime.as_os_str())),
            _ => None,
        },
        home.clone(),
    )
    .expect("injected browser paths");
    assert_eq!(paths.home, home);
    assert_eq!(paths.config_home, config);
    assert_eq!(paths.runtime_dir, Some(runtime));
}

#[test]
fn profile_discovery_results_are_cached_for_the_broker_lifetime() {
    let home = TestHome::new();
    let broker = home.broker(Arc::new(StaticKeys::without_key()));
    let browsers = [Browser::Chrome];
    assert_eq!(
        candidates_with_broker(&broker, &browsers).expect_err("initial miss"),
        BrowserError::NoBrowserSession
    );

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
    assert_eq!(
        candidates_with_broker(&broker, &browsers).expect_err("cached miss"),
        BrowserError::NoBrowserSession
    );
    assert_eq!(
        candidates_for(&home, &browsers)
            .expect("new broker rescan")
            .len(),
        1
    );
}

struct CancellingKeys;

impl SafeStorageKeyProvider for CancellingKeys {
    fn chromium_safe_storage<'a>(
        &'a self,
        _app_attribute: &'a str,
        call: &'a BrokerCall,
    ) -> ariadusage_engine::brokers::browser::SafeStorageFuture<'a> {
        call.cancel.cancel();
        Box::pin(async { Err(SafeStorageError::Dismissed) })
    }
}

#[test]
fn cancellation_after_priming_surfaces_no_browser_session_instead_of_stale_error() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    #[cfg(target_os = "macos")]
    let encrypted: &[u8] = b"v10synthetic-ciphertext";
    #[cfg(not(target_os = "macos"))]
    let encrypted: &[u8] = b"v11synthetic-ciphertext";
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
    let broker = home.broker(Arc::new(CancellingKeys));
    let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
    let query = CookieQuery {
        provider: ProviderId::new("claude").expect("provider id"),
        domains: &domains,
        names: None,
        browsers: &[Browser::Chrome, Browser::Brave],
        acknowledged_retry: false,
    };
    let call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: tokio_util::sync::CancellationToken::new(),
        request_id: "cancel-test".to_owned(),
    };
    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(broker.candidates(&call, authorization(), query))
        .expect_err("cancelled during priming returns NoBrowserSession");
    assert_eq!(error, BrowserError::NoBrowserSession);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_discovers_application_support_profiles_across_all_supported_browsers() {
    let home = TestHome::new();
    let app_support = home.home.join("Library/Application Support");
    for (browser, rel_dir) in [
        (Browser::Chrome, "Google/Chrome"),
        (Browser::Edge, "Microsoft Edge"),
        (Browser::Brave, "BraveSoftware/Brave-Browser"),
        (Browser::Chromium, "Chromium"),
        (Browser::Vivaldi, "Vivaldi"),
        (Browser::Opera, "com.operasoftware.Opera"),
    ] {
        let profile = home.chromium_profile(browser, browser.display_name());
        assert_eq!(profile, app_support.join(rel_dir).join("Default"));
        insert_chromium(
            &profile.join("Cookies"),
            ChromiumRow {
                host: ".claude.ai",
                name: "sid",
                expires_utc: 0,
                value: browser.display_name(),
                encrypted: b"",
                partition_key: "",
            },
        );
    }

    let ff_profile = home.firefox_profile("default-release", 17);
    assert_eq!(
        ff_profile,
        app_support.join("Firefox/Profiles/test-profile")
    );
    insert_firefox(
        &ff_profile.join("cookies.sqlite"),
        ".claude.ai",
        "sid",
        0,
        "Firefox",
        "",
    );

    cookie_db::write_safari_cookies(
        &home.safari_container_cookies_path(),
        &[cookie_db::SafariRow {
            host: ".claude.ai",
            name: "sid",
            path: "/",
            value: "Safari",
            expiration_mac_secs: 0.0,
            secure: true,
            http_only: true,
        }],
    );

    let candidates = candidates_for(&home, &[]).expect("all macOS browsers discovered");
    assert_eq!(
        candidates.iter().map(|c| c.browser).collect::<Vec<_>>(),
        DEFAULT_IMPORT_ORDER
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_safari_parses_binarycookies_filters_domains_and_skips_permission_denied() {
    use std::os::unix::fs::PermissionsExt;

    let home = TestHome::new();
    let future_mac_secs = (jiff::Timestamp::now().as_second() - 978_307_200 + 7_200) as f64;
    let expired_mac_secs = (jiff::Timestamp::now().as_second() - 978_307_200 - 3_600) as f64;

    cookie_db::write_safari_cookies(
        &home.safari_container_cookies_path(),
        &[
            cookie_db::SafariRow {
                host: ".claude.ai",
                name: "sessionKey",
                path: "/",
                value: "invented-safari-session",
                expiration_mac_secs: future_mac_secs,
                secure: true,
                http_only: true,
            },
            cookie_db::SafariRow {
                host: ".claude.ai",
                name: "expiredKey",
                path: "/",
                value: "invented-expired-safari-session",
                expiration_mac_secs: expired_mac_secs,
                secure: true,
                http_only: false,
            },
            cookie_db::SafariRow {
                host: ".example.com",
                name: "otherSiteKey",
                path: "/",
                value: "invented-other-site",
                expiration_mac_secs: future_mac_secs,
                secure: true,
                http_only: true,
            },
        ],
    );

    let keys = Arc::new(StaticKeys::failing(SafeStorageError::Dismissed));
    let broker = home.broker(keys.clone());
    let candidates = candidates_with_broker(&broker, &[Browser::Safari]).expect("Safari candidate");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].browser, Browser::Safari);
    assert_eq!(candidates[0].label, "Safari");
    assert_eq!(candidates[0].diagnostics.rows_read, 2);
    assert_eq!(candidates[0].diagnostics.expired, 1);
    assert_eq!(candidates[0].records.len(), 1);
    assert_eq!(candidates[0].records[0].name, "sessionKey");
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "invented-safari-session"
    );
    assert_eq!(candidates[0].records[0].domain, "claude.ai");
    assert!(!candidates[0].records[0].host_only);
    assert!(candidates[0].records[0].secure);
    assert!(candidates[0].records[0].http_only);
    assert!(candidates[0].records[0].expires.is_some());
    assert_eq!(
        keys.calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "Safari cookie import must never touch Keychain"
    );

    // When the container `Cookies.binarycookies` is unreadable due to TCC (`PermissionDenied`),
    // Safari skips it without surfacing `BrowserError::Io` or recording a cooldown, and falls
    // back to the legacy `~/Library/Cookies/Cookies.binarycookies` path when present.
    let container_path = home.safari_container_cookies_path();
    std::fs::set_permissions(&container_path, std::fs::Permissions::from_mode(0o000))
        .expect("simulate TCC permission denied");
    cookie_db::write_safari_cookies(
        &home.safari_legacy_cookies_path(),
        &[cookie_db::SafariRow {
            host: "claude.ai",
            name: "sessionKey",
            path: "/",
            value: "invented-legacy-safari-session",
            expiration_mac_secs: 0.0,
            secure: true,
            http_only: false,
        }],
    );

    let fallback_candidates =
        candidates_for(&home, &[Browser::Safari]).expect("legacy Safari fallback candidate");
    assert_eq!(fallback_candidates.len(), 1);
    assert_eq!(
        fallback_candidates[0].records[0].value.expose_secret(),
        "invented-legacy-safari-session"
    );
    assert!(fallback_candidates[0].records[0].host_only);
    assert_eq!(
        fallback_candidates[0].diagnostics.access_denied, 1,
        "denied container root is counted in Safari diagnostics alongside the fallback session"
    );

    // If both Safari paths are unreadable (`PermissionDenied`), Safari surfaces a single
    // diagnostic candidate with `records.is_empty()` and `diagnostics.access_denied == 2`
    // (for a Full Disk Access hint) rather than `BrowserError::Io` or a cooldown.
    let legacy_path = home.safari_legacy_cookies_path();
    std::fs::set_permissions(&legacy_path, std::fs::Permissions::from_mode(0o000))
        .expect("simulate TCC permission denied on legacy path");
    let denied_candidates =
        candidates_for(&home, &[Browser::Safari]).expect("Safari access-denied diagnostic");
    assert_eq!(denied_candidates.len(), 1);
    assert!(denied_candidates[0].records.is_empty());
    assert_eq!(denied_candidates[0].diagnostics.access_denied, 2);

    // Restore permissions so TempDir cleanup succeeds.
    let _ = std::fs::set_permissions(&container_path, std::fs::Permissions::from_mode(0o600));
    let _ = std::fs::set_permissions(&legacy_path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_firefox_fallback_scans_profiles_subdirectory_when_profiles_ini_is_missing() {
    let home = TestHome::new();
    let ff_profile = home
        .home
        .join("Library/Application Support/Firefox/Profiles/abcd1234.default-release");
    std::fs::create_dir_all(&ff_profile).expect("Firefox Profiles directory");
    let db = ff_profile.join("cookies.sqlite");
    cookie_db::create_firefox_database(&db, 17);
    insert_firefox(&db, ".claude.ai", "sid", 0, "fallback-session", "");

    let candidates = firefox_candidates(&home).expect("Firefox fallback profile");
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "fallback-session"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_browser_paths_falls_back_to_tmpdir_when_xdg_runtime_dir_is_unset() {
    use std::ffi::OsString;

    let home = PathBuf::from("/tmp/synthetic-browser-home");
    let tmpdir = PathBuf::from("/var/folders/xx/synthetic/T");
    let paths = BrowserPaths::from_env(
        |key| match key {
            "TMPDIR" => Some(OsString::from(tmpdir.as_os_str())),
            _ => None,
        },
        home.clone(),
    )
    .expect("injected browser paths");
    assert_eq!(paths.home, home);
    assert_eq!(paths.runtime_dir, Some(tmpdir));
}
