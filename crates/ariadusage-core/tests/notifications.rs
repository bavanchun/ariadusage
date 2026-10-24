// Ported from CodexBar Tests/CodexBarTests/CodexSessionQuotaFalseRestoreTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexLimitResetOwnerKeyTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/LimitResetNotificationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeExtraWindowQuotaWarningTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/UsageStoreAccountQuotaWarningTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCredentialQuotaWarningTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeSyntheticPlaceholderNotificationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CredentialNotificationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/UsageStoreSessionQuotaTransitionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{HashMap, HashSet};

use ariadusage_core::{
    ClaudeOAuthActiveAccountObservation, CodexAccountScopedRefreshGuard, CodexActiveSource,
    CodexIdentity, CodexLimitResetOwnerKey, CodexSessionQuotaOwnerKey, CredentialFetchOutcome,
    CredentialNotificationAction, CredentialNotificationKey, CredentialNotificationTracker,
    LimitResetDetectorState, LimitResetNotificationLogic, LimitResetNotificationReceipt,
    LimitResetObservation, NamedRateWindow, NotificationPolicyEngine, NotificationPreferences,
    ProviderCostSnapshot, ProviderCredentialError, ProviderCredentialFailure,
    ProviderFetchErrorKind, ProviderIdentitySnapshot, QuotaWarningState, QuotaWarningStateKey,
    QuotaWarningWindow, RateWindow, SessionQuotaTransition, SessionQuotaTransitionObservation,
    SessionQuotaTransitionOutcome, SessionQuotaTransitionReducer, SessionQuotaTransitionState,
    SessionQuotaWindowSource, UsageSnapshot, deliver_if_current, prepare_limit_reset_notification,
};
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;

const START_SECS: i64 = 1_700_000_000;

fn pid(id: &str) -> ProviderId {
    ProviderId::new(id).unwrap()
}

fn ts(secs: i64) -> Timestamp {
    Timestamp::from_second(secs).unwrap()
}

fn rate_window(
    used_percent: f64,
    window_minutes: Option<i64>,
    resets_at: Option<Timestamp>,
) -> RateWindow {
    RateWindow::new(used_percent, window_minutes, resets_at, None, None, false).unwrap()
}

fn placeholder_window(
    used_percent: f64,
    window_minutes: Option<i64>,
    resets_at: Option<Timestamp>,
) -> RateWindow {
    RateWindow::new(used_percent, window_minutes, resets_at, None, None, true).unwrap()
}

fn make_snapshot(
    provider: &str,
    used: f64,
    reset_boundary: Option<Timestamp>,
    updated_at: Timestamp,
    email: &str,
) -> UsageSnapshot {
    let provider_id = pid(provider);
    UsageSnapshot::new(
        Some(rate_window(used, Some(300), reset_boundary)),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        updated_at,
        Some(ProviderIdentitySnapshot {
            provider_id: Some(provider_id),
            account_email: Some(email.to_owned()),
            account_organization: None,
            login_method: Some("test".to_owned()),
            account_id: None,
        }),
        Confidence::Exact,
    )
    .unwrap()
}

fn codex_owner(suffix: &str) -> CodexSessionQuotaOwnerKey {
    CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
        CodexActiveSource::LiveSystem,
        CodexIdentity::ProviderAccount {
            id: format!("workspace-fixture-{suffix}"),
        },
        Some("session-fixture@example.test".to_owned()),
        None,
    ))
    .unwrap()
}

fn observe_codex(
    engine: &mut NotificationPolicyEngine,
    transitions: &mut Vec<SessionQuotaTransition>,
    used: f64,
    boundary: Option<Timestamp>,
    at: Timestamp,
    evaluated_at: Option<Timestamp>,
    owner: Option<&CodexSessionQuotaOwnerKey>,
) {
    let codex = pid("codex");
    let snap = make_snapshot("codex", used, boundary, at, "session-fixture@example.test");
    let (post, _, transition) = engine.handle_session_quota_transition(
        &codex,
        &snap,
        None,
        owner,
        false,
        evaluated_at.unwrap_or(at),
    );
    if post.is_some() {
        transitions.push(transition);
    }
}

// =========================================================================
// CodexSessionQuotaFalseRestoreTests
// =========================================================================

// upstream: CodexSessionQuotaFalseRestoreTests.swift:11 same future boundary suppresses restore and duplicate depletion
#[test]
fn codex_session_same_future_boundary_suppresses_restore_and_duplicate_depletion() {
    let owner = codex_owner("same-boundary");
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        0.0,
        boundary,
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 180),
        None,
        Some(&owner),
    );

    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(engine.session_quota_states[&pid("codex")].remaining, 0.0);
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:27 advanced boundary before trusted expiry stays suppressed
#[test]
fn codex_session_advanced_boundary_before_trusted_expiry_stays_suppressed() {
    let owner = codex_owner("early-advanced");
    let boundary = ts(START_SECS + 5 * 3600);
    let advanced = ts(START_SECS + 10 * 3600);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(boundary),
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(boundary),
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        0.0,
        Some(advanced),
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        Some(advanced),
        ts(START_SECS + 180),
        None,
        Some(&owner),
    );

    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(boundary)
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:44 depleted boundary stays frozen until its reset can be proven
#[test]
fn codex_session_depleted_boundary_stays_frozen_until_its_reset_can_be_proven() {
    let owner = codex_owner("depleted-advanced");
    let boundary = ts(START_SECS + 5 * 3600);
    let advanced = ts(START_SECS + 10 * 3600);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(boundary),
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(boundary),
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(advanced),
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(boundary)
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(advanced),
        ts(START_SECS + 5 * 3600 + 60),
        None,
        Some(&owner),
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(boundary)
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        0.0,
        Some(advanced),
        ts(START_SECS + 5 * 3600 + 120),
        None,
        Some(&owner),
    );

    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(advanced)
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:64 depleted observation recovers a missing trusted boundary
#[test]
fn codex_session_depleted_observation_recovers_missing_trusted_boundary() {
    let owner = codex_owner("depleted-recovered-boundary");
    let boundary = ts(START_SECS + 5 * 3600);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(boundary),
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(boundary)
    );

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(boundary),
        ts(START_SECS + 180),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        Some(boundary),
        ts(START_SECS + 240),
        None,
        Some(&owner),
    );

    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(
        engine.session_quota_states[&pid("codex")].pending_codex_restore_observation_at,
        None
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:83 expired baseline boundary cannot produce a single sample restore
#[test]
fn codex_session_expired_baseline_boundary_cannot_produce_single_sample_restore() {
    let owner = codex_owner("expired-baseline");
    let expired = ts(START_SECS - 60);
    let future = ts(START_SECS + 5 * 3600);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(expired),
        ts(START_SECS),
        None,
        Some(&owner),
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        None
    );

    let first_positive = ts(START_SECS + 60);
    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(future),
        first_positive,
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(
        engine.session_quota_states[&pid("codex")].pending_codex_restore_observation_at,
        Some(first_positive)
    );

    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        Some(future),
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(future)
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:106 boundary expired before evaluation is not trusted
#[test]
fn codex_session_boundary_expired_before_evaluation_is_not_trusted() {
    let owner = codex_owner("expired-before-evaluation");
    let observed_at = ts(START_SECS + 60);
    let boundary = ts(START_SECS + 120);
    let evaluated_at = ts(START_SECS + 180);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(boundary),
        observed_at,
        Some(evaluated_at),
        Some(&owner),
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        None
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:125 depletion cannot advance a still future trusted boundary
#[test]
fn codex_session_depletion_cannot_advance_still_future_trusted_boundary() {
    let owner = codex_owner("depletion-advanced");
    let boundary = ts(START_SECS + 5 * 3600);
    let advanced = ts(START_SECS + 10 * 3600);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(boundary),
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(advanced),
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(boundary)
    );

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(advanced),
        ts(START_SECS + 5 * 3600 + 60),
        None,
        Some(&owner),
    );
    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(advanced)
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:143 pre boundary observation cannot advance metadata when processed later
#[test]
fn codex_session_pre_boundary_observation_cannot_advance_metadata_when_processed_later() {
    let owner = codex_owner("delayed-observation");
    let boundary = ts(START_SECS + 5 * 3600);
    let advanced = ts(START_SECS + 10 * 3600);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(boundary),
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        30.0,
        Some(advanced),
        ts(START_SECS + 60),
        Some(ts(START_SECS + 5 * 3600 + 60)),
        Some(&owner),
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(boundary)
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:162 ambiguous post expiry restore requires two fresh observations
#[test]
fn codex_session_ambiguous_post_expiry_restore_requires_two_fresh_observations() {
    for boundary_present in [false, true] {
        let owner = codex_owner(if boundary_present {
            "expired-equivalent"
        } else {
            "expired-missing"
        });
        let boundary = ts(START_SECS + 5 * 3600);
        let mut engine = NotificationPolicyEngine::default();
        let mut transitions = Vec::new();

        observe_codex(
            &mut engine,
            &mut transitions,
            20.0,
            Some(boundary),
            ts(START_SECS),
            None,
            Some(&owner),
        );
        observe_codex(
            &mut engine,
            &mut transitions,
            100.0,
            Some(boundary),
            ts(START_SECS + 60),
            None,
            Some(&owner),
        );
        let first = ts(START_SECS + 5 * 3600 + 60);
        let post_boundary = boundary_present.then_some(boundary);
        observe_codex(
            &mut engine,
            &mut transitions,
            20.0,
            post_boundary,
            first,
            None,
            Some(&owner),
        );
        assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
        assert_eq!(
            engine.session_quota_states[&pid("codex")].pending_codex_restore_observation_at,
            Some(first)
        );

        observe_codex(
            &mut engine,
            &mut transitions,
            10.0,
            post_boundary,
            ts(START_SECS + 5 * 3600 + 120),
            None,
            Some(&owner),
        );
        observe_codex(
            &mut engine,
            &mut transitions,
            5.0,
            post_boundary,
            ts(START_SECS + 5 * 3600 + 180),
            None,
            Some(&owner),
        );
        assert_eq!(
            transitions,
            vec![
                SessionQuotaTransition::Depleted,
                SessionQuotaTransition::Restored
            ]
        );
    }
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:192 boundaryless restore requires two fresh observations
#[test]
fn codex_session_boundaryless_restore_requires_two_fresh_observations() {
    let owner = codex_owner("boundaryless");
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert!(
        engine.session_quota_states[&pid("codex")]
            .pending_codex_restore_observation_at
            .is_some()
    );

    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        None,
        ts(START_SECS + 180),
        None,
        Some(&owner),
    );
    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:209 advanced post expiry boundary restores exactly once
#[test]
fn codex_session_advanced_post_expiry_boundary_restores_exactly_once() {
    let owner = codex_owner("post-expiry-advanced");
    let boundary = ts(START_SECS + 5 * 3600);
    let advanced = ts(START_SECS + 10 * 3600);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(boundary),
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(boundary),
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(advanced),
        ts(START_SECS + 5 * 3600 + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        Some(advanced),
        ts(START_SECS + 5 * 3600 + 120),
        None,
        Some(&owner),
    );

    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].trusted_reset_boundary,
        Some(advanced)
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:226 advanced boundary expired at observation time requires confirmation
#[test]
fn codex_session_advanced_boundary_expired_at_observation_time_requires_confirmation() {
    let owner = codex_owner("advanced-expired-at-observation");
    let boundary = ts(START_SECS + 100);
    let candidate = ts(START_SECS + 250);
    let previous = SessionQuotaTransitionState {
        remaining: 0.0,
        source: SessionQuotaWindowSource::Primary,
        observed_at: ts(START_SECS),
        account_discriminator: Some(owner.raw_value.clone()),
        trusted_reset_boundary: Some(boundary),
        pending_codex_restore_observation_at: None,
    };

    let evaluation = SessionQuotaTransitionReducer::evaluate(
        Some(&previous),
        &SessionQuotaTransitionObservation {
            provider: pid("codex"),
            remaining: 80.0,
            source: SessionQuotaWindowSource::Primary,
            reset_boundary: Some(candidate),
            observed_at: ts(START_SECS + 300),
            evaluation_time: ts(START_SECS + 200),
            account_discriminator: Some(owner.raw_value),
        },
        true,
        false,
    );

    assert_eq!(
        evaluation.outcome,
        SessionQuotaTransitionOutcome::AwaitingCodexRestoreConfirmation
    );
    assert_eq!(evaluation.state.trusted_reset_boundary, Some(boundary));
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:255 regressed post expiry boundary requires two fresh observations
#[test]
fn codex_session_regressed_post_expiry_boundary_requires_two_fresh_observations() {
    let owner = codex_owner("regressed");
    let boundary = ts(START_SECS + 5 * 3600);
    let regressed = ts(START_SECS + 10 * 60);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(boundary),
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        Some(boundary),
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        Some(regressed),
        ts(START_SECS + 5 * 3600 + 60),
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert!(
        engine.session_quota_states[&pid("codex")]
            .pending_codex_restore_observation_at
            .is_some()
    );

    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        Some(regressed),
        ts(START_SECS + 5 * 3600 + 120),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        5.0,
        Some(regressed),
        ts(START_SECS + 5 * 3600 + 180),
        None,
        Some(&owner),
    );

    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
    assert_eq!(
        engine.session_quota_states[&pid("codex")].pending_codex_restore_observation_at,
        None
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:276 older and equal observations cannot change the depleted baseline
#[test]
fn codex_session_older_and_equal_observations_cannot_change_depleted_baseline() {
    let owner = codex_owner("observation-order");
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let depleted_at = ts(START_SECS + 120);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        depleted_at,
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        0.0,
        boundary,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        0.0,
        boundary,
        depleted_at,
        None,
        Some(&owner),
    );

    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    let state = &engine.session_quota_states[&pid("codex")];
    assert_eq!(state.remaining, 0.0);
    assert_eq!(state.observed_at, depleted_at);
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:294 owner change establishes a new baseline without restoring
#[test]
fn codex_session_owner_change_establishes_new_baseline_without_restoring() {
    let owner_a = codex_owner("owner-a");
    let owner_b = codex_owner("owner-b");
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS),
        None,
        Some(&owner_a),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 60),
        None,
        Some(&owner_a),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        0.0,
        boundary,
        ts(START_SECS + 120),
        None,
        Some(&owner_b),
    );

    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    let state = &engine.session_quota_states[&pid("codex")];
    assert_eq!(
        state.account_discriminator.as_deref(),
        Some(owner_b.raw_value.as_str())
    );
    assert_eq!(state.remaining, 100.0);
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:311 source change establishes a new reducer baseline
#[test]
fn codex_session_source_change_establishes_new_reducer_baseline() {
    let owner = codex_owner("source-change");
    let boundary = ts(START_SECS + 5 * 3600);
    let previous = SessionQuotaTransitionState {
        remaining: 0.0,
        source: SessionQuotaWindowSource::Primary,
        observed_at: ts(START_SECS),
        account_discriminator: Some(owner.raw_value.clone()),
        trusted_reset_boundary: Some(boundary),
        pending_codex_restore_observation_at: None,
    };

    let evaluation = SessionQuotaTransitionReducer::evaluate(
        Some(&previous),
        &SessionQuotaTransitionObservation {
            provider: pid("codex"),
            remaining: 100.0,
            source: SessionQuotaWindowSource::CopilotSecondaryFallback,
            reset_boundary: Some(boundary),
            observed_at: ts(START_SECS + 60),
            evaluation_time: ts(START_SECS + 60),
            account_discriminator: Some(owner.raw_value),
        },
        true,
        false,
    );

    assert_eq!(
        evaluation.outcome,
        SessionQuotaTransitionOutcome::BaselineChanged
    );
    assert_eq!(evaluation.state.remaining, 100.0);
    assert_eq!(
        evaluation.state.source,
        SessionQuotaWindowSource::CopilotSecondaryFallback
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:340 missing owner fails closed and clears prior state
#[test]
fn codex_session_missing_owner_fails_closed_and_clears_prior_state() {
    let owner = codex_owner("missing-owner");
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 60),
        None,
        None,
    );

    assert!(!engine.session_quota_states.contains_key(&pid("codex")));
    assert!(engine.codex_session_quota_baseline_required());
    assert!(transitions.is_empty());

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    assert!(transitions.is_empty());
    assert_eq!(engine.session_quota_states[&pid("codex")].remaining, 0.0);
    assert!(!engine.codex_session_quota_baseline_required());
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:372 missing owner keeps stale observations behind the fresh baseline barrier
#[test]
fn codex_session_missing_owner_keeps_stale_observations_behind_fresh_baseline_barrier() {
    let owner = codex_owner("missing-owner-watermark");
    let invalidated_at = ts(START_SECS + 120);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        invalidated_at,
        None,
        None,
    );

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS + 90),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        None,
        ts(START_SECS + 100),
        None,
        Some(&owner),
    );

    assert!(transitions.is_empty());
    assert!(!engine.session_quota_states.contains_key(&pid("codex")));
    assert_eq!(
        engine
            .codex_session_quota_baseline_requirement
            .as_ref()
            .and_then(|r| r.observed_at_watermark),
        Some(invalidated_at)
    );

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 121),
        None,
        Some(&owner),
    );
    assert!(transitions.is_empty());
    assert_eq!(engine.session_quota_states[&pid("codex")].remaining, 0.0);
    assert!(!engine.codex_session_quota_baseline_required());
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:401 windowless Codex result advances a matching depleted baseline watermark
#[test]
fn codex_session_windowless_result_advances_matching_depleted_baseline_watermark() {
    let owner = codex_owner("windowless-partial");
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );

    let windowless = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS + 120),
        None,
        Confidence::Exact,
    )
    .unwrap();
    engine.handle_session_quota_transition(
        &codex,
        &windowless,
        None,
        Some(&owner),
        false,
        ts(START_SECS + 120),
    );

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS + 90),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        boundary,
        ts(START_SECS + 100),
        None,
        Some(&owner),
    );

    assert_eq!(engine.session_quota_states[&codex].remaining, 0.0);
    assert_eq!(
        engine.session_quota_states[&codex].observed_at,
        ts(START_SECS + 120)
    );
    assert_eq!(
        engine.session_quota_states[&codex].trusted_reset_boundary,
        boundary
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 180),
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:437 windowless Codex result blocks stale boundaryless restore confirmation
#[test]
fn codex_session_windowless_result_blocks_stale_boundaryless_restore_confirmation() {
    let owner = codex_owner("windowless-boundaryless");
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );

    let windowless = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS + 120),
        None,
        Confidence::Exact,
    )
    .unwrap();
    engine.handle_session_quota_transition(
        &codex,
        &windowless,
        None,
        Some(&owner),
        false,
        ts(START_SECS + 120),
    );

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS + 90),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        None,
        ts(START_SECS + 100),
        None,
        Some(&owner),
    );

    assert_eq!(engine.session_quota_states[&codex].remaining, 0.0);
    assert_eq!(
        engine.session_quota_states[&codex].observed_at,
        ts(START_SECS + 120)
    );
    assert_eq!(
        engine.session_quota_states[&codex].pending_codex_restore_observation_at,
        None
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 180),
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:472 disabled provider cleanup does not refire Codex depletion
#[test]
fn codex_session_disabled_provider_cleanup_does_not_refire_depletion() {
    let owner = codex_owner("disabled-provider-cleanup");
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    engine.clear_disabled_provider_state(&HashSet::new());

    assert!(!engine.session_quota_states.contains_key(&codex));
    assert!(engine.codex_session_quota_baseline_required());

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(engine.session_quota_states[&codex].remaining, 0.0);
    assert!(!engine.codex_session_quota_baseline_required());
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:493 unavailable provider cleanup does not refire Codex depletion
#[test]
fn codex_session_unavailable_provider_cleanup_does_not_refire_depletion() {
    let owner = codex_owner("unavailable-provider-cleanup");
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    engine.clear_session_quota_transition_state(&codex);

    assert!(!engine.session_quota_states.contains_key(&codex));
    assert!(engine.codex_session_quota_baseline_required());

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(engine.session_quota_states[&codex].remaining, 0.0);
    assert!(!engine.codex_session_quota_baseline_required());
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:516 cleanup after a positive Codex baseline still reports depletion on recovery
#[test]
fn codex_session_cleanup_after_positive_baseline_still_reports_depletion_on_recovery() {
    let owner = codex_owner("positive-provider-cleanup");
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    engine.clear_disabled_provider_state(&HashSet::new());

    assert!(!engine.session_quota_states.contains_key(&codex));
    assert!(!engine.codex_session_quota_baseline_required());

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        boundary,
        ts(START_SECS + 120),
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(engine.session_quota_states[&codex].remaining, 0.0);
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:535 cleanup without a prior Codex baseline keeps startup depletion semantics
#[test]
fn codex_session_cleanup_without_prior_baseline_keeps_startup_depletion_semantics() {
    let owner = codex_owner("startup-cleanup");
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    engine.clear_disabled_provider_state(&HashSet::new());
    assert!(!engine.codex_session_quota_baseline_required());

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(engine.session_quota_states[&codex].remaining, 0.0);
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:551 notifications disabled keep stale observations behind the fresh baseline barrier
#[test]
fn codex_session_notifications_disabled_keep_stale_observations_behind_fresh_baseline_barrier() {
    let owner = codex_owner("disabled-watermark");
    let invalidated_at = ts(START_SECS + 120);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    engine.preferences.session_quota_notifications_enabled = false;
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        invalidated_at,
        None,
        Some(&owner),
    );

    assert!(transitions.is_empty());
    assert!(!engine.session_quota_states.contains_key(&codex));
    assert!(engine.codex_session_quota_baseline_required());

    engine.preferences.session_quota_notifications_enabled = true;
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 60),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS + 90),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        None,
        ts(START_SECS + 100),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        invalidated_at,
        None,
        Some(&owner),
    );

    assert!(transitions.is_empty());
    assert!(!engine.session_quota_states.contains_key(&codex));

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 121),
        None,
        Some(&owner),
    );
    assert!(transitions.is_empty());
    assert_eq!(engine.session_quota_states[&codex].remaining, 0.0);

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS + 122),
        None,
        Some(&owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        None,
        ts(START_SECS + 123),
        None,
        Some(&owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Restored]);

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 124),
        None,
        Some(&owner),
    );
    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Restored,
            SessionQuotaTransition::Depleted
        ]
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:593 non Codex providers preserve immediate restore semantics
#[test]
fn non_codex_providers_preserve_immediate_restore_semantics() {
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let claude = pid("claude");

    for (used, offset) in [(20.0, 0), (100.0, 60), (0.0, 120)] {
        let snap = make_snapshot(
            "claude",
            used,
            boundary,
            ts(START_SECS + offset),
            "session-fixture@example.test",
        );
        let (post, _, t) = engine.handle_session_quota_transition(
            &claude,
            &snap,
            None,
            None,
            false,
            ts(START_SECS + offset),
        );
        if post.is_some() {
            transitions.push(t);
        }
    }

    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:618 non Codex providers preserve disabled baseline tracking
#[test]
fn non_codex_providers_preserve_disabled_baseline_tracking() {
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.session_quota_notifications_enabled = false;
    let mut transitions = Vec::new();
    let claude = pid("claude");

    for (used, offset) in [(20.0, 0), (100.0, 60)] {
        let snap = make_snapshot(
            "claude",
            used,
            None,
            ts(START_SECS + offset),
            "session-fixture@example.test",
        );
        let (post, _, t) = engine.handle_session_quota_transition(
            &claude,
            &snap,
            None,
            None,
            false,
            ts(START_SECS + offset),
        );
        if post.is_some() {
            transitions.push(t);
        }
    }
    assert!(transitions.is_empty());

    engine.preferences.session_quota_notifications_enabled = true;
    let snap = make_snapshot(
        "claude",
        20.0,
        None,
        ts(START_SECS + 120),
        "session-fixture@example.test",
    );
    let (post, _, t) = engine.handle_session_quota_transition(
        &claude,
        &snap,
        None,
        None,
        false,
        ts(START_SECS + 120),
    );
    if post.is_some() {
        transitions.push(t);
    }
    assert_eq!(transitions, vec![SessionQuotaTransition::Restored]);
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:645 selected Codex account caller forwards its stable owner
#[test]
fn codex_session_selected_codex_account_caller_forwards_its_stable_owner() {
    let expected_owner = codex_owner("selected-caller");
    let limit_reset_owner = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: "workspace-fixture-selected-caller".to_owned(),
        },
        Some("session-fixture@example.test"),
    )
    .unwrap();
    let now = ts(START_SECS);
    let snapshot = make_snapshot(
        "codex",
        20.0,
        Some(ts(START_SECS + 5 * 3600)),
        now,
        "session-fixture@example.test",
    );
    let mut engine = NotificationPolicyEngine::default();
    let codex = pid("codex");

    engine.handle_session_quota_transition(
        &codex,
        &snapshot,
        None,
        Some(&expected_owner),
        false,
        now,
    );
    engine.handle_limit_reset_transitions(&codex, &snapshot, None, Some(&limit_reset_owner), false);

    assert_eq!(
        engine.session_quota_states[&codex]
            .account_discriminator
            .as_deref(),
        Some(expected_owner.raw_value.as_str())
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:685 selected Codex accounts keep independent quota warning episodes
#[test]
fn codex_session_selected_codex_accounts_keep_independent_quota_warning_episodes() {
    let codex = pid("codex");
    let first_owner =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::LiveSystem,
            CodexIdentity::ProviderAccount {
                id: "workspace-first-quota-account".to_owned(),
            },
            Some("first-quota@example.test".to_owned()),
            None,
        ))
        .unwrap();
    let second_owner =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::ManagedAccount {
                id: "00000000-0000-0000-0000-000000000002".to_owned(),
            },
            CodexIdentity::ProviderAccount {
                id: "workspace-second-quota-account".to_owned(),
            },
            Some("second-quota@example.test".to_owned()),
            None,
        ))
        .unwrap();

    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.session_quota_notifications_enabled = false;
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);

    let mut fired_emails = Vec::new();
    let mut fired_thresholds = Vec::new();
    for (owner, email, used_percent) in [
        (&first_owner, "first-quota@example.test", 40.0),
        (&second_owner, "second-quota@example.test", 40.0),
        (&first_owner, "first-quota@example.test", 55.0),
        (&second_owner, "second-quota@example.test", 30.0),
        (&first_owner, "first-quota@example.test", 55.0),
        (&second_owner, "second-quota@example.test", 55.0),
    ] {
        let snapshot = make_snapshot("codex", used_percent, None, ts(START_SECS), email);
        let (events, _) = engine.handle_quota_warning_transitions(
            &codex,
            &snapshot,
            Some(&owner.raw_value),
            true,
        );
        for event in events {
            fired_emails.push(email);
            fired_thresholds.push(event.threshold);
        }
    }

    assert_eq!(
        fired_emails,
        vec!["first-quota@example.test", "second-quota@example.test"]
    );
    assert_eq!(fired_thresholds, vec![50, 50]);
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:757 selected email only Codex account keeps session notifications
#[test]
fn codex_session_selected_email_only_codex_account_keeps_session_notifications() {
    let email = "email-only-session@example.test";
    let owner =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::LiveSystem,
            CodexIdentity::EmailOnly {
                normalized_email: email.to_owned(),
            },
            Some(email.to_owned()),
            Some("fixture-auth-fingerprint".to_owned()),
        ))
        .unwrap();
    let boundary = ts(START_SECS + 5 * 3600);
    let advanced_boundary = ts(START_SECS + 10 * 3600);
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    for (used, observed_at, reset_boundary) in [
        (20.0, ts(START_SECS), boundary),
        (100.0, ts(START_SECS + 60), boundary),
        (20.0, ts(START_SECS + 5 * 3600 + 60), advanced_boundary),
        (10.0, ts(START_SECS + 5 * 3600 + 120), advanced_boundary),
    ] {
        let snap = make_snapshot("codex", used, Some(reset_boundary), observed_at, email);
        let (post, _, t) = engine.handle_session_quota_transition(
            &codex,
            &snap,
            None,
            Some(&owner),
            false,
            observed_at,
        );
        if post.is_some() {
            transitions.push(t);
        }
    }

    assert!(
        engine.session_quota_states[&codex]
            .account_discriminator
            .is_some()
    );
    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:806 email only notification owners isolate source and credential rotation
