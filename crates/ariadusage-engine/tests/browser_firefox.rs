// Ported from CodexBar Tests/CodexBarTests/BrowserDetectionTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/cookie_db.rs"]
mod cookie_db;

use std::sync::Arc;

use ariadusage_core::cookie as cookie_core;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::browser::{Browser, BrowserError, CookieQuery};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_protocol::ids::ProviderId;

use cookie_db::{StaticKeys, TestHome, authorization, call, insert_firefox};

fn read_firefox(
    home: &TestHome,
) -> Result<Vec<ariadusage_engine::brokers::browser::CookieCandidate>, BrowserError> {
    let broker = home.broker(Arc::new(StaticKeys::without_key()));
    let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
    let browsers = [Browser::Firefox];
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
            &call(FetchInteraction::UserInitiated),
            authorization(),
            query,
        ))
}

#[test]
fn firefox_container_rows_are_separate_candidates_and_partitioned_rows_are_dropped() {
    let home = TestHome::new();
    let profile = home.firefox_profile("default-release", 17);
    let database = profile.join("cookies.sqlite");
    let expires = (jiff::Timestamp::now().as_second() + 3600) * 1000;
    insert_firefox(
        &database,
        ".claude.ai",
        "default",
        expires,
        "default-value",
        "",
    );
    insert_firefox(
        &database,
        ".claude.ai",
        "work",
        expires,
        "work-value",
        "^userContextId=1",
    );
    insert_firefox(
        &database,
        ".claude.ai",
        "personal",
        expires,
        "personal-value",
        "^userContextId=2",
    );
    insert_firefox(
        &database,
        ".claude.ai",
        "partitioned",
        expires,
        "partition-value",
        "^userContextId=2^partitionKey=(https,example.invalid)",
    );
    let candidates = read_firefox(&home).expect("Firefox candidates");
    assert_eq!(candidates.len(), 3);
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "default-value"
    );
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.label.ends_with("· Work"))
    );
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.label.ends_with("· Personal"))
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.records[0].value.expose_secret() != "partition-value")
    );
    assert_eq!(candidates[0].diagnostics.rows_read, 1);
    assert_eq!(candidates[0].diagnostics.partitioned, 0);
    let work = candidates
        .iter()
        .find(|candidate| candidate.label.ends_with("· Work"))
        .expect("Work candidate");
    assert_eq!(work.diagnostics.rows_read, 1);
    assert_eq!(work.diagnostics.partitioned, 0);
    let personal = candidates
        .iter()
        .find(|candidate| candidate.label.ends_with("· Personal"))
        .expect("Personal candidate");
    assert_eq!(personal.diagnostics.rows_read, 2);
    assert_eq!(personal.diagnostics.partitioned, 1);
}

#[test]
// CodexBar: BrowserDetectionTests.swift:752
fn firefox_profile_without_its_default_database_has_no_session() {
    let home = TestHome::new();
    let root = home.config.join("mozilla/firefox");
    std::fs::create_dir_all(root.join("default-release")).expect("Firefox profile directory");
    std::fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=default-release\nIsRelative=1\nPath=default-release\nDefault=1\n",
    )
    .expect("Firefox profile listing");
    assert_eq!(
        read_firefox(&home).expect_err("profile without database"),
        BrowserError::NoBrowserSession
    );
}

#[test]
fn firefox_expiry_uses_seconds_before_schema_16_and_milliseconds_after() {
    for (version, multiplier) in [(15, 1), (17, 1000)] {
        let home = TestHome::new();
        let profile = home.firefox_profile("default", version);
        let database = profile.join("cookies.sqlite");
        let now = jiff::Timestamp::now().as_second();
        insert_firefox(
            &database,
            ".claude.ai",
            "active",
            (now + 3600) * multiplier,
            "active-value",
            "",
        );
        insert_firefox(
            &database,
            ".claude.ai",
            "expired",
            (now - 3600) * multiplier,
            "expired-value",
            "",
        );
        let candidates = read_firefox(&home).expect("live Firefox candidate");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].records.len(), 1);
        assert_eq!(
            candidates[0].records[0].value.expose_secret(),
            "active-value"
        );
        assert_eq!(candidates[0].diagnostics.expired, 1);
    }
}

#[test]
fn firefox_reader_does_not_request_chromium_keys() {
    let home = TestHome::new();
    let profile = home.firefox_profile("default-release", 17);
    let database = profile.join("cookies.sqlite");
    insert_firefox(&database, ".claude.ai", "sid", 0, "synthetic", "");
    let keys = Arc::new(StaticKeys::failing(
        ariadusage_engine::brokers::secret_store::safe_storage::SafeStorageError::Locked,
    ));
    let broker = home.broker(keys.clone());
    let call = BrokerCall {
        interaction: FetchInteraction::Background,
        cancel: tokio_util::sync::CancellationToken::new(),
        request_id: "firefox-no-key".to_owned(),
    };
    let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
    let browsers = [Browser::Firefox];
    let query = CookieQuery {
        provider: ProviderId::new("claude").expect("provider id"),
        domains: &domains,
        names: None,
        browsers: &browsers,
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(broker.candidates(&call, authorization(), query))
        .expect("Firefox candidates");
    assert_eq!(result.len(), 1);
    assert_eq!(keys.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}
