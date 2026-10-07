// Ported from CodexBar Tests/CodexBarTests/BrowserCookieImportSupportTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/cookie_db.rs"]
mod cookie_db;

use std::sync::Arc;

use ariadusage_core::cookie as cookie_core;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::browser::{
    Browser, CookieQuery, SafeStorageKeyProvider, test_support,
};
use ariadusage_engine::brokers::browser::{BrowserError, CookieCandidate};
use ariadusage_protocol::ids::ProviderId;
use sha2::{Digest, Sha256};

use cookie_db::{ChromiumRow, StaticKeys, TestHome, authorization, call, insert_chromium};

const CHROMIUM_EPOCH_MICROS: i64 = 11_644_473_600_000_000;
type CandidatesResult = Result<Vec<CookieCandidate>, BrowserError>;

fn encrypted_value(host: &str, value: &[u8], password: &[u8], tag: &[u8; 3]) -> Vec<u8> {
    let mut plaintext = Sha256::digest(host.as_bytes()).to_vec();
    plaintext.extend_from_slice(value);
    let encrypted = test_support::encrypt_test(&plaintext, password).expect("synthetic ciphertext");
    let mut tagged = tag.to_vec();
    tagged.extend_from_slice(&encrypted);
    tagged
}

fn expiry_after(seconds: i64) -> i64 {
    (jiff::Timestamp::now().as_second() + seconds) * 1_000_000 + CHROMIUM_EPOCH_MICROS
}

fn read_chrome(
    home: &TestHome,
    profile: Browser,
    keys: Arc<dyn SafeStorageKeyProvider>,
) -> CandidatesResult {
    read_in_order(home, &[profile], keys)
}

fn read_in_order(
    home: &TestHome,
    browsers: &[Browser],
    keys: Arc<dyn SafeStorageKeyProvider>,
) -> CandidatesResult {
    let broker = home.broker(keys);
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
            &call(FetchInteraction::UserInitiated),
            authorization(),
            query,
        ))
}

#[test]
// CodexBar: BrowserCookieImportSupportTests.swift:78
fn imports_in_declared_browser_order_and_skips_unavailable_profiles() {
    let home = TestHome::new();
    let chrome = home.chromium_profile(Browser::Chrome, "Work");
    let brave = home.chromium_profile(Browser::Brave, "Personal");
    let chrome_db = chrome.join("Cookies");
    let brave_db = brave.join("Cookies");
    insert_chromium(
        &chrome_db,
        ChromiumRow {
            host: ".claude.ai",
            name: "session",
            expires_utc: 0,
            value: "chrome-plain",
            encrypted: b"",
            partition_key: "",
        },
    );
    insert_chromium(
        &brave_db,
        ChromiumRow {
            host: ".claude.ai",
            name: "session",
            expires_utc: 0,
            value: "brave-plain",
            encrypted: b"",
            partition_key: "",
        },
    );
    let candidates = read_in_order(
        &home,
        &[Browser::Brave, Browser::Edge, Browser::Chrome],
        Arc::new(StaticKeys::without_key()),
    )
    .expect("available browser profiles");
    assert_eq!(
        candidates
            .iter()
            .map(|entry| entry.browser)
            .collect::<Vec<_>>(),
        [Browser::Brave, Browser::Chrome]
    );
}

#[test]
// CodexBar: BrowserCookieImportSupportTests.swift:97
// CodexBar: BrowserDetectionTests.swift:218
fn missing_browser_profiles_report_no_session() {
    let home = TestHome::new();
    let error = read_chrome(&home, Browser::Chrome, Arc::new(StaticKeys::without_key()))
        .expect_err("missing profile");
    assert_eq!(error, BrowserError::NoBrowserSession);
}

