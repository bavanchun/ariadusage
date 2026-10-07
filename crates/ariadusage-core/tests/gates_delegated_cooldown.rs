use jiff::Timestamp;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use ariadusage_core::gates::delegated_cooldown::{
    DelegatedRefreshCooldown, DelegatedRefreshOutcome,
};
use ariadusage_core::pipeline::FetchInteraction;

fn make_test_clock(start_epoch_secs: i64) -> (Arc<AtomicI64>, impl Fn() -> Timestamp + Clone) {
    let current = Arc::new(AtomicI64::new(start_epoch_secs));
    let current_clone = Arc::clone(&current);
    let clock = move || {
        let secs = current_clone.load(Ordering::SeqCst);
        Timestamp::from_second(secs).expect("valid test timestamp")
    };
    (current, clock)
}

#[test]
fn test_reservation_sets_short_cooldown() {
    let (time_secs, clock) = make_test_clock(1_700_000_000);
    let gate = DelegatedRefreshCooldown::new(clock);
    let profile = "profile-digest-1";

    assert!(gate.check(profile, FetchInteraction::Background).is_ok());

    gate.reserve(profile, FetchInteraction::Background)
        .expect("reserve succeeds");

    let entry = gate.get_entry(profile).expect("entry recorded");
    assert_eq!(entry.interval(), Duration::from_secs(20));

    // At 19 seconds: background check is suppressed
    time_secs.fetch_add(19, Ordering::SeqCst);
    assert!(gate.check(profile, FetchInteraction::Background).is_err());
    // User-initiated check bypasses
    assert!(gate.check(profile, FetchInteraction::UserInitiated).is_ok());

    // At 21 seconds: cooldown expired
    time_secs.fetch_add(2, Ordering::SeqCst);
    assert!(gate.check(profile, FetchInteraction::Background).is_ok());
}

#[test]
fn test_finalize_durations() {
    let (time_secs, clock) = make_test_clock(1_700_000_000);
    let gate = DelegatedRefreshCooldown::new(clock);

    // 1. ObservedSuccess -> 5 min (300 s)
    let p_success = "profile-success";
    gate.finalize(p_success, DelegatedRefreshOutcome::ObservedSuccess);
    assert_eq!(
        gate.get_entry(p_success).unwrap().interval(),
        Duration::from_secs(300)
    );

    // 2. UnreadableResult -> 5 min (300 s)
    let p_unreadable = "profile-unreadable";
    gate.finalize(p_unreadable, DelegatedRefreshOutcome::UnreadableResult);
    assert_eq!(
        gate.get_entry(p_unreadable).unwrap().interval(),
        Duration::from_secs(300)
    );

    // 3. FailedOrUnchanged -> 20 s
    let p_failed = "profile-failed";
    gate.finalize(p_failed, DelegatedRefreshOutcome::FailedOrUnchanged);
    assert_eq!(
        gate.get_entry(p_failed).unwrap().interval(),
        Duration::from_secs(20)
    );

    // Advance 25 seconds: p_failed expired, but p_success and p_unreadable remain active
    time_secs.fetch_add(25, Ordering::SeqCst);
    assert!(gate.check(p_failed, FetchInteraction::Background).is_ok());
    assert!(gate.check(p_success, FetchInteraction::Background).is_err());
    assert!(
        gate.check(p_unreadable, FetchInteraction::Background)
            .is_err()
    );

    // Advance to 301 seconds total: all expired
    time_secs.fetch_add(280, Ordering::SeqCst);
    assert!(gate.check(p_success, FetchInteraction::Background).is_ok());
    assert!(
        gate.check(p_unreadable, FetchInteraction::Background)
            .is_ok()
    );
}

#[test]
fn test_user_initiated_bypass_and_reservation() {
    let (time_secs, clock) = make_test_clock(1_700_000_000);
    let gate = DelegatedRefreshCooldown::new(clock);
    let profile = "profile-bypass";

    // Set 5 min cooldown
    gate.finalize(profile, DelegatedRefreshOutcome::ObservedSuccess);
    assert!(gate.check(profile, FetchInteraction::Background).is_err());

    // User initiated check bypasses
    assert!(gate.check(profile, FetchInteraction::UserInitiated).is_ok());

    // User initiated reserve bypasses and resets to short cooldown (20 s)
    time_secs.fetch_add(10, Ordering::SeqCst);
    assert!(
        gate.reserve(profile, FetchInteraction::UserInitiated)
            .is_ok()
    );
    assert_eq!(
        gate.get_entry(profile).unwrap().interval(),
        Duration::from_secs(20)
    );
}

#[test]
fn test_serialization_round_trip() {
    let (_time_secs, clock) = make_test_clock(1_700_000_000);
    let gate = DelegatedRefreshCooldown::new(clock.clone());

    gate.finalize("prof-1", DelegatedRefreshOutcome::ObservedSuccess);
    gate.finalize("prof-2", DelegatedRefreshOutcome::FailedOrUnchanged);

    let serialized = serde_json::to_string(&gate).expect("serialize to json");
    let deserialized: DelegatedRefreshCooldown =
        serde_json::from_str(&serialized).expect("deserialize from json");

    let restored = deserialized.with_clock(clock);
    let entries = restored.all_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries["prof-1"].interval_secs, 300);
    assert_eq!(entries["prof-2"].interval_secs, 20);
}

#[test]
fn test_debug_never_leaks_tokens_or_profile_keys() {
    let invented_token = ["secret", "bearer", "token", "xyz987"].join("_");
    let invented_profile = format!("profile-{invented_token}");

    let gate = DelegatedRefreshCooldown::default();
    gate.finalize(&invented_profile, DelegatedRefreshOutcome::ObservedSuccess);

    let debug_str = format!("{gate:?}");
    assert!(!debug_str.contains(&invented_token));
    assert!(!debug_str.contains(&invented_profile));
    assert!(debug_str.contains("entries_count: 1"));
}
