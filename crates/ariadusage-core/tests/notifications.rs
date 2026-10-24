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
    ProviderFetchErrorKind, ProviderIdentitySnapshot, QuotaWarningStateKey, QuotaWarningWindow,
    RateWindow, SessionQuotaTransition, SessionQuotaTransitionObservation,
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:12 same future boundary suppresses restore and duplicate depletion
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:28 advanced boundary before trusted expiry stays suppressed
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:45 depleted boundary stays frozen until its reset can be proven
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:65 depleted observation recovers a missing trusted boundary
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:84 expired baseline boundary cannot produce a single sample restore
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:107 boundary expired before evaluation is not trusted
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:126 depletion cannot advance a still future trusted boundary
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:144 pre boundary observation cannot advance metadata when processed later
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:163 ambiguous post expiry restore requires two fresh observations
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:193 boundaryless restore requires two fresh observations
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:210 advanced post expiry boundary restores exactly once
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:227 advanced boundary expired at observation time requires confirmation
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:256 regressed post expiry boundary requires two fresh observations
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:277 older and equal observations cannot change the depleted baseline
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:295 owner change establishes a new baseline without restoring
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:312 source change establishes a new reducer baseline
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:341 missing owner fails closed and clears prior state
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:373 missing owner keeps stale observations behind the fresh baseline barrier
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:402 windowless Codex result advances a matching depleted baseline watermark
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:438 windowless Codex result blocks stale boundaryless restore confirmation
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:473 disabled provider cleanup does not refire Codex depletion
// upstream: CodexSessionQuotaFalseRestoreTests.swift:494 unavailable provider cleanup does not refire Codex depletion
#[test]
fn codex_session_disabled_or_unavailable_provider_cleanup_does_not_refire_depletion() {
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:517 cleanup after a positive Codex baseline still reports depletion on recovery
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:536 cleanup without a prior Codex baseline keeps startup depletion semantics
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:552 notifications disabled keep stale observations behind the fresh baseline barrier
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:594 non Codex providers preserve immediate restore semantics
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:619 non Codex providers preserve disabled baseline tracking
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

// upstream: CodexSessionQuotaFalseRestoreTests.swift:646 selected Codex account caller forwards its stable owner
// upstream: CodexSessionQuotaFalseRestoreTests.swift:686 selected Codex accounts keep independent quota warning episodes
// upstream: CodexSessionQuotaFalseRestoreTests.swift:758 selected email only Codex account keeps session notifications
// upstream: CodexSessionQuotaFalseRestoreTests.swift:807 email only notification owners isolate source and credential rotation
// upstream: CodexSessionQuotaFalseRestoreTests.swift:849 email only credential rotation establishes a new baseline
// upstream: CodexSessionQuotaFalseRestoreTests.swift:911 provider owner survives source and credential changes
// upstream: CodexSessionQuotaFalseRestoreTests.swift:966 regular refresh owner builder supports email only identity
// upstream: CodexSessionQuotaFalseRestoreTests.swift:979 clearing published Codex usage clears typed transition state
#[test]
fn codex_session_owner_keys_and_account_isolation() {
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

    // Clearing published Codex usage clears typed transition state and requires fresh baseline
    let mut engine = NotificationPolicyEngine::default();
    let mut transitions = Vec::new();
    observe_codex(
        &mut engine,
        &mut transitions,
        20.0,
        None,
        ts(START_SECS),
        None,
        Some(&live_a),
    );
    engine.require_fresh_codex_session_quota_baseline(None);
    assert!(!engine.session_quota_states.contains_key(&pid("codex")));
    assert!(engine.codex_session_quota_baseline_required());
}

// =========================================================================
// CodexLimitResetOwnerKeyTests
// =========================================================================

// upstream: CodexLimitResetOwnerKeyTests.swift:10 limit reset owner stays stable for the same provider workspace and email
// upstream: CodexLimitResetOwnerKeyTests.swift:39 different emails in the same provider workspace use different owner keys
// upstream: CodexLimitResetOwnerKeyTests.swift:61 different provider workspaces with the same email use different owner keys
// upstream: CodexLimitResetOwnerKeyTests.swift:88 email only owner fails closed even for one visible row
// upstream: CodexLimitResetOwnerKeyTests.swift:103 duplicate email only rows fail closed
// upstream: CodexLimitResetOwnerKeyTests.swift:118 provider row wins its own identity while same email fallback fails closed
// upstream: CodexLimitResetOwnerKeyTests.swift:140 guard and visible row normalize the same provider owner
// upstream: CodexLimitResetOwnerKeyTests.swift:162 unresolved owner identity fails closed
#[test]
fn codex_limit_reset_owner_key_stability_and_fail_closed_invariants() {
    let original = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: "workspace-fixture-stable".to_owned(),
        },
        Some(" Person-One@Example.Test "),
    )
    .unwrap();
    let relabeled = CodexLimitResetOwnerKey::new(
        &CodexIdentity::ProviderAccount {
            id: " WORKSPACE-FIXTURE-STABLE ".to_owned(),
        },
        Some("person-one@example.test"),
    )
    .unwrap();
    assert_eq!(original, relabeled);
    assert_eq!(original.raw_value.len(), 64);
    assert!(!original.raw_value.contains("workspace-fixture-stable"));
    assert!(!original.raw_value.contains("person-one@example.test"));

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

    assert_eq!(
        CodexLimitResetOwnerKey::new(
            &CodexIdentity::EmailOnly {
                normalized_email: "unique-person@example.test".to_owned()
            },
            Some("unique-person@example.test")
        ),
        None
    );
    assert_eq!(
        CodexLimitResetOwnerKey::new(&CodexIdentity::Unresolved, None),
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

// upstream: LimitResetNotificationTests.swift:50 limit reset notifications default off and persist when enabled
// upstream: LimitResetNotificationTests.swift:64 weekly reset notice names provider window and account
// upstream: LimitResetNotificationTests.swift:78 session reset notice names the session window
// upstream: LimitResetNotificationTests.swift:92 reset notices stay silent when disabled
// upstream: LimitResetNotificationTests.swift:106 hide personal info omits the account from reset notices
// upstream: LimitResetNotificationTests.swift:142 reset notice copy names provider and window
// upstream: LimitResetNotificationTests.swift:161 reset notice copy is localized for simplified Chinese
// upstream: LimitResetNotificationTests.swift:172 portable preferences carry the reset toggle in both directions
#[test]
fn limit_reset_preferences_copy_and_basic_transitions() {
    let prefs = NotificationPreferences::default();
    assert!(!prefs.limit_reset_notifications_enabled);

    let (w_title, w_body) =
        LimitResetNotificationLogic::notification_copy("Claude", QuotaWarningWindow::Weekly);
    assert_eq!(w_title, "Claude weekly limit reset");
    assert_eq!(w_body, "Fresh quota is available.");
    assert!(!w_body.contains('@'));

    let (s_title, s_body) =
        LimitResetNotificationLogic::notification_copy("Codex", QuotaWarningWindow::Session);
    assert_eq!(s_title, "Codex session limit reset");
    assert_eq!(s_body, "Fresh quota is available.");

    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    // Silent when disabled
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

    // Weekly and session reset when enabled
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.limit_reset_notifications_enabled = true;
    engine.handle_limit_reset_transitions(&claude, &s1, None, None, false);
    let posts = engine.handle_limit_reset_transitions(&claude, &s2, None, None, false);
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].id_prefix, "limit-reset-claude-weekly");
    assert_eq!(posts[0].title, "Claude weekly limit reset");
}