#[test]
// CodexBar: BrowserCookieImportSupportTests.swift:113
// CodexBar: BrowserDetectionTests.swift:142
fn browser_candidates_keep_profiles_separate() {
    let home = TestHome::new();
    let chrome = home.chromium_profile(Browser::Chrome, "Work");
    let brave = home.chromium_profile(Browser::Brave, "Personal");
    insert_chromium(
        &chrome.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "session",
            expires_utc: 0,
            value: "chrome-profile",
            encrypted: b"",
            partition_key: "",
        },
    );
    insert_chromium(
        &brave.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "session",
            expires_utc: 0,
            value: "brave-profile",
            encrypted: b"",
            partition_key: "",
        },
    );
    let candidates = read_in_order(
        &home,
        &[Browser::Brave, Browser::Chrome],
        Arc::new(StaticKeys::without_key()),
    )
    .expect("two profile candidates");
    assert_eq!(candidates.len(), 2);
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "brave-profile"
    );
    assert_eq!(
        candidates[1].records[0].value.expose_secret(),
        "chrome-profile"
    );
}

#[test]
// CodexBar: BrowserDetectionTests.swift:167
fn only_requested_browser_profiles_are_discovered() {
    let home = TestHome::new();
    let chrome = home.chromium_profile(Browser::Chrome, "Work");
    let brave = home.chromium_profile(Browser::Brave, "Personal");
    for database in [chrome.join("Cookies"), brave.join("Cookies")] {
        insert_chromium(
            &database,
            ChromiumRow {
                host: ".claude.ai",
                name: "session",
                expires_utc: 0,
                value: "synthetic-session",
                encrypted: b"",
                partition_key: "",
            },
        );
    }
    let candidates = read_in_order(
        &home,
        &[Browser::Chrome],
        Arc::new(StaticKeys::without_key()),
    )
    .expect("filtered browser candidate");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].browser, Browser::Chrome);
}

#[test]
fn stored_hosts_with_quotes_and_like_markers_do_not_expand_matches() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    let database = profile.join("Cookies");
    for (host, name) in [
        (".claude.ai", "allowed"),
        ("sub.claude.ai", "subdomain"),
        ("claude.ai' OR 1=1 --", "quoted"),
        (".claude%ai", "percent"),
        (".claude_ai", "underscore"),
    ] {
        insert_chromium(
            &database,
            ChromiumRow {
                host,
                name,
                expires_utc: 0,
                value: name,
                encrypted: b"",
                partition_key: "",
            },
        );
    }
    let candidates = read_chrome(&home, Browser::Chrome, Arc::new(StaticKeys::without_key()))
        .expect("declared-domain rows");
    let mut names = candidates[0]
        .records
        .iter()
        .map(|record| record.name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, ["allowed", "subdomain"]);
    assert_eq!(candidates[0].diagnostics.rows_read, 2);
}

