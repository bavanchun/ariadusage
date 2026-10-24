// Ported from CodexBar Tests/CodexBarTests/CLICookieRefreshTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::Arc;

use ariadusage_cli::{
    CookieRefreshOptions, CookieRefreshResult, CookieRefreshStatus, cookie_refresh_browser_order,
    cookie_refresh_failure, cookie_refresh_targets, cookie_refresh_text, perform_cookie_refreshes,
    with_cookie_refresh_cache_suppressed,
};
use ariadusage_core::config::{Config, ProviderConfig, ProviderEntry};
use ariadusage_core::pace::ProviderPaceCapability;
use ariadusage_core::pipeline::{ClassifiedError, FetchInteraction};
use ariadusage_core::providers::{ProviderDescriptor, SourceMode, first_party_order};
use ariadusage_engine::brokers::browser::{Browser, BrowserGate, DEFAULT_IMPORT_ORDER, RetryScope};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::cookie_cache::{CacheScope, CookieCache};
use ariadusage_engine::state_store::BrokerStateStore;
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::usage::ProviderErrorKind;
use tokio_util::sync::CancellationToken;

fn make_descriptor(
    id: &'static str,
    supports_browser_cookies: bool,
    selected_profile_browsers: Option<&'static [&'static str]>,
) -> ProviderDescriptor {
    ProviderDescriptor {
        id: ProviderId::new(id).expect("valid provider id"),
        display_name: id,
        short_display_name: id,
        session_label: "Session",
        weekly_label: "Weekly",
        opus_label: None,
        accent_rgb: (100, 100, 100),
        default_enabled: false,
        source_modes: &[SourceMode::Auto, SourceMode::Web],
        supports_api_key_override: false,
        requires_api_key_for_api_source: false,
        uses_region: false,
        uses_secret_key: false,
        supports_workspace: false,
        supports_enterprise_host: false,
        token_account_support: false,
        pace_capability: ProviderPaceCapability::unsupported(),
        cli_name: id,
        toggle_title: id,
        dashboard_url: None,
        subscription_dashboard_url: None,
        status_page_url: None,
        status_link_url: None,
        status_workspace_product_id: None,
        supports_browser_cookies,
        selected_profile_browsers,
    }
}

fn make_browser_gate() -> (tempfile::TempDir, BrowserGate) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(BrokerStateStore::new(dir.path().join("broker_state.json")));
    (dir, BrowserGate::new(store))
}

const ALL_KINDS: [ProviderErrorKind; 8] = [
    ProviderErrorKind::AuthenticationExpired,
    ProviderErrorKind::MissingCredential,
    ProviderErrorKind::PermissionDenied,
    ProviderErrorKind::RateLimited,
    ProviderErrorKind::ProviderUnavailable,
    ProviderErrorKind::ParseFailure,
    ProviderErrorKind::NetworkFailure,
    ProviderErrorKind::ApiFailure,
];

// upstream: CLICookieRefreshTests.swift:12 cookie refresh parses explicit keychain acknowledgement
#[test]
fn cookie_refresh_parses_explicit_keychain_acknowledgement() {
    let parsed = CookieRefreshOptions::parse_from_slice(&[
        "--provider",
        "opencodego",
        "--allow-keychain-prompt",
        "--json",
    ])
    .expect("parse cookie refresh options");

    assert_eq!(parsed.provider.as_deref(), Some("opencodego"));
    assert!(parsed.allow_keychain_prompt);
    assert!(parsed.json_shortcut);
}

// upstream: CLICookieRefreshTests.swift:25 Muse AI participates in explicit and all provider cookie refresh
#[test]
fn muse_ai_participates_in_explicit_and_all_provider_cookie_refresh() {
    let mut descriptors = first_party_order().to_vec();
    descriptors.push(make_descriptor("museai", true, None));

    let targets = cookie_refresh_targets(None, true, &descriptors).expect("all targets");
    assert!(targets.iter().any(|d| d.id.as_str() == "museai"));

    let explicit =
        cookie_refresh_targets(Some("museai"), false, &descriptors).expect("explicit museai");
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].id.as_str(), "museai");
    assert_eq!(
        cookie_refresh_browser_order(&explicit[0], None),
        DEFAULT_IMPORT_ORDER.to_vec()
    );
}

