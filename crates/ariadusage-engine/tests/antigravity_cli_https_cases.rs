// Ported from CodexBar Tests/CodexBarTests/AntigravityCLIHTTPSFetchStrategyTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ariadusage_core::model::{ProviderIdentitySnapshot, UsageSnapshot};
use ariadusage_core::pipeline::{
    BoxFuture, FetchContext, FetchError, FetchInteraction, FetchPhase, FetchResult, FetchRuntime,
    FetchStrategy, ProviderPlan, Sleeper, fetch_outcome,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::providers::antigravity::cli_https::{
    SnapshotWaitDependencies, wait_for_snapshot,
};
use ariadusage_engine::providers::antigravity::oauth_store::{
    AntigravityOAuthCredentials, AntigravityOAuthCredentialsStore,
};
use ariadusage_engine::providers::antigravity::{
    ANTIGRAVITY_APP_LOCAL_STRATEGY_ID, ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
    ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID, ANTIGRAVITY_OAUTH_STRATEGY_ID,
    ANTIGRAVITY_OFFLINE_STRATEGY_ID, AntigravityConnectionEndpoint, AntigravityModelQuota,
    AntigravityOfflineFetchStrategy, AntigravityProviderPlan, AntigravitySelectedAccountGuard,
    AntigravityStatusFetchStrategy, AntigravityStatusProbeError, AntigravityStatusSnapshot,
    AntigravityStrategyConfig, EndpointSource, OperationDeadline, RequestContext, RequestPayload,
};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::metric::Confidence;
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use base64::Engine;
use jiff::Timestamp;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:100 local strategy falls back to cli HTTPS in cli source mode
#[test]
fn local_strategy_falls_back_to_cli_https_in_cli_source_mode() {
    let strategy = local_strategy();
    assert!(strategy.should_fallback(
        &FetchError::Classified(AntigravityStatusProbeError::NotRunning.into_classified_error()),
        &context(SourceMode::Cli),
    ));
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:108 local strategy falls back to cli HTTPS in auto source mode
#[test]
fn local_strategy_falls_back_to_cli_https_in_auto_source_mode() {
    let strategy = local_strategy();
    assert!(strategy.should_fallback(
        &FetchError::Classified(AntigravityStatusProbeError::NotRunning.into_classified_error()),
        &context(SourceMode::Auto),
    ));
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:116 local strategy does not fallback for unrelated source modes
#[test]
fn local_strategy_does_not_fallback_for_unrelated_source_modes() {
    let strategy = local_strategy();
    for mode in [SourceMode::Oauth, SourceMode::Web, SourceMode::Api] {
        assert!(!strategy.should_fallback(
            &FetchError::Classified(
                AntigravityStatusProbeError::NotRunning.into_classified_error()
            ),
            &context(mode),
        ));
    }
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:131 strategy pipeline includes cli HTTPS fallback in cli and auto modes
#[test]
fn strategy_pipeline_includes_cli_https_fallback_in_cli_and_auto_modes() {
    let plan = AntigravityProviderPlan::new(config());
    for mode in [SourceMode::Cli, SourceMode::Auto] {
        assert_eq!(
            plan.resolve_strategy_ids(&context(mode)),
            vec![
                ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
                ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
                ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
                ANTIGRAVITY_OFFLINE_STRATEGY_ID,
            ]
        );
    }
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:154 strategy pipeline keeps source mode authoritative with selected token account
#[test]
fn strategy_pipeline_keeps_source_mode_authoritative_with_selected_token_account() {
    let plan = AntigravityProviderPlan::new(config());
    let mut auto = context(SourceMode::Auto);
    auto.selected_token_account = Some("selected".into());
    let mut cli = context(SourceMode::Cli);
    cli.selected_token_account = Some("selected".into());
    let mut oauth = context(SourceMode::Oauth);
    oauth.selected_token_account = Some("selected".into());

    assert_eq!(
        plan.resolve_strategy_ids(&auto),
        vec![
            ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_OAUTH_STRATEGY_ID,
            ANTIGRAVITY_OFFLINE_STRATEGY_ID,
        ]
    );
    assert_eq!(
        plan.resolve_strategy_ids(&cli),
        vec![
            ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_OFFLINE_STRATEGY_ID,
        ]
    );
    assert_eq!(
        plan.resolve_strategy_ids(&oauth),
        vec![ANTIGRAVITY_OAUTH_STRATEGY_ID]
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:182 auto strategy pipeline includes oauth when credentials are injected
#[test]
fn auto_strategy_pipeline_includes_oauth_when_credentials_are_injected() {
    let plan = AntigravityProviderPlan::new(config());
    let mut cx = context(SourceMode::Auto);
    cx.env.insert(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
        SecretString::new(selected_credentials("selected@example.com")),
    );
    assert_eq!(
        plan.resolve_strategy_ids(&cx),
        vec![
            ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_OAUTH_STRATEGY_ID,
            ANTIGRAVITY_OFFLINE_STRATEGY_ID,
        ]
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:200 auto strategy pipeline preserves oauth fallback for shared credentials file
#[test]
fn auto_strategy_pipeline_preserves_oauth_fallback_for_shared_credentials_file() {
    let home_dir = tempdir().unwrap();
    let home = home_dir.path();
    let credentials = AntigravityOAuthCredentials::new(
        "access",
        Some("refresh".into()),
        Some(Timestamp::now()),
        None,
        Some("legacy@example.com".into()),
        None,
        None,
        None,
    );
    let url = AntigravityOAuthCredentialsStore::credentials_url(home);
    std::fs::create_dir_all(url.parent().unwrap()).unwrap();
    AntigravityOAuthCredentialsStore::save(&credentials, &url).unwrap();

    let plan = AntigravityProviderPlan::new(config_at(home));
    let mut cx = context(SourceMode::Auto);
    cx.env.insert(
        "HOME".into(),
        SecretString::new(home.to_string_lossy().to_string()),
    );
    assert_eq!(
        plan.resolve_strategy_ids(&cx),
        vec![
            ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_OAUTH_STRATEGY_ID,
            ANTIGRAVITY_OFFLINE_STRATEGY_ID,
        ]
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:330 cli HTTPS resets session only for one-shot CLI runtime
#[test]
fn cli_https_resets_session_only_for_one_shot_cli_runtime() {
    assert!(ariadusage_engine::providers::antigravity::AntigravityCLIHTTPSFetchStrategy::should_reset_session_after_fetch(FetchRuntime::Cli, false));
    assert!(!ariadusage_engine::providers::antigravity::AntigravityCLIHTTPSFetchStrategy::should_reset_session_after_fetch(FetchRuntime::Daemon, false));
    assert!(!ariadusage_engine::providers::antigravity::AntigravityCLIHTTPSFetchStrategy::should_reset_session_after_fetch(FetchRuntime::Cli, true));
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:341 cli HTTPS reports public source as cli
#[test]
fn cli_https_reports_public_source_as_cli() {
    assert_eq!(
        ariadusage_engine::providers::antigravity::AntigravityCLIHTTPSFetchStrategy::SOURCE_LABEL,
        "cli"
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:346 cli local strategy availability requires binary
#[tokio::test]
async fn cli_local_strategy_availability_requires_binary() {
    let runtime = ariadusage_engine::providers::antigravity::CliRuntime {
        binary_resolver: Some(Arc::new(|_, home| Some(home.join("synthetic-agy")))),
        ..Default::default()
    };
    let strategy =
        ariadusage_engine::providers::antigravity::AntigravityCLIHTTPSFetchStrategy::new(config())
            .with_runtime(Arc::new(runtime));
    assert!(strategy.is_available(&context(SourceMode::Cli)).await);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:363 cli local endpoints remain HTTPS only on macOS
#[test]
fn cli_local_endpoints_are_https_only() {
    let endpoints =
        ariadusage_engine::providers::antigravity::AntigravityStatusProbe::cli_endpoints_for_os(
            &[55624],
            false,
        );
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].scheme, "https");
    assert_eq!(endpoints[0].port, 55624);
    assert!(endpoints[0].csrf_token.is_empty());
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:880 cli HTTPS endpoint does not require CSRF token
#[test]
fn cli_https_endpoint_does_not_require_csrf_token() {
    let endpoint = AntigravityConnectionEndpoint::new(
        "https",
        55624,
        "ignored-by-cli",
        EndpointSource::CliHttps,
    );
    assert!(!endpoint.requires_csrf_token());
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:890 languageServer endpoint requires CSRF token
#[test]
fn language_server_endpoint_requires_csrf_token() {
    let endpoint =
        AntigravityConnectionEndpoint::new("https", 64440, "", EndpointSource::LanguageServer);
    assert!(endpoint.requires_csrf_token());
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:900 extensionServer endpoint requires CSRF token
#[test]
fn extension_server_endpoint_requires_csrf_token() {
    let endpoint =
        AntigravityConnectionEndpoint::new("http", 64432, "", EndpointSource::ExtensionServer);
    assert!(endpoint.requires_csrf_token());
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:229 account guard ignores fetches without a selected account
#[test]
fn account_guard_ignores_fetches_without_a_selected_account() {
    let usage = usage(Some("ambient@example.com"));
    let cx = context(SourceMode::Auto);
    assert!(AntigravitySelectedAccountGuard::validate(&usage, &cx).is_ok());
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:239 account guard accepts matching ambient snapshot in auto mode
#[test]
fn account_guard_accepts_matching_ambient_snapshot_in_auto_mode() {
    let usage = usage(Some("selected@example.com"));
    let mut cx = context(SourceMode::Auto);
    cx.selected_token_account = Some("selected".into());
    cx.env.insert(
        "ANTIGRAVITY_OAUTH_CREDENTIALS_JSON".into(),
        SecretString::new(r#"{"email":"selected@example.com","access_token":"synthetic"}"#),
    );
    assert!(AntigravitySelectedAccountGuard::validate(&usage, &cx).is_ok());
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:250 account guard rejects mismatched ambient snapshot in auto mode
#[test]
fn account_guard_rejects_mismatched_ambient_snapshot_in_auto_mode() {
    let usage = usage(Some("ambient@example.com"));
    let mut cx = context(SourceMode::Auto);
    cx.selected_token_account = Some("selected".into());
    cx.env.insert(
        "ANTIGRAVITY_OAUTH_CREDENTIALS_JSON".into(),
        SecretString::new(r#"{"email":"selected@example.com","access_token":"synthetic"}"#),
    );
    assert!(matches!(
        AntigravitySelectedAccountGuard::validate(&usage, &cx),
        Err(FetchError::Classified(_))
    ));
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:266 account guard rejects snapshot without an identity email
#[test]
fn account_guard_rejects_snapshot_without_an_identity_email() {
    let usage = usage(None);
    let mut cx = context(SourceMode::Auto);
    cx.selected_token_account = Some("selected".into());
    cx.env.insert(
        "ANTIGRAVITY_OAUTH_CREDENTIALS_JSON".into(),
        SecretString::new(r#"{"email":"selected@example.com","access_token":"synthetic"}"#),
    );
    assert!(AntigravitySelectedAccountGuard::validate(&usage, &cx).is_err());
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:282 account guard rejects when selected account email cannot be resolved
#[test]
fn account_guard_rejects_when_selected_account_email_cannot_be_resolved() {
    let usage = usage(Some("ambient@example.com"));
    let mut cx = context(SourceMode::Auto);
    cx.selected_token_account = Some("selected".into());
    assert!(matches!(
        AntigravitySelectedAccountGuard::validate(&usage, &cx),
        Err(FetchError::Classified(_))
    ));
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:308 selected account email resolves from id_token when email field missing
#[test]
fn selected_account_email_resolves_from_id_token_when_email_field_missing() {
    let mut cx = context(SourceMode::Auto);
    cx.selected_token_account = Some("selected".into());
    cx.env.insert(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
        SecretString::new(selected_credentials_with_id_token("jwt@example.com", None)),
    );
    assert_eq!(
        AntigravitySelectedAccountGuard::selected_account_email(&cx).as_deref(),
        Some("jwt@example.com")
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:319 selected account email prefers id_token over stored email field
#[test]
fn selected_account_email_prefers_id_token_over_stored_email_field() {
    let mut cx = context(SourceMode::Auto);
    cx.selected_token_account = Some("selected".into());
    cx.env.insert(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
        SecretString::new(selected_credentials_with_id_token(
            "jwt@example.com",
            Some("stored@example.com"),
        )),
    );
    assert_eq!(
        AntigravitySelectedAccountGuard::selected_account_email(&cx).as_deref(),
        Some("jwt@example.com")
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:297 account guard leaves explicit cli source mode authoritative
#[test]
fn account_guard_leaves_explicit_cli_source_mode_authoritative() {
    let usage = usage(Some("ambient@example.com"));
    let mut cx = context(SourceMode::Cli);
    cx.selected_token_account = Some("selected".into());
    assert!(AntigravitySelectedAccountGuard::validate(&usage, &cx).is_ok());
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:988 unavailable IDE cannot replace an attempted source failure
#[test]
fn unavailable_ide_cannot_replace_an_attempted_source_failure() {
    let attempted = [
        AntigravityStatusProbeError::AuthenticationRequired,
        AntigravityStatusProbeError::ApiError("quota request rejected".into()),
        AntigravityStatusProbeError::TimedOut,
        AntigravityStatusProbeError::ParseFailed("missing quota fields".into()),
        AntigravityStatusProbeError::CliReportFailed(
            ariadusage_engine::providers::antigravity::cli_print_failure::AntigravityCLIPrintFailure::Exited {
                code: 1,
                reason: ariadusage_engine::providers::antigravity::cli_print_failure::ExitReason::Unspecified,
            },
        ),
    ];
    for cli_error in attempted {
        for ide_error in [
            AntigravityStatusProbeError::NotRunning,
            AntigravityStatusProbeError::MissingCSRFToken,
        ] {
            let resolved =
                AntigravityProviderPlan::resolve_probe_fallback_error(Some(&cli_error), ide_error);
            assert_eq!(resolved, cli_error);
        }
    }
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1040 first error and newly detected fallback remain authoritative
#[test]
fn first_error_and_newly_detected_fallback_remain_authoritative() {
    for current in [
        AntigravityStatusProbeError::NotRunning,
        AntigravityStatusProbeError::MissingCSRFToken,
        AntigravityStatusProbeError::TimedOut,
    ] {
        assert_eq!(
            AntigravityProviderPlan::resolve_probe_fallback_error(None, current.clone()),
            current
        );
        assert_eq!(
            AntigravityProviderPlan::resolve_probe_fallback_error(
                Some(&AntigravityStatusProbeError::NotRunning),
                current.clone(),
            ),
            current
        );
    }
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1069 later substantive fallback retains its existing precedence
#[test]
fn later_substantive_fallback_retains_existing_precedence() {
    let app = AntigravityStatusProbeError::ApiError("quota request rejected".into());
    for oauth in [
        AntigravityStatusProbeError::AuthenticationRequired,
        AntigravityStatusProbeError::ApiError("OAuth credentials expired".into()),
        AntigravityStatusProbeError::ParseFailed("empty quota payload".into()),
    ] {
        assert_eq!(
            AntigravityProviderPlan::resolve_probe_fallback_error(Some(&app), oauth.clone()),
            oauth
        );
    }
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1168 offline fallback explains the masked live failure
#[test]
fn offline_fallback_explains_the_masked_live_failure() {
    let error = AntigravityStatusProbeError::CliReportFailed(
        ariadusage_engine::providers::antigravity::cli_print_failure::AntigravityCLIPrintFailure::Exited {
            code: 1,
            reason: ariadusage_engine::providers::antigravity::cli_print_failure::ExitReason::EligibilityNetwork,
        },
    );
    let diagnostic =
        AntigravityOfflineFetchStrategy::diagnostic_for_prior_probe_failure(&error).unwrap();
    assert_eq!(
        diagnostic,
        "Live Antigravity usage is unavailable; showing offline data. Antigravity CLI usage report failed: agy exited 1; the eligibility check failed on a network request (check network or proxy settings)"
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1203 offline diagnostic never leaks raw process output
#[test]
fn offline_diagnostic_never_leaks_raw_process_output() {
    let error =
        AntigravityStatusProbeError::ApiError("HTTP 500: {\"secret\":\"token-value\"}".into());
    let diagnostic =
        AntigravityOfflineFetchStrategy::diagnostic_for_prior_probe_failure(&error).unwrap();
    assert!(!diagnostic.contains("token-value"));
    assert!(diagnostic.contains("HTTP 500"));
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1257 clean offline fallback stays silent about earlier sources
#[test]
fn clean_offline_fallback_stays_silent_about_earlier_sources() {
    assert_eq!(
        AntigravityOfflineFetchStrategy::diagnostic_for_prior_probe_failure(
            &AntigravityStatusProbeError::Cancelled,
        ),
        None
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:824 cli HTTPS reports last readiness error when ports never become usable
#[tokio::test]
async fn cli_https_reports_last_readiness_error_when_ports_never_become_usable() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let clock_ticks = Arc::new(AtomicUsize::new(0));
    let attempts_for_fetch = Arc::clone(&attempts);
    let clock_for_fetch = Arc::clone(&clock_ticks);
    let start = std::time::Instant::now();
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(|_, _| Box::pin(async { Ok(vec![50080]) })),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(move |_, _| {
            let attempt = attempts_for_fetch.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                Err(AntigravityStatusProbeError::ApiError(format!(
                    "HTTP 500: warming attempt {attempt}"
                )))
            })
        }),
        now: Arc::new(move || {
            start + Duration::from_secs((clock_for_fetch.fetch_add(1, Ordering::SeqCst)) as u64)
        }),
    };
    let error = wait_for_snapshot(
        123,
        start + Duration::from_secs(5),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        AntigravityStatusProbeError::ApiError("HTTP 500: warming attempt 2".into())
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:459 cli HTTPS retries empty quota snapshots until usage is parseable
#[tokio::test]
async fn cli_https_retries_empty_quota_snapshots_until_usage_is_parseable() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let fetches_for_snapshot = Arc::clone(&fetches);
    let clock_ticks = Arc::new(AtomicUsize::new(0));
    let clock_for_snapshot = Arc::clone(&clock_ticks);
    let start = std::time::Instant::now();
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(|_, _| Box::pin(async { Ok(vec![50080]) })),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(move |_, _| {
            let attempt = fetches_for_snapshot.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if attempt == 0 {
                    Ok(AntigravityStatusSnapshot::new(vec![], None, None))
                } else {
                    Ok(AntigravityStatusSnapshot::new(
                        vec![AntigravityModelQuota::new(
                            "Claude Sonnet",
                            "claude-sonnet",
                            Some(0.5),
                            None,
                            None,
                        )],
                        Some("user@example.com".into()),
                        Some("Pro".into()),
                    ))
                }
            })
        }),
        now: Arc::new(move || {
            start + Duration::from_secs((clock_for_snapshot.fetch_add(1, Ordering::SeqCst)) as u64)
        }),
    };
    let snapshot = wait_for_snapshot(
        123,
        start + Duration::from_secs(5),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
    assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:419 cli HTTPS waits for user status after ports appear
#[tokio::test]
async fn cli_https_waits_for_user_status_after_ports_appear() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let fetches_for_snapshot = Arc::clone(&fetches);
    let drains = Arc::new(AtomicUsize::new(0));
    let drains_for_output = Arc::clone(&drains);
    let clock_ticks = Arc::new(AtomicUsize::new(0));
    let clock_for_snapshot = Arc::clone(&clock_ticks);
    let start = std::time::Instant::now();
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(|_, _| Box::pin(async { Ok(vec![50080, 50081]) })),
        drain_output: Arc::new(move || {
            drains_for_output.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Vec::new() })
        }),
        fetch_snapshot: Arc::new(move |_, _| {
            let attempt = fetches_for_snapshot.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if attempt == 0 {
                    Err(AntigravityStatusProbeError::ApiError(
                        "HTTP 500: GetCascadeModelConfigData() is nil".into(),
                    ))
                } else {
                    Ok(ready_snapshot())
                }
            })
        }),
        now: Arc::new(move || {
            start + Duration::from_secs((clock_for_snapshot.fetch_add(1, Ordering::SeqCst)) as u64)
        }),
    };
    let snapshot = wait_for_snapshot(
        123,
        start + Duration::from_secs(5),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
    assert_eq!(drains.load(Ordering::SeqCst), 4);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:496 cli HTTPS keeps waiting while snapshot account is not ready yet
#[tokio::test]
async fn cli_https_keeps_waiting_while_snapshot_account_is_not_ready_yet() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let fetches_for_snapshot = Arc::clone(&fetches);
    let clock_ticks = Arc::new(AtomicUsize::new(0));
    let clock_for_snapshot = Arc::clone(&clock_ticks);
    let start = std::time::Instant::now();
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(|_, _| Box::pin(async { Ok(vec![50080]) })),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(move |_, _| {
            let attempt = fetches_for_snapshot.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(AntigravityStatusSnapshot::new(
                    vec![AntigravityModelQuota::new(
                        "Claude Sonnet",
                        "claude-sonnet",
                        Some(0.5),
                        None,
                        None,
                    )],
                    (attempt != 0).then(|| "user@example.com".into()),
                    Some("Pro".into()),
                ))
            })
        }),
        now: Arc::new(move || {
            start + Duration::from_secs((clock_for_snapshot.fetch_add(1, Ordering::SeqCst)) as u64)
        }),
    };
    let snapshot = wait_for_snapshot(
        123,
        start + Duration::from_secs(5),
        Some("user@example.com"),
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
    assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:541 cli HTTPS drains output before ports appear
#[tokio::test]
async fn cli_https_drains_output_before_ports_appear() {
    let polls = Arc::new(AtomicUsize::new(0));
    let drains = Arc::new(AtomicUsize::new(0));
    let polls_for_ports = Arc::clone(&polls);
    let drains_for_output = Arc::clone(&drains);
    let clock_ticks = Arc::new(AtomicUsize::new(0));
    let clock_for_ports = Arc::clone(&clock_ticks);
    let start = std::time::Instant::now();
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(move |_, _| {
            let poll = polls_for_ports.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if poll == 0 {
                    Ok(vec![])
                } else {
                    Ok(vec![50080])
                }
            })
        }),
        drain_output: Arc::new(move || {
            drains_for_output.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Vec::new() })
        }),
        fetch_snapshot: Arc::new(|_, _| {
            Box::pin(async {
                Ok(AntigravityStatusSnapshot::new(
                    vec![AntigravityModelQuota::new(
                        "Claude Sonnet",
                        "claude-sonnet",
                        Some(1.0),
                        None,
                        None,
                    )],
                    Some("user@example.com".into()),
                    Some("Pro".into()),
                ))
            })
        }),
        now: Arc::new(move || {
            start + Duration::from_secs((clock_for_ports.fetch_add(1, Ordering::SeqCst)) as u64)
        }),
    };
    let snapshot = wait_for_snapshot(
        123,
        start + Duration::from_secs(5),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(polls.load(Ordering::SeqCst), 2);
    assert_eq!(drains.load(Ordering::SeqCst), 3);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:577 cli HTTPS stops before probing when signed out prompt spans output chunks
#[tokio::test]
async fn cli_https_stops_before_probing_when_signed_out_prompt_spans_output_chunks() {
    let chunks = Arc::new(std::sync::Mutex::new(vec![
        b"Welcome. You are currently ".to_vec(),
        b"Welcome. You are currently not signed in.\nSelect login method:".to_vec(),
    ]));
    let polls = Arc::new(AtomicUsize::new(0));
    let chunks_for_output = Arc::clone(&chunks);
    let polls_for_ports = Arc::clone(&polls);
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(move |_, _| {
            polls_for_ports.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(vec![]) })
        }),
        drain_output: Arc::new(move || {
            let chunk = chunks_for_output.lock().unwrap().remove(0);
            Box::pin(async move { chunk })
        }),
        fetch_snapshot: Arc::new(|_, _| {
            Box::pin(async { Err(AntigravityStatusProbeError::TimedOut) })
        }),
        now: Arc::new(std::time::Instant::now),
    };
    let error = wait_for_snapshot(
        123,
        std::time::Instant::now() + Duration::from_secs(2),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error, AntigravityStatusProbeError::AuthenticationRequired);
    assert_eq!(polls.load(Ordering::SeqCst), 1);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:852 cli HTTPS preserves non transient port detection errors
#[tokio::test]
async fn cli_https_preserves_non_transient_port_detection_errors() {
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(|_, _| {
            Box::pin(async {
                Err(AntigravityStatusProbeError::PortDetectionFailed(
                    "lsof not available".into(),
                ))
            })
        }),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(|_, _| {
            Box::pin(async { Err(AntigravityStatusProbeError::TimedOut) })
        }),
        now: Arc::new(std::time::Instant::now),
    };
    let error = wait_for_snapshot(
        123,
        std::time::Instant::now() + Duration::from_secs(2),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        AntigravityStatusProbeError::PortDetectionFailed("lsof not available".into())
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:752 parsed requests recompute timeout from shared deadline between endpoints
#[tokio::test]
async fn parsed_requests_recompute_timeout_from_shared_deadline_between_endpoints() {
    let timeouts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let attempts = Arc::new(AtomicUsize::new(0));
    let timeouts_for_sender = Arc::clone(&timeouts);
    let attempts_for_sender = Arc::clone(&attempts);
    let endpoints = vec![
        AntigravityConnectionEndpoint::new("https", 50080, "", EndpointSource::CliHttps),
        AntigravityConnectionEndpoint::new("https", 50081, "", EndpointSource::CliHttps),
    ];
    let deadline = OperationDeadline::new(Duration::from_secs(2));
    let result =
        ariadusage_engine::providers::antigravity::AntigravityStatusProbe::make_parsed_request(
            RequestPayload::new("/status", serde_json::json!({})),
            &RequestContext {
                endpoints,
                timeout: Duration::from_secs(2),
            },
            &deadline,
            Arc::new(move |_, _, timeout| {
                timeouts_for_sender.lock().unwrap().push(timeout);
                let attempt = attempts_for_sender.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if attempt == 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        Err(AntigravityStatusProbeError::ApiError(
                            "first endpoint failed".into(),
                        ))
                    } else {
                        Ok(b"ok".to_vec())
                    }
                })
            }),
            |bytes| {
                String::from_utf8(bytes.to_vec()).map_err(|_| {
                    AntigravityStatusProbeError::ParseFailed("invalid test data".into())
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(result, "ok");
    let recorded = timeouts.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[1] < recorded[0]);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:797 parsed request reports timeout when shared deadline is already expired
#[tokio::test]
async fn parsed_request_reports_timeout_when_shared_deadline_is_already_expired() {
    let endpoint = AntigravityConnectionEndpoint::new("https", 50080, "", EndpointSource::CliHttps);
    let sends = Arc::new(AtomicUsize::new(0));
    let sends_for_sender = Arc::clone(&sends);
    let error =
        ariadusage_engine::providers::antigravity::AntigravityStatusProbe::make_parsed_request(
            RequestPayload::new("/status", serde_json::json!({})),
            &RequestContext {
                endpoints: vec![endpoint],
                timeout: Duration::from_secs(10),
            },
            &OperationDeadline::new(Duration::ZERO),
            Arc::new(move |_, _, _| {
                sends_for_sender.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err(AntigravityStatusProbeError::TimedOut) })
            }),
            |_| Ok::<_, AntigravityStatusProbeError>("ok"),
        )
        .await
        .unwrap_err();
    assert_eq!(error, AntigravityStatusProbeError::TimedOut);
    assert_eq!(sends.load(Ordering::SeqCst), 0);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1290 cli HTTPS retains the last pending port diagnostic until the readiness deadline
#[tokio::test]
async fn cli_https_retains_last_pending_port_diagnostic_until_readiness_deadline() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let clock_ticks = Arc::new(AtomicUsize::new(0));
    let attempts_for_ports = Arc::clone(&attempts);
    let clock_for_ports = Arc::clone(&clock_ticks);
    let start = std::time::Instant::now();
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(move |_, _| {
            let attempt = attempts_for_ports.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                Err(AntigravityStatusProbeError::PortDiscoveryPending {
                    code: 1,
                    stderr: SecretString::new(format!("namespace warning {attempt}")),
                })
            })
        }),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(|_, _| {
            Box::pin(async { Err(AntigravityStatusProbeError::TimedOut) })
        }),
        now: Arc::new(move || {
            start + Duration::from_secs((clock_for_ports.fetch_add(1, Ordering::SeqCst)) as u64)
        }),
    };
    let error = wait_for_snapshot(
        123,
        start + Duration::from_secs(5),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    match error {
        AntigravityStatusProbeError::PortDiscoveryFailed { code, stderr } => {
            assert_eq!(code, 1);
            assert_eq!(stderr.expose_secret(), "namespace warning 2");
        }
        other => panic!("expected final pending diagnostic, got {other:?}"),
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1322 cli HTTPS retains endpoint readiness failure over a later port warning
#[tokio::test]
async fn cli_https_retains_endpoint_readiness_failure_over_later_port_warning() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let clock_ticks = Arc::new(AtomicUsize::new(0));
    let attempts_for_ports = Arc::clone(&attempts);
    let clock_for_ports = Arc::clone(&clock_ticks);
    let start = std::time::Instant::now();
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(move |_, _| {
            let attempt = attempts_for_ports.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                if attempt == 1 {
                    Ok(vec![50080])
                } else {
                    Err(AntigravityStatusProbeError::PortDiscoveryPending {
                        code: 1,
                        stderr: SecretString::new("namespace warning"),
                    })
                }
            })
        }),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(|_, _| {
            Box::pin(async {
                Err(AntigravityStatusProbeError::ApiError(
                    "quota service warming".into(),
                ))
            })
        }),
        now: Arc::new(move || {
            start + Duration::from_secs((clock_for_ports.fetch_add(1, Ordering::SeqCst)) as u64)
        }),
    };
    let error = wait_for_snapshot(
        123,
        start + Duration::from_secs(5),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        AntigravityStatusProbeError::ApiError("quota service warming".into())
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1397 identified account mismatch skips the readiness wait
#[tokio::test]
async fn identified_account_mismatch_skips_the_readiness_wait() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_fetch = Arc::clone(&attempts);
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(|_, _| Box::pin(async { Ok(vec![50080]) })),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(move |_, _| {
            attempts_for_fetch.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok(AntigravityStatusSnapshot::new(
                    vec![AntigravityModelQuota::new(
                        "Claude Sonnet",
                        "claude-sonnet",
                        Some(0.5),
                        None,
                        None,
                    )],
                    Some("ambient@example.com".into()),
                    Some("Pro".into()),
                ))
            })
        }),
        now: Arc::new(std::time::Instant::now),
    };
    let start = std::time::Instant::now();
    let error = wait_for_snapshot(
        123,
        start + Duration::from_secs(30),
        Some("selected@example.com"),
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        AntigravityStatusProbeError::AccountMismatch {
            expected: Some("selected@example.com".into()),
            found: Some("ambient@example.com".into()),
        }
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1033 unavailable fallback preserves transport errors
#[test]
fn unavailable_fallback_preserves_transport_errors() {
    let net_error = FetchError::Classified(
        ariadusage_core::pipeline::ClassifiedError::new(
            ProviderErrorKind::NetworkFailure,
            "connection failed",
        )
        .with_transport(ariadusage_core::pipeline::TransportClass::CannotConnect),
    );
    let plan = FixturePlan::new(vec![]);
    let current =
        FetchError::Classified(AntigravityStatusProbeError::NotRunning.into_classified_error());
    let resolved = plan.resolve_fallback_error(Some(net_error), &current);
    match resolved {
        FetchError::Classified(error) => {
            assert_eq!(
                error.transport,
                Some(ariadusage_core::pipeline::TransportClass::CannotConnect)
            );
        }
        other => panic!("expected preserved transport error, got {other:?}"),
    }
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1050 signed out CLI still allows a usable IDE fallback
#[tokio::test]
async fn signed_out_cli_still_allows_a_usable_ide_fallback() {
    let plan = FixturePlan::new(vec![
        FixtureStrategy::failure(
            "antigravity.cli-https",
            AntigravityStatusProbeError::AuthenticationRequired,
        ),
        FixtureStrategy::success("antigravity.ide-local", "ide", None),
    ]);
    let outcome = fetch_outcome(
        &ProviderId::new("antigravity").unwrap(),
        &plan,
        &context(SourceMode::Auto),
        &NoopSleeper,
    )
    .await;
    let result = outcome.result.unwrap();
    assert_eq!(result.source_label, "ide");
    assert_eq!(outcome.attempts.len(), 2);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1081 auto keeps later substantive error and every source outcome
#[tokio::test]
async fn auto_keeps_later_substantive_error_and_every_source_outcome() {
    let plan = FixturePlan::new(vec![
        FixtureStrategy::failure(
            "antigravity.app-local",
            AntigravityStatusProbeError::ApiError("quota request rejected".into()),
        ),
        FixtureStrategy::unavailable(
            "antigravity.cli-https",
            AntigravityStatusProbeError::TimedOut,
        ),
        FixtureStrategy::failure(
            "antigravity.ide-local",
            AntigravityStatusProbeError::NotRunning,
        ),
        FixtureStrategy::failure(
            "antigravity.oauth",
            AntigravityStatusProbeError::AuthenticationRequired,
        ),
    ]);
    let outcome = fetch_outcome(
        &ProviderId::new("antigravity").unwrap(),
        &plan,
        &context(SourceMode::Auto),
        &NoopSleeper,
    )
    .await;
    assert_eq!(
        outcome
            .attempts
            .iter()
            .map(|attempt| attempt.strategy_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "antigravity.app-local",
            "antigravity.cli-https",
            "antigravity.ide-local",
            "antigravity.oauth",
        ]
    );
    assert_eq!(
        outcome.attempts[1].outcome(),
        ariadusage_core::pipeline::AttemptOutcome::Skipped
    );
    match outcome.result {
        Err(FetchError::Classified(error)) => {
            assert_eq!(error.kind, ProviderErrorKind::AuthenticationExpired)
        }
        other => panic!("expected later OAuth failure, got {other:?}"),
    }
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1118 mismatch followed by OAuth failure never claims successful fallback
#[tokio::test]
async fn mismatch_followed_by_oauth_failure_never_claims_successful_fallback() {
    let mismatch = AntigravityStatusProbeError::AccountMismatch {
        expected: Some("selected@example.com".into()),
        found: Some("other@example.com".into()),
    };
    let oauth = AntigravityStatusProbeError::ApiError("OAuth credentials expired".into());
    for allows_fallback in [true, false] {
        let plan = FixturePlan::new(vec![
            FixtureStrategy::failure("antigravity.app-local", mismatch.clone()),
            FixtureStrategy::with_fallback("antigravity.oauth", oauth.clone(), allows_fallback),
        ]);
        let outcome = fetch_outcome(
            &ProviderId::new("antigravity").unwrap(),
            &plan,
            &context(SourceMode::Auto),
            &NoopSleeper,
        )
        .await;
        assert!(outcome.result.is_err());
        assert_eq!(
            outcome.attempts[0].failure.as_ref().unwrap().kind,
            ProviderErrorKind::ProviderUnavailable
        );
    }
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1145 a successful later source suppresses error surfacing entirely
#[tokio::test]
async fn a_successful_later_source_suppresses_error_surfacing_entirely() {
    let plan = FixturePlan::new(vec![
        FixtureStrategy::failure(
            "antigravity.app-local",
            AntigravityStatusProbeError::ApiError("quota request rejected".into()),
        ),
        FixtureStrategy::success("antigravity.cli-https", "ide", None),
    ]);
    let outcome = fetch_outcome(
        &ProviderId::new("antigravity").unwrap(),
        &plan,
        &context(SourceMode::Auto),
        &NoopSleeper,
    )
    .await;
    let result = outcome.result.unwrap();
    assert_eq!(result.source_label, "ide");
    assert_eq!(result.diagnostic, None);
    assert_eq!(outcome.attempts.len(), 2);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:1236 a winner's own diagnostic outranks the prior failure note
#[tokio::test]
async fn a_winners_own_diagnostic_outranks_the_prior_failure_note() {
    let plan = FixturePlan::new(vec![
        FixtureStrategy::failure(
            "antigravity.app-local",
            AntigravityStatusProbeError::NotRunning,
        ),
        FixtureStrategy::success_with_diagnostics(
            "antigravity.ide-local",
            "ide",
            Some("winner-note"),
            Some("competing-fallback-note"),
        ),
    ]);
    let outcome = fetch_outcome(
        &ProviderId::new("antigravity").unwrap(),
        &plan,
        &context(SourceMode::Auto),
        &NoopSleeper,
    )
    .await;
    assert_eq!(
        outcome.result.unwrap().diagnostic.as_deref(),
        Some("winner-note")
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:375 cli HTTPS falls back to command model configs when quota summary and user status fail
#[tokio::test]
async fn cli_https_falls_back_to_command_model_configs_when_quota_summary_and_user_status_fail() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_for_sender = Arc::clone(&seen);
    let endpoint = AntigravityConnectionEndpoint::new("https", 50080, "", EndpointSource::CliHttps);
    let context = RequestContext {
        endpoints: vec![endpoint],
        timeout: Duration::from_secs(4),
    };
    let snapshot = ariadusage_engine::providers::antigravity::AntigravityStatusProbe::fetch_snapshot(
        &context,
        &OperationDeadline::new(Duration::from_secs(4)),
        Arc::new(move |payload, _, _| {
            let seen = Arc::clone(&seen_for_sender);
            Box::pin(async move {
                seen.lock().unwrap().push(payload.path.clone());
                match payload.path.as_str() {
                    ariadusage_engine::providers::antigravity::QUOTA_SUMMARY_PATH
                    | ariadusage_engine::providers::antigravity::GET_USER_STATUS_PATH => {
                        Err(AntigravityStatusProbeError::ApiError(
                            "endpoint unavailable".into(),
                        ))
                    }
                    ariadusage_engine::providers::antigravity::COMMAND_MODEL_CONFIG_PATH => Ok(
                        br#"{"clientModelConfigs":[{"label":"Claude Sonnet","modelOrAlias":{"model":"claude-sonnet"},"quotaInfo":{"remainingFraction":0.5}}]}"#.to_vec(),
                    ),
                    other => panic!("unexpected path {other}"),
                }
            })
        }),
    )
    .await
    .unwrap();
    assert_eq!(snapshot.model_quotas[0].label, "Claude Sonnet");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[
            ariadusage_engine::providers::antigravity::QUOTA_SUMMARY_PATH.to_owned(),
            ariadusage_engine::providers::antigravity::GET_USER_STATUS_PATH.to_owned(),
            ariadusage_engine::providers::antigravity::COMMAND_MODEL_CONFIG_PATH.to_owned(),
        ]
    );
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:614 cli HTTPS allows transient automatic sign in banner
#[tokio::test]
async fn cli_https_allows_transient_automatic_sign_in_banner() {
    let output = Arc::new(std::sync::Mutex::new(vec![
        b"Welcome. You are currently not signed in.\nSigning in...".to_vec(),
        b"user@example.com\nGemini 3.1 Pro (High)".to_vec(),
    ]));
    let output_for_drain = Arc::clone(&output);
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(|_, _| Box::pin(async { Ok(vec![50080]) })),
        drain_output: Arc::new(move || {
            let chunk = output_for_drain.lock().unwrap().remove(0);
            Box::pin(async move { chunk })
        }),
        fetch_snapshot: Arc::new(|_, _| Box::pin(async { Ok(ready_snapshot()) })),
        now: Arc::new(std::time::Instant::now),
    };
    let snapshot = wait_for_snapshot(
        123,
        std::time::Instant::now() + Duration::from_secs(2),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:648 cli HTTPS stops when managed agy reports exhausted keyring authentication
#[tokio::test]
async fn cli_https_stops_when_managed_agy_reports_exhausted_keyring_authentication() {
    let output = Arc::new(std::sync::Mutex::new(vec![
        b"You are currently not signed in.\nSigning in...".to_vec(),
        b"keyringAuth: timed out after 10s, skipping keyring auth".to_vec(),
    ]));
    let output_for_drain = Arc::clone(&output);
    let ports = Arc::new(AtomicUsize::new(0));
    let ports_for_probe = Arc::clone(&ports);
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(move |_, _| {
            ports_for_probe.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Vec::new()) })
        }),
        drain_output: Arc::new(move || {
            let chunk = output_for_drain.lock().unwrap().remove(0);
            Box::pin(async move { chunk })
        }),
        fetch_snapshot: Arc::new(|_, _| {
            Box::pin(async { Err(AntigravityStatusProbeError::TimedOut) })
        }),
        now: Arc::new(std::time::Instant::now),
    };
    let error = wait_for_snapshot(
        123,
        std::time::Instant::now() + Duration::from_secs(2),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error, AntigravityStatusProbeError::AuthenticationRequired);
    assert_eq!(ports.load(Ordering::SeqCst), 1);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:679 cli HTTPS rechecks signed out prompt after snapshot readiness
#[tokio::test]
async fn cli_https_rechecks_signed_out_prompt_after_snapshot_readiness() {
    let output = Arc::new(std::sync::Mutex::new(vec![
        Vec::new(),
        b"You are currently not signed in.\nSelect login method:".to_vec(),
    ]));
    let output_for_drain = Arc::clone(&output);
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(|_, _| Box::pin(async { Ok(vec![50080]) })),
        drain_output: Arc::new(move || {
            let chunk = output_for_drain.lock().unwrap().remove(0);
            Box::pin(async move { chunk })
        }),
        fetch_snapshot: Arc::new(|_, _| Box::pin(async { Ok(ready_snapshot()) })),
        now: Arc::new(std::time::Instant::now),
    };
    let error = wait_for_snapshot(
        123,
        std::time::Instant::now() + Duration::from_secs(2),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error, AntigravityStatusProbeError::AuthenticationRequired);
}

// upstream: AntigravityCLIHTTPSFetchStrategyTests.swift:718 cli HTTPS treats empty lsof exit as ports not ready
#[tokio::test]
async fn cli_https_treats_empty_lsof_exit_as_ports_not_ready() {
    let polls = Arc::new(AtomicUsize::new(0));
    let polls_for_ports = Arc::clone(&polls);
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(move |_, _| {
            let poll = polls_for_ports.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if poll == 0 {
                    Err(AntigravityStatusProbeError::PortDetectionFailed(
                        "no listening ports found".into(),
                    ))
                } else {
                    Ok(vec![50080])
                }
            })
        }),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(|_, _| Box::pin(async { Ok(ready_snapshot()) })),
        now: Arc::new(std::time::Instant::now),
    };
    let snapshot = wait_for_snapshot(
        123,
        std::time::Instant::now() + Duration::from_secs(2),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(polls.load(Ordering::SeqCst), 2);
}

struct NoopSleeper;

impl Sleeper for NoopSleeper {
    fn sleep<'a>(&'a self, _duration: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

#[derive(Clone)]
struct FixtureStrategy {
    id: &'static str,
    available: bool,
    error: Option<AntigravityStatusProbeError>,
    source_label: &'static str,
    diagnostic: Option<&'static str>,
    prior_failure_diagnostic: Option<&'static str>,
    allows_fallback: bool,
}

impl FixtureStrategy {
    fn failure(id: &'static str, error: AntigravityStatusProbeError) -> Self {
        Self::with_fallback(id, error, true)
    }

    fn with_fallback(
        id: &'static str,
        error: AntigravityStatusProbeError,
        allows_fallback: bool,
    ) -> Self {
        Self {
            id,
            available: true,
            error: Some(error),
            source_label: "fixture",
            diagnostic: None,
            prior_failure_diagnostic: None,
            allows_fallback,
        }
    }

    fn unavailable(id: &'static str, error: AntigravityStatusProbeError) -> Self {
        let mut strategy = Self::failure(id, error);
        strategy.available = false;
        strategy
    }

    fn success(
        id: &'static str,
        source_label: &'static str,
        diagnostic: Option<&'static str>,
    ) -> Self {
        Self {
            id,
            available: true,
            error: None,
            source_label,
            diagnostic,
            prior_failure_diagnostic: None,
            allows_fallback: true,
        }
    }

    fn success_with_diagnostics(
        id: &'static str,
        source_label: &'static str,
        diagnostic: Option<&'static str>,
        prior_failure_diagnostic: Option<&'static str>,
    ) -> Self {
        let mut strategy = Self::success(id, source_label, diagnostic);
        strategy.prior_failure_diagnostic = prior_failure_diagnostic;
        strategy
    }
}

impl FetchStrategy for FixtureStrategy {
    fn id(&self) -> &str {
        self.id
    }

    fn kind(&self) -> SourceKind {
        SourceKind::LocalProbe
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move { self.available })
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            if let Some(error) = &self.error {
                return Err(FetchError::Classified(
                    error.clone().into_classified_error(),
                ));
            }
            let mut result = FetchResult::new(
                usage(Some("synthetic@example.com")),
                self.source_label,
                self.id,
                SourceKind::LocalProbe,
            );
            if let Some(diagnostic) = self.diagnostic {
                result = result.with_diagnostic(diagnostic);
            }
            Ok(result)
        })
    }

    fn should_fallback(&self, _error: &FetchError, _cx: &FetchContext) -> bool {
        self.allows_fallback
    }

    fn diagnostic_for_prior_failure(&self, _error: &FetchError) -> Option<String> {
        self.prior_failure_diagnostic.map(str::to_owned)
    }
}

struct FixturePlan {
    strategies: Vec<FixtureStrategy>,
}

impl FixturePlan {
    fn new(strategies: Vec<FixtureStrategy>) -> Self {
        Self { strategies }
    }
}

impl ProviderPlan for FixturePlan {
    fn resolve_strategies<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(async move {
            self.strategies
                .iter()
                .cloned()
                .map(|strategy| Box::new(strategy) as Box<dyn FetchStrategy>)
                .collect()
        })
    }

    fn resolve_fallback_error(
        &self,
        previous: Option<FetchError>,
        current: &FetchError,
    ) -> FetchError {
        if matches!(current, FetchError::Classified(error) if error.kind == ProviderErrorKind::ProviderUnavailable)
            && let Some(previous) = previous
            && previous.is_preservable()
        {
            return previous.clone_fallback();
        }
        current.clone_fallback()
    }
}

fn config() -> AntigravityStrategyConfig {
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let home_dir = tempdir().unwrap();
    let home = home_dir.path();
    AntigravityStrategyConfig::new(net, home, home)
}

fn config_at(home: &std::path::Path) -> AntigravityStrategyConfig {
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let mut config = AntigravityStrategyConfig::new(net, home, home);
    config.cli = Arc::new(ariadusage_engine::providers::antigravity::CliRuntime {
        binary_resolver: Some(Arc::new(|_, _| None)),
        ..Default::default()
    });
    config
}

fn local_strategy() -> AntigravityStatusFetchStrategy {
    AntigravityStatusFetchStrategy::app_local(config())
}

fn context(source_mode: SourceMode) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("antigravity").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "synthetic-cli-https".into(),
        source_mode,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}

fn usage(email: Option<&str>) -> UsageSnapshot {
    UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        Timestamp::now(),
        email.map(|email| {
            ProviderIdentitySnapshot::new(
                Some(ProviderId::new("antigravity").unwrap()),
                Some(email.into()),
                None,
                None,
                None,
            )
        }),
        Confidence::Exact,
    )
    .unwrap()
}

fn ready_snapshot() -> AntigravityStatusSnapshot {
    AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Claude Sonnet",
            "claude-sonnet",
            Some(0.5),
            None,
            None,
        )],
        Some("user@example.com".into()),
        Some("Pro".into()),
    )
}

fn selected_credentials(email: &str) -> String {
    let credentials = AntigravityOAuthCredentials::new(
        "synthetic-access-token",
        None,
        None,
        None,
        Some(email.into()),
        None,
        None,
        None,
    );
    AntigravityOAuthCredentialsStore::token_account_value(&credentials).unwrap()
}

fn selected_credentials_with_id_token(id_email: &str, stored_email: Option<&str>) -> String {
    let payload = format!(r#"{{"email":"{id_email}"}}"#);
    let id_token = format!(
        "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
    );
    let credentials = AntigravityOAuthCredentials::new(
        "synthetic-access-token",
        None,
        None,
        Some(id_token),
        stored_email.map(str::to_owned),
        None,
        None,
        None,
    );
    AntigravityOAuthCredentialsStore::token_account_value(&credentials).unwrap()
}