// upstream: LimitResetNotificationTests.swift:121 session restored notice covers the matching session reset notice
// upstream: LimitResetNotificationTests.swift:188 restored notice for one account does not suppress another account reset
// upstream: LimitResetNotificationTests.swift:210 weekly reset boundary is notified once across refreshes and restarts
// upstream: LimitResetNotificationTests.swift:231 a restored confirmation after restart does not repeat a reset banner
// upstream: LimitResetNotificationTests.swift:251 reset notices recover when a provider stops reporting reset boundaries
// upstream: LimitResetNotificationTests.swift:272 partial restored notice also covers the later confirmed reset
// upstream: LimitResetNotificationTests.swift:296 known reset boundaries remain deduplicated after a metadata gap
// upstream: LimitResetNotificationTests.swift:316 new depleted episodes still deliver restored notices without full resets
// upstream: LimitResetNotificationTests.swift:340 advanced boundaries and independent windows notify separately
// upstream: LimitResetNotificationTests.swift:357 pending reset delivery rechecks opt in and personal info
// upstream: LimitResetNotificationTests.swift:373 a later restored confirmation does not repeat an announced reset
// upstream: LimitResetNotificationTests.swift:393 switching from a depleted account does not cover another accounts reset
// upstream: LimitResetNotificationTests.swift:419 delayed restored confirmation stays covered when usage has resumed
#[test]
fn limit_reset_arbitration_deduplication_and_restored_coverage() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.session_quota_notifications_enabled = true;
    engine.preferences.limit_reset_notifications_enabled = true;

    let depleted = claude_snapshot(100.0, 60.0, 0, "limit-reset-notice@example.com", None);
    let reset = claude_snapshot(0.0, 0.0, 3600, "limit-reset-notice@example.com", None);

    let (post1, pending1, _) = engine.handle_session_quota_transition(
        &claude,
        &depleted,
        None,
        None,
        false,
        depleted.updated_at,
    );
    assert!(post1.is_some());
    assert!(!pending1);
    engine.handle_limit_reset_transitions(&claude, &depleted, None, None, false);

    let (post2, pending2, _) = engine.handle_session_quota_transition(
        &claude,
        &reset,
        None,
        None,
        false,
        reset.updated_at,
    );
    assert!(post2.is_none());
    assert!(pending2);
    let reset_posts = engine.handle_limit_reset_transitions(&claude, &reset, None, None, pending2);
    assert_eq!(reset_posts.len(), 2);
    assert_eq!(reset_posts[0].id_prefix, "session-claude-restored");
    assert_eq!(reset_posts[1].id_prefix, "limit-reset-claude-weekly");

    // Delayed restored confirmation stays covered when usage has resumed
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

