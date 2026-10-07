// Ported from CodexBar Tests/CodexBarTests/BrowserCookieAccessGateTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/cookie_db.rs"]
mod cookie_db;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ariadusage_core::cookie as cookie_core;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::browser::{
    Browser, BrowserError, CookieQuery, SafeStorageKeyProvider, test_support,
};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::secret_store::safe_storage::SafeStorageError;
use ariadusage_protocol::ids::ProviderId;
use sha2::{Digest, Sha256};

use cookie_db::{ChromiumRow, StaticKeys, TestHome, authorization, insert_chromium};

fn encrypted_v11(host: &str, value: &str) -> Vec<u8> {
    let mut plaintext = Sha256::digest(host.as_bytes()).to_vec();
    plaintext.extend_from_slice(value.as_bytes());
    let encrypted = test_support::encrypt_test(&plaintext, b"family-key").expect("test encryption");
    let mut tagged = b"v11".to_vec();
    tagged.extend_from_slice(&encrypted);
    tagged
}

fn add_v11_profile(home: &TestHome, browser: Browser, name: &str) {
    let profile = home.chromium_profile(browser, name);
    let value = encrypted_v11(".claude.ai", name);
    insert_chromium(
        &profile.join("Cookies"),
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

fn read(
    home: &TestHome,
    browsers: &[Browser],
    interaction: FetchInteraction,
    keys: Arc<dyn SafeStorageKeyProvider>,
) -> Result<Vec<ariadusage_engine::brokers::browser::CookieCandidate>, BrowserError> {
    let broker = home.broker(keys);
    let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
    let query = CookieQuery {
        provider: ProviderId::new("claude").expect("provider id"),
        domains: &domains,
        names: None,
        browsers,
    };
    let call = BrokerCall {
        interaction,
        cancel: tokio_util::sync::CancellationToken::new(),
        request_id: "gate-test".to_owned(),
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(broker.candidates(&call, authorization(), query))
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:54
fn background_locked_key_does_not_unlock_and_plaintext_version_ten_remains_available() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    let mut plaintext = Sha256::digest(b".claude.ai").to_vec();
    plaintext.extend_from_slice(b"legacy-value");
    let encrypted = test_support::encrypt_test(&plaintext, b"peanuts").expect("ciphertext");
    let mut tagged = b"v10".to_vec();
    tagged.extend_from_slice(&encrypted);
    insert_chromium(
        &profile.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "legacy",
            expires_utc: 0,
            value: "",
            encrypted: &tagged,
            partition_key: "",
        },
    );
    let v11 = encrypted_v11(".claude.ai", "locked-v11");
    insert_chromium(
        &profile.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "locked",
            expires_utc: 0,
            value: "",
            encrypted: &v11,
            partition_key: "",
        },
    );
    let keys = Arc::new(StaticKeys::failing(SafeStorageError::Locked));
    let candidates = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::Background,
        keys.clone(),
    )
    .expect("legacy candidate");
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "legacy-value"
    );
    assert_eq!(keys.calls.load(Ordering::SeqCst), 1);
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:74
fn a_dismissal_persists_browser_and_family_cooldowns() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    let error = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::failing(SafeStorageError::Dismissed)),
    )
    .expect_err("dismissal surfaced");
    assert_eq!(error, BrowserError::Dismissed);
    let state = home.state_store().load().expect("saved state");
    assert_eq!(state.browser_cooldowns.len(), 2);
    assert!(
        state
            .browser_cooldowns
            .keys()
            .all(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
    );
    let now = jiff::Timestamp::now().as_second();
    assert!(
        state
            .browser_cooldowns
            .values()
            .all(|until| *until >= now + 21_600 && *until <= now + 21_601)
    );
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:82
fn background_reads_skip_a_dismissed_chromium_family() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    add_v11_profile(&home, Browser::Brave, "Personal");
    let dismissed = read(
        &home,
        &[Browser::Chrome, Browser::Brave],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::failing(SafeStorageError::Dismissed)),
    );
    assert_eq!(
        dismissed.expect_err("first request dismissed"),
        BrowserError::Dismissed
    );

    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    let skipped = read(
        &home,
        &[Browser::Brave],
        FetchInteraction::Background,
        keys.clone(),
    );
    assert_eq!(
        skipped.expect_err("family cooldown active"),
        BrowserError::Dismissed
    );
    assert_eq!(keys.calls.load(Ordering::SeqCst), 0);
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:92
fn one_user_retry_can_clear_a_family_dismissal() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    let _ = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::failing(SafeStorageError::Dismissed)),
    );
    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    let candidates = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        keys.clone(),
    )
    .expect("explicit retry");
    assert_eq!(candidates[0].records[0].value.expose_secret(), "Work");
    assert_eq!(keys.calls.load(Ordering::SeqCst), 1);
    assert!(
        home.state_store()
            .load()
            .expect("cleared cooldown")
            .browser_cooldowns
            .is_empty()
    );
}