#[test]
fn codex_session_email_only_notification_owners_isolate_source_and_credential_rotation() {
    let email = "email-only-owner@example.test";
    let identity = CodexIdentity::EmailOnly {
        normalized_email: email.to_owned(),
    };
    let live_a =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::LiveSystem,
            identity.clone(),
            Some(email.to_owned()),
            Some("fixture-a".to_owned()),
        ))
        .unwrap();
    let live_b =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::LiveSystem,
            identity.clone(),
            Some(email.to_owned()),
            Some("fixture-b".to_owned()),
        ))
        .unwrap();
    let profile =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::ProfileHome {
                path: "/tmp/codex-email-only-owner".to_owned(),
            },
            identity.clone(),
            Some(email.to_owned()),
            Some("fixture-a".to_owned()),
        ))
        .unwrap();
    let provider_identity = CodexIdentity::ProviderAccount {
        id: "workspace-email-only-owner".to_owned(),
    };
    let provider_a =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::LiveSystem,
            provider_identity.clone(),
            Some(email.to_owned()),
            Some("fixture-a".to_owned()),
        ))
        .unwrap();
    let provider_b =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::ProfileHome {
                path: "/tmp/codex-email-only-owner".to_owned(),
            },
            provider_identity,
            Some(email.to_owned()),
            Some("fixture-b".to_owned()),
        ))
        .unwrap();

    assert_ne!(live_a, live_b);
    assert_ne!(live_a, profile);
    assert_eq!(provider_a, provider_b);
    assert_eq!(CodexLimitResetOwnerKey::new(&identity, Some(email)), None);
    assert_eq!(
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::LiveSystem,
            identity,
            Some(email.to_owned()),
            None,
        )),
        None
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:848 email only credential rotation establishes a new baseline
#[test]
fn codex_session_email_only_credential_rotation_establishes_a_new_baseline() {
    let email = "rotating-email-only-owner@example.test";
    let identity = CodexIdentity::EmailOnly {
        normalized_email: email.to_owned(),
    };
    let old_owner =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::LiveSystem,
            identity.clone(),
            Some(email.to_owned()),
            Some("fixture-old".to_owned()),
        ))
        .unwrap();
    let new_owner =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::LiveSystem,
            identity,
            Some(email.to_owned()),
            Some("fixture-new".to_owned()),
        ))
        .unwrap();
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS),
        None,
        Some(&old_owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 60),
        None,
        Some(&old_owner),
    );
    engine.require_fresh_codex_session_quota_baseline(None);

    assert!(!engine.session_quota_states.contains_key(&codex));
    assert!(engine.codex_session_quota_baseline_required());

    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 120),
        None,
        Some(&new_owner),
    );
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(
        engine.session_quota_states[&codex]
            .account_discriminator
            .as_deref(),
        Some(new_owner.raw_value.as_str())
    );
    assert_eq!(engine.session_quota_states[&codex].remaining, 0.0);
    assert!(!engine.codex_session_quota_baseline_required());

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS + 180),
        None,
        Some(&new_owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        None,
        ts(START_SECS + 240),
        None,
        Some(&new_owner),
    );
    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:910 provider owner survives source and credential changes
#[test]
fn codex_session_provider_owner_survives_source_and_credential_changes() {
    let email = "provider-source-owner@example.test";
    let identity = CodexIdentity::ProviderAccount {
        id: "workspace-provider-source-owner".to_owned(),
    };
    let old_owner =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::LiveSystem,
            identity.clone(),
            Some(email.to_owned()),
            Some("fixture-old".to_owned()),
        ))
        .unwrap();
    let new_owner =
        CodexSessionQuotaOwnerKey::from_refresh_guard(&CodexAccountScopedRefreshGuard::new(
            CodexActiveSource::ProfileHome {
                path: "/tmp/codex-provider-source-owner".to_owned(),
            },
            identity,
            Some(email.to_owned()),
            Some("fixture-new".to_owned()),
        ))
        .unwrap();
    assert_eq!(old_owner, new_owner);

    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS),
        None,
        Some(&old_owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        100.0,
        None,
        ts(START_SECS + 60),
        None,
        Some(&old_owner),
    );
    assert_eq!(engine.session_quota_states[&codex].remaining, 0.0);

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS + 120),
        None,
        Some(&new_owner),
    );
    observe_codex(
        &mut engine,
        &mut transitions,
        10.0,
        None,
        ts(START_SECS + 180),
        None,
        Some(&new_owner),
    );
    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );

    engine.require_fresh_codex_session_quota_baseline(None);
    assert!(!engine.session_quota_states.contains_key(&codex));
    assert!(engine.codex_session_quota_baseline_required());
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:965 regular refresh owner builder supports email only identity
#[test]
fn codex_session_regular_refresh_owner_builder_supports_email_only_identity() {
    let email = "regular-email-only-owner@example.test";
    let refresh_guard = CodexAccountScopedRefreshGuard::new(
        CodexActiveSource::LiveSystem,
        CodexIdentity::EmailOnly {
            normalized_email: email.to_owned(),
        },
        Some(email.to_owned()),
        Some("fixture-regular".to_owned()),
    );
    let owner = CodexSessionQuotaOwnerKey::from_refresh_guard(&refresh_guard).unwrap();
    assert!(!owner.raw_value.is_empty());
}