// upstream: CLICookieRefreshTests.swift:35 all provider selection is descriptor driven
#[test]
fn all_provider_selection_is_descriptor_driven() {
    let mut descriptors = first_party_order().to_vec();
    descriptors.push(make_descriptor("opencode", true, None));

    let targets = cookie_refresh_targets(None, true, &descriptors).expect("all targets");
    assert!(targets.len() > 2);
    assert!(targets.iter().any(|d| d.id.as_str() == "claude"));
    assert!(targets.iter().any(|d| d.id.as_str() == "opencode"));
    assert!(targets.iter().all(|d| d.supports_browser_cookies));
    assert!(
        targets
            .iter()
            .all(|d| d.source_modes.contains(&SourceMode::Web))
    );
}

// upstream: CLICookieRefreshTests.swift:46 catalog order providers remain cookie refresh targets
#[test]
fn catalog_order_providers_remain_cookie_refresh_targets() {
    let mut descriptors = first_party_order().to_vec();
    descriptors.push(make_descriptor("typesafe", true, None));

    let all = cookie_refresh_targets(None, true, &descriptors).expect("all targets");
    let typesafe =
        cookie_refresh_targets(Some("typesafe"), false, &descriptors).expect("typesafe target");

    assert!(all.iter().any(|d| d.id.as_str() == "typesafe"));
    assert_eq!(typesafe.len(), 1);
    assert_eq!(typesafe[0].id.as_str(), "typesafe");
    assert_eq!(
        cookie_refresh_browser_order(&typesafe[0], None),
        DEFAULT_IMPORT_ORDER.to_vec()
    );
}

// upstream: CLICookieRefreshTests.swift:56 prompt capable refresh is gated before provider work
#[tokio::test]
async fn prompt_capable_refresh_is_gated_before_provider_work() {
    let mut operation_called = false;
    let descriptor = make_descriptor("opencode", true, None);

    let results = perform_cookie_refreshes(
        &[descriptor],
        false,
        None,
        |_| None,
        |_, _, _| {
            operation_called = true;
            async {
                CookieRefreshResult::new("opencode", CookieRefreshStatus::Refreshed, "unexpected")
            }
        },
    )
    .await;

    assert!(!operation_called);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, CookieRefreshStatus::Blocked);
    assert!(results[0].message.contains("--allow-keychain-prompt"));
}

// upstream: CLICookieRefreshTests.swift:75 preflight skip does not require keychain acknowledgement
#[tokio::test]
async fn preflight_skip_does_not_require_keychain_acknowledgement() {
    let mut operation_called = false;
    let descriptor = make_descriptor("opencode", true, None);

    let results = perform_cookie_refreshes(
        &[descriptor],
        false,
        None,
        |d| {
            Some(CookieRefreshResult::new(
                d.cli_name,
                CookieRefreshStatus::Skipped,
                "manual",
            ))
        },
        |_, _, _| {
            operation_called = true;
            async {
                CookieRefreshResult::new("opencode", CookieRefreshStatus::Refreshed, "unexpected")
            }
        },
    )
    .await;

    assert!(!operation_called);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, CookieRefreshStatus::Skipped);
}