#[test]
fn decrypts_v10_v11_and_plaintext_rows_and_reports_rejections_by_count() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    let database = profile.join("Cookies");
    let v10 = encrypted_value(".claude.ai", b"v10-value", b"peanuts", b"v10");
    let v11 = encrypted_value(".claude.ai", b"v11-value", b"synthetic-password", b"v11");
    let expired = encrypted_value(".claude.ai", b"old", b"peanuts", b"v10");
    let partitioned = encrypted_value(".claude.ai", b"partitioned", b"peanuts", b"v10");
    let mut unsupported = b"v12".to_vec();
    unsupported.extend_from_slice(b"future-encryption");
    insert_chromium(
        &database,
        ChromiumRow {
            host: ".claude.ai",
            name: "v10",
            expires_utc: expiry_after(3600),
            value: "",
            encrypted: &v10,
            partition_key: "",
        },
    );
    insert_chromium(
        &database,
        ChromiumRow {
            host: ".claude.ai",
            name: "v11",
            expires_utc: 0,
            value: "",
            encrypted: &v11,
            partition_key: "",
        },
    );
    insert_chromium(
        &database,
        ChromiumRow {
            host: ".claude.ai",
            name: "plain",
            expires_utc: 0,
            value: "plaintext-wins",
            encrypted: b"v10invalid",
            partition_key: "",
        },
    );
    insert_chromium(
        &database,
        ChromiumRow {
            host: ".claude.ai",
            name: "future",
            expires_utc: 0,
            value: "",
            encrypted: &unsupported,
            partition_key: "",
        },
    );
    insert_chromium(
        &database,
        ChromiumRow {
            host: ".claude.ai",
            name: "expired",
            expires_utc: expiry_after(-3600),
            value: "",
            encrypted: &expired,
            partition_key: "",
        },
    );
    insert_chromium(
        &database,
        ChromiumRow {
            host: ".claude.ai",
            name: "partitioned",
            expires_utc: 0,
            value: "",
            encrypted: &partitioned,
            partition_key: "https://example.invalid",
        },
    );
    insert_chromium(
        &database,
        ChromiumRow {
            host: "notclaude.ai",
            name: "outside",
            expires_utc: 0,
            value: "",
            encrypted: b"v10ignored",
            partition_key: "",
        },
    );

    let keys = Arc::new(StaticKeys::available(b"synthetic-password".to_vec()));
    let candidates = read_chrome(&home, Browser::Chrome, keys.clone()).expect("candidate");
    assert_eq!(keys.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let candidate = &candidates[0];
    let values = candidate
        .records
        .iter()
        .map(|record| record.value.expose_secret())
        .collect::<Vec<_>>();
    assert!(values.contains(&"v10-value"));
    assert!(values.contains(&"v11-value"));
    assert!(values.contains(&"plaintext-wins"));
    assert_eq!(values.len(), 3);
    assert_eq!(candidate.diagnostics.rows_read, 6);
    assert_eq!(candidate.diagnostics.expired, 1);
    assert_eq!(candidate.diagnostics.partitioned, 1);
    assert_eq!(
        candidate.diagnostics.unsupported_by_tag.get("v12"),
        Some(&1)
    );
    let debug = format!("{candidate:?}");
    assert!(!debug.contains("plaintext-wins"));
    assert!(!debug.contains("synthetic-password"));
}

#[test]
fn v11_falls_back_to_the_empty_password_and_bad_v24_host_hash_is_counted() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    let database = profile.join("Cookies");
    let v11 = encrypted_value(".claude.ai", b"empty-password-value", b"", b"v11");
    let mut mismatch_plain = vec![0xA5; 32];
    mismatch_plain.extend_from_slice(b"wrong-host-prefix");
    let encrypted = test_support::encrypt_test(&mismatch_plain, b"peanuts").expect("ciphertext");
    let mut tagged_mismatch = b"v10".to_vec();
    tagged_mismatch.extend_from_slice(&encrypted);
    insert_chromium(
        &database,
        ChromiumRow {
            host: ".claude.ai",
            name: "empty-key",
            expires_utc: 0,
            value: "",
            encrypted: &v11,
            partition_key: "",
        },
    );
    insert_chromium(
        &database,
        ChromiumRow {
            host: ".claude.ai",
            name: "wrong-hash",
            expires_utc: 0,
            value: "",
            encrypted: &tagged_mismatch,
            partition_key: "",
        },
    );
    let candidates = read_chrome(
        &home,
        Browser::Chrome,
        Arc::new(StaticKeys::available(b"wrong-password".to_vec())),
    )
    .expect("candidate");
    assert_eq!(candidates[0].records.len(), 1);
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "empty-password-value"
    );
    assert_eq!(candidates[0].diagnostics.rejected_by_hash, 1);
}

#[test]
fn chromium_kdf_known_answer_is_stable() {
    assert_eq!(
        test_support::derive_test_key(b"peanuts"),
        [
            253, 98, 31, 229, 162, 180, 2, 83, 157, 250, 20, 124, 169, 39, 39, 120
        ]
    );
    assert_eq!(
        test_support::derive_test_key(b""),
        [
            208, 208, 236, 156, 125, 119, 212, 58, 197, 65, 135, 250, 72, 24, 209, 127
        ]
    );
}