// upstream: CodexSessionQuotaFalseRestoreTests.swift:978 clearing published Codex usage clears typed transition state
#[test]
fn codex_session_clearing_published_codex_usage_clears_typed_transition_state() {
    let owner = codex_owner("cleanup");
    let boundary = Some(ts(START_SECS + 5 * 3600));
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    let codex = pid("codex");

    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        boundary,
        ts(START_SECS),
        None,
        Some(&owner),
    );
    assert!(engine.session_quota_states.contains_key(&codex));

    engine.require_fresh_codex_session_quota_baseline(None);
    assert!(!engine.session_quota_states.contains_key(&codex));
    assert!(engine.codex_session_quota_baseline_required());
}

// =========================================================================
// CodexLimitResetOwnerKeyTests
// =========================================================================

// partial: constructor half only (visible-row half belongs to the multi-account runtime slice)
// upstream: CodexLimitResetOwnerKeyTests.swift:9 limit reset owner stays stable for the same provider workspace and email
#[test]
fn codex_limit_reset_owner_stays_stable_for_same_provider_workspace_and_email() {
    let original = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: "workspace-fixture-stable".to_owned(),
        },
        Some(" Person-One@Example.Test "),
    )
    .unwrap();
    let relabeled = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: " workspace-fixture-stable ".to_owned(),
        },
        Some("person-one@example.test"),
    )
    .unwrap();
    assert_eq!(original, relabeled);
    assert_eq!(original.raw_value.len(), 64);
    assert!(!original.raw_value.contains("workspace-fixture-stable"));
    assert!(!original.raw_value.contains("person-one@example.test"));
}

// partial: constructor half only (visible-row half belongs to the multi-account runtime slice)
// upstream: CodexLimitResetOwnerKeyTests.swift:38 different emails in the same provider workspace use different owner keys
#[test]
fn codex_limit_reset_different_emails_in_same_provider_workspace_use_different_owner_keys() {
    let member_1 = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: "workspace-fixture-shared".to_owned(),
        },
        Some("first-member@example.test"),
    )
    .unwrap();
    let member_2 = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: "workspace-fixture-shared".to_owned(),
        },
        Some("second-member@example.test"),
    )
    .unwrap();
    assert_ne!(member_1, member_2);
}

// partial: constructor half only (visible-row half belongs to the multi-account runtime slice)
// upstream: CodexLimitResetOwnerKeyTests.swift:60 different provider workspaces with the same email use different owner keys
#[test]
fn codex_limit_reset_different_provider_workspaces_with_same_email_use_different_owner_keys() {
    let ws_1 = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: "workspace-fixture-one".to_owned(),
        },
        Some("shared-person@example.test"),
    )
    .unwrap();
    let ws_2 = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: "workspace-fixture-two".to_owned(),
        },
        Some("shared-person@example.test"),
    )
    .unwrap();
    assert_ne!(ws_1, ws_2);
    assert!(!ws_1.raw_value.contains("workspace-fixture-one"));
    assert!(!ws_1.raw_value.contains("shared-person@example.test"));
    assert!(!ws_2.raw_value.contains("workspace-fixture-two"));
    assert!(!ws_2.raw_value.contains("shared-person@example.test"));
}

// partial: constructor half only (visible-row half belongs to the multi-account runtime slice)
// upstream: CodexLimitResetOwnerKeyTests.swift:87 email only owner fails closed even for one visible row
#[test]
fn codex_limit_reset_email_only_owner_fails_closed_even_for_one_visible_row() {
    assert_eq!(
        CodexLimitResetOwnerKey::new(
            &CodexIdentity::EmailOnly {
                normalized_email: "unique-person@example.test".to_owned()
            },
            Some("unique-person@example.test")
        ),
        None
    );
}

// partial: constructor half only (visible-row half belongs to the multi-account runtime slice)
// upstream: CodexLimitResetOwnerKeyTests.swift:102 duplicate email only rows fail closed
#[test]
fn codex_limit_reset_duplicate_email_only_rows_fail_closed() {
    let identity = CodexIdentity::EmailOnly {
        normalized_email: "ambiguous-person@example.test".to_owned(),
    };
    assert_eq!(
        CodexLimitResetOwnerKey::new(&identity, Some("ambiguous-person@example.test")),
        None
    );
    assert_eq!(
        CodexLimitResetOwnerKey::new(&identity, Some(" Ambiguous-Person@Example.Test ")),
        None
    );
}

// partial: constructor half only (visible-row half belongs to the multi-account runtime slice)
// upstream: CodexLimitResetOwnerKeyTests.swift:117 provider row wins its own identity while same email fallback fails closed
#[test]
fn codex_limit_reset_provider_row_wins_its_own_identity_while_same_email_fallback_fails_closed() {
    let provider_key = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: "workspace-fixture-provider".to_owned(),
        },
        Some("mixed-person@example.test"),
    );
    assert!(provider_key.is_some());
    assert_eq!(
        CodexLimitResetOwnerKey::new(
            &CodexIdentity::EmailOnly {
                normalized_email: "mixed-person@example.test".to_owned(),
            },
            Some("mixed-person@example.test"),
        ),
        None
    );
}

// partial: constructor half only (visible-row half belongs to the multi-account runtime slice)
// upstream: CodexLimitResetOwnerKeyTests.swift:139 guard and visible row normalize the same provider owner
#[test]
fn codex_limit_reset_guard_and_visible_row_normalize_the_same_provider_owner() {
    let visible_key = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: " workspace-fixture-mixed-case ".to_owned(),
        },
        Some("provider-person@example.test"),
    )
    .unwrap();
    let guard_key = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: " WORKSPACE-FIXTURE-MIXED-CASE ".to_owned(),
        },
        Some("provider-person@example.test"),
    )
    .unwrap();
    assert_eq!(visible_key, guard_key);
}

// partial: constructor half only (visible-row half belongs to the multi-account runtime slice)
// upstream: CodexLimitResetOwnerKeyTests.swift:161 unresolved owner identity fails closed
#[test]
fn codex_limit_reset_unresolved_owner_identity_fails_closed() {
    assert_eq!(
        CodexLimitResetOwnerKey::new(&CodexIdentity::Unresolved, None),
        None
    );
    assert_eq!(
        CodexLimitResetOwnerKey::new(&CodexIdentity::Unresolved, Some("   ")),
        None
    );
}

// =========================================================================
// LimitResetNotificationTests
// =========================================================================

fn claude_snapshot(
    session_used: f64,
    weekly_used: f64,
    offset_secs: i64,
    email: &str,
    reset_boundary: Option<Timestamp>,
) -> UsageSnapshot {
    UsageSnapshot::new(
        Some(rate_window(session_used, Some(300), reset_boundary)),
        Some(rate_window(weekly_used, Some(10080), reset_boundary)),
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS + offset_secs),
        Some(ProviderIdentitySnapshot {
            provider_id: Some(pid("claude")),
            account_email: Some(email.to_owned()),
            account_organization: None,
            login_method: Some("max".to_owned()),
            account_id: None,
        }),
        Confidence::Exact,
    )
    .unwrap()
}

// upstream: LimitResetNotificationTests.swift:49 limit reset notifications default off and persist when enabled
#[test]
fn limit_reset_notifications_default_off_and_persist_when_enabled() {
    let mut prefs = NotificationPreferences::default();
    assert!(!prefs.limit_reset_notifications_enabled);

    prefs.limit_reset_notifications_enabled = true;
    let engine = NotificationPolicyEngine::new(prefs.clone());
    assert!(engine.preferences.limit_reset_notifications_enabled);

    let persisted = engine.export_persisted_state();
    let mut restored = NotificationPolicyEngine::new(prefs);
    restored.restore_persisted_state(persisted);
    assert!(restored.preferences.limit_reset_notifications_enabled);
}

// upstream: LimitResetNotificationTests.swift:63 weekly reset notice names provider window and account
#[test]
fn limit_reset_weekly_reset_notice_names_provider_window_and_account() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;

    let s1 = claude_snapshot(20.0, 60.0, 0, "limit-reset-notice@example.com", None);
    let s2 = claude_snapshot(20.0, 0.0, 3600, "limit-reset-notice@example.com", None);
    assert!(
        engine
            .handle_limit_reset_transitions(&claude, &s1, None, None, false)
            .is_empty()
    );
    let posts = engine.handle_limit_reset_transitions(&claude, &s2, None, None, false);
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].id_prefix, "limit-reset-claude-weekly");
    assert_eq!(posts[0].title, "Claude weekly limit reset");
    assert_eq!(posts[0].body, "Fresh quota is available.");
}

// upstream: LimitResetNotificationTests.swift:77 session reset notice names the session window
#[test]
fn limit_reset_session_reset_notice_names_the_session_window() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;

    let s1 = claude_snapshot(65.0, 20.0, 0, "limit-reset-notice@example.com", None);
    let s2 = claude_snapshot(0.0, 20.0, 3600, "limit-reset-notice@example.com", None);
    assert!(
        engine
            .handle_limit_reset_transitions(&claude, &s1, None, None, false)
            .is_empty()
    );
    let posts = engine.handle_limit_reset_transitions(&claude, &s2, None, None, false);
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].id_prefix, "limit-reset-claude-session");
    assert_eq!(posts[0].title, "Claude session limit reset");
    assert_eq!(posts[0].body, "Fresh quota is available.");
}

// upstream: LimitResetNotificationTests.swift:91 reset notices stay silent when disabled
#[test]
fn limit_reset_notices_stay_silent_when_disabled() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = false;

    let s1 = claude_snapshot(20.0, 60.0, 0, "limit-reset-notice@example.com", None);
    let s2 = claude_snapshot(20.0, 0.0, 3600, "limit-reset-notice@example.com", None);
    assert!(
        engine
            .handle_limit_reset_transitions(&claude, &s1, None, None, false)
            .is_empty()
    );
    assert!(
        engine
            .handle_limit_reset_transitions(&claude, &s2, None, None, false)
            .is_empty()
    );
    assert!(
        engine
            .weekly_limit_reset_states
            .contains_key("claude:limit-reset-notice@example.com")
    );
}

// upstream: LimitResetNotificationTests.swift:105 hide personal info omits the account from reset notices
#[test]
fn limit_reset_hide_personal_info_omits_the_account_from_reset_notices() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;
    engine.preferences.hide_personal_info = true;

    let s1 = claude_snapshot(20.0, 60.0, 0, "limit-reset-notice@example.com", None);
    let s2 = claude_snapshot(20.0, 0.0, 3600, "limit-reset-notice@example.com", None);
    engine.handle_limit_reset_transitions(&claude, &s1, None, None, false);
    let posts = engine.handle_limit_reset_transitions(&claude, &s2, None, None, false);
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].id_prefix, "limit-reset-claude-weekly");
    assert!(!posts[0].title.contains("limit-reset-notice@example.com"));
    assert!(!posts[0].body.contains("limit-reset-notice@example.com"));
}

// upstream: LimitResetNotificationTests.swift:141 reset notice copy names provider and window
#[test]
fn limit_reset_notice_copy_names_provider_and_window() {
    let (w_title, w_body) =
        LimitResetNotificationLogic::notification_copy("Claude", QuotaWarningWindow::Weekly);
    assert_eq!(w_title, "Claude weekly limit reset");
    assert_eq!(w_body, "Fresh quota is available.");

    let (s_title, s_body) =
        LimitResetNotificationLogic::notification_copy("Codex", QuotaWarningWindow::Session);
    assert_eq!(s_title, "Codex session limit reset");
    assert_eq!(s_body, "Fresh quota is available.");
}

// upstream: LimitResetNotificationTests.swift:171 portable preferences carry the reset toggle in both directions
#[test]
fn limit_reset_portable_preferences_carry_the_reset_toggle_in_both_directions() {
    let mut source = NotificationPreferences::default();
    assert!(!source.limit_reset_notifications_enabled);
    source.limit_reset_notifications_enabled = true;
    let encoded = serde_json::to_string(&source).unwrap();
    let decoded: NotificationPreferences = serde_json::from_str(&encoded).unwrap();
    assert!(decoded.limit_reset_notifications_enabled);

    source.limit_reset_notifications_enabled = false;
    let encoded_off = serde_json::to_string(&source).unwrap();
    let decoded_off: NotificationPreferences = serde_json::from_str(&encoded_off).unwrap();
    assert!(!decoded_off.limit_reset_notifications_enabled);
}

// upstream: LimitResetNotificationTests.swift:120 session restored notice covers the matching session reset notice
#[test]
fn limit_reset_session_restored_notice_covers_the_matching_session_reset_notice() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.session_quota_notifications_enabled = true;
    engine.preferences.limit_reset_notifications_enabled = true;

    let depleted = claude_snapshot(100.0, 60.0, 0, "limit-reset-notice@example.com", None);
    let reset = claude_snapshot(0.0, 0.0, 3600, "limit-reset-notice@example.com", None);

    let (post1, pending1, t1) = engine.handle_session_quota_transition(
        &claude,
        &depleted,
        None,
        None,
        false,
        depleted.updated_at,
    );
    assert!(post1.is_some());
    assert!(!pending1);
    assert_eq!(t1, SessionQuotaTransition::Depleted);
    engine.handle_limit_reset_transitions(&claude, &depleted, None, None, false);

    let (post2, pending2, t2) = engine.handle_session_quota_transition(
        &claude,
        &reset,
        None,
        None,
        false,
        reset.updated_at,
    );
    assert!(post2.is_none());
    assert!(pending2);
    assert_eq!(t2, SessionQuotaTransition::Restored);
    let reset_posts = engine.handle_limit_reset_transitions(&claude, &reset, None, None, pending2);
    assert_eq!(reset_posts.len(), 2);
    assert_eq!(reset_posts[0].id_prefix, "session-claude-restored");
    assert_eq!(reset_posts[1].id_prefix, "limit-reset-claude-weekly");
}

// upstream: LimitResetNotificationTests.swift:187 restored notice for one account does not suppress another account reset
#[test]
fn limit_reset_restored_notice_for_one_account_does_not_suppress_another_account_reset() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.session_quota_notifications_enabled = true;
    engine.preferences.limit_reset_notifications_enabled = true;

    let depleted = claude_snapshot(100.0, 20.0, 0, "limit-reset-notice@example.com", None);
    let restored = claude_snapshot(0.0, 20.0, 60, "limit-reset-notice@example.com", None);
    let (post1, _, _) = engine.handle_session_quota_transition(
        &claude,
        &depleted,
        None,
        None,
        false,
        depleted.updated_at,
    );
    assert!(post1.is_some());
    let (_, pending, _) = engine.handle_session_quota_transition(
        &claude,
        &restored,
        None,
        None,
        false,
        restored.updated_at,
    );
    let first_posts =
        engine.handle_limit_reset_transitions(&claude, &restored, None, None, pending);
    assert_eq!(first_posts.len(), 1);
    assert_eq!(first_posts[0].id_prefix, "session-claude-restored");

    let other_high = claude_snapshot(60.0, 20.0, 0, "other@example.com", None);
    let other_low = claude_snapshot(0.0, 20.0, 60, "other@example.com", None);
    assert!(
        engine
            .handle_limit_reset_transitions(&claude, &other_high, None, None, false)
            .is_empty()
    );
    let other_posts = engine.handle_limit_reset_transitions(&claude, &other_low, None, None, false);
    assert_eq!(other_posts.len(), 1);
    assert_eq!(other_posts[0].id_prefix, "limit-reset-claude-session");
}

