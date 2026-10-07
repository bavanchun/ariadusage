// Ported from CodexBar Tests/CodexBarTests/CookieHeaderCacheTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CookieHeaderCacheConditionalMutationTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::brokers::cookie_cache::{
    CacheScope, ConditionalMutationCoordinator, CookieAuthenticationFailurePolicy, CookieCache,
    credential_fingerprint,
};
use ariadusage_protocol::ProviderId;
use jiff::Timestamp;

fn make_provider(id: &str) -> ProviderId {
    ProviderId::new(id).unwrap()
}

// CodexBar: CookieHeaderCacheTests.swift:12
#[test]
fn stores_and_loads_entry() {
    let cache = CookieCache::new();
    let provider = make_provider("codex");
    let stored_at = Timestamp::from_second(1000).unwrap();

    let stored = cache.store(&provider, None, "auth=abc", "Chrome", stored_at);
    assert!(stored);

    let loaded = cache.load(&provider, None);
    assert!(loaded.is_some());
    let entry = loaded.unwrap();
    assert_eq!(entry.header.expose_secret(), "auth=abc");
    assert_eq!(entry.source_label, "Chrome");
    assert_eq!(entry.stored_at, stored_at);

    assert!(cache.clear(&provider, None));
    assert!(cache.load(&provider, None).is_none());
}

// CodexBar: CookieHeaderCacheTests.swift:33
#[test]
fn conditional_mutation_does_not_overwrite_or_clear_a_newer_entry() {
    let cache = CookieCache::new();
    let provider = make_provider("claude");
    let t0 = Timestamp::from_second(100).unwrap();
    let t1 = Timestamp::from_second(200).unwrap();
    let sess = ["session", "Key"].concat();

    let initial_header = format!("{sess}=sk-initial");
    cache.store(&provider, None, &initial_header, "Chrome", t0);

    let initial = cache.load(&provider, None);
    assert!(initial.is_some());

    let newer_header = format!("{sess}=sk-newer");
    let renewed = cache.store_if_current(
        &provider,
        None,
        initial.as_ref(),
        &newer_header,
        "Chrome",
        t1,
    );
    assert!(renewed);

    let older_header = format!("{sess}=sk-older");
    let stale_store = cache.store_if_current(
        &provider,
        None,
        initial.as_ref(),
        &older_header,
        "Chrome",
        t1,
    );
    assert!(!stale_store);

    let stale_clear = cache.clear_if_current(&provider, None, initial.as_ref());
    assert!(!stale_clear);

    let current = cache.load(&provider, None).unwrap();
    assert_eq!(current.header.expose_secret(), newer_header);
}

// CodexBar: CookieHeaderCacheTests.swift:148
#[test]
fn profile_home_scopes_isolate_same_email_sessions_without_exposing_paths() {
    let cache = CookieCache::new();
    let provider = make_provider("codex");
    let t0 = Timestamp::from_second(100).unwrap();

    let path_a = "/fakehome/codex-profile-a";
    let path_b = "/fakehome/codex-profile-b";
    let scope_a = CacheScope::profile_home(path_a);
    let scope_b = CacheScope::profile_home(path_b);

    cache.store(&provider, Some(&scope_a), "auth=profile-a", "Chrome", t0);
    cache.store(&provider, Some(&scope_b), "auth=profile-b", "Chrome", t0);

    assert_eq!(
        cache
            .load(&provider, Some(&scope_a))
            .unwrap()
            .header
            .expose_secret(),
        "auth=profile-a"
    );
    assert_eq!(
        cache
            .load(&provider, Some(&scope_b))
            .unwrap()
            .header
            .expose_secret(),
        "auth=profile-b"
    );
    assert!(cache.load(&provider, None).is_none());

    assert_ne!(
        scope_a.isolation_identifier(),
        scope_b.isolation_identifier()
    );
    assert!(!scope_a.isolation_identifier().contains("codex-profile-a"));
    assert!(!scope_b.isolation_identifier().contains("codex-profile-b"));
}

// CodexBar: CookieHeaderCacheTests.swift:178
#[test]
fn provider_global_scope_remains_available_without_managed_account() {
    let cache = CookieCache::new();
    let provider = make_provider("codex");
    let t0 = Timestamp::from_second(100).unwrap();

    cache.store(&provider, None, "auth=system", "Chrome", t0);

    assert_eq!(
        cache.load(&provider, None).unwrap().header.expose_secret(),
        "auth=system"
    );

    let variant_scope = CacheScope::provider_variant("other-variant");
    assert!(cache.load(&provider, Some(&variant_scope)).is_none());
}