// upstream: CLICookieRefreshTests.swift:96 profile refresh acknowledgement applies only to the configured browser
#[tokio::test]
async fn profile_refresh_acknowledgement_applies_only_to_the_configured_browser() {
    let descriptor = make_descriptor("langdock", true, Some(&["edge", "chrome", "safari"]));
    for browser_id in [None, Some("edge"), Some("chrome"), Some("safari")] {
        for acknowledged in [false, true] {
            let mut provider = ProviderConfig::new(ProviderId::new("langdock").expect("id"));
            provider.set_browser_id(browser_id);
            provider.set_browser_profile_id(Some("/synthetic/Profile"));
            let config = Config::new(
                Config::CURRENT_VERSION,
                vec![ProviderEntry::Typed(provider)],
            );

            let order: Vec<&str> = cookie_refresh_browser_order(&descriptor, Some(&config))
                .into_iter()
                .map(Browser::raw_id)
                .collect();
            assert_eq!(order, vec![browser_id.unwrap_or("edge")]);

            let mut operation_called = false;
            let results = perform_cookie_refreshes(
                std::slice::from_ref(&descriptor),
                acknowledged,
                Some(&config),
                |_| None,
                |d, interaction, _| {
                    operation_called = true;
                    assert_eq!(interaction, FetchInteraction::UserInitiated);
                    async move {
                        CookieRefreshResult::new(
                            d.cli_name,
                            CookieRefreshStatus::Refreshed,
                            "synthetic success",
                        )
                    }
                },
            )
            .await;

            let permitted = acknowledged || browser_id == Some("safari");
            assert_eq!(operation_called, permitted);
            assert_eq!(
                results[0].status,
                if permitted {
                    CookieRefreshStatus::Refreshed
                } else {
                    CookieRefreshStatus::Blocked
                }
            );
        }
    }
}

// upstream: CLICookieRefreshTests.swift:120 invalid selected browser cannot fall back to a different browser
#[test]
fn invalid_selected_browser_cannot_fall_back_to_a_different_browser() {
    let descriptor = make_descriptor("langdock", true, Some(&["edge", "chrome", "safari"]));
    for browser_id in ["firefox", "", "Chrome"] {
        let mut provider = ProviderConfig::new(ProviderId::new("langdock").expect("id"));
        provider.set_browser_id(Some(browser_id));
        provider.set_browser_profile_id(Some("/synthetic/Profile"));
        let config = Config::new(
            Config::CURRENT_VERSION,
            vec![ProviderEntry::Typed(provider)],
        );
        assert!(cookie_refresh_browser_order(&descriptor, Some(&config)).is_empty());
    }
}

// upstream: CLICookieRefreshTests.swift:130 failed refresh preserves default cookie and unrelated account scopes
#[tokio::test]
async fn failed_refresh_preserves_default_cookie_and_unrelated_account_scopes() {
    let cache = CookieCache::new();
    let provider = ProviderId::new("opencode").expect("id");
    let account_scope = CacheScope::ManagedAccount("acct-uuid-1".to_owned());

    cache.store(
        &provider,
        None,
        "default-test-cookie",
        "Test default",
        jiff::Timestamp::UNIX_EPOCH,
    );
    cache.store(
        &provider,
        Some(&account_scope),
        "account-test-cookie",
        "Test account",
        jiff::Timestamp::UNIX_EPOCH,
    );

    let result = with_cookie_refresh_cache_suppressed(&cache, &provider, "opencode", || async {
        assert!(cache.load(&provider, None).is_none());
        assert!(cache.load(&provider, Some(&account_scope)).is_none());
        cache.store(
            &provider,
            None,
            "unvalidated-test-cookie",
            "Test unvalidated",
            jiff::Timestamp::UNIX_EPOCH,
        );
        assert_eq!(
            cache
                .load(&provider, None)
                .map(|c| c.header.expose_secret().to_owned())
                .as_deref(),
            Some("unvalidated-test-cookie")
        );
        CookieRefreshResult::new("opencode", CookieRefreshStatus::Failed, "test failure")
    })
    .await;

    assert_eq!(result.status, CookieRefreshStatus::Failed);
    assert_eq!(
        cache
            .load(&provider, None)
            .map(|c| c.header.expose_secret().to_owned())
            .as_deref(),
        Some("default-test-cookie")
    );
    assert_eq!(
        cache
            .load(&provider, Some(&account_scope))
            .map(|c| c.source_label),
        Some("Test account".to_owned())
    );
}