// upstream: LimitResetNotificationTests.swift:209 weekly reset boundary is notified once across refreshes and restarts
#[test]
fn limit_reset_weekly_reset_boundary_is_notified_once_across_refreshes_and_restarts() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;
    let boundary = Some(ts(START_SECS + 7 * 86_400));
    let mut total_posts = Vec::new();

    for (index, used) in [60.0, 0.0, 20.0, 30.0, 0.0].into_iter().enumerate() {
        if index == 2 {
            let persisted = engine.export_persisted_state();
            let mut restarted = NotificationPolicyEngine::default();
            restarted.preferences.limit_reset_notifications_enabled = true;
            restarted.restore_persisted_state(persisted);
            engine = restarted;
        }
        let snapshot = claude_snapshot(
            20.0,
            used,
            (index as i64) * 60,
            "limit-reset-notice@example.com",
            boundary,
        );
        total_posts
            .extend(engine.handle_limit_reset_transitions(&claude, &snapshot, None, None, false));
    }
    assert_eq!(total_posts.len(), 1);
}

// upstream: LimitResetNotificationTests.swift:230 a restored confirmation after restart does not repeat a reset banner
#[test]
fn limit_reset_restored_confirmation_after_restart_does_not_repeat_a_reset_banner() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;

    let s1 = claude_snapshot(60.0, 20.0, 0, "limit-reset-notice@example.com", None);
    let s2 = claude_snapshot(0.0, 20.0, 60, "limit-reset-notice@example.com", None);
    engine.handle_limit_reset_transitions(&claude, &s1, None, None, false);
    let initial_posts = engine.handle_limit_reset_transitions(&claude, &s2, None, None, false);
    assert_eq!(initial_posts.len(), 1);

    let persisted = engine.export_persisted_state();
    let mut restarted = NotificationPolicyEngine::default();
    restarted.preferences.limit_reset_notifications_enabled = true;
    restarted.preferences.session_quota_notifications_enabled = true;
    restarted.restore_persisted_state(persisted);

    let depleted = claude_snapshot(100.0, 20.0, 90, "limit-reset-notice@example.com", None);
    let reset = claude_snapshot(0.0, 20.0, 120, "limit-reset-notice@example.com", None);
    let (dep_post, _, _) = restarted.handle_session_quota_transition(
        &claude,
        &depleted,
        None,
        None,
        false,
        depleted.updated_at,
    );
    assert!(dep_post.is_some());
    let (res_post, pending, _) = restarted.handle_session_quota_transition(
        &claude,
        &reset,
        None,
        None,
        false,
        reset.updated_at,
    );
    assert!(res_post.is_none());
    let after_restart_posts =
        restarted.handle_limit_reset_transitions(&claude, &reset, None, None, pending);
    assert!(after_restart_posts.is_empty());
}

// upstream: LimitResetNotificationTests.swift:250 reset notices recover when a provider stops reporting reset boundaries
#[test]
fn limit_reset_notices_recover_when_a_provider_stops_reporting_reset_boundaries() {
    let claude = pid("claude");
    for missing_from in [2usize, 4usize] {
        let mut engine = NotificationPolicyEngine::default();
        engine.preferences.limit_reset_notifications_enabled = true;
        let mut total_posts = Vec::new();
        for (index, used) in [60.0, 0.0, 20.0, 30.0, 0.0].into_iter().enumerate() {
            if index == 2 {
                let persisted = engine.export_persisted_state();
                let mut restarted = NotificationPolicyEngine::default();
                restarted.preferences.limit_reset_notifications_enabled = true;
                restarted.restore_persisted_state(persisted);
                engine = restarted;
            }
            let boundary = (index < missing_from).then(|| ts(START_SECS + 7 * 86_400));
            let snapshot = claude_snapshot(
                20.0,
                used,
                (index as i64) * 60,
                "limit-reset-notice@example.com",
                boundary,
            );
            total_posts.extend(
                engine.handle_limit_reset_transitions(&claude, &snapshot, None, None, false),
            );
        }
        assert_eq!(total_posts.len(), 2);
    }
}

// upstream: LimitResetNotificationTests.swift:271 partial restored notice also covers the later confirmed reset
#[test]
fn limit_reset_partial_restored_notice_also_covers_the_later_confirmed_reset() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.session_quota_notifications_enabled = true;
    engine.preferences.limit_reset_notifications_enabled = true;
    let mut session_posts = Vec::new();
    let mut reset_posts = Vec::new();

    for (index, used) in [100.0, 50.0].into_iter().enumerate() {
        let snapshot = claude_snapshot(
            used,
            20.0,
            (index as i64) * 60,
            "limit-reset-notice@example.com",
            None,
        );
        let (post, pending, _) = engine.handle_session_quota_transition(
            &claude,
            &snapshot,
            None,
            None,
            false,
            snapshot.updated_at,
        );
        if let Some(p) = post {
            session_posts.push(p);
        }
        for p in engine.handle_limit_reset_transitions(&claude, &snapshot, None, None, pending) {
            if p.id_prefix.starts_with("session-") {
                session_posts.push(p);
            } else {
                reset_posts.push(p);
            }
        }
    }
    let confirmed_zero = claude_snapshot(0.0, 20.0, 120, "limit-reset-notice@example.com", None);
    reset_posts.extend(engine.handle_limit_reset_transitions(
        &claude,
        &confirmed_zero,
        None,
        None,
        false,
    ));
    assert_eq!(session_posts.len(), 2);
    assert!(reset_posts.is_empty());

    let next_high = claude_snapshot(60.0, 20.0, 180, "limit-reset-notice@example.com", None);
    let next_zero = claude_snapshot(0.0, 20.0, 240, "limit-reset-notice@example.com", None);
    engine.handle_limit_reset_transitions(&claude, &next_high, None, None, false);
    reset_posts
        .extend(engine.handle_limit_reset_transitions(&claude, &next_zero, None, None, false));
    assert_eq!(reset_posts.len(), 1);
}

// upstream: LimitResetNotificationTests.swift:295 known reset boundaries remain deduplicated after a metadata gap
#[test]
fn limit_reset_known_reset_boundaries_remain_deduplicated_after_a_metadata_gap() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;
    let boundary = ts(START_SECS + 7 * 86_400);
    let mut reset_posts = Vec::new();

    for (index, used) in [60.0, 0.0, 20.0, 30.0, 0.0, 20.0, 30.0, 0.0]
        .into_iter()
        .enumerate()
    {
        let reset_boundary = if (2..=4).contains(&index) {
            None
        } else {
            Some(boundary)
        };
        let snapshot = claude_snapshot(
            20.0,
            used,
            (index as i64) * 60,
            "limit-reset-notice@example.com",
            reset_boundary,
        );
        reset_posts
            .extend(engine.handle_limit_reset_transitions(&claude, &snapshot, None, None, false));
    }
    assert_eq!(reset_posts.len(), 2);
}

// upstream: LimitResetNotificationTests.swift:315 new depleted episodes still deliver restored notices without full resets
#[test]
fn limit_reset_new_depleted_episodes_still_deliver_restored_notices_without_full_resets() {
    let claude = pid("claude");
    for with_reset_metadata in [false, true] {
        let mut engine = NotificationPolicyEngine::default();
        engine.preferences.session_quota_notifications_enabled = true;
        engine.preferences.limit_reset_notifications_enabled = true;
        let mut transitions = Vec::new();
        let mut limit_reset_posts = Vec::new();

        for (index, used) in [100.0, 50.0, 100.0, 50.0].into_iter().enumerate() {
            let boundary = with_reset_metadata.then(|| ts(START_SECS + 5 * 3600));
            let snapshot = claude_snapshot(
                used,
                20.0,
                (index as i64) * 60,
                "limit-reset-notice@example.com",
                boundary,
            );
            let (post, pending, t) = engine.handle_session_quota_transition(
                &claude,
                &snapshot,
                None,
                None,
                false,
                snapshot.updated_at,
            );
            if post.is_some() {
                transitions.push(t);
            }
            for p in engine.handle_limit_reset_transitions(&claude, &snapshot, None, None, pending)
            {
                if p.id_prefix.starts_with("session-") {
                    transitions.push(SessionQuotaTransition::Restored);
                } else {
                    limit_reset_posts.push(p);
                }
            }
        }
        assert_eq!(
            transitions,
            vec![
                SessionQuotaTransition::Depleted,
                SessionQuotaTransition::Restored,
                SessionQuotaTransition::Depleted,
                SessionQuotaTransition::Restored,
            ]
        );
        assert!(limit_reset_posts.is_empty());
    }
}

// upstream: LimitResetNotificationTests.swift:339 advanced boundaries and independent windows notify separately
#[test]
fn limit_reset_advanced_boundaries_and_independent_windows_notify_separately() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;
    let mut prefixes = Vec::new();

    for (index, used) in [60.0, 0.0, 20.0, 30.0, 0.0].into_iter().enumerate() {
        let mult = if index < 4 { 1 } else { 2 };
        let boundary = Some(ts(START_SECS + mult * 7 * 86_400));
        let snapshot = claude_snapshot(
            used,
            used,
            (index as i64) * 60,
            "limit-reset-notice@example.com",
            boundary,
        );
        for post in engine.handle_limit_reset_transitions(&claude, &snapshot, None, None, false) {
            prefixes.push(post.id_prefix);
        }
    }
    assert_eq!(
        prefixes,
        vec![
            "limit-reset-claude-weekly",
            "limit-reset-claude-session",
            "limit-reset-claude-weekly",
        ]
    );
}

// upstream: LimitResetNotificationTests.swift:356 pending reset delivery rechecks opt in and personal info
#[test]
fn limit_reset_pending_reset_delivery_rechecks_opt_in_and_personal_info() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;

    let s1 = claude_snapshot(20.0, 60.0, 0, "limit-reset-notice@example.com", None);
    let s2 = claude_snapshot(20.0, 0.0, 60, "limit-reset-notice@example.com", None);
    engine.handle_limit_reset_transitions(&claude, &s1, None, None, false);
    let posts = engine.handle_limit_reset_transitions(&claude, &s2, None, None, false);
    assert_eq!(posts.len(), 1);

    let initial_hide = engine.preferences.hide_personal_info;
    let is_current = |prefs: &NotificationPreferences| {
        prefs.limit_reset_notifications_enabled && prefs.hide_personal_info == initial_hide
    };
    assert!(is_current(&engine.preferences));
    engine.preferences.limit_reset_notifications_enabled = false;
    assert!(!is_current(&engine.preferences));
    engine.preferences.limit_reset_notifications_enabled = true;
    engine.preferences.hide_personal_info = true;
    assert!(!is_current(&engine.preferences));
}

// upstream: LimitResetNotificationTests.swift:372 a later restored confirmation does not repeat an announced reset
#[test]
fn limit_reset_later_restored_confirmation_does_not_repeat_an_announced_reset() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;

    let s1 = claude_snapshot(60.0, 20.0, 0, "limit-reset-notice@example.com", None);
    let s2 = claude_snapshot(0.0, 20.0, 60, "limit-reset-notice@example.com", None);
    engine.handle_limit_reset_transitions(&claude, &s1, None, None, false);
    let mut posts = engine.handle_limit_reset_transitions(&claude, &s2, None, None, false);
    assert_eq!(posts.len(), 1);

    let persisted = engine.export_persisted_state();
    let mut restarted = NotificationPolicyEngine::default();
    restarted.preferences.limit_reset_notifications_enabled = true;
    restarted.preferences.session_quota_notifications_enabled = true;
    restarted.restore_persisted_state(persisted);

    let confirmed = claude_snapshot(0.0, 20.0, 120, "limit-reset-notice@example.com", None);
    assert!(
        restarted
            .handle_limit_reset_transitions(&claude, &confirmed, None, None, true)
            .is_empty()
    );

    let s3 = claude_snapshot(60.0, 20.0, 180, "limit-reset-notice@example.com", None);
    let s4 = claude_snapshot(0.0, 20.0, 240, "limit-reset-notice@example.com", None);
    restarted.handle_limit_reset_transitions(&claude, &s3, None, None, false);
    posts.extend(restarted.handle_limit_reset_transitions(&claude, &s4, None, None, false));
    assert_eq!(posts.len(), 2);
}

// upstream: LimitResetNotificationTests.swift:392 switching from a depleted account does not cover another accounts reset
#[test]
fn limit_reset_switching_from_a_depleted_account_does_not_cover_another_accounts_reset() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.session_quota_notifications_enabled = true;
    engine.preferences.limit_reset_notifications_enabled = true;

    let snapshots = [
        claude_snapshot(60.0, 20.0, 0, "other@example.com", None),
        claude_snapshot(100.0, 20.0, 60, "limit-reset-notice@example.com", None),
        claude_snapshot(0.0, 20.0, 120, "other@example.com", None),
    ];
    let mut session_restored_posts = Vec::new();
    let mut limit_reset_posts = Vec::new();
    for snapshot in &snapshots {
        let (_, pending, _) = engine.handle_session_quota_transition(
            &claude,
            snapshot,
            None,
            None,
            false,
            snapshot.updated_at,
        );
        for post in engine.handle_limit_reset_transitions(&claude, snapshot, None, None, pending) {
            if post.id_prefix == "session-claude-restored" {
                session_restored_posts.push(post);
            } else {
                limit_reset_posts.push(post);
            }
        }
    }
    assert!(session_restored_posts.is_empty());
    assert_eq!(limit_reset_posts.len(), 1);
    assert_eq!(limit_reset_posts[0].id_prefix, "limit-reset-claude-session");
}

// upstream: LimitResetNotificationTests.swift:418 delayed restored confirmation stays covered when usage has resumed
#[test]
fn limit_reset_delayed_restored_confirmation_stays_covered_when_usage_has_resumed() {
    let previous = LimitResetDetectorState {
        notification_receipt: Some(LimitResetNotificationReceipt {
            reset_boundary: None,
        }),
        ..LimitResetDetectorState::new(false, ts(START_SECS), None)
    };
    let mut current = LimitResetDetectorState {
        notification_receipt: previous.notification_receipt.clone(),
        ..LimitResetDetectorState::new(true, ts(START_SECS + 60), None)
    };
    let current_observed_at = current.last_observed_at;
    let notice = prepare_limit_reset_notification(
        &mut current,
        Some(&previous),
        &LimitResetObservation {
            used_percent: 2.0,
            observed_at: current_observed_at,
            reset_boundary: None,
            source: None,
        },
        false,
        true,
        true,
        true,
    );
    assert_eq!(notice, None);

    let mut next = LimitResetDetectorState {
        notification_receipt: current.notification_receipt.clone(),
        ..LimitResetDetectorState::new(false, ts(START_SECS + 120), None)
    };
    let next_observed_at = next.last_observed_at;
    let next_notice = prepare_limit_reset_notification(
        &mut next,
        Some(&current),
        &LimitResetObservation {
            used_percent: 0.0,
            observed_at: next_observed_at,
            reset_boundary: None,
            source: None,
        },
        true,
        false,
        true,
        true,
    );
    assert!(next_notice.is_some());
}

// =========================================================================
// ClaudeExtraWindowQuotaWarningTests & UsageStoreAccountQuotaWarningTests
// =========================================================================

fn claude_extra_window_snapshot(
    fable_used: Option<f64>,
    routines_used: Option<f64>,
) -> UsageSnapshot {
    let mut windows = Vec::new();
    if let Some(fable_used) = fable_used {
        windows.push(NamedRateWindow::new(
            "claude-weekly-scoped-fable",
            "Fable only",
            rate_window(fable_used, Some(7 * 24 * 60), None),
        ));
    }
    if let Some(routines_used) = routines_used {
        windows.push(NamedRateWindow::new(
            "claude-routines",
            "Daily Routines",
            rate_window(routines_used, Some(7 * 24 * 60), None),
        ));
    }
    UsageSnapshot::new(
        None,
        None,
        None,
        Some(windows),
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS),
        None,
        Confidence::Exact,
    )
    .unwrap()
}

// upstream: ClaudeExtraWindowQuotaWarningTests.swift:42 claude scoped weekly and routines extra windows fire independent weekly warnings
#[test]
fn claude_extra_windows_scoped_weekly_and_routines_fire_independent_weekly_warnings() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(40.0), Some(40.0)),
        None,
        false,
    );
    let (events, _) = engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(55.0), Some(55.0)),
        None,
        false,
    );
    assert_eq!(events.len(), 2);
    let fable = events
        .iter()
        .find(|e| e.window_id.as_deref() == Some("claude-weekly-scoped-fable"))
        .unwrap();
    let routines = events
        .iter()
        .find(|e| e.window_id.as_deref() == Some("claude-routines"))
        .unwrap();
    assert_eq!(fable.window, QuotaWarningWindow::Weekly);
    assert_eq!(fable.threshold, 50);
    assert_eq!(fable.window_display_label.as_deref(), Some("Fable only"));
    assert_eq!(routines.threshold, 50);
    assert_eq!(
        routines.window_display_label.as_deref(),
        Some("Daily Routines")
    );

    let fable_key = QuotaWarningStateKey {
        provider: claude.clone(),
        window: QuotaWarningWindow::Weekly,
        account_discriminator: None,
        window_id: Some("claude-weekly-scoped-fable".to_owned()),
    };
    let routines_key = QuotaWarningStateKey {
        provider: claude,
        window: QuotaWarningWindow::Weekly,
        account_discriminator: None,
        window_id: Some("claude-routines".to_owned()),
    };
    assert!(
        engine.quota_warning_states[&fable_key]
            .fired_thresholds
            .contains(&50)
    );
    assert!(
        engine.quota_warning_states[&routines_key]
            .fired_thresholds
            .contains(&50)
    );
}

