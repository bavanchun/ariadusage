// Ported from CodexBar Tests/CodexBarTests/AntigravityScopedPrintFetchTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ariadusage_core::pipeline::{
    FetchContext, FetchInteraction, FetchPhase, FetchResult, FetchRuntime,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::providers::antigravity::cli_https::fetch_with_report_fallback;
use ariadusage_engine::providers::antigravity::cli_print_failure::{
    AntigravityCLIPrintFailure, ExitReason,
};
use ariadusage_engine::providers::antigravity::oauth_store::{
    AntigravityOAuthCredentials, AntigravityOAuthCredentialsStore,
};
use ariadusage_engine::providers::antigravity::quota_summary::{
    AntigravityQuotaSummary, AntigravityQuotaSummaryBucket, AntigravityQuotaSummaryGroup,
};
use ariadusage_engine::providers::antigravity::scoped_print_fetch::{
    ANTIGRAVITY_USERINFO_URL, AntigravityAgyFileTokenPayload, AntigravityScopedAgyStaging,
    AntigravityScopedPrintError, AntigravityScopedStagingError, ScopedPrintInvocation,
    ScopedUserInfoError, fetch_scoped_print_snapshot,
};
use ariadusage_engine::providers::antigravity::status_probe::{
    AntigravityModelQuota, AntigravitySnapshotSource, AntigravityStatusProbeError,
    AntigravityStatusSnapshot,
};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use tokio_util::sync::CancellationToken;

fn credentials(email: &str) -> AntigravityOAuthCredentials {
    AntigravityOAuthCredentials::new(
        "scoped-access-token",
        Some("refresh".to_owned()),
        Some("2027-01-15T08:00:00Z".parse().unwrap()),
        Some(id_token(email)),
        Some(email.to_owned()),
        None,
        None,
        None,
    )
}

fn id_token(email: &str) -> String {
    use base64::Engine;
    let payload = serde_json::json!({"email": email});
    format!(
        "e30.{}.sig",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}

fn account_environment(value: &AntigravityOAuthCredentials) -> BTreeMap<String, String> {
    BTreeMap::from([(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
        AntigravityOAuthCredentialsStore::token_account_value(value).unwrap(),
    )])
}

fn report() -> AntigravityStatusSnapshot {
    AntigravityStatusSnapshot {
        model_quotas: vec![AntigravityModelQuota::new(
            "Gemini Pro",
            "gemini-pro",
            Some(0.6),
            None,
            None,
        )],
        account_email: None,
        account_plan: None,
        source: AntigravitySnapshotSource::Cli,
        quota_summary: None,
    }
}

fn fallback_context(
    source_mode: SourceMode,
    selected_token_account: Option<&str>,
    env: BTreeMap<String, SecretString>,
) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("antigravity").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "scoped-print-fallback-test".to_owned(),
        source_mode,
        env,
        include_credits: false,
        selected_token_account: selected_token_account.map(str::to_owned),
        cancel: CancellationToken::new(),
    }
}

fn fallback_result(email: Option<&str>) -> FetchResult {
    let mut report = report();
    report.account_email = email.map(str::to_owned);
    let usage = report.to_usage_snapshot().unwrap();
    FetchResult::new(
        usage,
        "cli",
        "antigravity.cli-https",
        ariadusage_protocol::metric::SourceKind::Cli,
    )
}

fn selected_environment() -> BTreeMap<String, SecretString> {
    BTreeMap::from([(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
        SecretString::new("synthetic-selected-token"),
    )])
}