#[test]
fn version_23_v10_plaintext_does_not_require_a_host_hash() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Legacy");
    let database = profile.join("Cookies");
    rusqlite::Connection::open(&database)
        .expect("open synthetic database")
        .execute("UPDATE meta SET version=23", [])
        .expect("set legacy version");
    let encrypted = test_support::encrypt_test(b"legacy-without-prefix", b"peanuts")
        .expect("version 23 ciphertext");
    let mut tagged = b"v10".to_vec();
    tagged.extend_from_slice(&encrypted);
    insert_chromium(
        &database,
        ChromiumRow {
            host: ".claude.ai",
            name: "legacy",
            expires_utc: 0,
            value: "",
            encrypted: &tagged,
            partition_key: "",
        },
    );
    let candidates = read_chrome(&home, Browser::Chrome, Arc::new(StaticKeys::without_key()))
        .expect("version 23 candidate");
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "legacy-without-prefix"
    );
    assert_eq!(candidates[0].diagnostics.rejected_by_hash, 0);
}

#[test]
fn unsupported_only_rows_remain_visible_in_diagnostics() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Future");
    let mut unsupported = b"v12".to_vec();
    unsupported.extend_from_slice(b"synthetic-future-value");
    insert_chromium(
        &profile.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "future",
            expires_utc: 0,
            value: "",
            encrypted: &unsupported,
            partition_key: "",
        },
    );
    let candidates = read_chrome(&home, Browser::Chrome, Arc::new(StaticKeys::without_key()))
        .expect("unsupported diagnostics candidate");
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].records.is_empty());
    assert_eq!(
        candidates[0].diagnostics.unsupported_by_tag.get("v12"),
        Some(&1)
    );
}

#[test]
fn safe_storage_is_queried_once_for_multiple_profiles_of_one_browser() {
    let home = TestHome::new();
    let default = home.chromium_profile(Browser::Chrome, "Work");
    let extra = home.config.join("google-chrome/Profile 1");
    std::fs::create_dir_all(&extra).expect("extra browser profile");
    cookie_db::create_chromium_database(&extra.join("Cookies"), 24);
    let password = b"synthetic-password";
    for database in [default.join("Cookies"), extra.join("Cookies")] {
        let value = encrypted_value(".claude.ai", b"same-browser", password, b"v11");
        insert_chromium(
            &database,
            ChromiumRow {
                host: ".claude.ai",
                name: "sid",
                expires_utc: 0,
                value: "",
                encrypted: &value,
                partition_key: "",
            },
        );
    }
    let keys = Arc::new(StaticKeys::available(password.to_vec()));
    let candidates = read_chrome(&home, Browser::Chrome, keys.clone()).expect("two profiles");
    assert_eq!(candidates.len(), 2);
    assert_eq!(keys.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

proptest::proptest! {
    #[test]
    fn v10_encrypt_decrypt_round_trips_for_synthetic_hosts(
        host in "[a-z][a-z0-9.-]{2,32}",
        value in "[a-zA-Z0-9_-]{0,96}",
    ) {
        let mut plaintext = Sha256::digest(host.as_bytes()).to_vec();
        plaintext.extend_from_slice(value.as_bytes());
        let encrypted = test_support::encrypt_test(&plaintext, b"peanuts")
            .expect("synthetic encryption");
        let decrypted = test_support::decrypt_v10_test(&encrypted, &host, 24)
            .expect("version 10 decrypt");
        proptest::prop_assert_eq!(decrypted.as_slice(), value.as_bytes());
    }

    #[test]
    fn a_different_v11_key_is_rejected(
        host in "[a-z][a-z0-9.-]{2,32}",
        value in "[a-zA-Z0-9_-]{0,96}",
    ) {
        let mut plaintext = Sha256::digest(host.as_bytes()).to_vec();
        plaintext.extend_from_slice(value.as_bytes());
        let encrypted = test_support::encrypt_test(&plaintext, b"synthetic-password")
            .expect("synthetic encryption");
        let result = test_support::decrypt_v11_test(
            &encrypted,
            b"another-password",
            &host,
            24,
        );
        proptest::prop_assert!(result.is_err());
    }
}