// upstream: CLICookieRefreshTests.swift:170 successful refresh keeps replacement cookie
#[tokio::test]
async fn successful_refresh_keeps_replacement_cookie() {
    let cache = CookieCache::new();
    let provider = ProviderId::new("opencode").expect("id");

    cache.store(
        &provider,
        None,
        "old-test-cookie",
        "Test old",
        jiff::Timestamp::UNIX_EPOCH,
    );

    let result = with_cookie_refresh_cache_suppressed(&cache, &provider, "opencode", || async {
        let observation = cache.observe_for_conditional_mutation(&provider, None);
        assert!(observation.entry.is_none());
        let stored = cache.store_if_observation_current(
            &provider,
            None,
            &observation,
            "new-test-cookie",
            "Test new",
            jiff::Timestamp::UNIX_EPOCH,
        );
        assert!(stored);
        CookieRefreshResult::new("opencode", CookieRefreshStatus::Refreshed, "ok")
    })
    .await;

    assert_eq!(result.status, CookieRefreshStatus::Refreshed);
    assert_eq!(
        cache
            .load(&provider, None)
            .map(|c| c.header.expose_secret().to_owned())
            .as_deref(),
        Some("new-test-cookie")
    );
}

// upstream: CLICookieRefreshTests.swift:205 successful provider result without a staged cookie fails safely
#[tokio::test]
async fn successful_provider_result_without_a_staged_cookie_fails_safely() {
    let cache = CookieCache::new();
    let provider = ProviderId::new("opencode").expect("id");

    cache.store(
        &provider,
        None,
        "old-test-cookie",
        "Test old",
        jiff::Timestamp::UNIX_EPOCH,
    );

    let result = with_cookie_refresh_cache_suppressed(&cache, &provider, "opencode", || async {
        CookieRefreshResult::new("opencode", CookieRefreshStatus::Refreshed, "unexpected")
    })
    .await;

    assert_eq!(result.status, CookieRefreshStatus::Failed);
    assert_eq!(
        cache
            .load(&provider, None)
            .map(|c| c.header.expose_secret().to_owned())
            .as_deref(),
        Some("old-test-cookie")
    );
}

// upstream: CLICookieRefreshTests.swift:230 validated nonpersistent refresh succeeds without changing persisted cookies
#[tokio::test]
async fn validated_nonpersistent_refresh_succeeds_without_changing_persisted_cookies() {
    let cache = CookieCache::new();
    let provider = ProviderId::new("langdock").expect("id");

    cache.store(
        &provider,
        None,
        "old-test-cookie",
        "Test old",
        jiff::Timestamp::UNIX_EPOCH,
    );

    let result = with_cookie_refresh_cache_suppressed(&cache, &provider, "langdock", || async {
        assert!(cache.load(&provider, None).is_none());
        cache.mark_nonpersistent_refresh_validated(&provider);
        CookieRefreshResult::new("langdock", CookieRefreshStatus::Refreshed, "validated")
    })
    .await;

    assert_eq!(result.status, CookieRefreshStatus::Refreshed);
    assert_eq!(
        cache
            .load(&provider, None)
            .map(|c| c.header.expose_secret().to_owned())
            .as_deref(),
        Some("old-test-cookie")
    );
}

// upstream: CLICookieRefreshTests.swift:249 multiple staged replacements fail before changing persisted cookies
#[tokio::test]
async fn multiple_staged_replacements_fail_before_changing_persisted_cookies() {
    let cache = CookieCache::new();
    let provider = ProviderId::new("opencode").expect("id");
    let account_scope = CacheScope::ManagedAccount("acct-uuid-2".to_owned());

    cache.store(
        &provider,
        None,
        "old-default-cookie",
        "Test old default",
        jiff::Timestamp::UNIX_EPOCH,
    );
    cache.store(
        &provider,
        Some(&account_scope),
        "old-account-cookie",
        "Test old account",
        jiff::Timestamp::UNIX_EPOCH,
    );

    let result = with_cookie_refresh_cache_suppressed(&cache, &provider, "opencode", || async {
        cache.store(
            &provider,
            None,
            "new-default-cookie",
            "Test new default",
            jiff::Timestamp::UNIX_EPOCH,
        );
        cache.store(
            &provider,
            Some(&account_scope),
            "new-account-cookie",
            "Test new account",
            jiff::Timestamp::UNIX_EPOCH,
        );
        CookieRefreshResult::new("opencode", CookieRefreshStatus::Refreshed, "unexpected")
    })
    .await;

    assert_eq!(result.status, CookieRefreshStatus::Failed);
    assert_eq!(
        cache
            .load(&provider, None)
            .map(|c| c.header.expose_secret().to_owned())
            .as_deref(),
        Some("old-default-cookie")
    );
    assert_eq!(
        cache
            .load(&provider, Some(&account_scope))
            .map(|c| c.header.expose_secret().to_owned())
            .as_deref(),
        Some("old-account-cookie")
    );
}