// upstream: ClaudeExtraWindowQuotaWarningTests.swift:43 claude scoped weekly and routines extra windows fire independent weekly warnings
// upstream: ClaudeExtraWindowQuotaWarningTests.swift:93 antigravity summary extra windows do not trigger the claude extra-window lane
// upstream: ClaudeExtraWindowQuotaWarningTests.swift:131 claude scoped weekly window refires after recovering above threshold
// upstream: ClaudeExtraWindowQuotaWarningTests.swift:161 claude extra-window fired state is pruned when a window disappears but others remain
// upstream: ClaudeExtraWindowQuotaWarningTests.swift:202 claude extra-window reconciliation preserves sibling account state
// upstream: ClaudeExtraWindowQuotaWarningTests.swift:249 disabling weekly warnings clears all account-scoped claude extra-window state
// upstream: ClaudeExtraWindowQuotaWarningTests.swift:296 claude extra-window state survives a transient extras miss without re-posting
// upstream: UsageStoreAccountQuotaWarningTests.swift:25 ordinary refresh keeps selected token account warning episodes independent
// upstream: UsageStoreAccountQuotaWarningTests.swift:71 selected outcome keeps token account warning episodes independent
#[test]
fn claude_extra_windows_and_account_scoped_quota_warnings() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];

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
    assert!(events.iter().any(|e| {
        e.window_id.as_deref() == Some("claude-weekly-scoped-fable")
            && e.window_display_label.as_deref() == Some("Fable only")
            && e.threshold == 50
    }));
    assert!(events.iter().any(|e| {
        e.window_id.as_deref() == Some("claude-routines")
            && e.window_display_label.as_deref() == Some("Daily Routines")
            && e.threshold == 50
    }));

    // Pruning when fable disappears while routines remains
    engine.handle_quota_warning_transitions(
        &claude,
        &claude_extra_window_snapshot(None, Some(55.0)),
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
    assert!(!engine.quota_warning_states.contains_key(&fable_key));
    assert!(engine.quota_warning_states.contains_key(&routines_key));

    // Transient nil extras miss does not drop remaining extra window state
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
    assert!(engine.quota_warning_states.contains_key(&routines_key));

    // Disabling weekly warnings clears all weekly states
    engine
        .preferences
        .set_quota_warning_window_enabled(QuotaWarningWindow::Weekly, false);
    engine.handle_quota_warning_transitions(&claude, &empty_snap, None, false);
    assert!(!engine.quota_warning_states.contains_key(&routines_key));
}