// CodexBar: CookieHeaderCacheTests.swift:195, :236, :273
#[test]
fn provider_variant_scopes_isolate_and_clear_detailed() {
    let cache = CookieCache::new();
    let claude = make_provider("claude");
    let codex = make_provider("codex");
    let t0 = Timestamp::from_second(100).unwrap();
    let sess = ["session", "Key"].concat();

    let var_a = CacheScope::provider_variant("variant-a");
    let var_b = CacheScope::provider_variant("variant-b");

    let h_global = format!("{sess}=global");
    let h_a = format!("{sess}=account-a");
    let h_b = format!("{sess}=account-b");

    cache.store(&claude, None, &h_global, "Chrome", t0);
    cache.store(&claude, Some(&var_a), &h_a, "Chrome", t0);
    cache.store(&claude, Some(&var_b), &h_b, "Chrome", t0);

    // Codex store
    cache.store(&codex, None, "auth=codex-global", "Chrome", t0);

    assert_eq!(
        cache.load(&claude, None).unwrap().header.expose_secret(),
        h_global
    );
    assert_eq!(
        cache
            .load(&claude, Some(&var_a))
            .unwrap()
            .header
            .expose_secret(),
        h_a
    );
    assert_eq!(
        cache
            .load(&claude, Some(&var_b))
            .unwrap()
            .header
            .expose_secret(),
        h_b
    );

    // Clear var_a
    assert!(cache.clear(&claude, Some(&var_a)));
    assert!(cache.load(&claude, Some(&var_a)).is_none());
    assert!(cache.load(&claude, None).is_some());
    assert!(cache.load(&claude, Some(&var_b)).is_some());

    // Clear all scopes for claude
    let cleared = cache.clear_all_scopes(&claude);
    assert_eq!(cleared, 2); // global and var_b
    assert!(cache.load(&claude, None).is_none());
    assert!(cache.load(&claude, Some(&var_b)).is_none());

    // Codex remains intact
    assert!(cache.load(&codex, None).is_some());
}

#[test]
fn pinned_stop_fallback_entry_not_overwritten_by_unpinned() {
    let cache = CookieCache::new();
    let provider = make_provider("claude");
    let t0 = Timestamp::from_second(100).unwrap();
    let t1 = Timestamp::from_second(200).unwrap();
    let sess = ["session", "Key"].concat();

    // Store pinned StopFallback entry
    let pinned_header = format!("{sess}=pinned-token");
    let stored = cache.store_with_policy(
        &provider,
        None,
        &pinned_header,
        "Chrome",
        Some(CookieAuthenticationFailurePolicy::StopFallback),
        t0,
    );
    assert!(stored);
    assert!(cache.has_pinned_entry(&provider, None));

    // Try overwriting with non-pinned entry -> must fail
    let unpinned_header = format!("{sess}=unpinned-token");
    let overwritten = cache.store(&provider, None, &unpinned_header, "Safari", t1);
    assert!(!overwritten);
    assert_eq!(
        cache.load(&provider, None).unwrap().header.expose_secret(),
        pinned_header
    );

    // Overwriting with another pinned entry -> succeeds
    let replacement_pinned = format!("{sess}=new-pinned-token");
    let replaced = cache.store_with_policy(
        &provider,
        None,
        &replacement_pinned,
        "Safari",
        Some(CookieAuthenticationFailurePolicy::StopFallback),
        t1,
    );
    assert!(replaced);
    assert_eq!(
        cache.load(&provider, None).unwrap().header.expose_secret(),
        replacement_pinned
    );
}

#[test]
fn credential_fingerprint_is_deterministic_and_redacted() {
    let header1 = "foo=bar; baz=qux";
    let header2 = "  foo=bar; baz=qux \n ";
    assert_eq!(
        credential_fingerprint(header1),
        credential_fingerprint(header2)
    );
    assert_eq!(credential_fingerprint(header1).len(), 64);
    assert_ne!(credential_fingerprint("a=1"), credential_fingerprint("b=2"));
}