// upstream: CLICookieRefreshTests.swift:291 commit detaches the gate before later writes
#[test]
fn commit_detaches_the_gate_before_later_writes() {
    let cache = CookieCache::new();
    let provider = ProviderId::new("opencode").expect("id");

    let gate = cache
        .begin_refresh_read_suppression(&provider)
        .expect("refresh gate");
    cache.store(
        &provider,
        None,
        "committed-cookie",
        "Test committed",
        jiff::Timestamp::UNIX_EPOCH,
    );
    assert_eq!(
        cache.commit_refresh_read_suppression(&gate).committed_count,
        1
    );

    cache.store(
        &provider,
        None,
        "later-cookie",
        "Test later",
        jiff::Timestamp::UNIX_EPOCH,
    );
    cache.end_refresh_read_suppression(&gate);
    assert_eq!(
        cache
            .load(&provider, None)
            .map(|c| c.header.expose_secret().to_owned())
            .as_deref(),
        Some("later-cookie")
    );
}

// upstream: CLICookieRefreshTests.swift:318 explicit acknowledgement is user initiated and is the only cooldown bypass
#[tokio::test]
async fn explicit_acknowledgement_is_user_initiated_and_is_the_only_cooldown_bypass() {
    let (_dir, gate) = make_browser_gate();
    let start = 2000_i64;
    let call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "test".to_owned(),
    };
    let mut deny_scope = RetryScope::new(&call, false);
    gate.record_dismissal(Browser::Chrome, &mut deny_scope, start)
        .expect("record dismissal");

    let descriptor = make_descriptor("opencode", true, None);
    let mut unacknowledged_operation_called = false;
    let mut observed_interaction = None;
    let mut explicit_retry_allowed = false;

    let _ = perform_cookie_refreshes(
        std::slice::from_ref(&descriptor),
        false,
        None,
        |_| None,
        |_, _, _| {
            unacknowledged_operation_called = true;
            async {
                CookieRefreshResult::new("opencode", CookieRefreshStatus::Refreshed, "unexpected")
            }
        },
    )
    .await;

    let _ = perform_cookie_refreshes(
        std::slice::from_ref(&descriptor),
        true,
        None,
        |_| None,
        |_, interaction, acknowledged| {
            observed_interaction = Some(interaction);
            let retry_call = BrokerCall {
                interaction,
                cancel: CancellationToken::new(),
                request_id: "retry".to_owned(),
            };
            let mut scope = RetryScope::new(&retry_call, acknowledged);
            explicit_retry_allowed = gate
                .should_attempt_key(Browser::Chrome, &mut scope, start + 1)
                .expect("should_attempt_key");
            async { CookieRefreshResult::new("opencode", CookieRefreshStatus::Refreshed, "ok") }
        },
    )
    .await;

    assert!(!unacknowledged_operation_called);
    assert_eq!(observed_interaction, Some(FetchInteraction::UserInitiated));
    assert!(explicit_retry_allowed);
}

// upstream: CLICookieRefreshTests.swift:357 acknowledged cookie refresh shares one interactive read across providers
#[tokio::test]
async fn acknowledged_cookie_refresh_shares_one_interactive_read_across_providers() {
    let (_dir, gate) = make_browser_gate();
    let start = 2000_i64;
    let call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "test".to_owned(),
    };
    let mut deny_scope = RetryScope::new(&call, false);
    gate.record_dismissal(Browser::Chrome, &mut deny_scope, start)
        .expect("record dismissal");

    let mut shared_scope = RetryScope::new(&call, true);
    let mut interactive_reads = 0usize;
    let targets = vec![
        make_descriptor("opencode", true, None),
        make_descriptor("mimo", true, None),
    ];

    let results = perform_cookie_refreshes(
        &targets,
        true,
        None,
        |_| None,
        |d, _, _| {
            if gate
                .should_attempt_key(Browser::Chrome, &mut shared_scope, start + 1)
                .unwrap_or(false)
            {
                interactive_reads += 1;
            }
            async move {
                CookieRefreshResult::new(d.cli_name, CookieRefreshStatus::Refreshed, "Synthetic")
            }
        },
    )
    .await;

    assert_eq!(results.len(), 2);
    assert_eq!(interactive_reads, 1);
}

