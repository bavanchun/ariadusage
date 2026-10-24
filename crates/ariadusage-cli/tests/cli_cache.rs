// Ported from CodexBar Tests/CodexBarTests/CLICacheTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_cli::{
    CacheClearOptions, OutputFormat, cache_clear_provider_scope_error, cache_help,
    execute_cache_clear,
};
use ariadusage_engine::brokers::cookie_cache::{CacheScope, CookieCache};
use ariadusage_protocol::ProviderId;
use jiff::Timestamp;

// upstream: CLICacheTests.swift:6 cache clear parses cookies provider flags
#[test]
fn cache_clear_parses_cookies_provider_flags() {
    let parsed =
        CacheClearOptions::parse_from_slice(&["--cookies", "--provider", "claude", "--json"])
            .expect("parse cache clear flags");

    assert!(parsed.cookies);
    assert!(parsed.json_shortcut);
    assert_eq!(parsed.provider.as_deref(), Some("claude"));
    assert_eq!(parsed.effective_format(), OutputFormat::Json);
}

// upstream: CLICacheTests.swift:17 provider scope is rejected for cost clearing
#[test]
fn provider_scope_is_rejected_for_cost_clearing() {
    assert_eq!(cache_clear_provider_scope_error(None, true), None);
    assert_eq!(
        cache_clear_provider_scope_error(Some("claude"), false),
        None
    );
    assert!(
        cache_clear_provider_scope_error(Some("claude"), true)
            .is_some_and(|err| err.contains("--provider only scopes cookie caches"))
    );
}

// upstream: CLICacheTests.swift:25 cache help documents provider as cookie scoped
#[test]
fn cache_help_documents_provider_as_cookie_scoped() {
    let help = cache_help("0.0.0");

    assert!(help.contains("--provider with --cookies"));
    assert!(help.contains("codexbar cache clear --cookies --provider claude"));
}

#[test]
fn cache_clear_removes_cookie_entries_and_cost_cache_files() {
    let cache = CookieCache::new();
    let claude = ProviderId::new("claude").expect("claude id");
    let account_scope = CacheScope::ManagedAccount("acct-1".to_owned());

    cache.store(
        &claude,
        None,
        "sessionKey=test",
        "Chrome",
        Timestamp::UNIX_EPOCH,
    );
    cache.store(
        &claude,
        Some(&account_scope),
        "sessionKey=test-acct",
        "Chrome",
        Timestamp::UNIX_EPOCH,
    );

    let summary = cache.clear_all_scopes_detailed(&claude);
    assert_eq!(summary.cleared_count, 2);
    assert_eq!(summary.failed_count, 0);
    assert!(cache.load(&claude, None).is_none());
    assert!(cache.load(&claude, Some(&account_scope)).is_none());

    let opts =
        CacheClearOptions::parse_from_slice(&["--cookies", "--provider", "claude", "--json"])
            .expect("parse options");
    let (results, rendered) = execute_cache_clear(&opts, None).expect("execute cache clear");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].cache, "cookies");
    assert_eq!(results[0].provider.as_deref(), Some("claude"));
    assert!(rendered.contains("\"cache\":\"cookies\""));
}