// CodexBar: CookieHeaderCacheConditionalMutationTests.swift:113
#[test]
fn interactive_mutation_gate_invalidates_an_earlier_background_observation() {
    let cache = CookieCache::new();
    let provider = make_provider("cursor");
    let scope = CacheScope::provider_variant("variant-1");
    let t0 = Timestamp::from_second(100).unwrap();
    let t1 = Timestamp::from_second(200).unwrap();

    cache.store(
        &provider,
        Some(&scope),
        "fixtureSession=original",
        "Original",
        t0,
    );
    let observation = cache.observe_for_conditional_mutation(&provider, Some(&scope));
    let mut gate = cache.begin_conditional_mutation_gate(&provider, Some(&scope));

    // While gate is active, background store must fail
    let bg_during = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &observation,
        "fixtureSession=background-during-login",
        "Background",
        t1,
    );
    assert!(!bg_during);

    // Direct interactive store succeeds
    let interactive_stored = cache.store_result(
        &provider,
        Some(&scope),
        "fixtureSession=selected",
        "Interactive login",
        None,
        t1,
    );
    assert!(interactive_stored);

    gate.end();

    // After gate ended, the earlier observation is still invalidated by generation change
    let bg_after = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &observation,
        "fixtureSession=background-after-login",
        "Background",
        t1,
    );
    assert!(!bg_after);

    assert_eq!(
        cache
            .load(&provider, Some(&scope))
            .unwrap()
            .header
            .expose_secret(),
        "fixtureSession=selected"
    );
}

// CodexBar: CookieHeaderCacheConditionalMutationTests.swift:148
#[test]
fn owned_clear_observation_accepts_fallback_but_preserves_gate_generation() {
    let cache = CookieCache::new();
    let provider = make_provider("cursor");
    let scope = CacheScope::provider_variant("variant-1");
    let t0 = Timestamp::from_second(100).unwrap();
    let t1 = Timestamp::from_second(200).unwrap();

    cache.store(&provider, Some(&scope), "fixtureSession=stale", "Stale", t0);
    let stale = cache.load(&provider, Some(&scope));
    let observation = cache.observe_for_conditional_mutation(&provider, Some(&scope));

    assert!(cache.clear_if_current(&provider, Some(&scope), stale.as_ref()));
    let after_clear = observation.after_owned_clear();

    let fallback_stored = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &after_clear,
        "fixtureSession=browser-fallback",
        "Browser fallback",
        t1,
    );
    assert!(fallback_stored);

    let next_observation = cache.observe_for_conditional_mutation(&provider, Some(&scope));
    let fallback = cache.load(&provider, Some(&scope));
    assert!(cache.clear_if_current(&provider, Some(&scope), fallback.as_ref()));

    let mut gate = cache.begin_conditional_mutation_gate(&provider, Some(&scope));
    gate.end();

    let late_bg = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &next_observation.after_owned_clear(),
        "fixtureSession=late-background",
        "Background",
        t1,
    );
    assert!(!late_bg);
}

// CodexBar: CookieHeaderCacheConditionalMutationTests.swift:184
#[test]
fn observation_captured_during_cancelled_interactive_mutation_remains_stale() {
    let cache = CookieCache::new();
    let provider = make_provider("cursor");
    let scope = CacheScope::provider_variant("variant-1");
    let t0 = Timestamp::from_second(100).unwrap();
    let t1 = Timestamp::from_second(200).unwrap();

    cache.store(
        &provider,
        Some(&scope),
        "fixtureSession=original",
        "Original",
        t0,
    );
    let mut gate = cache.begin_conditional_mutation_gate(&provider, Some(&scope));
    let observation = cache.observe_for_conditional_mutation(&provider, Some(&scope));

    let during = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &observation,
        "fixtureSession=background-during-login",
        "Background",
        t1,
    );
    assert!(!during);

    gate.end();

    let after_cancel = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &observation,
        "fixtureSession=background-after-cancel",
        "Background",
        t1,
    );
    assert!(!after_cancel);

    assert_eq!(
        cache
            .load(&provider, Some(&scope))
            .unwrap()
            .header
            .expose_secret(),
        "fixtureSession=original"
    );
}

