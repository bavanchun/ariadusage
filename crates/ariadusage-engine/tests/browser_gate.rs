// Ported from CodexBar Tests/CodexBarTests/BrowserCookieAccessGateTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/BrowserDetectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
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

use cookie_db::{
    ChromiumRow, StaticKeys, TestHome, authorization, create_chromium_database, insert_chromium,
    insert_firefox,
};

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

type CandidatesResult =
    Result<Vec<ariadusage_engine::brokers::browser::CookieCandidate>, BrowserError>;

fn read(
    home: &TestHome,
    browsers: &[Browser],
    interaction: FetchInteraction,
    keys: Arc<dyn SafeStorageKeyProvider>,
) -> CandidatesResult {
    read_with(home, browsers, interaction, false, keys)
}

/// Reads as a retry the user acknowledged, the only kind allowed through a saved cooldown.
fn read_acknowledged(
    home: &TestHome,
    browsers: &[Browser],
    interaction: FetchInteraction,
    keys: Arc<dyn SafeStorageKeyProvider>,
) -> CandidatesResult {
    read_with(home, browsers, interaction, true, keys)
}

fn read_with(
    home: &TestHome,
    browsers: &[Browser],
    interaction: FetchInteraction,
    acknowledged_retry: bool,
    keys: Arc<dyn SafeStorageKeyProvider>,
) -> CandidatesResult {
    let broker = home.broker(keys);
    let domains = cookie_core::DeclaredDomains::new(["claude.ai"]).expect("declared host");
    let query = CookieQuery {
        provider: ProviderId::new("claude").expect("provider id"),
        domains: &domains,
        names: None,
        browsers,
        acknowledged_retry,
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
// CodexBar: BrowserCookieAccessGateTests.swift:55
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
// CodexBar: BrowserCookieAccessGateTests.swift:73
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
// CodexBar: BrowserDetectionTests.swift:397
fn background_reads_proceed_during_a_dismissed_chromium_family() {
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
    let candidates = read(
        &home,
        &[Browser::Brave],
        FetchInteraction::Background,
        keys.clone(),
    )
    .expect("background read proceeds during family cooldown");
    assert_eq!(candidates[0].records[0].value.expose_secret(), "Personal");
    assert_eq!(keys.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cooldown_count(&home), 2);
}

fn dismiss(home: &TestHome, browser: Browser) {
    let error = read(
        home,
        &[browser],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::failing(SafeStorageError::Dismissed)),
    )
    .expect_err("dismissal surfaced");
    assert_eq!(error, BrowserError::Dismissed);
}

fn cooldown_count(home: &TestHome) -> usize {
    home.state_store()
        .load()
        .expect("saved state")
        .browser_cooldowns
        .len()
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:55
// CodexBar: BrowserDetectionTests.swift:529
fn manual_retry_respects_browser_and_family_denial_cooldowns() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    add_v11_profile(&home, Browser::Brave, "Personal");
    dismiss(&home, Browser::Chrome);

    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    for browser in [Browser::Chrome, Browser::Brave] {
        let error = read(
            &home,
            &[browser],
            FetchInteraction::UserInitiated,
            keys.clone(),
        )
        .expect_err("cooldown blocks a manual retry");
        assert_eq!(error, BrowserError::Dismissed);
    }
    assert_eq!(keys.calls.load(Ordering::SeqCst), 0);
    assert_eq!(cooldown_count(&home), 2);
}

#[test]
// CodexBar: BrowserDetectionTests.swift:397
fn acknowledged_retry_reads_through_the_cooldown_and_clears_it() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    dismiss(&home, Browser::Chrome);

    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    let candidates = read_acknowledged(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        keys.clone(),
    )
    .expect("acknowledged retry");
    assert_eq!(candidates[0].records[0].value.expose_secret(), "Work");
    assert_eq!(keys.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cooldown_count(&home), 0);
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:55
fn acknowledgement_does_not_clear_a_cooldown_in_the_background() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    dismiss(&home, Browser::Chrome);

    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    let candidates = read_acknowledged(
        &home,
        &[Browser::Chrome],
        FetchInteraction::Background,
        keys.clone(),
    )
    .expect("background read recovers during cooldown");
    assert_eq!(candidates[0].records[0].value.expose_secret(), "Work");
    assert_eq!(keys.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cooldown_count(&home), 2);

    let error = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        keys.clone(),
    )
    .expect_err("cooldown still blocks user-initiated retry");
    assert_eq!(error, BrowserError::Dismissed);
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:102
fn background_refresh_recovers_during_browser_and_family_cooldowns_when_access_becomes_allowed() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Chrome Work");
    add_v11_profile(&home, Browser::Brave, "Brave Personal");
    dismiss(&home, Browser::Chrome);

    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    for (browser, expected_val) in [
        (Browser::Chrome, "Chrome Work"),
        (Browser::Brave, "Brave Personal"),
    ] {
        let candidates = read(
            &home,
            &[browser],
            FetchInteraction::Background,
            keys.clone(),
        )
        .expect("background refresh recovers during cooldown");
        assert_eq!(candidates[0].records[0].value.expose_secret(), expected_val);

        // A background success does not clear the interactive path's persisted denial policy.
        let error = read(
            &home,
            &[browser],
            FetchInteraction::UserInitiated,
            keys.clone(),
        )
        .expect_err("user-initiated retry stays blocked by cooldown");
        assert_eq!(error, BrowserError::Dismissed);
    }
    assert_eq!(cooldown_count(&home), 2);
}