// upstream: AntigravityScopedPrintFetchTests.swift:156 selected auto account uses the scoped report when legacy fails
#[tokio::test]
async fn selected_auto_account_uses_scoped_report_when_legacy_fails() {
    let expected = fallback_result(Some("scoped@example.com"));
    let result = fetch_with_report_fallback(
        &fallback_context(
            SourceMode::Auto,
            Some("scoped@example.com"),
            selected_environment(),
        ),
        || async { Err(AntigravityStatusProbeError::TimedOut) },
        || async { Err(AntigravityStatusProbeError::NotRunning) },
        Some(|| {
            let expected = expected.clone();
            async move { Ok(expected) }
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        result
            .usage
            .identity
            .and_then(|identity| identity.account_email),
        Some("scoped@example.com".to_owned())
    );
}

// upstream: AntigravityScopedPrintFetchTests.swift:172 scoped failure preserves the original error and never runs ambient print
#[tokio::test]
async fn scoped_failure_preserves_original_error_and_never_runs_ambient_print() {
    let ambient_called = Arc::new(Mutex::new(false));
    let ambient_seen = Arc::clone(&ambient_called);
    let result = fetch_with_report_fallback(
        &fallback_context(
            SourceMode::Auto,
            Some("scoped@example.com"),
            selected_environment(),
        ),
        || async { Err(AntigravityStatusProbeError::TimedOut) },
        move || {
            *ambient_seen.lock().unwrap() = true;
            async { Err(AntigravityStatusProbeError::NotRunning) }
        },
        Some(|| async {
            Err(AntigravityStatusProbeError::CliReportFailed(
                AntigravityCLIPrintFailure::ExecutableNotFound,
            ))
        }),
    )
    .await;
    assert_eq!(result, Err(AntigravityStatusProbeError::TimedOut));
    assert!(!*ambient_called.lock().unwrap());
}

// upstream: AntigravityScopedPrintFetchTests.swift:188 cancellation stops the pipeline before the scoped fetch
#[tokio::test]
async fn fallback_cancellation_stops_pipeline_before_scoped_fetch() {
    let context = fallback_context(
        SourceMode::Auto,
        Some("scoped@example.com"),
        selected_environment(),
    );
    context.cancel.cancel();
    let scoped_called = Arc::new(Mutex::new(false));
    let scoped_seen = Arc::clone(&scoped_called);
    let result = fetch_with_report_fallback(
        &context,
        || async { Err(AntigravityStatusProbeError::Cancelled) },
        || async { Err(AntigravityStatusProbeError::NotRunning) },
        Some(move || {
            *scoped_seen.lock().unwrap() = true;
            async { Err(AntigravityStatusProbeError::NotRunning) }
        }),
    )
    .await;
    assert_eq!(result, Err(AntigravityStatusProbeError::Cancelled));
    assert!(!*scoped_called.lock().unwrap());
}

// upstream: AntigravityScopedPrintFetchTests.swift:205 unselected auto fetch still uses the ambient print report
#[tokio::test]
async fn unselected_auto_fetch_uses_ambient_print_report() {
    let expected = fallback_result(None);
    let scoped_called = Arc::new(Mutex::new(false));
    let scoped_seen = Arc::clone(&scoped_called);
    let result = fetch_with_report_fallback(
        &fallback_context(SourceMode::Auto, None, BTreeMap::new()),
        || async { Err(AntigravityStatusProbeError::TimedOut) },
        || {
            let expected = expected.clone();
            async move { Ok(expected) }
        },
        Some(move || {
            *scoped_seen.lock().unwrap() = true;
            async { Err(AntigravityStatusProbeError::NotRunning) }
        }),
    )
    .await
    .unwrap();
    assert!(
        result
            .usage
            .identity
            .and_then(|identity| identity.account_email)
            .is_none()
    );
    assert!(!*scoped_called.lock().unwrap());
}

// upstream: AntigravityScopedPrintFetchTests.swift:220 explicit cli mode never reaches the scoped fetch
#[tokio::test]
async fn explicit_cli_mode_never_reaches_scoped_fetch() {
    let expected = fallback_result(None);
    let scoped_called = Arc::new(Mutex::new(false));
    let scoped_seen = Arc::clone(&scoped_called);
    let result = fetch_with_report_fallback(
        &fallback_context(
            SourceMode::Cli,
            Some("scoped@example.com"),
            selected_environment(),
        ),
        || async { Err(AntigravityStatusProbeError::TimedOut) },
        || {
            let expected = expected.clone();
            async move { Ok(expected) }
        },
        Some(move || {
            *scoped_seen.lock().unwrap() = true;
            async { Err(AntigravityStatusProbeError::NotRunning) }
        }),
    )
    .await
    .unwrap();
    assert!(
        result
            .usage
            .identity
            .and_then(|identity| identity.account_email)
            .is_none()
    );
    assert!(!*scoped_called.lock().unwrap());
}

// upstream: AntigravityScopedPrintFetchTests.swift:9 file token payload encodes the agy storage format
#[test]
fn file_token_payload_encodes_the_agy_storage_format() {
    let payload =
        AntigravityAgyFileTokenPayload::from_credentials(&credentials("user@example.com")).unwrap();
    let encoded = serde_json::to_value(&payload).unwrap();
    assert_eq!(encoded["token"]["access_token"], "scoped-access-token");
    assert_eq!(encoded["token"]["token_type"], "Bearer");
    assert_eq!(encoded["token"]["refresh_token"], "refresh");
    assert_eq!(encoded["auth_method"], "consumer");
    assert_eq!(encoded["id_token"], id_token("user@example.com"));
}

// upstream: AntigravityScopedPrintFetchTests.swift:27 file token payload refuses credentials without refresh token or expiry
#[test]
fn file_token_payload_refuses_credentials_without_refresh_token_or_expiry() {
    let no_refresh = AntigravityOAuthCredentials::new(
        "access",
        None,
        Some("2030-01-01T00:00:00Z".parse().unwrap()),
        None,
        Some("a@b.c".into()),
        None,
        None,
        None,
    );
    let no_expiry = AntigravityOAuthCredentials::new(
        "access",
        Some("refresh".into()),
        None,
        None,
        Some("a@b.c".into()),
        None,
        None,
        None,
    );
    assert!(AntigravityAgyFileTokenPayload::from_credentials(&no_refresh).is_none());
    assert!(AntigravityAgyFileTokenPayload::from_credentials(&no_expiry).is_none());
}

// upstream: AntigravityScopedPrintFetchTests.swift:39 scoped child environment inherits only allowlisted keys
#[test]
fn scoped_child_environment_inherits_only_allowlisted_keys() {
    let parent = BTreeMap::<String, String>::from([
        ("HOME".into(), "/ambient".into()),
        ("PATH".into(), "/ambient/bin".into()),
        ("HTTPS_PROXY".into(), "http://proxy.invalid".into()),
        ("GEMINI_API_KEY".into(), "ambient-secret".into()),
        ("AWS_PROFILE".into(), "ambient-profile".into()),
        (
            AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
            "injected".into(),
        ),
    ]);
    let child =
        AntigravityScopedAgyStaging::child_environment(&parent, std::path::Path::new("/scoped"));
    assert_eq!(child.get("HOME").map(String::as_str), Some("/scoped"));
    assert_eq!(child.get("PWD").map(String::as_str), Some("/scoped"));
    assert_eq!(
        child.get("SSH_TTY").map(String::as_str),
        Some("ariadusage-scoped")
    );
    assert!(!child.contains_key("GEMINI_API_KEY"));
    assert!(!child.contains_key("AWS_PROFILE"));
    assert!(!child.contains_key(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY));
}

// upstream: AntigravityScopedPrintFetchTests.swift:70 staging writes a private token file verified against the account claim
#[test]
fn staging_writes_a_private_token_file_verified_against_the_account_claim() {
    let staged = AntigravityScopedAgyStaging::stage(
        &credentials("scoped@example.com"),
        "scoped@example.com",
    )
    .unwrap();
    let token = staged
        .home
        .join(AntigravityScopedAgyStaging::TOKEN_RELATIVE_PATH);
    assert!(token.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(token).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(staged.home).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

// upstream: AntigravityScopedPrintFetchTests.swift:85 staging rejects a token whose identity does not match the account
#[test]
fn staging_rejects_a_token_whose_identity_does_not_match_the_account() {
    let result =
        AntigravityScopedAgyStaging::stage(&credentials("other@example.com"), "scoped@example.com");
    assert_eq!(
        result.unwrap_err(),
        AntigravityScopedStagingError::IdentityUnverifiable
    );
}

// upstream: AntigravityScopedPrintFetchTests.swift:95 staging accepts token-only credentials without an id_token claim
#[test]
fn staging_accepts_token_only_credentials_without_an_id_token_claim() {
    let mut value = credentials("scoped@example.com");
    value.id_token = None;
    let staged = AntigravityScopedAgyStaging::stage(&value, "scoped@example.com").unwrap();
    let payload = AntigravityScopedAgyStaging::staged_token_payload(&staged.home).unwrap();
    assert_eq!(payload.token.access_token, "scoped-access-token");
    assert!(payload.id_token.is_none());
}

// upstream: AntigravityScopedPrintFetchTests.swift:118 refreshed credentials follow the staged payload after agy rewrites it
#[test]
fn refreshed_credentials_follow_the_staged_payload_after_agy_rewrites_it() {
    let original = credentials("scoped@example.com");
    let mut refreshed = AntigravityAgyFileTokenPayload::from_credentials(&original).unwrap();
    refreshed.token.access_token = "fresh-access".into();
    refreshed.token.refresh_token = "fresh-refresh".into();
    refreshed.token.expiry = "2030-01-01T00:00:00Z".into();
    let updated =
        AntigravityScopedAgyStaging::refreshed_credentials(&refreshed, &original).unwrap();
    assert_eq!(updated.access_token, "fresh-access");
    assert_eq!(updated.refresh_token.as_deref(), Some("fresh-refresh"));
    assert_eq!(
        updated.expiry_date.unwrap().to_string(),
        "2030-01-01T00:00:00Z"
    );
}

// upstream: AntigravityScopedPrintFetchTests.swift:325 scoped print runs agy against the staged private home
#[tokio::test]
async fn scoped_print_runs_against_the_staged_private_home() {
    let mut environment = account_environment(&credentials("scoped@example.com"));
    environment.insert("HOME".into(), "/ambient/home".into());
    environment.insert("LEAKED_PARENT_SECRET".into(), "must-not-reach-child".into());
    let seen = Arc::new(Mutex::new(None));
    let seen_runner = Arc::clone(&seen);
    let result = fetch_scoped_print_snapshot(
        &environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        move |invocation: ScopedPrintInvocation| {
            let token_path = invocation
                .home
                .join(AntigravityScopedAgyStaging::TOKEN_RELATIVE_PATH);
            let token_payload: AntigravityAgyFileTokenPayload =
                serde_json::from_slice(&std::fs::read(token_path).unwrap()).unwrap();
            assert_eq!(token_payload.token.access_token, "scoped-access-token");
            assert_ne!(
                invocation.environment.get("HOME").map(String::as_str),
                Some("/ambient/home")
            );
            assert!(!invocation.environment.contains_key("LEAKED_PARENT_SECRET"));
            *seen_runner.lock().unwrap() = Some(invocation);
            async { Ok(report()) }
        },
        |_request| async { Ok(Some("scoped@example.com".into())) },
        Some(|_updated| async { Ok(()) }),
    )
    .await
    .unwrap();
    let invocation = seen.lock().unwrap().take().unwrap();
    assert_eq!(
        invocation.environment.get("SSH_TTY").map(String::as_str),
        Some("ariadusage-scoped")
    );
    assert!(
        !invocation
            .environment
            .contains_key(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY)
    );
    assert_eq!(result.account_email.as_deref(), Some("scoped@example.com"));
    assert!(!invocation.home.exists());
}

// upstream: AntigravityScopedPrintFetchTests.swift:239 scoped Starter reports preserve both weekly groups
#[tokio::test]
async fn scoped_reports_preserve_both_weekly_groups() {
    let environment = account_environment(&credentials("starter@example.com"));
    let result = fetch_scoped_print_snapshot(
        &environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        |_invocation| async {
            Ok(AntigravityStatusSnapshot {
                model_quotas: Vec::new(),
                account_email: None,
                account_plan: None,
                source: AntigravitySnapshotSource::Cli,
                quota_summary: Some(AntigravityQuotaSummary {
                    raw_description: None,
                    groups: vec![
                        AntigravityQuotaSummaryGroup {
                            key: "gemini".into(),
                            label: "Gemini Models".into(),
                            buckets: vec![AntigravityQuotaSummaryBucket {
                                bucket_id: "gemini-weekly".into(),
                                display_name: Some("Weekly Limit Remaining".into()),
                                window: Some("weekly".into()),
                                disabled: false,
                                remaining_fraction: Some(1.0),
                                reset_time: None,
                                reset_description: None,
                            }],
                        },
                        AntigravityQuotaSummaryGroup {
                            key: "3p".into(),
                            label: "Claude and GPT models".into(),
                            buckets: vec![AntigravityQuotaSummaryBucket {
                                bucket_id: "3p-weekly".into(),
                                display_name: Some("Weekly Limit Remaining".into()),
                                window: Some("weekly".into()),
                                disabled: false,
                                remaining_fraction: Some(1.0),
                                reset_time: None,
                                reset_description: None,
                            }],
                        },
                    ],
                }),
            })
        },
        |_request| async { Ok(Some("starter@example.com".into())) },
        Some(|_updated| async { Ok(()) }),
    )
    .await
    .unwrap();
    assert_eq!(result.quota_summary.as_ref().unwrap().groups.len(), 2);
    let usage = result.to_usage_snapshot_at(Timestamp::now()).unwrap();
    let windows = usage.extra_rate_windows.unwrap();
    assert_eq!(windows.len(), 2);
    assert_eq!(
        windows
            .iter()
            .map(|window| window.window.window_minutes)
            .collect::<Vec<_>>(),
        vec![Some(10_080), Some(10_080)]
    );
    assert_eq!(
        windows
            .iter()
            .map(|window| window.window.remaining_percent())
            .collect::<Vec<_>>(),
        vec![100.0, 100.0]
    );
}

// upstream: AntigravityScopedPrintFetchTests.swift:293 refreshed id token cannot relabel a verified account on the next fetch
#[tokio::test]
async fn refreshed_id_token_cannot_relabel_a_verified_account_on_the_next_fetch() {
    let environment = account_environment(&credentials("scoped@example.com"));
    let persisted = Arc::new(Mutex::new(false));
    let persisted_copy = Arc::clone(&persisted);
    let result = fetch_scoped_print_snapshot(
        &environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        |invocation| async move {
            let path = invocation
                .home
                .join(AntigravityScopedAgyStaging::TOKEN_RELATIVE_PATH);
            let mut payload: AntigravityAgyFileTokenPayload =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert!(payload.id_token.is_some());
            payload.id_token = Some(id_token("sibling@example.com"));
            std::fs::write(path, serde_json::to_vec(&payload).unwrap()).unwrap();
            Ok(report())
        },
        |_request| async { Ok(Some("scoped@example.com".into())) },
        Some(move |_updated| {
            *persisted_copy.lock().unwrap() = true;
            async { Ok(()) }
        }),
    )
    .await;
    assert!(matches!(
        result,
        Err(AntigravityScopedPrintError::Staging(
            AntigravityScopedStagingError::IdentityUnverifiable
        ))
    ));
    assert!(!*persisted.lock().unwrap());
}

// upstream: AntigravityScopedPrintFetchTests.swift:355 scoped print rejects a report when the effective token belongs to another account
#[tokio::test]
async fn scoped_print_rejects_a_report_when_the_effective_token_belongs_to_another_account() {
    let environment = account_environment(&credentials("scoped@example.com"));
    let persisted = Arc::new(Mutex::new(false));
    let persisted_copy = Arc::clone(&persisted);
    let result = fetch_scoped_print_snapshot(
        &environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        |invocation| async move {
            let path = invocation
                .home
                .join(AntigravityScopedAgyStaging::TOKEN_RELATIVE_PATH);
            let mut payload: AntigravityAgyFileTokenPayload =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            payload.token.access_token = "donor-access-token".into();
            std::fs::write(path, serde_json::to_vec(&payload).unwrap()).unwrap();
            Ok(report())
        },
        |request| async move {
            assert_eq!(request.access_token.expose_secret(), "donor-access-token");
            Ok(Some("donor@example.com".into()))
        },
        Some(move |_updated| {
            *persisted_copy.lock().unwrap() = true;
            async { Ok(()) }
        }),
    )
    .await;
    assert!(matches!(
        result,
        Err(AntigravityScopedPrintError::AccountMismatch { .. })
    ));
    assert!(!*persisted.lock().unwrap());
}

// upstream: AntigravityScopedPrintFetchTests.swift:390 scoped print persists refreshed staged credentials for the next run
#[tokio::test]
async fn scoped_print_persists_refreshed_staged_credentials_for_the_next_run() {
    let environment = account_environment(&credentials("scoped@example.com"));
    let persisted = Arc::new(Mutex::new(None));
    let persisted_copy = Arc::clone(&persisted);
    let result = fetch_scoped_print_snapshot(
        &environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        |invocation| async move {
            let path = invocation
                .home
                .join(AntigravityScopedAgyStaging::TOKEN_RELATIVE_PATH);
            let mut payload: AntigravityAgyFileTokenPayload =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert!(payload.id_token.is_some());
            payload.token.access_token = "refreshed-access-token".into();
            payload.token.refresh_token = "refreshed-refresh".into();
            payload.token.expiry = "2030-01-01T00:00:00Z".into();
            std::fs::write(path, serde_json::to_vec(&payload).unwrap()).unwrap();
            Ok(report())
        },
        |_request| async { Ok(Some("scoped@example.com".into())) },
        Some(move |updated| {
            *persisted_copy.lock().unwrap() = Some(updated);
            async { Ok(()) }
        }),
    )
    .await
    .unwrap();
    assert_eq!(result.account_email.as_deref(), Some("scoped@example.com"));
    let updated = persisted.lock().unwrap().take().unwrap();
    assert_eq!(updated.access_token, "refreshed-access-token");
    assert_eq!(updated.refresh_token.as_deref(), Some("refreshed-refresh"));
    let second_environment = account_environment(&updated);
    let second_result = fetch_scoped_print_snapshot(
        &second_environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        |invocation| async move {
            let path = invocation
                .home
                .join(AntigravityScopedAgyStaging::TOKEN_RELATIVE_PATH);
            let payload: AntigravityAgyFileTokenPayload =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            assert_eq!(payload.token.access_token, "refreshed-access-token");
            assert!(!payload.token.access_token.contains("scoped-access-token"));
            Ok(report())
        },
        |_request| async { Ok(Some("scoped@example.com".into())) },
        Some(|_updated| async { Ok(()) }),
    )
    .await
    .unwrap();
    assert_eq!(
        second_result.account_email.as_deref(),
        Some("scoped@example.com")
    );
}

// upstream: AntigravityScopedPrintFetchTests.swift:449 scoped print attributes token-only credentials after userinfo match
#[tokio::test]
async fn scoped_print_attributes_token_only_credentials_after_userinfo_match() {
    let mut token_only = credentials("scoped@example.com");
    token_only.id_token = None;
    let environment = account_environment(&token_only);
    let result = fetch_scoped_print_snapshot(
        &environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        |_invocation| async { Ok(report()) },
        |_request| async { Ok(Some("scoped@example.com".into())) },
        Some(|_updated| async { Ok(()) }),
    )
    .await
    .unwrap();
    assert_eq!(result.account_email.as_deref(), Some("scoped@example.com"));
}

// upstream: AntigravityScopedPrintFetchTests.swift:469 scoped print rejects a report when the effective account cannot be verified
#[tokio::test]
async fn scoped_print_rejects_a_report_when_the_effective_account_cannot_be_verified() {
    let environment = account_environment(&credentials("scoped@example.com"));
    let result = fetch_scoped_print_snapshot(
        &environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        |_invocation| async { Ok(report()) },
        |_request| async { Ok(None) },
        Some(|_updated| async { Ok(()) }),
    )
    .await;
    assert!(matches!(
        result,
        Err(AntigravityScopedPrintError::Staging(
            AntigravityScopedStagingError::IdentityUnverifiable
        ))
    ));
}

// upstream: AntigravityScopedPrintFetchTests.swift:490 scoped print refuses undecodable injected credentials without spawning
#[tokio::test]
async fn scoped_print_refuses_undecodable_injected_credentials_without_spawning() {
    let environment = BTreeMap::<String, String>::from([(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
        "malformed".into(),
    )]);
    let invoked = Arc::new(Mutex::new(false));
    let invoked_copy = Arc::clone(&invoked);
    let result = fetch_scoped_print_snapshot(
        &environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        move |_invocation| {
            *invoked_copy.lock().unwrap() = true;
            async { Ok(report()) }
        },
        |_request| async { Ok(Some("scoped@example.com".into())) },
        Some(|_updated| async { Ok(()) }),
    )
    .await;
    assert!(matches!(
        result,
        Err(AntigravityScopedPrintError::Staging(
            AntigravityScopedStagingError::CredentialsMissingRequiredFields
        ))
    ));
    assert!(!*invoked.lock().unwrap());
}

// upstream: AntigravityScopedPrintFetchTests.swift:505 scoped print maps stderr to a classified failure
#[tokio::test]
async fn scoped_print_maps_stderr_to_a_classified_failure() {
    let environment = account_environment(&credentials("scoped@example.com"));
    let result = fetch_scoped_print_snapshot(
        &environment,
        Duration::from_secs(5),
        &CancellationToken::new(),
        |_invocation| async {
            Err(AntigravityCLIPrintFailure::non_zero_exit_error(
                1,
                b"Eligibility check failed: account does not support Google ToS",
            ))
        },
        |_request| async { panic!("userinfo must not run after a classified print failure") },
        Some(|_updated| async { Ok(()) }),
    )
    .await;
    assert_eq!(
        result,
        Err(AntigravityScopedPrintError::Print(
            AntigravityStatusProbeError::CliReportFailed(AntigravityCLIPrintFailure::Exited {
                code: 1,
                reason: ExitReason::Ineligible,
            },),
        ))
    );
}

// upstream: AntigravityScopedPrintFetchTests.swift:271 userinfo cancellation propagates without persisting credentials
#[tokio::test]
async fn userinfo_cancellation_propagates_without_persisting_credentials() {
    for url_cancellation in [false, true] {
        let environment = account_environment(&credentials("scoped@example.com"));
        let persisted = Arc::new(Mutex::new(false));
        let persisted_copy = Arc::clone(&persisted);
        let result = fetch_scoped_print_snapshot(
            &environment,
            Duration::from_secs(5),
            &CancellationToken::new(),
            |_invocation| async { Ok(report()) },
            move |_request| async move {
                if url_cancellation {
                    Err(ScopedUserInfoError::TransportCancelled)
                } else {
                    Err(ScopedUserInfoError::Cancelled)
                }
            },
            Some(move |_updated| {
                *persisted_copy.lock().unwrap() = true;
                async { Ok(()) }
            }),
        )
        .await;
        assert!(matches!(
            result,
            Err(AntigravityScopedPrintError::Cancelled)
        ));
        assert!(!*persisted.lock().unwrap());
    }
}

#[test]
fn userinfo_uses_the_pinned_google_endpoint() {
    assert_eq!(
        ANTIGRAVITY_USERINFO_URL,
        "https://www.googleapis.com/oauth2/v3/userinfo"
    );
}

// Keep the secret wrapper in this file's type inventory so future test seams
// cannot regress to a plain debug-printable token argument.
#[allow(dead_code)]
fn _secret_wrapper_is_used(token: &str) -> SecretString {
    SecretString::new(token.to_owned())
}