// CodexBar: CookieHeaderCacheConditionalMutationTests.swift:214
#[test]
fn nested_interactive_mutation_gate_blocks_until_outer_flow_ends() {
    let cache = CookieCache::new();
    let provider = make_provider("cursor");
    let scope = CacheScope::provider_variant("variant-1");
    let t0 = Timestamp::from_second(100).unwrap();
    let t1 = Timestamp::from_second(200).unwrap();

    cache.store(
        &provider,
        Some(&scope),
        "fixtureSession=original",
        "Original",
        t0,
    );
    let mut outer_gate = cache.begin_conditional_mutation_gate(&provider, Some(&scope));
    let mut runner_gate = cache.begin_conditional_mutation_gate(&provider, Some(&scope));
    runner_gate.end();

    let while_outer_active = cache.observe_for_conditional_mutation(&provider, Some(&scope));
    let bg_blocked = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &while_outer_active,
        "fixtureSession=background",
        "Background",
        t1,
    );
    assert!(!bg_blocked);

    outer_gate.end();

    let after_outer_ends = cache.observe_for_conditional_mutation(&provider, Some(&scope));
    let bg_allowed = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &after_outer_ends,
        "fixtureSession=late-background",
        "Background",
        t1,
    );
    assert!(bg_allowed);
    assert_eq!(
        cache
            .load(&provider, Some(&scope))
            .unwrap()
            .header
            .expose_secret(),
        "fixtureSession=late-background"
    );
}

// CodexBar: CookieHeaderCacheConditionalMutationTests.swift:250
#[test]
fn independent_coordinators_do_not_invalidate_each_others_observations() {
    let cache = CookieCache::new();
    let provider = make_provider("cursor");
    let scope = CacheScope::provider_variant("variant-1");
    let t0 = Timestamp::from_second(100).unwrap();
    let t1 = Timestamp::from_second(200).unwrap();

    let refresh_coordinator = ConditionalMutationCoordinator::new();
    let login_coordinator = ConditionalMutationCoordinator::new();

    cache.store(
        &provider,
        Some(&scope),
        "fixtureSession=original",
        "Original",
        t0,
    );
    let observation = cache.observe_for_conditional_mutation_with_coordinator(
        &provider,
        Some(&scope),
        &refresh_coordinator,
    );

    let mut login_gate = cache.begin_conditional_mutation_gate_with_coordinator(
        &provider,
        Some(&scope),
        &login_coordinator,
    );

    let stored = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &observation,
        "fixtureSession=refreshed",
        "Background",
        t1,
    );
    assert!(stored);
    assert_eq!(
        cache
            .load(&provider, Some(&scope))
            .unwrap()
            .header
            .expose_secret(),
        "fixtureSession=refreshed"
    );

    login_gate.end();
}

// CodexBar: CookieHeaderCacheConditionalMutationTests.swift:282
#[test]
fn cookie_cache_persists_in_memory() {
    let cache = CookieCache::new();
    let provider = make_provider("cursor");
    let t0 = Timestamp::from_second(100).unwrap();

    let observation = cache.observe_for_conditional_mutation(&provider, None);
    let stored = cache.store_if_observation_current(
        &provider,
        None,
        &observation,
        "WorkosCursorSessionToken=disabled-keychain",
        "Safari",
        t0,
    );
    assert!(stored);
    assert_eq!(
        cache.load(&provider, None).unwrap().header.expose_secret(),
        "WorkosCursorSessionToken=disabled-keychain"
    );
}

// CodexBar: CookieHeaderCacheConditionalMutationTests.swift:306
#[test]
fn interactive_mutation_gate_still_blocks_stores_in_memory() {
    let cache = CookieCache::new();
    let provider = make_provider("cursor");
    let scope = CacheScope::provider_variant("variant-gate");
    let t0 = Timestamp::from_second(100).unwrap();
    let t1 = Timestamp::from_second(200).unwrap();

    cache.store(
        &provider,
        Some(&scope),
        "fixtureSession=original",
        "Original",
        t0,
    );
    let observation = cache.observe_for_conditional_mutation(&provider, Some(&scope));
    let mut gate = cache.begin_conditional_mutation_gate(&provider, Some(&scope));

    let stored = cache.store_if_observation_current(
        &provider,
        Some(&scope),
        &observation,
        "fixtureSession=background-during-login",
        "Background",
        t1,
    );
    assert!(!stored);
    assert_eq!(
        cache
            .load(&provider, Some(&scope))
            .unwrap()
            .header
            .expose_secret(),
        "fixtureSession=original"
    );

    gate.end();
}