// upstream: ClaudeExtraWindowQuotaWarningTests.swift:92 antigravity summary extra windows do not trigger the claude extra-window lane
#[test]
fn claude_extra_windows_antigravity_summary_extra_windows_do_not_trigger_claude_lane() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    for used in [40.0, 55.0] {
        let snap = UsageSnapshot::new(
            None,
            None,
            None,
            Some(vec![NamedRateWindow::new(
                "antigravity-quota-summary-model-weekly",
                "Weekly",
                rate_window(used, Some(7 * 24 * 60), None),
            )]),
            None,
            Vec::new(),
            None,
            None,
            ts(START_SECS),
            None,
            Confidence::Exact,
        )
        .unwrap();
        let (events, _) = engine.handle_quota_warning_transitions(&claude, &snap, None, false);
        assert!(events.is_empty());
    }
}

// upstream: ClaudeExtraWindowQuotaWarningTests.swift:130 claude scoped weekly window refires after recovering above threshold
#[test]
fn claude_extra_windows_scoped_weekly_window_refires_after_recovering_above_threshold() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    let mut events = Vec::new();
    for used in [40.0, 55.0, 40.0, 55.0] {
        let (fired, _) = engine.handle_quota_warning_transitions(
            &claude,
            &claude_extra_window_snapshot(Some(used), None),
            None,
            false,
        );
        events.extend(fired);
    }
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|e| e.window_id.as_deref() == Some("claude-weekly-scoped-fable"))
    );
}

// upstream: ClaudeExtraWindowQuotaWarningTests.swift:160 claude extra-window fired state is pruned when a window disappears but others remain
#[test]
fn claude_extra_windows_fired_state_is_pruned_when_window_disappears_but_others_remain() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(40.0), Some(40.0)),
        None,
        false,
    );
    engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(55.0), Some(55.0)),
        None,
        false,
    );
    let fable_key = QuotaWarningStateKey {
        provider: claude.clone(),
        window: QuotaWarningWindow::Weekly,
        account_discriminator: None,
        window_id: Some("claude-weekly-scoped-fable".to_owned()),
    };
    let routines_key = QuotaWarningStateKey {
        provider: claude.clone(),
        window: QuotaWarningWindow::Weekly,
        account_discriminator: None,
        window_id: Some("claude-routines".to_owned()),
    };
    assert!(engine.quota_warning_states.contains_key(&fable_key));
    assert!(engine.quota_warning_states.contains_key(&routines_key));

    engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(None, Some(55.0)),
        None,
        false,
    );
    assert!(!engine.quota_warning_states.contains_key(&fable_key));
    assert!(engine.quota_warning_states.contains_key(&routines_key));
}

// upstream: ClaudeExtraWindowQuotaWarningTests.swift:201 claude extra-window reconciliation preserves sibling account state
#[test]
fn claude_extra_windows_reconciliation_preserves_sibling_account_state() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(40.0), None),
        Some("account-a"),
        false,
    );
    let (events1, _) = engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(55.0), None),
        Some("account-a"),
        false,
    );
    assert_eq!(events1.len(), 1);
    let account_a_fable_key = QuotaWarningStateKey {
        provider: claude.clone(),
        window: QuotaWarningWindow::Weekly,
        account_discriminator: Some("account-a".to_owned()),
        window_id: Some("claude-weekly-scoped-fable".to_owned()),
    };
    assert!(
        engine
            .quota_warning_states
            .contains_key(&account_a_fable_key)
    );

    engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(None, Some(40.0)),
        Some("account-b"),
        false,
    );
    assert!(
        engine
            .quota_warning_states
            .contains_key(&account_a_fable_key)
    );

    let (events2, _) = engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(55.0), None),
        Some("account-a"),
        false,
    );
    assert!(events2.is_empty());
}

// upstream: ClaudeExtraWindowQuotaWarningTests.swift:248 disabling weekly warnings clears all account-scoped claude extra-window state
#[test]
fn claude_extra_windows_disabling_weekly_warnings_clears_all_account_scoped_state() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    let account_ids = ["account-a", "account-b"];
    for account_id in account_ids {
        engine.handle_quota_warning_transitions(
            &claude,
            &claude_extra_window_snapshot(Some(40.0), Some(40.0)),
            Some(account_id),
            false,
        );
    }
    let seeded_keys: Vec<QuotaWarningStateKey> = account_ids
        .into_iter()
        .flat_map(|account_id| {
            [
                QuotaWarningStateKey {
                    provider: claude.clone(),
                    window: QuotaWarningWindow::Weekly,
                    account_discriminator: Some(account_id.to_owned()),
                    window_id: Some("claude-weekly-scoped-fable".to_owned()),
                },
                QuotaWarningStateKey {
                    provider: claude.clone(),
                    window: QuotaWarningWindow::Weekly,
                    account_discriminator: Some(account_id.to_owned()),
                    window_id: Some("claude-routines".to_owned()),
                },
            ]
        })
        .collect();
    assert!(
        seeded_keys
            .iter()
            .all(|k| engine.quota_warning_states.contains_key(k))
    );

    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);
    let empty_snap = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS + 60),
        None,
        Confidence::Exact,
    )
    .unwrap();
    engine.handle_quota_warning_transitions(&claude, &empty_snap, Some("account-a"), false);
    assert!(
        seeded_keys
            .iter()
            .all(|k| !engine.quota_warning_states.contains_key(k))
    );
}

// upstream: ClaudeExtraWindowQuotaWarningTests.swift:295 claude extra-window state survives a transient extras miss without re-posting
#[test]
fn claude_extra_windows_state_survives_transient_extras_miss_without_reposting() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(40.0), None),
        None,
        false,
    );
    let (events1, _) = engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(55.0), None),
        None,
        false,
    );
    assert_eq!(events1.len(), 1);
    let fable_key = QuotaWarningStateKey {
        provider: claude.clone(),
        window: QuotaWarningWindow::Weekly,
        account_discriminator: None,
        window_id: Some("claude-weekly-scoped-fable".to_owned()),
    };

    let empty_snap = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS + 60),
        None,
        Confidence::Exact,
    )
    .unwrap();
    engine.handle_quota_warning_transitions(&claude, &empty_snap, None, false);
    assert!(engine.quota_warning_states.contains_key(&fable_key));

    let (events2, _) = engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(Some(55.0), None),
        None,
        false,
    );
    assert!(events2.is_empty());
}

// upstream: UsageStoreAccountQuotaWarningTests.swift:24 ordinary refresh keeps selected token account warning episodes independent
#[test]
fn usage_store_ordinary_refresh_keeps_selected_token_account_warning_episodes_independent() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);

    let accounts = [
        ("00000000-0000-0000-0000-000000000001", "First"),
        ("00000000-0000-0000-0000-000000000002", "Second"),
    ];
    let mut fired_labels = Vec::new();
    let mut fired_thresholds = Vec::new();
    for (step, (account_idx, used_percent)) in [
        (0usize, 40.0),
        (1, 40.0),
        (0, 55.0),
        (1, 30.0),
        (0, 55.0),
        (1, 55.0),
    ]
    .into_iter()
    .enumerate()
    {
        let (acc_id, label) = accounts[account_idx];
        let snap = make_snapshot(
            "claude",
            used_percent,
            None,
            ts(1_780_000_000 + step as i64),
            label,
        );
        let disc = format!("token-account:{acc_id}");
        let (events, _) =
            engine.handle_quota_warning_transitions(&claude, &snap, Some(&disc), true);
        for event in events {
            fired_labels.push(label);
            fired_thresholds.push(event.threshold);
        }
    }
    assert_eq!(fired_labels, vec!["First", "Second"]);
    assert_eq!(fired_thresholds, vec![50, 50]);
}

// upstream: UsageStoreAccountQuotaWarningTests.swift:70 selected outcome keeps token account warning episodes independent
#[test]
fn usage_store_selected_outcome_keeps_token_account_warning_episodes_independent() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);

    let accounts = [
        ("00000000-0000-0000-0000-000000000001", "First"),
        ("00000000-0000-0000-0000-000000000002", "Second"),
    ];
    let mut fired_labels = Vec::new();
    let mut fired_thresholds = Vec::new();
    for (step, (account_idx, used_percent)) in [
        (0usize, 40.0),
        (1, 40.0),
        (0, 55.0),
        (1, 30.0),
        (0, 55.0),
        (1, 55.0),
    ]
    .into_iter()
    .enumerate()
    {
        let (acc_id, label) = accounts[account_idx];
        let snap = make_snapshot(
            "claude",
            used_percent,
            None,
            ts(1_780_000_000 + step as i64),
            label,
        );
        let disc = format!("token-account:{acc_id}");
        let (events, _) =
            engine.handle_quota_warning_transitions(&claude, &snap, Some(&disc), true);
        for event in events {
            fired_labels.push(label);
            fired_thresholds.push(event.threshold);
        }
    }
    assert_eq!(fired_labels, vec!["First", "Second"]);
    assert_eq!(fired_thresholds, vec![50, 50]);
}

// =========================================================================
// ClaudeCredentialQuotaWarningTests & ClaudeSyntheticPlaceholderNotificationTests
// =========================================================================