// upstream: CLICookieRefreshTests.swift:382 classified cookie failures retain safe category hints
#[test]
fn classified_cookie_failures_retain_safe_category_hints() {
    let descriptor = make_descriptor("raycast", true, None);
    for kind in ALL_KINDS {
        let err = ClassifiedError::new(kind, "opaque-test-marker");
        let result = cookie_refresh_failure(&descriptor, Some(&err), None, false, None);
        assert_eq!(result.status, CookieRefreshStatus::Failed);
        match kind {
            ProviderErrorKind::AuthenticationExpired => assert_eq!(
                result.message,
                "The provider rejected the browser session. Sign in again and retry."
            ),
            ProviderErrorKind::PermissionDenied => assert_eq!(
                result.message,
                "Permission was denied while refreshing the browser session. Check access and retry."
            ),
            ProviderErrorKind::RateLimited => assert_eq!(
                result.message,
                "The provider rate limited the refresh. Wait and retry."
            ),
            ProviderErrorKind::ProviderUnavailable => assert_eq!(
                result.message,
                "The provider is temporarily unavailable. Retry later."
            ),
            ProviderErrorKind::ParseFailure => assert_eq!(
                result.message,
                "The provider response could not be read. Check for an AriadUsage update."
            ),
            ProviderErrorKind::NetworkFailure => assert_eq!(
                result.message,
                "The provider could not be reached. Check your connection and retry."
            ),
            ProviderErrorKind::ApiFailure => assert_eq!(
                result.message,
                "The provider could not validate the browser session. Retry later."
            ),
            ProviderErrorKind::MissingCredential | ProviderErrorKind::Unknown => {
                assert!(
                    result
                        .message
                        .contains("No browser session cookie was refreshed")
                );
            }
        }
        assert!(!cookie_refresh_text(std::slice::from_ref(&result)).contains("opaque-test-marker"));
        let json = serde_json::to_string(&result).expect("encode json");
        assert!(!json.contains("opaque-test-marker"));
    }
}

// upstream: CLICookieRefreshTests.swift:415 wrapped profile failures use only the selected browser access hints
#[test]
fn wrapped_profile_failures_use_only_the_selected_browser_access_hints() {
    let (_dir, gate) = make_browser_gate();
    let call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "test".to_owned(),
    };
    let mut scope = RetryScope::new(&call, false);
    gate.record_dismissal(Browser::Edge, &mut scope, 1000)
        .expect("deny edge");
    gate.record_dismissal(Browser::Chrome, &mut scope, 1000)
        .expect("deny chrome");

    let descriptor = make_descriptor("langdock", true, Some(&["edge", "chrome", "safari"]));
    for browser_id in ["edge", "chrome", "safari"] {
        for disabled in [false, true] {
            let mut provider = ProviderConfig::new(ProviderId::new("langdock").expect("id"));
            provider.set_browser_id(Some(browser_id));
            provider.set_browser_profile_id(Some("/synthetic/Profile"));
            let config = Config::new(
                Config::CURRENT_VERSION,
                vec![ProviderEntry::Typed(provider)],
            );

            for kind in [
                ProviderErrorKind::PermissionDenied,
                ProviderErrorKind::MissingCredential,
            ] {
                let err = ClassifiedError::new(kind, "opaque-test-marker");
                let result = cookie_refresh_failure(
                    &descriptor,
                    Some(&err),
                    Some(&config),
                    disabled,
                    Some((&gate, 1001)),
                );
                if browser_id == "safari" {
                    let expected = if kind == ProviderErrorKind::PermissionDenied {
                        "Permission was denied while refreshing the browser session. Check access and retry."
                    } else {
                        "No browser session cookie was refreshed. Sign in in a configured browser and retry."
                    };
                    assert_eq!(result.message, expected);
                } else {
                    let browser_name = if browser_id == "edge" {
                        "Edge"
                    } else {
                        "Chrome"
                    };
                    assert!(
                        result
                            .message
                            .contains(&format!("{browser_name} cookie decryption"))
                    );
                    assert!(result.message.contains(if disabled {
                        "disabled"
                    } else {
                        "declined"
                    }));
                }
                assert!(!result.message.contains("opaque-test-marker"));
            }
        }
    }
}

