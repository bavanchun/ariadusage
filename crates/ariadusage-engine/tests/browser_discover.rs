// Ported from CodexBar Tests/CodexBarTests/BrowserDetectionTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/cookie_db.rs"]
mod cookie_db;

use std::path::PathBuf;
use std::sync::Arc;

use ariadusage_core::cookie as cookie_core;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::browser::{
    Browser, BrowserBroker, BrowserError, BrowserPaths, CookieQuery, DEFAULT_IMPORT_ORDER,
};
use ariadusage_engine::brokers::call::BrokerCall;
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
// CodexBar: BrowserDetectionTests.swift:44
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
// CodexBar: BrowserDetectionTests.swift:66
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
// CodexBar: BrowserDetectionTests.swift:80
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
    let root = home.config.join("mozilla/firefox");
    let outside = home.config.join("outside-profile");
    std::fs::create_dir_all(&outside).expect("outside profile");
    let outside_db = outside.join("cookies.sqlite");
    cookie_db::create_firefox_database(&outside_db, 17);
    insert_firefox(&outside_db, ".claude.ai", "sid", 0, "synthetic", "");
    std::fs::create_dir_all(&root).expect("Firefox root");
    std::os::unix::fs::symlink(&outside, root.join("escape-link")).expect("profile symlink");
    std::fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=escape-relative\nIsRelative=1\nPath=../../outside-profile\n[Profile1]\nName=escape-link\nIsRelative=1\nPath=escape-link\n",
    )
    .expect("profile listing");
    assert_eq!(
        firefox_candidates(&home).expect_err("no contained profiles"),
        BrowserError::NoBrowserSession
    );
}

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