fn check_claude_identity_samples(
    samples: &[(Option<&str>, f64, Option<i64>)],
    expected_thresholds: &[u32],
    lane: &str,
) {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    let now = 1_900_000_000_i64;
    let mut fired_thresholds = Vec::new();
    for (index, &(identity, remaining, reset_offset)) in samples.iter().enumerate() {
        let obs = ClaudeOAuthActiveAccountObservation::Stable {
            identity: identity.map(str::to_owned),
        };
        let (quota_disc, _) = engine.warning_claude_account_discriminators("cli", &obs, None);
        let window = rate_window(
            100.0 - remaining,
            Some(if lane == "session" { 300 } else { 10080 }),
            reset_offset.map(|off| ts(now + off)),
        );
        let snap = UsageSnapshot::new(
            (lane == "session").then(|| window.clone()),
            (lane == "weekly").then(|| window.clone()),
            None,
            (lane == "scoped").then(|| {
                vec![NamedRateWindow::new(
                    "claude-weekly-scoped-fable",
                    "Fable",
                    window,
                )]
            }),
            None,
            Vec::new(),
            None,
            None,
            ts(now + index as i64),
            None,
            Confidence::Exact,
        )
        .unwrap();
        let (events, _) =
            engine.handle_quota_warning_transitions(&claude, &snap, quota_disc.as_deref(), true);
        fired_thresholds.extend(events.into_iter().map(|e| e.threshold));
    }
    assert_eq!(fired_thresholds, expected_thresholds);
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:25 weekly OAuth fallback does not notify or rearm the session lane
#[test]
fn claude_credential_weekly_oauth_fallback_does_not_notify_or_rearm_the_session_lane() {
    let claude = pid("claude");
    for weekly_only in [true, false] {
        let mut engine = NotificationPolicyEngine::default();
        engine.preferences.quota_warning_notifications_enabled = true;
        engine.preferences.quota_warning_thresholds = vec![50, 20];
        engine
            .preferences
            .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
        engine
            .preferences
            .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

        let session_usage: Vec<Option<f64>> = if weekly_only {
            vec![None]
        } else {
            vec![Some(51.0), None, Some(52.0), Some(81.0)]
        };
        let mut thresholds = Vec::new();
        let mut windows = Vec::new();
        for (idx, used) in session_usage.into_iter().enumerate() {
            let weekly_used = if weekly_only { 95.0 } else { 20.0 };
            let (primary, secondary) = match used {
                Some(s) => (
                    Some(rate_window(s, Some(300), None)),
                    Some(rate_window(weekly_used, Some(10080), None)),
                ),
                None => (
                    Some(rate_window(weekly_used, Some(10080), None)),
                    Some(rate_window(weekly_used, Some(10080), None)),
                ),
            };
            let snap = UsageSnapshot::new(
                primary,
                secondary,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
                ts(START_SECS + idx as i64),
                None,
                Confidence::Exact,
            )
            .unwrap();
            let (events, _) = engine.handle_quota_warning_transitions(
                &claude,
                &snap,
                Some("fixture-account-a"),
                false,
            );
            for e in events {
                thresholds.push(e.threshold);
                windows.push(e.window);
            }
        }
        assert_eq!(
            thresholds,
            if weekly_only { vec![20] } else { vec![50, 20] }
        );
        assert_eq!(
            windows,
            if weekly_only {
                vec![QuotaWarningWindow::Weekly]
            } else {
                vec![QuotaWarningWindow::Session, QuotaWarningWindow::Session]
            }
        );
    }
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:57 credential rewrites preserve known account threshold episodes
#[test]
fn claude_credential_rewrites_preserve_known_account_threshold_episodes() {
    let claude = pid("claude");
    for has_active_account in [true, false] {
        let mut engine = NotificationPolicyEngine::default();
        engine.preferences.quota_warning_notifications_enabled = true;
        engine.preferences.quota_warning_thresholds = vec![50, 20];
        engine
            .preferences
            .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
        engine
            .preferences
            .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);
        if has_active_account {
            engine.persist_claude_oauth_account_uuid_map(HashMap::from([(
                "fixture-oauth-owner".to_owned(),
                "fixture-account-a".to_owned(),
            )]));
        }

        let mut thresholds = Vec::new();
        for (idx, remaining) in [60.0, 49.0, 48.0, 47.0, 60.0, 49.0, 19.0]
            .into_iter()
            .enumerate()
        {
            let obs = if idx == 2 {
                ClaudeOAuthActiveAccountObservation::Changed
            } else {
                ClaudeOAuthActiveAccountObservation::Stable {
                    identity: has_active_account.then(|| "fixture-account-a".to_owned()),
                }
            };
            let (quota_disc, _) = engine.warning_claude_account_discriminators(
                "oauth",
                &obs,
                Some("fixture-oauth-owner"),
            );
            let snap = make_snapshot(
                "claude",
                100.0 - remaining,
                None,
                ts(START_SECS + idx as i64),
                "test@example.com",
            );
            let (events, _) = engine.handle_quota_warning_transitions(
                &claude,
                &snap,
                quota_disc.as_deref(),
                true,
            );
            thresholds.extend(events.into_iter().map(|e| e.threshold));
        }
        assert_eq!(thresholds, vec![50, 50, 20]);
    }
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:65 credential rewrites preserve unresolved account warnings
#[test]
fn claude_credential_rewrites_preserve_unresolved_account_warnings() {
    let claude = pid("claude");
    for strategy_kind in ["oauth", "cli"] {
        for recovers in [true, false] {
            let mut engine = NotificationPolicyEngine::default();
            engine.preferences.quota_warning_notifications_enabled = true;
            engine.preferences.quota_warning_thresholds = vec![50, 20];
            engine
                .preferences
                .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
            engine
                .preferences
                .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);

            let samples: &[f64] = if recovers {
                &[60.0, 49.0, 48.0, 47.0, 60.0, 49.0, 19.0]
            } else {
                &[60.0, 49.0, 48.0, 47.0, 19.0, 18.0]
            };
            let mut thresholds = Vec::new();
            for (idx, &remaining) in samples.iter().enumerate() {
                let obs = ClaudeOAuthActiveAccountObservation::Stable { identity: None };
                let (quota_disc, _) =
                    engine.warning_claude_account_discriminators(strategy_kind, &obs, None);
                let snap = make_snapshot(
                    "claude",
                    100.0 - remaining,
                    None,
                    ts(START_SECS + idx as i64),
                    "test@example.com",
                );
                let (events, _) = engine.handle_quota_warning_transitions(
                    &claude,
                    &snap,
                    quota_disc.as_deref(),
                    true,
                );
                thresholds.extend(events.into_iter().map(|e| e.threshold));
            }
            assert_eq!(
                thresholds,
                if recovers {
                    vec![50, 50, 20]
                } else {
                    vec![50, 20]
                }
            );
        }
    }
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:78 unresolved history migrates to a stable account without repeating a threshold
#[test]
fn claude_credential_unresolved_history_migrates_to_stable_account_without_repeating_threshold() {
    let claude = pid("claude");
    for strategy_kind in ["oauth", "cli"] {
        let mut engine = NotificationPolicyEngine::default();
        engine.preferences.quota_warning_notifications_enabled = true;
        engine.preferences.quota_warning_thresholds = vec![50, 20];
        engine
            .preferences
            .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
        engine
            .preferences
            .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);

        let unknown_key = QuotaWarningStateKey {
            provider: claude.clone(),
            window: QuotaWarningWindow::Session,
            account_discriminator: Some("claude-account:unknown".to_owned()),
            window_id: None,
        };
        let account_key = QuotaWarningStateKey {
            provider: claude.clone(),
            window: QuotaWarningWindow::Session,
            account_discriminator: Some("claude-account:account-a".to_owned()),
            window_id: None,
        };
        let now = 1_900_000_000_i64;
        engine.quota_warning_states.insert(
            account_key.clone(),
            QuotaWarningState {
                last_remaining: Some(60.0),
                observed_at: ts(now - 1),
                ..QuotaWarningState::default()
            },
        );
        let mut thresholds = Vec::new();
        for (index, remaining) in [49.0, 48.0, 47.0, 19.0].into_iter().enumerate() {
            let obs = if index < 2 {
                ClaudeOAuthActiveAccountObservation::Changed
            } else {
                ClaudeOAuthActiveAccountObservation::Stable {
                    identity: Some("account-a".to_owned()),
                }
            };
            let (quota_disc, _) =
                engine.warning_claude_account_discriminators(strategy_kind, &obs, None);
            let snap = make_snapshot(
                "claude",
                100.0 - remaining,
                None,
                ts(now + index as i64),
                "test@example.com",
            );
            let (events, _) = engine.handle_quota_warning_transitions(
                &claude,
                &snap,
                quota_disc.as_deref(),
                true,
            );
            thresholds.extend(events.into_iter().map(|e| e.threshold));
            assert_eq!(thresholds, if index == 3 { vec![50, 20] } else { vec![50] });
            assert_eq!(
                engine.quota_warning_states.contains_key(&unknown_key),
                index < 2
            );
        }
        assert!(
            engine.quota_warning_states[&account_key]
                .fired_thresholds
                .contains(&50)
        );
        assert!(
            engine.quota_warning_states[&account_key]
                .fired_thresholds
                .contains(&20)
        );
    }
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:124 verified OAuth owner and active identity share threshold episodes across CLI fallback
#[test]
fn claude_credential_verified_oauth_owner_and_active_identity_share_threshold_episodes_across_cli_fallback()
 {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);
    engine.persist_claude_oauth_account_uuid_map(HashMap::from([
        ("owner-a".to_owned(), "account-a".to_owned()),
        ("rotated-owner-a".to_owned(), "account-a".to_owned()),
    ]));

    let samples: Vec<(f64, &str, ClaudeOAuthActiveAccountObservation, Option<&str>)> = vec![
        (
            60.0,
            "oauth",
            ClaudeOAuthActiveAccountObservation::Stable { identity: None },
            Some("owner-a"),
        ),
        (
            49.0,
            "oauth",
            ClaudeOAuthActiveAccountObservation::Stable { identity: None },
            Some("owner-a"),
        ),
        (
            48.0,
            "oauth",
            ClaudeOAuthActiveAccountObservation::Stable {
                identity: Some("account-a".to_owned()),
            },
            Some("owner-a"),
        ),
        (
            47.0,
            "cli",
            ClaudeOAuthActiveAccountObservation::Stable {
                identity: Some("account-a".to_owned()),
            },
            None,
        ),
        (
            46.0,
            "cli",
            ClaudeOAuthActiveAccountObservation::Stable { identity: None },
            None,
        ),
        (
            45.0,
            "oauth",
            ClaudeOAuthActiveAccountObservation::Changed,
            Some("owner-a"),
        ),
        (
            44.0,
            "oauth",
            ClaudeOAuthActiveAccountObservation::Stable { identity: None },
            Some("owner-a"),
        ),
        (
            43.0,
            "oauth",
            ClaudeOAuthActiveAccountObservation::Stable {
                identity: Some("account-a".to_owned()),
            },
            Some("rotated-owner-a"),
        ),
        (
            19.0,
            "cli",
            ClaudeOAuthActiveAccountObservation::Stable {
                identity: Some("account-a".to_owned()),
            },
            None,
        ),
        (
            60.0,
            "cli",
            ClaudeOAuthActiveAccountObservation::Stable {
                identity: Some("account-a".to_owned()),
            },
            None,
        ),
        (
            49.0,
            "oauth",
            ClaudeOAuthActiveAccountObservation::Stable {
                identity: Some("account-a".to_owned()),
            },
            Some("rotated-owner-a"),
        ),
        (
            48.0,
            "oauth",
            ClaudeOAuthActiveAccountObservation::Stable {
                identity: Some("account-b".to_owned()),
            },
            Some("owner-b"),
        ),
        (
            47.0,
            "cli",
            ClaudeOAuthActiveAccountObservation::Stable {
                identity: Some("account-a".to_owned()),
            },
            None,
        ),
    ];

    let mut fired_thresholds = Vec::new();
    for (idx, (remaining, kind, obs, owner)) in samples.into_iter().enumerate() {
        let (quota_disc, _) = engine.warning_claude_account_discriminators(kind, &obs, owner);
        if let Some(disc) = quota_disc {
            let snap = make_snapshot(
                "claude",
                100.0 - remaining,
                None,
                ts(START_SECS + idx as i64),
                "test@example.com",
            );
            let (events, _) =
                engine.handle_quota_warning_transitions(&claude, &snap, Some(&disc), false);
            fired_thresholds.extend(events.into_iter().map(|e| e.threshold));
        }
    }
    assert_eq!(fired_thresholds, vec![50, 50, 20, 50, 50]);
    assert_eq!(
        engine
            .warning_claude_account_discriminators(
                "oauth",
                &ClaudeOAuthActiveAccountObservation::Stable { identity: None },
                Some("owner-a")
            )
            .0
            .as_deref(),
        Some("claude-account:account-a")
    );
    assert_eq!(
        engine
            .warning_claude_account_discriminators(
                "oauth",
                &ClaudeOAuthActiveAccountObservation::Stable {
                    identity: Some("account-b".to_owned())
                },
                Some("owner-a")
            )
            .0,
        None
    );
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:183 verified owner migration preserves the latest recovery and other warning lanes
#[test]
fn claude_credential_verified_owner_migration_preserves_latest_recovery_and_other_warning_lanes() {
    let claude = pid("claude");
    for owner_is_newest in [true, false] {
        let mut engine = NotificationPolicyEngine::default();
        engine.preferences.quota_warning_notifications_enabled = true;
        engine.preferences.quota_warning_thresholds = vec![50, 20];
        engine
            .preferences
            .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);

        let owner = "claude-oauth-owner:owner-a";
        let account = "claude-account:account-a";
        let now = 1_900_000_000_i64;
        let owner_key = QuotaWarningStateKey {
            provider: claude.clone(),
            window: QuotaWarningWindow::Session,
            account_discriminator: Some(owner.to_owned()),
            window_id: None,
        };
        let account_key = QuotaWarningStateKey {
            provider: claude.clone(),
            window: QuotaWarningWindow::Session,
            account_discriminator: Some(account.to_owned()),
            window_id: None,
        };
        let mut older = QuotaWarningState {
            last_remaining: Some(49.0),
            observed_at: ts(now),
            ..QuotaWarningState::default()
        };
        older.fired_thresholds.insert(50);
        let newer = QuotaWarningState {
            last_remaining: Some(60.0),
            observed_at: ts(now + 1),
            ..QuotaWarningState::default()
        };
        engine.quota_warning_states.insert(
            owner_key.clone(),
            if owner_is_newest {
                newer.clone()
            } else {
                older.clone()
            },
        );
        engine.quota_warning_states.insert(
            account_key.clone(),
            if owner_is_newest { older } else { newer },
        );
        engine.persist_claude_oauth_account_uuid_map(HashMap::from([(
            "owner-a".to_owned(),
            "account-a".to_owned(),
        )]));

        let (quota, source) = engine.warning_claude_account_discriminators(
            "oauth",
            &ClaudeOAuthActiveAccountObservation::Stable { identity: None },
            Some("owner-a"),
        );
        assert_eq!(quota.as_deref(), Some(account));
        assert_eq!(source.as_deref(), Some(owner));
        assert!(!engine.quota_warning_states.contains_key(&owner_key));
        assert!(
            engine.quota_warning_states[&account_key]
                .fired_thresholds
                .is_empty()
        );
        assert_eq!(
            engine.quota_warning_states[&account_key].last_remaining,
            Some(60.0)
        );

        let snap = make_snapshot("claude", 51.0, None, ts(now + 2), "test@example.com");
        let (events, _) =
            engine.handle_quota_warning_transitions(&claude, &snap, quota.as_deref(), false);
        assert_eq!(
            events.into_iter().map(|e| e.threshold).collect::<Vec<_>>(),
            vec![50]
        );
    }
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:253 repeated CLI identity gaps preserve threshold history
#[test]
fn claude_credential_repeated_cli_identity_gaps_preserve_threshold_history() {
    for has_reset in [true, false] {
        for lane in ["session", "weekly", "scoped"] {
            let reset = has_reset.then_some(3600_i64);
            let samples: Vec<(Option<&str>, f64, Option<i64>)> =
                [49.0, 48.0, 47.0, 46.0, 45.0, 44.0]
                    .into_iter()
                    .enumerate()
                    .map(|(idx, rem)| ((idx % 2 == 0).then_some("account-a"), rem, reset))
                    .collect();
            let expected: &[u32] = if has_reset { &[50] } else { &[50, 50] };
            check_claude_identity_samples(&samples, expected, lane);
        }
    }
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:263 later thresholds do not repeat across either identity key
#[test]
fn claude_credential_later_thresholds_do_not_repeat_across_either_identity_key() {
    for has_reset in [true, false] {
        for crossing_is_known in [true, false] {
            let remaining = if crossing_is_known {
                [49.0, 48.0, 19.0, 18.0, 17.0, 16.0]
            } else {
                [49.0, 48.0, 47.0, 19.0, 18.0, 17.0]
            };
            let reset = has_reset.then_some(3600_i64);
            let samples: Vec<(Option<&str>, f64, Option<i64>)> = remaining
                .into_iter()
                .enumerate()
                .map(|(idx, val)| ((idx % 2 == 0).then_some("account-a"), val, reset))
                .collect();
            let expected: &[u32] = if has_reset { &[50, 20] } else { &[50, 50, 20] };
            check_claude_identity_samples(&samples, expected, "session");
        }
    }
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:273 discontinuous identity gaps start one independent fallback episode
#[test]
fn claude_credential_discontinuous_identity_gaps_start_one_independent_fallback_episode() {
    for discontinuity in ["reset", "increase", "missing"] {
        let reset = match discontinuity {
            "missing" => None,
            "reset" => Some(7200_i64),
            _ => Some(3600_i64),
        };
        let samples = [
            (Some("account-a"), 40.0, Some(3600)),
            (
                None,
                if discontinuity == "increase" {
                    49.0
                } else {
                    39.0
                },
                reset,
            ),
            (Some("account-a"), 38.0, Some(3600)),
            (
                None,
                if discontinuity == "increase" {
                    49.0
                } else {
                    37.0
                },
                reset,
            ),
            (Some("account-a"), 36.0, Some(3600)),
            (
                None,
                if discontinuity == "increase" {
                    49.0
                } else {
                    35.0
                },
                reset,
            ),
        ];
        check_claude_identity_samples(&samples, &[50, 50], "session");
    }
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:288 identity gaps follow the most recently known account without merging known accounts
#[test]
fn claude_credential_identity_gaps_follow_most_recently_known_account_without_merging_known_accounts()
 {
    let samples = [
        (Some("account-a"), 49.0, Some(3600)),
        (Some("account-b"), 48.0, Some(3600)),
        (None, 47.0, Some(3600)),
        (Some("account-a"), 46.0, Some(3600)),
        (None, 45.0, Some(3600)),
        (Some("account-b"), 44.0, Some(3600)),
        (None, 43.0, Some(3600)),
    ];
    check_claude_identity_samples(&samples, &[50, 50], "session");
}

// upstream: ClaudeCredentialQuotaWarningTests.swift:303 initial unresolved history survives repeated resolution and quota recovery
#[test]
fn claude_credential_initial_unresolved_history_survives_repeated_resolution_and_quota_recovery() {
    for has_reset in [true, false] {
        let reset = has_reset.then_some(3600_i64);
        let samples = [
            (None, 49.0, reset),
            (Some("account-a"), 48.0, reset),
            (None, 47.0, reset),
            (Some("account-a"), 19.0, reset),
            (None, 18.0, reset),
            (Some("account-a"), 60.0, reset),
            (None, 49.0, reset),
            (Some("account-a"), 48.0, reset),
            (None, 47.0, reset),
        ];
        check_claude_identity_samples(&samples, &[50, 20, 50], "session");
    }
}

// =========================================================================
// ClaudeSyntheticPlaceholderNotificationTests
// =========================================================================

fn claude_placeholder_snapshot(
    session_used: f64,
    weekly_used: f64,
    session_reset: Option<Timestamp>,
    weekly_reset: Option<Timestamp>,
    session_is_placeholder: bool,
    offset_secs: i64,
) -> UsageSnapshot {
    let primary = if session_is_placeholder {
        placeholder_window(session_used, Some(300), session_reset)
    } else {
        rate_window(session_used, Some(300), session_reset)
    };
    UsageSnapshot::new(
        Some(primary),
        Some(rate_window(weekly_used, Some(10080), weekly_reset)),
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS + offset_secs),
        Some(ProviderIdentitySnapshot {
            provider_id: Some(pid("claude")),
            account_email: Some("placeholder@example.com".to_owned()),
            account_organization: None,
            login_method: Some("web".to_owned()),
            account_id: None,
        }),
        Confidence::Exact,
    )
    .unwrap()
}

// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:11 placeholder preserves depleted state without a reset boundary
#[test]
fn claude_placeholder_preserves_depleted_state_without_a_reset_boundary() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    for (used, is_placeholder) in [(20.0, false), (100.0, false), (0.0, true), (100.0, false)] {
        let snap = claude_placeholder_snapshot(used, 20.0, None, None, is_placeholder, 0);
        let (post, _, t) = engine.handle_session_quota_transition(
            &claude,
            &snap,
            None,
            None,
            false,
            snap.updated_at,
        );
        if post.is_some() {
            transitions.push(t);
        }
    }
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(engine.session_quota_states[&claude].remaining, 0.0);
}

// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:29 placeholder stays non authoritative after the prior boundary elapses
#[test]
fn claude_placeholder_stays_non_authoritative_after_the_prior_boundary_elapses() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    let boundary = Some(ts(START_SECS + 5 * 60));
    let mut transitions = Vec::new();

    for (used, is_placeholder, offset) in [
        (20.0, false, 0),
        (100.0, false, 60),
        (0.0, true, 6 * 60),
        (100.0, false, 7 * 60),
    ] {
        let snap = claude_placeholder_snapshot(used, 20.0, boundary, None, is_placeholder, offset);
        let (post, _, t) = engine.handle_session_quota_transition(
            &claude,
            &snap,
            None,
            None,
            false,
            snap.updated_at,
        );
        if post.is_some() {
            transitions.push(t);
        }
    }
    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(engine.session_quota_states[&claude].remaining, 0.0);
}

// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:58 placeholder cannot rearm depletion while notifications are disabled
#[test]
fn claude_placeholder_cannot_rearm_depletion_while_notifications_are_disabled() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    for used in [20.0, 100.0] {
        let snap = claude_placeholder_snapshot(used, 20.0, None, None, false, 0);
        let (post, _, t) = engine.handle_session_quota_transition(
            &claude,
            &snap,
            None,
            None,
            false,
            snap.updated_at,
        );
        if post.is_some() {
            transitions.push(t);
        }
    }

    engine.preferences.session_quota_notifications_enabled = false;
    let ph = claude_placeholder_snapshot(0.0, 20.0, None, None, true, 0);
    engine.handle_session_quota_transition(&claude, &ph, None, None, false, ph.updated_at);
    engine.preferences.session_quota_notifications_enabled = true;

    let dep = claude_placeholder_snapshot(100.0, 20.0, None, None, false, 0);
    let (post, _, t) =
        engine.handle_session_quota_transition(&claude, &dep, None, None, false, dep.updated_at);
    if post.is_some() {
        transitions.push(t);
    }

    assert_eq!(transitions, vec![SessionQuotaTransition::Depleted]);
    assert_eq!(engine.session_quota_states[&claude].remaining, 0.0);
}

// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:79 real zero usage remains an authoritative restore
#[test]
fn claude_placeholder_real_zero_usage_remains_an_authoritative_restore() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();

    for used in [20.0, 100.0, 0.0] {
        let snap = claude_placeholder_snapshot(used, 20.0, None, None, false, 0);
        let (post, _, t) = engine.handle_session_quota_transition(
            &claude,
            &snap,
            None,
            None,
            false,
            snap.updated_at,
        );
        if post.is_some() {
            transitions.push(t);
        }
    }
    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
    assert_eq!(engine.session_quota_states[&claude].remaining, 100.0);
}

// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:94 placeholder preserves threshold state while weekly warnings continue
#[test]
fn claude_placeholder_preserves_threshold_state_while_weekly_warnings_continue() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);
    let session_reset = Some(ts(START_SECS + 2 * 3600));
    let weekly_reset = Some(ts(START_SECS + 2 * 86_400));

    let mut windows = Vec::new();
    let mut thresholds = Vec::new();
    for (s_used, w_used, is_ph, offset) in [
        (40.0, 40.0, false, 0),
        (60.0, 40.0, false, 60),
        (0.0, 60.0, true, 120),
        (60.0, 60.0, false, 180),
    ] {
        let snap =
            claude_placeholder_snapshot(s_used, w_used, session_reset, weekly_reset, is_ph, offset);
        let (events, _) = engine.handle_quota_warning_transitions(&claude, &snap, None, false);
        for e in events {
            windows.push(e.window);
            thresholds.push(e.threshold);
        }
    }
    assert_eq!(
        windows,
        vec![QuotaWarningWindow::Session, QuotaWarningWindow::Weekly]
    );
    assert_eq!(thresholds, vec![50, 50]);
}

// =========================================================================
// CredentialNotificationTests
// =========================================================================

// upstream: CredentialNotificationTests.swift:8 episodes survive network quota and cached data and recover only for the same account
#[test]
fn credential_notification_episodes_survive_network_quota_and_cached_data_and_recover_only_for_same_account()
 {
    let kimi = pid("kimi");
    let claude = pid("claude");
    let expired = CredentialFetchOutcome::Failure(ProviderCredentialError::TokenExpired);
    let mut tracker = CredentialNotificationTracker::new();
    let key = CredentialNotificationKey::new(kimi.clone(), "first");

    assert_eq!(
        tracker.handle_outcome(&kimi, "first", &expired, false),
        CredentialNotificationAction::None
    );
    assert!(tracker.episodes.is_empty());

    let action = tracker.handle_outcome(&kimi, "first", &expired, true);
    assert!(matches!(action, CredentialNotificationAction::Post(_)));
    let episode = tracker.episodes[&key];

    assert_eq!(
        tracker.handle_outcome(&kimi, "first", &expired, true),
        CredentialNotificationAction::None
    );
    assert_eq!(
        tracker.handle_outcome(
            &kimi,
            "first",
            &CredentialFetchOutcome::Failure(ProviderCredentialError::Classified {
                kind: ProviderFetchErrorKind::Network,
                message: "timed out".to_owned(),
            }),
            true
        ),
        CredentialNotificationAction::None
    );
    assert_eq!(
        tracker.handle_outcome(
            &kimi,
            "first",
            &CredentialFetchOutcome::Failure(ProviderCredentialError::Classified {
                kind: ProviderFetchErrorKind::RateLimited,
                message: "Token quota exhausted".to_owned(),
            }),
            true
        ),
        CredentialNotificationAction::None
    );
    assert_eq!(
        tracker.handle_outcome(
            &kimi,
            "first",
            &CredentialFetchOutcome::Success {
                source_label: "cache".to_owned(),
                is_local_probe: false,
                diagnostic: None,
            },
            true
        ),
        CredentialNotificationAction::None
    );
    assert_eq!(
        tracker.handle_outcome(
            &kimi,
            "first",
            &CredentialFetchOutcome::Success {
                source_label: "oauth".to_owned(),
                is_local_probe: false,
                diagnostic: Some("offline fallback".to_owned()),
            },
            true
        ),
        CredentialNotificationAction::None
    );
    assert_eq!(
        tracker.handle_outcome(
            &kimi,
            "second",
            &CredentialFetchOutcome::fresh_success("oauth"),
            true
        ),
        CredentialNotificationAction::None
    );
    assert_eq!(tracker.episodes.get(&key), Some(&episode));

    tracker.handle_outcome(&kimi, "second", &expired, true);
    tracker.handle_outcome(&claude, "first", &expired, true);
    assert_eq!(tracker.episodes.len(), 3);

    let recovered = tracker.handle_outcome(
        &kimi,
        "first",
        &CredentialFetchOutcome::fresh_success("oauth"),
        true,
    );
    assert!(matches!(
        recovered,
        CredentialNotificationAction::Remove { .. }
    ));
    assert!(!tracker.episodes.contains_key(&key));

    tracker.handle_outcome(&kimi, "first", &expired, true);
    assert_ne!(tracker.episodes.get(&key), Some(&episode));
    assert_eq!(tracker.episodes.len(), 3);
}

// upstream: CredentialNotificationTests.swift:39 turning notifications off does not forget an unresolved episode
#[test]
fn credential_notification_turning_notifications_off_does_not_forget_an_unresolved_episode() {
    let claude = pid("claude");
    let expired = CredentialFetchOutcome::Failure(ProviderCredentialError::TokenExpired);
    let mut tracker = CredentialNotificationTracker::new();

    tracker.handle_outcome(&claude, "default", &expired, true);
    let episodes = tracker.episodes.clone();
    assert_eq!(episodes.len(), 1);

    tracker.handle_outcome(&claude, "default", &expired, false);
    tracker.handle_outcome(&claude, "default", &expired, true);
    assert_eq!(tracker.episodes, episodes);

    tracker.stopped = true;
    tracker.episodes.clear();
    tracker.handle_outcome(&claude, "default", &expired, true);
    assert!(tracker.episodes.is_empty());
}

// upstream: CredentialNotificationTests.swift:58 classified provider failures are authoritative
#[test]
fn credential_notification_classified_provider_failures_are_authoritative() {
    for kind in [
        ProviderFetchErrorKind::AuthenticationExpired,
        ProviderFetchErrorKind::MissingCredential,
        ProviderFetchErrorKind::RateLimited,
        ProviderFetchErrorKind::Network,
        ProviderFetchErrorKind::PermissionDenied,
        ProviderFetchErrorKind::ParseFailed,
        ProviderFetchErrorKind::Other,
    ] {
        let err = ProviderCredentialError::Classified {
            kind,
            message: "token expired, login required".to_owned(),
        };
        let expected = matches!(
            kind,
            ProviderFetchErrorKind::AuthenticationExpired
                | ProviderFetchErrorKind::MissingCredential
        );
        assert_eq!(
            ProviderCredentialFailure::is_authentication_failure(&err),
            expected
        );
    }
}

// upstream: CredentialNotificationTests.swift:65 native credential errors exclude quota network permission and parse failures
#[test]
fn credential_notification_native_credential_errors_exclude_quota_network_permission_and_parse_failures()
 {
    let auth = [
        ProviderCredentialError::Unauthorized,
        ProviderCredentialError::TokenExpired,
        ProviderCredentialError::TokenRevoked,
        ProviderCredentialError::TokenReused,
        ProviderCredentialError::NativeRefreshRequired,
        ProviderCredentialError::ReadOnlySource,
        ProviderCredentialError::NotLoggedIn,
        ProviderCredentialError::SessionExpired,
        ProviderCredentialError::NoSessionCookie,
        ProviderCredentialError::ClaudeOAuthRefreshFailed("invalid_grant".to_owned()),
        ProviderCredentialError::ClaudeLegacyOAuthFailed(
            "Claude OAuth request unauthorized.".to_owned(),
        ),
    ];
    assert!(
        auth.iter()
            .all(ProviderCredentialFailure::is_authentication_failure)
    );

    let other = [
        ProviderCredentialError::KeychainAccessRevoked,
        ProviderCredentialError::ClaudeOAuthRefreshFailed("network offline".to_owned()),
        ProviderCredentialError::Classified {
            kind: ProviderFetchErrorKind::RateLimited,
            message: "You've reached your token usage limit".to_owned(),
        },
        ProviderCredentialError::Classified {
            kind: ProviderFetchErrorKind::Network,
            message: "token expired".to_owned(),
        },
        ProviderCredentialError::Classified {
            kind: ProviderFetchErrorKind::PermissionDenied,
            message: "login required".to_owned(),
        },
        ProviderCredentialError::Classified {
            kind: ProviderFetchErrorKind::ParseFailed,
            message: "invalid json".to_owned(),
        },
    ];
    assert!(
        other
            .iter()
            .all(|e| !ProviderCredentialFailure::is_authentication_failure(e))
    );
}

// upstream: CredentialNotificationTests.swift:93 ordinary and selected account outcomes share account-specific episodes
#[test]
fn credential_notification_ordinary_and_selected_account_outcomes_share_account_specific_episodes()
{
    let claude = pid("claude");
    let expired = CredentialFetchOutcome::Failure(ProviderCredentialError::Classified {
        kind: ProviderFetchErrorKind::AuthenticationExpired,
        message: "synthetic expired credential".to_owned(),
    });
    for _selected in [false, true] {
        let mut tracker = CredentialNotificationTracker::new();
        let accounts = ["token-account:first", "token-account:second"];
        for idx in [0usize, 1, 0] {
            tracker.handle_outcome(&claude, accounts[idx], &expired, true);
        }
        assert_eq!(tracker.episodes.len(), 2);
        let recorded: HashSet<String> =
            tracker.episodes.keys().map(|k| k.account.clone()).collect();
        assert_eq!(
            recorded,
            HashSet::from([
                "token-account:first".to_owned(),
                "token-account:second".to_owned()
            ])
        );
    }
}

// upstream: CredentialNotificationTests.swift:128 shared delivery retires notifications when consent or lifecycle changes
#[test]
fn credential_notification_shared_delivery_retires_notifications_when_consent_or_lifecycle_changes()
{
    for retire_at in ["before", "authorization", "submission", "denied", "never"] {
        let current = std::cell::Cell::new(retire_at != "before");
        let mut authorized = 0;
        let mut submitted = 0;
        let mut removed = 0;
        deliver_if_current(
            || {
                authorized += 1;
                if retire_at == "authorization" {
                    current.set(false);
                }
                retire_at != "denied"
            },
            || {
                submitted += 1;
                if retire_at == "submission" {
                    current.set(false);
                }
            },
            || {
                removed += 1;
            },
            || current.get(),
        );
        assert_eq!(authorized, if retire_at == "before" { 0 } else { 1 });
        assert_eq!(
            submitted,
            if retire_at == "submission" || retire_at == "never" {
                1
            } else {
                0
            }
        );
        assert_eq!(removed, if retire_at == "submission" { 1 } else { 0 });
    }
}

// upstream: CredentialNotificationTests.swift:151 failed delivery retries while delivered episodes retire on recovery or shutdown
#[test]
fn credential_notification_failed_delivery_retries_while_delivered_episodes_retire_on_recovery_or_shutdown()
 {
    let claude = pid("claude");
    let expired = CredentialFetchOutcome::Failure(ProviderCredentialError::TokenExpired);
    let mut tracker = CredentialNotificationTracker::new();

    let CredentialNotificationAction::Post(first_post) =
        tracker.handle_outcome(&claude, "default", &expired, true)
    else {
        panic!("expected post");
    };
    let first_id = first_post.identifier.unwrap();
    tracker.complete_delivery(&claude, "default", &first_id, false);
    assert!(tracker.episodes.is_empty());

    let CredentialNotificationAction::Post(second_post) =
        tracker.handle_outcome(&claude, "default", &expired, true)
    else {
        panic!("expected retry post");
    };
    let second_id = second_post.identifier.unwrap();
    // Late callback for first_id must not clear the replacement episode
    tracker.complete_delivery(&claude, "default", &first_id, false);
    tracker.complete_delivery(&claude, "default", &second_id, true);
    assert_eq!(
        tracker.handle_outcome(&claude, "default", &expired, true),
        CredentialNotificationAction::None
    );

    let CredentialNotificationAction::Remove { identifier } = tracker.handle_outcome(
        &claude,
        "default",
        &CredentialFetchOutcome::fresh_success("oauth"),
        true,
    ) else {
        panic!("expected remove");
    };
    assert_eq!(identifier, second_id);

    let CredentialNotificationAction::Post(third_post) =
        tracker.handle_outcome(&claude, "default", &expired, true)
    else {
        panic!("expected third post");
    };
    let third_id = third_post.identifier.unwrap();
    tracker.complete_delivery(&claude, "default", &third_id, true);
    assert_eq!(tracker.retire_provider(Some(&claude)), vec![third_id]);
    assert!(tracker.episodes.is_empty());

    let CredentialNotificationAction::Post(fourth_post) =
        tracker.handle_outcome(&claude, "default", &expired, true)
    else {
        panic!("expected fourth post");
    };
    let fourth_id = fourth_post.identifier.unwrap();
    tracker.complete_delivery(&claude, "default", &fourth_id, true);
    assert_eq!(tracker.retire_provider(None), vec![fourth_id]);
}

// upstream: CredentialNotificationTests.swift:187 Claude identity gaps and later identity resolution preserve independent episodes
#[test]
fn credential_notification_claude_identity_gaps_and_later_identity_resolution_preserve_independent_episodes()
 {
    let claude = pid("claude");
    let expired = CredentialFetchOutcome::Failure(ProviderCredentialError::Unauthorized);
    let mut tracker = CredentialNotificationTracker::new();

    let first = tracker.claude_credential_notification_scope(None, Some("first-file"));
    let second = tracker.claude_credential_notification_scope(None, Some("second-file"));
    assert_ne!(first, second);

    tracker.handle_outcome(&claude, &first, &expired, true);
    tracker.handle_outcome(&claude, &second, &expired, true);
    assert_eq!(tracker.episodes.len(), 2);

    assert_eq!(
        tracker.claude_credential_notification_scope(Some("first-account"), Some("first-file")),
        first
    );
    assert_eq!(
        tracker.claude_credential_notification_scope(Some("first-account"), None),
        first
    );
    assert_eq!(
        tracker.claude_credential_notification_scope(Some("first-account"), Some("rotated-file")),
        first
    );
    assert_eq!(
        tracker.claude_credential_notification_scope(None, Some("rotated-file")),
        first
    );
    let switched =
        tracker.claude_credential_notification_scope(Some("second-account"), Some("rotated-file"));
    assert_ne!(switched, first);

    tracker.handle_outcome(
        &claude,
        &switched,
        &CredentialFetchOutcome::fresh_success("oauth"),
        true,
    );
    assert_eq!(tracker.episodes.len(), 2);
    tracker.handle_outcome(
        &claude,
        &first,
        &CredentialFetchOutcome::fresh_success("oauth"),
        true,
    );
    assert_eq!(tracker.episodes.len(), 1);
}

// upstream: CredentialNotificationTests.swift:212 disabling a provider retires only its delivered credential alerts
#[test]
fn credential_notification_disabling_a_provider_retires_only_its_delivered_credential_alerts() {
    let claude = pid("claude");
    let codex = pid("codex");
    let expired = CredentialFetchOutcome::Failure(ProviderCredentialError::Classified {
        kind: ProviderFetchErrorKind::AuthenticationExpired,
        message: "Synthetic expiry".to_owned(),
    });
    let mut tracker = CredentialNotificationTracker::new();

    let CredentialNotificationAction::Post(claude_post) =
        tracker.handle_outcome(&claude, "default", &expired, true)
    else {
        panic!("expected claude post");
    };
    let CredentialNotificationAction::Post(codex_post) =
        tracker.handle_outcome(&codex, "default", &expired, true)
    else {
        panic!("expected codex post");
    };

    let removed_claude = tracker.retire_disabled_providers(&HashSet::from([codex.clone()]));
    assert_eq!(removed_claude, vec![claude_post.identifier.unwrap()]);
    assert!(
        tracker
            .retire_disabled_providers(&HashSet::from([codex.clone()]))
            .is_empty()
    );
    let remaining_providers: HashSet<ProviderId> = tracker
        .episodes
        .keys()
        .map(|k| k.provider.clone())
        .collect();
    assert_eq!(remaining_providers, HashSet::from([codex]));

    let removed_codex = tracker.retire_disabled_providers(&HashSet::new());
    assert_eq!(removed_codex, vec![codex_post.identifier.unwrap()]);
    assert!(tracker.episodes.is_empty());
}