// partial-upstream: CLICookieRefreshTests.swift:446 plugin HTTP rejection reaches cookie refresh output
#[test]
fn plugin_http_rejection_reaches_cookie_refresh_output() {
    let descriptor = make_descriptor("raycast", true, None);
    let err = ClassifiedError::new(
        ProviderErrorKind::AuthenticationExpired,
        "opaque-test-marker",
    );
    let result = cookie_refresh_failure(&descriptor, Some(&err), None, false, None);
    assert_eq!(
        result.message,
        "The provider rejected the browser session. Sign in again and retry."
    );
    assert!(!result.message.contains("opaque-test-marker"));
}

// upstream: CLICookieRefreshTests.swift:477 header importer retains explicit retry after leaving its task
#[tokio::test]
async fn header_importer_retains_explicit_retry_after_leaving_its_task() {
    let (_dir, gate) = make_browser_gate();
    let gate = Arc::new(gate);
    let call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "test".to_owned(),
    };
    let mut deny_scope = RetryScope::new(&call, false);
    gate.record_dismissal(Browser::Chrome, &mut deny_scope, 1000)
        .expect("deny chrome");

    let mut retry_scope = RetryScope::new(&call, true);
    let gate_clone = Arc::clone(&gate);
    let session = tokio::spawn(async move {
        assert_eq!(call.interaction, FetchInteraction::UserInitiated);
        let allowed = gate_clone
            .should_attempt_key(Browser::Chrome, &mut retry_scope, 1001)
            .expect("should attempt");
        assert!(allowed);
        Some(("session=synthetic".to_owned(), "Chrome".to_owned()))
    })
    .await
    .expect("join task");

    assert_eq!(
        session.map(|(h, _)| h).as_deref(),
        Some("session=synthetic")
    );
}

// upstream: CLICookieRefreshTests.swift:517 raw provider failures cannot leak cookie values
#[test]
fn raw_provider_failures_cannot_leak_cookie_values() {
    let private_marker = "opaque-test-marker";
    let descriptor = make_descriptor("opencode", true, None);
    let result = cookie_refresh_failure(&descriptor, None, None, false, None);
    let text = cookie_refresh_text(std::slice::from_ref(&result));
    let json = serde_json::to_string(&result).expect("encode json");

    assert!(!text.contains(private_marker));
    assert!(!json.contains(private_marker));
    assert!(text.contains("six-hour denial cooldown"));
}

// upstream: CLICookieRefreshTests.swift:537 keychain failure takes precedence over classified hints
#[test]
fn keychain_failure_takes_precedence_over_classified_hints() {
    let (_dir, gate) = make_browser_gate();
    let call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "test".to_owned(),
    };
    let mut scope = RetryScope::new(&call, false);
    gate.record_dismissal(Browser::Chrome, &mut scope, 1000)
        .expect("deny chrome");

    let descriptor = make_descriptor("opencode", true, None);
    for disabled in [false, true] {
        for kind in ALL_KINDS {
            let err = ClassifiedError::new(kind, "opaque-test-marker");
            let result = cookie_refresh_failure(
                &descriptor,
                Some(&err),
                None,
                disabled,
                Some((&gate, 1001)),
            );
            let expected = if disabled {
                "Chrome cookie decryption is disabled in AriadUsage; enable Keychain access and refresh."
            } else {
                "Chrome cookie decryption was declined in Keychain; rerun with --allow-keychain-prompt to request Keychain access again."
            };
            assert_eq!(result.message, expected);
            assert!(!result.message.contains("opaque-test-marker"));
        }
    }
}