// =========================================================================
// ClaudeCredentialQuotaWarningTests & ClaudeSyntheticPlaceholderNotificationTests
// =========================================================================

// upstream: ClaudeCredentialQuotaWarningTests.swift:26 weekly OAuth fallback does not notify or rearm the session lane
// upstream: ClaudeCredentialQuotaWarningTests.swift:58 credential rewrites preserve known account threshold episodes
// upstream: ClaudeCredentialQuotaWarningTests.swift:66 credential rewrites preserve unresolved account warnings
// upstream: ClaudeCredentialQuotaWarningTests.swift:79 unresolved history migrates to a stable account without repeating a threshold
// upstream: ClaudeCredentialQuotaWarningTests.swift:125 verified OAuth owner and active identity share threshold episodes across CLI fallback
// upstream: ClaudeCredentialQuotaWarningTests.swift:184 verified owner migration preserves the latest recovery and other warning lanes
// upstream: ClaudeCredentialQuotaWarningTests.swift:254 repeated CLI identity gaps preserve threshold history
// upstream: ClaudeCredentialQuotaWarningTests.swift:264 later thresholds do not repeat across either identity key
// upstream: ClaudeCredentialQuotaWarningTests.swift:274 discontinuous identity gaps start one independent fallback episode
// upstream: ClaudeCredentialQuotaWarningTests.swift:289 identity gaps follow the most recently known account without merging known accounts
// upstream: ClaudeCredentialQuotaWarningTests.swift:304 initial unresolved history survives repeated resolution and quota recovery
#[test]
fn claude_credential_quota_warning_reconciliation_and_identity_gaps() {
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
                &ClaudeOAuthActiveAccountObservation::Stable {
                    identity: Some("account-b".to_owned())
                },
                Some("owner-a")
            )
            .0,
        None
    );
}

// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:12 placeholder preserves depleted state without a reset boundary
// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:30 placeholder stays non authoritative after the prior boundary elapses
// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:59 placeholder cannot rearm depletion while notifications are disabled
// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:80 real zero usage remains an authoritative restore
// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:95 placeholder preserves threshold state while weekly warnings continue
// upstream: ClaudeSyntheticPlaceholderNotificationTests.swift:145 placeholder preserves predictive episode while weekly risk continues
#[test]
fn claude_synthetic_placeholder_preserves_depleted_and_warning_state() {
    let claude = pid("claude");
    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50];
    let mut transitions = Vec::new();
    let mut warning_windows = Vec::new();

    for (session_used, weekly_used, is_placeholder, offset) in [
        (20.0, 40.0, false, 0),
        (100.0, 40.0, false, 60),
        (0.0, 60.0, true, 120),
        (100.0, 60.0, false, 180),
        (0.0, 60.0, false, 240),
    ] {
        let primary = if is_placeholder {
            placeholder_window(session_used, Some(300), None)
        } else {
            rate_window(session_used, Some(300), None)
        };
        let snap = UsageSnapshot::new(
            Some(primary),
            Some(rate_window(weekly_used, Some(10080), None)),
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
            ts(START_SECS + offset),
            None,
            Confidence::Exact,
        )
        .unwrap();
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
        let (events, _) = engine.handle_quota_warning_transitions(&claude, &snap, None, false);
        warning_windows.extend(events.into_iter().map(|e| e.window));
    }

    assert_eq!(
        transitions,
        vec![
            SessionQuotaTransition::Depleted,
            SessionQuotaTransition::Restored
        ]
    );
    assert_eq!(
        warning_windows,
        vec![QuotaWarningWindow::Session, QuotaWarningWindow::Weekly]
    );
}

// =========================================================================
// CredentialNotificationTests
// =========================================================================