// =========================================================================
// UsageStoreSessionQuotaTransitionTests
// =========================================================================

fn antigravity_quota_summary_snapshot(session_used: f64, weekly_used: f64) -> UsageSnapshot {
    UsageSnapshot::new(
        Some(rate_window(weekly_used, Some(7 * 24 * 60), None)),
        Some(rate_window(session_used, Some(5 * 60), None)),
        None,
        Some(vec![
            NamedRateWindow::new(
                "antigravity-quota-summary-gemini-5h",
                "Gemini Models Five Hour Limit",
                rate_window(session_used, Some(5 * 60), None),
            ),
            NamedRateWindow::new(
                "antigravity-quota-summary-gemini-weekly",
                "Gemini Models Weekly Limit",
                rate_window(weekly_used, Some(7 * 24 * 60), None),
            ),
        ]),
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS),
        None,
        Confidence::Exact,
    )
    .unwrap()
}

fn antigravity_legacy_snapshot(gemini_used: f64, claude_used: f64) -> UsageSnapshot {
    UsageSnapshot::new(
        Some(rate_window(gemini_used, None, None)),
        Some(rate_window(claude_used, None, None)),
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS + 60),
        None,
        Confidence::Exact,
    )
    .unwrap()
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:103 claude weekly primary fallback does not emit session quota notifications
#[test]
fn usage_store_claude_weekly_primary_fallback_does_not_emit_session_quota_notifications() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    let baseline = UsageSnapshot::new(
        Some(rate_window(20.0, Some(7 * 24 * 60), None)),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS),
        None,
        Confidence::Exact,
    )
    .unwrap();
    let depleted = UsageSnapshot::new(
        Some(rate_window(100.0, Some(7 * 24 * 60), None)),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS + 60),
        None,
        Confidence::Exact,
    )
    .unwrap();
    assert!(
        engine
            .handle_session_quota_transition(&claude, &baseline, None, None, false, ts(START_SECS))
            .0
            .is_none()
    );
    assert!(
        engine
            .handle_session_quota_transition(
                &claude,
                &depleted,
                None,
                None,
                false,
                ts(START_SECS + 60)
            )
            .0
            .is_none()
    );
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:132 claude spend limit fallback does not emit session or quota warning notifications
#[test]
fn usage_store_claude_spend_limit_fallback_does_not_emit_session_or_quota_warning_notifications() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];

    let spend_only = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        Some({
            let mut cost = ProviderCostSnapshot::new(434.43, 600.0, "USD", ts(START_SECS));
            cost.period = Some("Spend limit".to_owned());
            cost
        }),
        Vec::new(),
        None,
        None,
        ts(START_SECS),
        None,
        Confidence::Exact,
    )
    .unwrap();
    assert!(
        engine
            .handle_session_quota_transition(
                &claude,
                &spend_only,
                None,
                None,
                false,
                ts(START_SECS)
            )
            .0
            .is_none()
    );
    assert!(
        engine
            .handle_quota_warning_transitions(&claude, &spend_only, None, false)
            .0
            .is_empty()
    );
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:211 claude five hour primary still emits session quota notifications
#[test]
fn usage_store_claude_five_hour_primary_still_emits_session_quota_notifications() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    let baseline = UsageSnapshot::new(
        Some(rate_window(20.0, Some(5 * 60), None)),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS),
        None,
        Confidence::Exact,
    )
    .unwrap();
    let depleted = UsageSnapshot::new(
        Some(rate_window(100.0, Some(5 * 60), None)),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        ts(START_SECS + 60),
        None,
        Confidence::Exact,
    )
    .unwrap();
    assert!(
        engine
            .handle_session_quota_transition(&claude, &baseline, None, None, false, ts(START_SECS))
            .0
            .is_none()
    );
    let (post, _, transition) = engine.handle_session_quota_transition(
        &claude,
        &depleted,
        None,
        None,
        false,
        ts(START_SECS + 60),
    );
    assert!(post.is_some());
    assert_eq!(transition, SessionQuotaTransition::Depleted);
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:240 antigravity session notification uses quota summary duration instead of family representative
#[test]
fn usage_store_antigravity_session_notification_uses_quota_summary_duration_instead_of_family_representative()
 {
    let antigravity = pid("antigravity");
    let mut engine = NotificationPolicyEngine::default();
    let s1 = antigravity_quota_summary_snapshot(20.0, 100.0);
    let s2 = antigravity_quota_summary_snapshot(100.0, 100.0);
    assert!(
        engine
            .handle_session_quota_transition(&antigravity, &s1, None, None, false, ts(START_SECS))
            .0
            .is_none()
    );
    let (post, _, transition) = engine.handle_session_quota_transition(
        &antigravity,
        &s2,
        None,
        None,
        false,
        ts(START_SECS + 60),
    );
    assert!(post.is_some());
    assert_eq!(transition, SessionQuotaTransition::Depleted);
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:265 antigravity preserves session notifications for durationless legacy family lanes
#[test]
fn usage_store_antigravity_preserves_session_notifications_for_durationless_legacy_family_lanes() {
    let antigravity = pid("antigravity");
    let mut engine = NotificationPolicyEngine::default();
    let s1 = antigravity_legacy_snapshot(20.0, 20.0);
    let s2 = antigravity_legacy_snapshot(20.0, 100.0);
    assert!(
        engine
            .handle_session_quota_transition(&antigravity, &s1, None, None, false, ts(START_SECS))
            .0
            .is_none()
    );
    let (post, _, transition) = engine.handle_session_quota_transition(
        &antigravity,
        &s2,
        None,
        None,
        false,
        ts(START_SECS + 60),
    );
    assert!(post.is_some());
    assert_eq!(transition, SessionQuotaTransition::Depleted);
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:290 antigravity snapshot mode change resets session notification baseline
#[test]
fn usage_store_antigravity_snapshot_mode_change_resets_session_notification_baseline() {
    let antigravity = pid("antigravity");
    let mut engine = NotificationPolicyEngine::default();
    let s1 = antigravity_quota_summary_snapshot(20.0, 20.0);
    let s2 = antigravity_legacy_snapshot(100.0, 100.0);
    assert!(
        engine
            .handle_session_quota_transition(&antigravity, &s1, None, None, false, ts(START_SECS))
            .0
            .is_none()
    );
    assert!(
        engine
            .handle_session_quota_transition(
                &antigravity,
                &s2,
                None,
                None,
                false,
                ts(START_SECS + 60)
            )
            .0
            .is_none()
    );
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:314 quota warning disabled does not post
#[test]
fn usage_store_quota_warning_disabled_does_not_post() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = false;
    let snap = make_snapshot("codex", 90.0, None, ts(START_SECS), "person@example.com");
    let (events, posts) = engine.handle_quota_warning_transitions(&codex, &snap, None, false);
    assert!(events.is_empty());
    assert!(posts.is_empty());
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:337 quota warning posts once per downward threshold crossing
#[test]
fn usage_store_quota_warning_posts_once_per_downward_threshold_crossing() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_on_screen_alert_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    let mut all_events = Vec::new();
    let mut all_posts = Vec::new();
    for (idx, used) in [40.0, 55.0, 60.0].into_iter().enumerate() {
        let snap = make_snapshot(
            "codex",
            used,
            None,
            ts(START_SECS + idx as i64),
            "person@example.com",
        );
        let (events, posts) = engine.handle_quota_warning_transitions(&codex, &snap, None, false);
        all_events.extend(events);
        all_posts.extend(posts);
    }
    assert_eq!(all_events.len(), 1);
    assert_eq!(all_events[0].window, QuotaWarningWindow::Session);
    assert_eq!(all_events[0].threshold, 50);
    assert!(all_posts[0].on_screen_alert_enabled);
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:396 quota warning omits account when personal info is hidden
#[test]
fn usage_store_quota_warning_omits_account_when_personal_info_is_hidden() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.hide_personal_info = true;
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);

    let mut all_posts = Vec::new();
    for (idx, used) in [40.0, 55.0].into_iter().enumerate() {
        let snap = make_snapshot(
            "codex",
            used,
            None,
            ts(START_SECS + idx as i64),
            "person@example.com",
        );
        let (_, posts) = engine.handle_quota_warning_transitions(&codex, &snap, None, false);
        all_posts.extend(posts);
    }
    assert_eq!(all_posts.len(), 1);
    assert!(!all_posts[0].title.contains("person@example.com"));
    assert!(!all_posts[0].body.contains("person@example.com"));
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:437 hidden quota warning markers do not disable warning notifications
#[test]
fn usage_store_hidden_quota_warning_markers_do_not_disable_warning_notifications() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    let s1 = make_snapshot("codex", 40.0, None, ts(START_SECS), "person@example.com");
    let s2 = make_snapshot(
        "codex",
        55.0,
        None,
        ts(START_SECS + 60),
        "person@example.com",
    );
    engine.handle_quota_warning_transitions(&codex, &s1, None, false);
    let (events, _) = engine.handle_quota_warning_transitions(&codex, &s2, None, false);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].threshold, 50);
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:472 quota warning crossing multiple thresholds posts most severe only
#[test]
fn usage_store_quota_warning_crossing_multiple_thresholds_posts_most_severe_only() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    let s1 = make_snapshot("codex", 10.0, None, ts(START_SECS), "person@example.com");
    let s2 = make_snapshot(
        "codex",
        85.0,
        None,
        ts(START_SECS + 60),
        "person@example.com",
    );
    engine.handle_quota_warning_transitions(&codex, &s1, None, false);
    let (events, _) = engine.handle_quota_warning_transitions(&codex, &s2, None, false);
    assert_eq!(
        events.into_iter().map(|e| e.threshold).collect::<Vec<_>>(),
        vec![20]
    );
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:505 quota warning recovers and can fire again
#[test]
fn usage_store_quota_warning_recovers_and_can_fire_again() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    let mut fired = Vec::new();
    for (idx, used) in [40.0, 55.0, 10.0, 55.0].into_iter().enumerate() {
        let snap = make_snapshot(
            "codex",
            used,
            None,
            ts(START_SECS + idx as i64),
            "person@example.com",
        );
        let (events, _) = engine.handle_quota_warning_transitions(&codex, &snap, None, false);
        fired.extend(events.into_iter().map(|e| e.threshold));
    }
    assert_eq!(fired, vec![50, 50]);
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:538 quota warning provider override beats global thresholds
#[test]
fn usage_store_quota_warning_provider_override_beats_global_thresholds() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);
    engine.preferences.set_provider_quota_warning_thresholds(
        codex.clone(),
        QuotaWarningWindow::Session,
        vec![10],
    );

    let s1 = make_snapshot("codex", 40.0, None, ts(START_SECS), "person@example.com");
    let s2 = make_snapshot(
        "codex",
        95.0,
        None,
        ts(START_SECS + 60),
        "person@example.com",
    );
    engine.handle_quota_warning_transitions(&codex, &s1, None, false);
    let (events, _) = engine.handle_quota_warning_transitions(&codex, &s2, None, false);
    assert_eq!(
        events.into_iter().map(|e| e.threshold).collect::<Vec<_>>(),
        vec![10]
    );
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:572 quota warning session only config ignores weekly crossings
#[test]
fn usage_store_quota_warning_session_only_config_ignores_weekly_crossings() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);

    for (idx, used) in [40.0, 60.0].into_iter().enumerate() {
        let snap = UsageSnapshot::new(
            Some(rate_window(used, None, None)),
            Some(rate_window(used, None, None)),
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
            ts(START_SECS + idx as i64),
            None,
            Confidence::Exact,
        )
        .unwrap();
        let (events, _) = engine.handle_quota_warning_transitions(&codex, &snap, None, false);
        if idx == 1 {
            assert_eq!(
                events.into_iter().map(|e| e.window).collect::<Vec<_>>(),
                vec![QuotaWarningWindow::Session]
            );
        }
    }
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:605 quota warning weekly only config ignores session crossings
#[test]
fn usage_store_quota_warning_weekly_only_config_ignores_session_crossings() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, false);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    for (idx, used) in [40.0, 60.0].into_iter().enumerate() {
        let snap = UsageSnapshot::new(
            Some(rate_window(used, None, None)),
            Some(rate_window(used, None, None)),
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
            ts(START_SECS + idx as i64),
            None,
            Confidence::Exact,
        )
        .unwrap();
        let (events, _) = engine.handle_quota_warning_transitions(&codex, &snap, None, false);
        if idx == 1 {
            assert_eq!(
                events.into_iter().map(|e| e.window).collect::<Vec<_>>(),
                vec![QuotaWarningWindow::Weekly]
            );
        }
    }
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:696 antigravity quota warnings use named session and weekly durations
#[test]
fn usage_store_antigravity_quota_warnings_use_named_session_and_weekly_durations() {
    let antigravity = pid("antigravity");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    engine.handle_quota_warning_transitions(
        &antigravity,
        &antigravity_quota_summary_snapshot(40.0, 40.0),
        None,
        false,
    );
    let (events, _) = engine.handle_quota_warning_transitions(
        &antigravity,
        &antigravity_quota_summary_snapshot(60.0, 60.0),
        None,
        false,
    );
    assert_eq!(
        events.iter().map(|e| e.window).collect::<Vec<_>>(),
        vec![QuotaWarningWindow::Session, QuotaWarningWindow::Weekly]
    );
    assert_eq!(
        events.iter().map(|e| e.threshold).collect::<Vec<_>>(),
        vec![50, 50]
    );
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:725 antigravity legacy quota warnings do not infer weekly from family slots
#[test]
fn usage_store_antigravity_legacy_quota_warnings_do_not_infer_weekly_from_family_slots() {
    let antigravity = pid("antigravity");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    engine.handle_quota_warning_transitions(
        &antigravity,
        &antigravity_legacy_snapshot(40.0, 40.0),
        None,
        false,
    );
    let (events, _) = engine.handle_quota_warning_transitions(
        &antigravity,
        &antigravity_legacy_snapshot(60.0, 60.0),
        None,
        false,
    );
    assert_eq!(
        events.into_iter().map(|e| e.window).collect::<Vec<_>>(),
        vec![QuotaWarningWindow::Session]
    );
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:752 antigravity quota warning mode change resets warning baseline
#[test]
fn usage_store_antigravity_quota_warning_mode_change_resets_warning_baseline() {
    let antigravity = pid("antigravity");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, true);
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, true);

    engine.handle_quota_warning_transitions(
        &antigravity,
        &antigravity_quota_summary_snapshot(20.0, 20.0),
        None,
        false,
    );
    let (events, _) = engine.handle_quota_warning_transitions(
        &antigravity,
        &antigravity_legacy_snapshot(80.0, 40.0),
        None,
        false,
    );
    assert!(events.is_empty());
    let key = QuotaWarningStateKey {
        provider: antigravity,
        window: QuotaWarningWindow::Session,
        account_discriminator: None,
        window_id: None,
    };
    assert_eq!(engine.quota_warning_states[&key].last_remaining, Some(20.0));
    assert_eq!(
        engine.quota_warning_states[&key].source,
        Some(SessionQuotaWindowSource::AntigravityLegacy)
    );
}

// upstream: UsageStoreSessionQuotaTransitionTests.swift:786 disabling quota warning window clears fired state
#[test]
fn usage_store_disabling_quota_warning_window_clears_fired_state() {
    let codex = pid("codex");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];

    let s1 = make_snapshot("codex", 40.0, None, ts(START_SECS), "person@example.com");
    let s2 = make_snapshot(
        "codex",
        60.0,
        None,
        ts(START_SECS + 60),
        "person@example.com",
    );
    engine.handle_quota_warning_transitions(&codex, &s1, None, false);
    let (events, _) = engine.handle_quota_warning_transitions(&codex, &s2, None, false);
    assert_eq!(events.len(), 1);

    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Session, false);
    engine.handle_quota_warning_transitions(&codex, &s2, None, false);

    let key = QuotaWarningStateKey {
        provider: codex,
        window: QuotaWarningWindow::Session,
        account_discriminator: None,
        window_id: None,
    };
    assert!(!engine.quota_warning_states.contains_key(&key));
}