#[test]
// CodexBar: BrowserDetectionTests.swift:397
fn acknowledged_retry_with_two_profiles_reuses_cached_key_under_remaining_family_cooldown() {
    let home = TestHome::new();
    let default_prof = home.chromium_profile(Browser::Chrome, "Default");
    let val_default = encrypted_v11(".claude.ai", "Default");
    insert_chromium(
        &default_prof.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "sid",
            expires_utc: 0,
            value: "",
            encrypted: &val_default,
            partition_key: "",
        },
    );
    let extra = home.config.join("google-chrome/Profile 1");
    std::fs::create_dir_all(&extra).expect("extra profile");
    create_chromium_database(&extra.join("Cookies"), 24);
    let val_work = encrypted_v11(".claude.ai", "Work");
    insert_chromium(
        &extra.join("Cookies"),
        ChromiumRow {
            host: ".claude.ai",
            name: "sid",
            expires_utc: 0,
            value: "",
            encrypted: &val_work,
            partition_key: "",
        },
    );
    let local_state = home.config.join("google-chrome/Local State");
    std::fs::write(
        &local_state,
        serde_json::json!({
            "profile": {
                "info_cache": {
                    "Default": { "name": "Default" },
                    "Profile 1": { "name": "Work" }
                }
            }
        })
        .to_string(),
    )
    .expect("local state");

    add_v11_profile(&home, Browser::Brave, "Personal");
    dismiss(&home, Browser::Brave);
    assert_eq!(cooldown_count(&home), 2);

    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    let candidates = read_acknowledged(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        keys.clone(),
    )
    .expect("acknowledged retry");

    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].records[0].value.expose_secret(), "Default");
    assert_eq!(candidates[1].records[0].value.expose_secret(), "Work");
    assert_eq!(keys.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cooldown_count(&home), 2);
}

#[test]
fn acknowledged_success_keeps_another_browsers_family_cooldown() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    add_v11_profile(&home, Browser::Brave, "Personal");
    dismiss(&home, Browser::Brave);
    let error = read_acknowledged(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        Arc::new(StaticKeys::failing(SafeStorageError::Dismissed)),
    )
    .expect_err("acknowledged retry dismissed");
    assert_eq!(error, BrowserError::Dismissed);
    assert_eq!(cooldown_count(&home), 3);

    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    read_acknowledged(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        keys.clone(),
    )
    .expect("acknowledged retry");
    assert_eq!(cooldown_count(&home), 2);

    let error = read(
        &home,
        &[Browser::Chrome],
        FetchInteraction::UserInitiated,
        keys.clone(),
    )
    .expect_err("the remaining family cooldown still applies");
    assert_eq!(error, BrowserError::Dismissed);
    assert_eq!(keys.calls.load(Ordering::SeqCst), 1);
}

#[test]
// CodexBar: BrowserCookieAccessGateTests.swift:180
fn firefox_reads_ignore_a_chromium_family_cooldown() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    dismiss(&home, Browser::Chrome);
    let profile = home.firefox_profile("default-release", 17);
    let expires = (jiff::Timestamp::now().as_second() + 3600) * 1000;
    insert_firefox(
        &profile.join("cookies.sqlite"),
        ".claude.ai",
        "sid",
        expires,
        "firefox-value",
        "",
    );

    let keys = Arc::new(StaticKeys::available(b"family-key".to_vec()));
    let candidates = read(
        &home,
        &[Browser::Firefox],
        FetchInteraction::Background,
        keys.clone(),
    )
    .expect("Firefox candidate");
    assert_eq!(
        candidates[0].records[0].value.expose_secret(),
        "firefox-value"
    );
    assert_eq!(keys.calls.load(Ordering::SeqCst), 0);
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
        FetchInteraction::UserInitiated,
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
// CodexBar: BrowserDetectionTests.swift:440
fn one_retry_scope_does_not_prompt_twice_after_a_retry_dismissal() {
    let home = TestHome::new();
    add_v11_profile(&home, Browser::Chrome, "Work");
    add_v11_profile(&home, Browser::Brave, "Personal");
    dismiss(&home, Browser::Chrome);
    let keys = Arc::new(StaticKeys::failing(SafeStorageError::Dismissed));
    let error = read_acknowledged(
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
// CodexBar: BrowserCookieAccessGateTests.swift:138
// CodexBar: BrowserDetectionTests.swift:312
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
// CodexBar: BrowserCookieAccessGateTests.swift:156
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
// CodexBar: BrowserCookieAccessGateTests.swift:172
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
// CodexBar: BrowserCookieAccessGateTests.swift:180
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
// CodexBar: BrowserCookieAccessGateTests.swift:209
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