// upstream: CredentialNotificationTests.swift:9 episodes survive network quota and cached data and recover only for the same account
// upstream: CredentialNotificationTests.swift:40 turning notifications off does not forget an unresolved episode
// upstream: CredentialNotificationTests.swift:59 classified provider failures are authoritative
// upstream: CredentialNotificationTests.swift:66 native credential errors exclude quota network permission and parse failures
// upstream: CredentialNotificationTests.swift:94 ordinary and selected account outcomes share account-specific episodes
// upstream: CredentialNotificationTests.swift:129 shared delivery retires notifications when consent or lifecycle changes
// upstream: CredentialNotificationTests.swift:152 failed delivery retries while delivered episodes retire on recovery or shutdown
// upstream: CredentialNotificationTests.swift:188 Claude identity gaps and later identity resolution preserve independent episodes
// upstream: CredentialNotificationTests.swift:213 disabling a provider retires only its delivered credential alerts
#[test]
fn credential_notification_episodes_classification_and_lifecycle() {
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

    assert!(ProviderCredentialFailure::is_authentication_failure(
        &ProviderCredentialError::ClaudeOAuthRefreshFailed("invalid_grant".to_owned())
    ));
    assert!(!ProviderCredentialFailure::is_authentication_failure(
        &ProviderCredentialError::ClaudeOAuthRefreshFailed("network offline".to_owned())
    ));
    assert!(!ProviderCredentialFailure::is_authentication_failure(
        &ProviderCredentialError::KeychainAccessRevoked
    ));

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

    // Survives duplicate expiry, network timeout, rate-limited, cache, and diagnostic fallback
    assert_eq!(
        tracker.handle_outcome(&kimi, "first", &expired, true),
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

    // Recovery for "first" removes its notification
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

    // Claude identity scopes
    let mut scope_tracker = CredentialNotificationTracker::new();
    let first_scope = scope_tracker.claude_credential_notification_scope(None, Some("first-file"));
    let second_scope =
        scope_tracker.claude_credential_notification_scope(None, Some("second-file"));
    assert_ne!(first_scope, second_scope);
    assert_eq!(
        scope_tracker
            .claude_credential_notification_scope(Some("first-account"), Some("first-file")),
        first_scope
    );
    assert_eq!(
        scope_tracker.claude_credential_notification_scope(Some("first-account"), None),
        first_scope
    );
    assert_eq!(
        scope_tracker
            .claude_credential_notification_scope(Some("first-account"), Some("rotated-file")),
        first_scope
    );
    assert_eq!(
        scope_tracker.claude_credential_notification_scope(None, Some("rotated-file")),
        first_scope
    );
    let switched = scope_tracker
        .claude_credential_notification_scope(Some("second-account"), Some("rotated-file"));
    assert_ne!(switched, first_scope);

    // deliver_if_current lifecycle
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

// =========================================================================
// UsageStoreSessionQuotaTransitionTests
// =========================================================================

// upstream: UsageStoreSessionQuotaTransitionTests.swift:46 copilot switch from primary to secondary resets baseline
// upstream: UsageStoreSessionQuotaTransitionTests.swift:75 copilot switch from secondary to primary resets baseline
// upstream: UsageStoreSessionQuotaTransitionTests.swift:104 claude weekly primary fallback does not emit session quota notifications
// upstream: UsageStoreSessionQuotaTransitionTests.swift:133 claude spend limit fallback does not emit session or quota warning notifications
// upstream: UsageStoreSessionQuotaTransitionTests.swift:173 mimo balance and monthly credits do not emit quota notifications
// upstream: UsageStoreSessionQuotaTransitionTests.swift:212 claude five hour primary still emits session quota notifications
// upstream: UsageStoreSessionQuotaTransitionTests.swift:241 antigravity session notification uses quota summary duration instead of family representative
// upstream: UsageStoreSessionQuotaTransitionTests.swift:266 antigravity preserves session notifications for durationless legacy family lanes
// upstream: UsageStoreSessionQuotaTransitionTests.swift:291 antigravity snapshot mode change resets session notification baseline
// upstream: UsageStoreSessionQuotaTransitionTests.swift:315 quota warning disabled does not post
// upstream: UsageStoreSessionQuotaTransitionTests.swift:338 quota warning posts once per downward threshold crossing
// upstream: UsageStoreSessionQuotaTransitionTests.swift:397 quota warning omits account when personal info is hidden
// upstream: UsageStoreSessionQuotaTransitionTests.swift:438 hidden quota warning markers do not disable warning notifications
// upstream: UsageStoreSessionQuotaTransitionTests.swift:473 quota warning crossing multiple thresholds posts most severe only
// upstream: UsageStoreSessionQuotaTransitionTests.swift:506 quota warning recovers and can fire again
// upstream: UsageStoreSessionQuotaTransitionTests.swift:539 quota warning provider override beats global thresholds
// upstream: UsageStoreSessionQuotaTransitionTests.swift:573 quota warning session only config ignores weekly crossings
// upstream: UsageStoreSessionQuotaTransitionTests.swift:606 quota warning weekly only config ignores session crossings
// upstream: UsageStoreSessionQuotaTransitionTests.swift:639 minimax quota warning posts for session and weekly windows
// upstream: UsageStoreSessionQuotaTransitionTests.swift:668 amp subscription quota warnings use pool labels
// upstream: UsageStoreSessionQuotaTransitionTests.swift:697 antigravity quota warnings use named session and weekly durations
// upstream: UsageStoreSessionQuotaTransitionTests.swift:726 antigravity legacy quota warnings do not infer weekly from family slots
// upstream: UsageStoreSessionQuotaTransitionTests.swift:753 antigravity quota warning mode change resets warning baseline
// upstream: UsageStoreSessionQuotaTransitionTests.swift:787 disabling quota warning window clears fired state
#[test]
fn usage_store_session_quota_and_quota_warning_transitions() {
    let antigravity = pid("antigravity");
    let claude = pid("claude");
    let mimo = pid("mimo");

    let mut engine = NotificationPolicyEngine::default();
    engine.preferences.quota_warning_notifications_enabled = true;
    engine.preferences.quota_warning_thresholds = vec![50, 20];

    // Claude 7-day primary does not emit session quota notifications
    let claude_weekly_primary = UsageSnapshot::new(
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
    let claude_weekly_depleted = UsageSnapshot::new(
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
            .handle_session_quota_transition(
                &claude,
                &claude_weekly_primary,
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
            .handle_session_quota_transition(
                &claude,
                &claude_weekly_depleted,
                None,
                None,
                false,
                ts(START_SECS + 60)
            )
            .0
            .is_none()
    );

    // Claude spend limit only (no rate windows) does not emit notifications
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

    // MiMo suppressed
    let mimo_snap = make_snapshot("mimo", 100.0, None, ts(START_SECS), "mimo@example.com");
    assert!(
        engine
            .handle_session_quota_transition(&mimo, &mimo_snap, None, None, false, ts(START_SECS))
            .0
            .is_none()
    );
    assert!(
        engine
            .handle_quota_warning_transitions(&mimo, &mimo_snap, None, false)
            .0
            .is_empty()
    );

    // Antigravity quota-summary vs legacy mode switch resets baseline
    let ag_summary = UsageSnapshot::new(
        Some(rate_window(20.0, Some(7 * 24 * 60), None)),
        Some(rate_window(20.0, Some(5 * 60), None)),
        None,
        Some(vec![
            NamedRateWindow::new(
                "antigravity-quota-summary-gemini-5h",
                "Gemini 5h",
                rate_window(20.0, Some(5 * 60), None),
            ),
            NamedRateWindow::new(
                "antigravity-quota-summary-gemini-weekly",
                "Gemini Weekly",
                rate_window(20.0, Some(7 * 24 * 60), None),
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
    .unwrap();
    let ag_legacy = UsageSnapshot::new(
        Some(rate_window(80.0, None, None)),
        Some(rate_window(40.0, None, None)),
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
    engine.handle_quota_warning_transitions(&antigravity, &ag_summary, None, false);
    let (events, _) =
        engine.handle_quota_warning_transitions(&antigravity, &ag_legacy, None, false);
    assert!(events.is_empty());
    let ag_key = QuotaWarningStateKey {
        provider: antigravity.clone(),
        window: QuotaWarningWindow::Session,
        account_discriminator: None,
        window_id: None,
    };
    assert_eq!(
        engine.quota_warning_states[&ag_key].last_remaining,
        Some(20.0)
    );
    assert_eq!(
        engine.quota_warning_states[&ag_key].source,
        Some(SessionQuotaWindowSource::AntigravityLegacy)
    );
}