#[test]
fn plaintext_cookie_does_not_clear_family_dismissal_for_a_later_profile() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    add_v11_profile(&home, Browser::Brave, "Personal");
    let _ = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::failing(SafeStorageError::Dismissed)),
    );
    let chrome_database = home.config.join("google-chrome/Default/Cookies");
    insert_chromium(
        &chrome_database,
        ChromiumRow {
            host: ".claude.ai",
            name: "legacy",
            expires_utc: 0,
            value: "v10-safe-value",
            encrypted: b"",
            partition_key: "",
        },
    );

    let keys = Arc::new(StaticKeys::available(b"synthetic-key".to_vec()));
    let candidates = read(
        &home,
        &[Browser::Chrome, Browser::Brave],
        FetchInteraction::Background,
        keys.clone(),
    )
    .expect("legacy plaintext remains available");
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "v10-safe-value"
    );
    assert_eq!(keys.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        home.state_store()
            .load()
            .expect("saved state")
            .browser_cooldowns
            .len(),
        2
    );
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:103
fn one_retry_scope_does_not_prompt_twice_after_a_retry_dismissal() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    add_v11_profile(&home, Browser::Brave, "Personal");
    let _ = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::failing(SafeStorageError::Dismissed)),
    );
    let keys = Arc::new(StaticKeys::failing(SafeStorageError::Dismissed));
    let error = read(
        &home,
        &[Browser::Chrome, Browser::Brave],
        FetchInteraction::UserInitiated,
        keys.clone(),
    )
    .expect_err("retry dismissed");
    assert_eq!(error, BrowserError::Dismissed);
    assert_eq!(keys.calls.load(Ordering::SeqCst), 1);
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:139
fn a_locked_key_does_not_persist_dismissal_cooldown() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    let error = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::failing(SafeStorageError::Locked)),
    )
    .expect_err("locked key surfaced");
    assert_eq!(error, BrowserError::Locked);
    assert!(
        home.state_store()
            .load()
            .expect("empty state")
            .browser_cooldowns
            .is_empty()
    );
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:157
fn safe_storage_key_is_requested_once_for_a_browser_refresh() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    let _ = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        keys.clone(),
    )
    .expect("candidate");
    assert_eq!(keys.calls.load(Ordering::SeqCst), 1);
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:173
fn v11_rows_can_use_the_empty_password_fallback() {
    let home = TestHome::new();
    let profile = home.chromium_profile(Browser::Chrome, "Work");
    let mut plaintext = Sha256::digest(b".claude.ai").to_vec();
    plaintext.extend_from_slice(b"empty-fallback");
    let encrypted = test_support::encrypt_test(&plaintext, b"").expect("empty-key ciphertext");
    let mut tagged = b"v11".to_vec();
    tagged.extend_from_slice(&encrypted);
    insert_chromium(
        &profile.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "sid",
            expires_utc: 0,
            value: "",
            encrypted: &tagged,
            partition_key: "",
        },
    );
    let candidates = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::available(b"wrong-key".to_vec())),
    )
    .expect("fallback candidate");
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "empty-fallback"
    );
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:181
fn background_key_errors_do_not_escape_candidate_debug_output() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    let result = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::Background,
        Arc::new(StaticKeys::failing(SafeStorageError::Locked)),
    )
    .expect_err("locked");
    let debug = format!("{result:?}");
    assert_eq!(debug, "Locked");
    assert!(!debug.contains("family-key"));
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:208
fn dismissal_cooldown_keeps_only_opaque_digest_keys() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "private profile name");
    let _ = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::failing(SafeStorageError::Dismissed)),
    );
    let state = home.state_store().load().expect("state");
    let serialized = serde_json::to_string(&state).expect("serialized state");
    assert!(!serialized.contains("private profile name"));
    assert!(state.browser_cooldowns.keys().all(|key| key.len() == 64));
}
