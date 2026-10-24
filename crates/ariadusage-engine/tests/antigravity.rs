// Ported from CodexBar Tests/CodexBarTests/AntigravityStatusProbeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityDeadlineTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityQuotaSummaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityQuotaWindowBoundaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityQuotaSourceParityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityOAuthCredentialsStoreTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityRemoteUsageFetcherTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravitySourceAccountIsolationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityModelLabelTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityGeminiRenameTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityCompactFallbackTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/AntigravityQuotaRepresentativeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/AntigravityLocalhostSessionLifetimeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::pipeline::{
    BoxFuture, FetchContext, FetchError, FetchInteraction, FetchPhase, FetchRuntime, ProviderPlan,
    Sleeper, fetch_outcome,
};
use ariadusage_core::providers::{SourceMode, find_by_id_str};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::providers::antigravity::{
    ANTIGRAVITY_APP_LOCAL_STRATEGY_ID, ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
    ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID, ANTIGRAVITY_OAUTH_STRATEGY_ID,
    ANTIGRAVITY_OFFLINE_STRATEGY_ID, AntigravityConnectionEndpoint, AntigravityModelQuota,
    AntigravityOAuthClient, AntigravityOAuthConfig, AntigravityOAuthCredentials,
    AntigravityOAuthCredentialsStore, AntigravityOAuthFetchStrategy,
    AntigravityOfflinePlaceholderStrategy, AntigravityProcessKind, AntigravityProviderPlan,
    AntigravityProviderPresentation, AntigravityQuotaSummary, AntigravityQuotaSummaryBucket,
    AntigravityQuotaSummaryGroup, AntigravityRemoteFetchError, AntigravityRemoteUsageFetcher,
    AntigravitySnapshotSource, AntigravityStatusProbe, AntigravityStatusProbeError,
    AntigravityStatusSnapshot, AntigravityStrategyConfig, AntigravityUsageDataSource,
    COMMAND_MODEL_CONFIG_PATH, EndpointRequestSender, EndpointSource, GET_USER_STATUS_PATH,
    IconWindowsContext, KnownFamily, LocalhostTrustPolicy, MenuBarMetric, MenuBarWindowContext,
    MenuBarWindowSelection, OperationDeadline, ProcessInfoResult, ProcessScope, RemoteHttpResponse,
    RequestContext, RequestPayload, idle_window_ids, known_family,
};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jiff::Timestamp;
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};
use tokio_util::sync::CancellationToken;

struct NoopSleeper;

impl Sleeper for NoopSleeper {
    fn sleep<'a>(&'a self, _duration: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async move {})
    }
}

fn make_fetch_context(source_mode: SourceMode) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("antigravity").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "test-antigravity".to_owned(),
        source_mode,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}

fn make_id_token(email: &str, hosted_domain: Option<&str>) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let mut payload = serde_json::Map::new();
    payload.insert("email".to_owned(), Value::String(email.to_owned()));
    if let Some(hd) = hosted_domain {
        payload.insert("hd".to_owned(), Value::String(hd.to_owned()));
    }
    let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("{header}.{payload_b64}.sig")
}

fn decode_form_body(bytes: &[u8]) -> BTreeMap<String, String> {
    let text = std::str::from_utf8(bytes).expect("utf8 form body");
    let mut map = BTreeMap::new();
    for pair in text.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').expect("key=value");
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            let byte = u8::from_str_radix(hex, 16).unwrap();
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).expect("utf8 decoded component")
}

struct TestEnvironment {
    dir: TempDir,
}

impl TestEnvironment {
    fn new() -> Self {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".gemini")).unwrap();
        fs::create_dir_all(AntigravityOAuthCredentialsStore::default_directory_url(
            dir.path(),
        ))
        .unwrap();
        Self { dir }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    #[allow(clippy::too_many_arguments)]
    fn write_antigravity_credentials(
        &self,
        access_token: &str,
        refresh_token: Option<&str>,
        expiry: Timestamp,
        id_token: Option<&str>,
        email: Option<&str>,
        project_id: Option<&str>,
        client_id: Option<&str>,
        client_secret: Option<&str>,
    ) {
        let creds = AntigravityOAuthCredentials::new(
            access_token,
            refresh_token.map(str::to_owned),
            Some(expiry),
            id_token.map(str::to_owned),
            email.map(str::to_owned),
            project_id.map(str::to_owned),
            client_id.map(str::to_owned),
            client_secret.map(str::to_owned),
        );
        let url = AntigravityOAuthCredentialsStore::credentials_url(self.home());
        AntigravityOAuthCredentialsStore::save(&creds, &url).unwrap();
    }

    fn read_antigravity_credentials_json(&self) -> serde_json::Map<String, Value> {
        let url = AntigravityOAuthCredentialsStore::credentials_url(self.home());
        let data = fs::read(url).unwrap();
        serde_json::from_slice(&data).unwrap()
    }

    fn write_gemini_credentials(&self, access_token: &str, expiry: Timestamp, id_token: &str) {
        let payload = json!({
            "access_token": access_token,
            "expiry_date": expiry.as_millisecond() as f64,
            "id_token": id_token,
        });
        fs::write(
            self.home().join(".gemini").join("oauth_creds.json"),
            serde_json::to_vec(&payload).unwrap(),
        )
        .unwrap();
    }
}

// ============================================================================
// 1. Planner, Descriptor, Selected Account Guard & Account Isolation Tests
// ============================================================================

#[test]
fn planner_resolves_strategies_across_source_modes() {
    let env = TestEnvironment::new();
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let config = AntigravityStrategyConfig::new(net, env.home(), env.home());
    let plan = AntigravityProviderPlan::new(config);

    // CLI mode: app -> cli-https -> ide -> offline
    let cli_cx = make_fetch_context(SourceMode::Cli);
    assert_eq!(
        plan.resolve_strategy_ids(&cli_cx),
        vec![
            ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_OFFLINE_STRATEGY_ID,
        ]
    );

    // OAuth mode: oauth only
    let oauth_cx = make_fetch_context(SourceMode::Oauth);
    assert_eq!(
        plan.resolve_strategy_ids(&oauth_cx),
        vec![ANTIGRAVITY_OAUTH_STRATEGY_ID]
    );

    // Web and API modes: empty
    assert!(
        plan.resolve_strategy_ids(&make_fetch_context(SourceMode::Web))
            .is_empty()
    );
    assert!(
        plan.resolve_strategy_ids(&make_fetch_context(SourceMode::Api))
            .is_empty()
    );

    // Auto mode without OAuth credentials: app -> cli-https -> ide -> offline
    let auto_cx = make_fetch_context(SourceMode::Auto);
    assert_eq!(
        plan.resolve_strategy_ids(&auto_cx),
        vec![
            ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_OFFLINE_STRATEGY_ID,
        ]
    );

    // Auto mode with selected_token_account: inserts oauth after ide
    let mut auto_selected_cx = make_fetch_context(SourceMode::Auto);
    auto_selected_cx.selected_token_account = Some("selected@example.com".to_owned());
    assert_eq!(
        plan.resolve_strategy_ids(&auto_selected_cx),
        vec![
            ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_OAUTH_STRATEGY_ID,
            ANTIGRAVITY_OFFLINE_STRATEGY_ID,
        ]
    );

    // Auto mode with env credentials key: inserts oauth after ide
    let mut auto_env_cx = make_fetch_context(SourceMode::Auto);
    auto_env_cx.env.insert(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
        SecretString::new(r#"{"access_token":"tok"}"#),
    );
    assert_eq!(
        plan.resolve_strategy_ids(&auto_env_cx),
        vec![
            ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_OAUTH_STRATEGY_ID,
            ANTIGRAVITY_OFFLINE_STRATEGY_ID,
        ]
    );

    // Auto mode with shared credentials file: inserts oauth after ide
    env.write_antigravity_credentials(
        "shared-token",
        None,
        Timestamp::from_second(1_900_000_000).unwrap(),
        None,
        Some("user@example.com"),
        None,
        None,
        None,
    );
    assert_eq!(
        plan.resolve_strategy_ids(&auto_cx),
        vec![
            ANTIGRAVITY_APP_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID,
            ANTIGRAVITY_OAUTH_STRATEGY_ID,
            ANTIGRAVITY_OFFLINE_STRATEGY_ID,
        ]
    );
}

#[test]
fn planner_resolves_fallback_error_preserving_richer_previous_errors() {
    let not_running = AntigravityStatusProbeError::NotRunning;
    let missing_csrf = AntigravityStatusProbeError::MissingCSRFToken;
    let api_err = AntigravityStatusProbeError::ApiError("HTTP 500: boom".to_owned());
    let parse_err = AntigravityStatusProbeError::ParseFailed("bad json".to_owned());

    // When previous is None -> current
    assert_eq!(
        AntigravityProviderPlan::resolve_probe_fallback_error(None, not_running.clone()),
        not_running
    );

    // When previous is NotRunning and current is MissingCSRFToken -> current (MissingCSRFToken)
    assert_eq!(
        AntigravityProviderPlan::resolve_probe_fallback_error(
            Some(&not_running),
            missing_csrf.clone()
        ),
        missing_csrf
    );

    // When previous is ApiError and current is NotRunning or MissingCSRFToken -> preserve previous
    assert_eq!(
        AntigravityProviderPlan::resolve_probe_fallback_error(Some(&api_err), not_running.clone()),
        api_err
    );
    assert_eq!(
        AntigravityProviderPlan::resolve_probe_fallback_error(Some(&api_err), missing_csrf.clone()),
        api_err
    );

    // When current is another error (ParseFailed) -> return current
    assert_eq!(
        AntigravityProviderPlan::resolve_probe_fallback_error(Some(&api_err), parse_err.clone()),
        parse_err
    );

    // Also verify ProviderPlan::resolve_fallback_error on FetchError
    let env = TestEnvironment::new();
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let plan =
        AntigravityProviderPlan::new(AntigravityStrategyConfig::new(net, env.home(), env.home()));
    let resolved = plan.resolve_fallback_error(
        Some(FetchError::Classified(
            api_err.clone().into_classified_error(),
        )),
        &FetchError::Classified(not_running.into_classified_error()),
    );
    match resolved {
        FetchError::Classified(c) => {
            assert_eq!(c.kind, ProviderErrorKind::ApiFailure);
            assert!(c.message.contains("HTTP 500"));
        }
        other => panic!("expected Classified, got {other:?}"),
    }
}

#[tokio::test]
async fn execute_fetch_plan_falls_back_from_app_to_ide_and_validates_selected_account() {
    let env = TestEnvironment::new();
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let selected_creds = AntigravityOAuthCredentials::new(
        "selected-token",
        None,
        Some(Timestamp::from_second(1_900_000_000).unwrap()),
        None,
        Some("selected@example.com".to_owned()),
        None,
        None,
        None,
    );
    let selected_token =
        AntigravityOAuthCredentialsStore::token_account_value(&selected_creds).unwrap();

    let config = AntigravityStrategyConfig::new(net, env.home(), env.home())
        .with_selected_token_override(Some(selected_token))
        .with_status_probe_override(|scope, _cx| {
            Box::pin(async move {
                match scope {
                    ProcessScope::AppOnly => Err(AntigravityStatusProbeError::NotRunning),
                    ProcessScope::IdeOnly => Ok(AntigravityStatusSnapshot::new(
                        vec![AntigravityModelQuota::new(
                            "Gemini 3 Pro",
                            "gemini-3-pro",
                            Some(0.6),
                            None,
                            None,
                        )],
                        Some("selected@example.com".to_owned()),
                        Some("Pro".to_owned()),
                    )),
                    ProcessScope::IdeAndCli => Err(AntigravityStatusProbeError::NotRunning),
                }
            })
        });

    let plan = AntigravityProviderPlan::new(config);
    let mut cx = make_fetch_context(SourceMode::Cli);
    cx.selected_token_account = Some("selected@example.com".to_owned());
    let outcome = fetch_outcome(
        &ProviderId::new("antigravity").unwrap(),
        &plan,
        &cx,
        &NoopSleeper,
    )
    .await;
    let res = outcome.result.expect("ide strategy should succeed");
    assert_eq!(res.strategy_id, ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID);
    assert_eq!(
        res.usage
            .primary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        60.0
    );
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravitySourceAccountIsolationTests.swift:9
fn local_source_keeps_saved_oauth_accounts_passive_without_deleting_them() {
    let account_id = "account-uuid-1";
    let token = "synthetic-oauth-credentials";
    let base_env = BTreeMap::new();

    assert_eq!(
        AntigravityUsageDataSource::Oauth.effective_selected_token_account(Some(&account_id)),
        Some(&account_id)
    );
    assert_eq!(
        AntigravityUsageDataSource::Cli.effective_selected_token_account(Some(&account_id)),
        None
    );
    let cli_env = AntigravityUsageDataSource::Cli.make_environment(&base_env, Some(token));
    assert!(!cli_env.contains_key(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY));

    assert_eq!(
        AntigravityUsageDataSource::Auto.effective_selected_token_account(Some(&account_id)),
        Some(&account_id)
    );
    let auto_env = AntigravityUsageDataSource::Auto.make_environment(&base_env, Some(token));
    assert_eq!(
        auto_env
            .get(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY)
            .map(String::as_str),
        Some(token)
    );
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityQuotaWindowBoundaryTests.swift
fn antigravity_descriptor_ignores_observed_quota_reset_boundaries_and_advertises_oauth() {
    let descriptor = find_by_id_str("antigravity").expect("antigravity descriptor");
    assert_eq!(
        descriptor.source_modes,
        &[SourceMode::Auto, SourceMode::Cli, SourceMode::Oauth]
    );
    const {
        assert!(AntigravityProviderPresentation::IGNORES_OBSERVED_QUOTA_RESET_BOUNDARIES);
    }
}

// ============================================================================
// 2. Deadlines & Localhost Session Tests
// ============================================================================

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityDeadlineTests.swift:8
fn remaining_time_computes_positive_interval() {
    let now = Instant::now();
    let deadline = now + Duration::from_secs(5);
    let remaining = AntigravityStatusProbe::remaining_time(now, deadline).unwrap();
    assert!((remaining.as_secs_f64() - 5.0).abs() < 0.01);
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityDeadlineTests.swift:16, 23
fn remaining_time_returns_none_when_expired_or_at_deadline() {
    let now = Instant::now();
    let past = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
    assert!(AntigravityStatusProbe::remaining_time(now, past).is_none());
    assert!(AntigravityStatusProbe::remaining_time(now, now).is_none());
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityDeadlineTests.swift:32
fn bounded_step_timeout_caps_to_remaining() {
    let bounded1 = AntigravityStatusProbe::bounded_step_timeout(
        Duration::from_millis(1500),
        Duration::from_secs(4),
    );
    assert_eq!(bounded1, Some(Duration::from_millis(1500)));

    let bounded2 = AntigravityStatusProbe::bounded_step_timeout(
        Duration::from_secs(6),
        Duration::from_secs(4),
    );
    assert_eq!(bounded2, Some(Duration::from_secs(4)));
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityDeadlineTests.swift:41
fn bounded_step_timeout_returns_none_when_expired() {
    assert!(
        AntigravityStatusProbe::bounded_step_timeout(Duration::ZERO, Duration::from_secs(4))
            .is_none()
    );
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityDeadlineTests.swift:49, 57, 65
fn should_abort_scan_when_expired_or_below_minimum_and_not_when_sufficient() {
    let now = Instant::now();
    let past = now.checked_sub(Duration::from_millis(100)).unwrap_or(now);
    assert!(AntigravityStatusProbe::should_abort_endpoint_scan(
        now, past
    ));

    let almost = now + Duration::from_millis(10);
    assert!(AntigravityStatusProbe::should_abort_endpoint_scan(
        now, almost
    ));

    let future = now + Duration::from_secs(2);
    assert!(!AntigravityStatusProbe::should_abort_endpoint_scan(
        now, future
    ));
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityDeadlineTests.swift:127, 151, 179
fn operation_deadline_timeout_for_next_step_and_caps_step_timeout() {
    let base = Instant::now();
    let offset_ms = Arc::new(AtomicUsize::new(0));
    let offset_clone = Arc::clone(&offset_ms);
    let op = OperationDeadline::with_now_provider(Duration::from_secs(6), move || {
        base + Duration::from_millis(offset_clone.load(Ordering::SeqCst) as u64)
    });

    // At t=0: remaining=6.0, capped at 4.0
    let step1 = op
        .timeout_for_next_step(Some(Duration::from_secs(4)))
        .unwrap();
    assert!((step1.as_secs_f64() - 4.0).abs() < 0.01);

    // At t=4.5s: remaining=1.5, below cap 4.0 -> returns 1.5
    offset_ms.store(4500, Ordering::SeqCst);
    let step2 = op
        .timeout_for_next_step(Some(Duration::from_secs(4)))
        .unwrap();
    assert!((step2.as_secs_f64() - 1.5).abs() < 0.01);

    // At t=6.1s: expired -> TimedOut
    offset_ms.store(6100, Ordering::SeqCst);
    assert!(op.has_expired());
    assert_eq!(
        op.timeout_for_next_step(Some(Duration::from_secs(4))),
        Err(AntigravityStatusProbeError::TimedOut)
    );
}

#[tokio::test]
// CodexBar: Tests/CodexBarTests/AntigravityDeadlineTests.swift:198
async fn probe_fails_fast_when_already_past_deadline() {
    let base = Instant::now();
    let offset_secs = Arc::new(AtomicUsize::new(0));
    let offset_clone = Arc::clone(&offset_secs);
    let op = OperationDeadline::with_now_provider(Duration::from_secs(1), move || {
        base + Duration::from_secs(offset_clone.load(Ordering::SeqCst) as u64)
    });
    offset_secs.store(2, Ordering::SeqCst);
    assert!(op.has_expired());

    let calls = Arc::new(AtomicUsize::new(0));
    let candidates = vec![AntigravityConnectionEndpoint::new(
        "https",
        51170,
        "token",
        EndpointSource::LanguageServer,
    )];
    let calls_clone = Arc::clone(&calls);
    let result = AntigravityStatusProbe::resolve_working_endpoint(&candidates, &op, move |_, _| {
        let calls_clone = Arc::clone(&calls_clone);
        async move {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    })
    .await;
    assert_eq!(result, Err(AntigravityStatusProbeError::TimedOut));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityStatusProbeTests.swift:318
fn localhost_trust_policy_accepts_only_loopback_hosts() {
    assert!(LocalhostTrustPolicy::should_accept_server_trust(
        "127.0.0.1"
    ));
    assert!(LocalhostTrustPolicy::should_accept_server_trust(
        "localhost"
    ));
    assert!(LocalhostTrustPolicy::should_accept_server_trust("::1"));
    assert!(!LocalhostTrustPolicy::should_accept_server_trust(
        "example.com"
    ));
    assert!(!LocalhostTrustPolicy::should_accept_server_trust(
        "127.0.0.1.evil.com"
    ));
}

#[tokio::test]
// CodexBar: TestsLinux/AntigravityLocalhostSessionLifetimeTests.swift:28
async fn concurrent_requests_to_closed_localhost_port_all_throw_without_teardown_churn() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let mut handles = Vec::new();
    for _ in 0..16 {
        let net = Arc::clone(&net);
        handles.push(tokio::spawn(async move {
            let call = BrokerCall {
                interaction: FetchInteraction::UserInitiated,
                cancel: CancellationToken::new(),
                request_id: "closed-loopback".to_owned(),
            };
            let payload = RequestPayload::new("/closed", json!({}));
            let endpoint = AntigravityConnectionEndpoint::new(
                "http",
                port,
                "",
                EndpointSource::LanguageServer,
            );
            AntigravityStatusProbe::send_loopback_request(
                &net,
                &call,
                &payload,
                &endpoint,
                Duration::from_millis(500),
            )
            .await
            .is_err()
        }));
    }

    let mut err_count = 0;
    for handle in handles {
        if handle.await.unwrap() {
            err_count += 1;
        }
    }
    assert_eq!(err_count, 16);
}

// ============================================================================
// 3. Gemini Rename, Model Labels, and Compact Fallback Tests
// ============================================================================

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityGeminiRenameTests.swift:10-35
fn gemini_rename_process_classification() {
    let gemini = "/applications/gemini.app/contents/resources/bin/language_server --csrf_token abc";
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(gemini),
        Some(AntigravityProcessKind::App)
    );

    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "/Applications/Gemini.app/Contents/MacOS/Gemini"
        ),
        None
    );

    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "node /usr/local/lib/node_modules/@google/gemini-cli/dist/index.js"
        ),
        None
    );

    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "/applications/notgemini.app/contents/resources/bin/language_server --csrf_token abc"
        ),
        None
    );
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityModelLabelTests.swift:6-46
fn antigravity_model_labels_and_retired_flash_canonicalization() {
    assert_eq!(
        AntigravityStatusSnapshot::humanized_model_id("gemini-3-pro-preview"),
        "Gemini 3 Pro Preview"
    );
    assert_eq!(
        AntigravityStatusSnapshot::humanized_model_id("gemini-2.5-flash"),
        "Gemini 2.5 Flash"
    );
    assert_eq!(
        AntigravityStatusSnapshot::humanized_model_id("example-3-1-pro-low"),
        "Example 3.1 Pro Low"
    );
    assert_eq!(
        AntigravityStatusSnapshot::humanized_model_id("gpt-api-oss"),
        "GPT API OSS"
    );
    assert!(AntigravityStatusSnapshot::humanized_model_id("").is_empty());

    let custom_quota = AntigravityModelQuota::new(
        "Custom enterprise label",
        "gemini-3-pro-preview",
        Some(1.0),
        None,
        None,
    );
    assert_eq!(
        AntigravityStatusSnapshot::quota_display_label(&custom_quota),
        "Custom enterprise label"
    );

    assert_eq!(
        AntigravityStatusSnapshot::canonical_model_id("gemini-3.6-flash"),
        "gemini-3.7-flash"
    );
    assert_eq!(
        AntigravityStatusSnapshot::canonical_model_id("gemini-3.6-flash-high"),
        "gemini-3.7-flash"
    );
    assert_eq!(
        AntigravityStatusSnapshot::canonical_model_id("GEMINI-3.6-FLASH"),
        "gemini-3.7-flash"
    );
    assert_eq!(
        AntigravityStatusSnapshot::canonical_model_id("gemini-3.5-flash-mid"),
        "gemini-3.7-flash"
    );
    assert_eq!(
        AntigravityStatusSnapshot::canonical_model_id("gemini-3-flash-agent"),
        "gemini-3.7-flash"
    );
    assert_eq!(
        AntigravityStatusSnapshot::canonical_model_id("gemini-3.7-flash"),
        "gemini-3.7-flash"
    );
    assert_eq!(
        AntigravityStatusSnapshot::canonical_model_id("claude-sonnet-4-6"),
        "claude-sonnet-4-6"
    );

    assert_eq!(
        AntigravityStatusSnapshot::humanized_model_id("gemini-3.6-flash-high"),
        "Gemini 3.7 Flash"
    );
    assert_eq!(
        AntigravityStatusSnapshot::quota_display_label(&AntigravityModelQuota::new(
            "gemini-3.6-flash",
            "gemini-3.6-flash",
            Some(0.5),
            None,
            None,
        )),
        "Gemini 3.7 Flash"
    );
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityCompactFallbackTests.swift:7-93
fn antigravity_compact_fallback_and_reset_proximity() {
    let reset_time = Timestamp::from_second(1_775_007_200).unwrap();
    let snapshot = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Gemini 3.1 Pro (Low)",
            "MODEL_PLACEHOLDER_M36",
            Some(0.5),
            Some(reset_time),
            None,
        )],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(usage.primary.as_ref().unwrap().window_minutes, None);
    assert_eq!(usage.primary.as_ref().unwrap().resets_at, Some(reset_time));

    // Local unclassified model remains available as compact fallback
    let local_unclassified = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Experimental Model",
            "MODEL_PLACEHOLDER_NEW",
            Some(0.36),
            None,
            None,
        )],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Local);
    let local_usage = local_unclassified.to_usage_snapshot().unwrap();
    assert!(local_usage.primary.is_none());
    assert!(local_usage.secondary.is_none());
    assert!(local_usage.tertiary.is_none());
    let local_extras = local_usage.extra_rate_windows.as_ref().unwrap();
    assert_eq!(
        local_extras
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        vec!["antigravity-compact-fallback-MODEL_PLACEHOLDER_NEW"]
    );
    assert_eq!(
        local_extras
            .iter()
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Experimental Model"]
    );
    assert_eq!(
        local_extras
            .iter()
            .map(|w| w.window.used_percent.round() as i64)
            .collect::<Vec<_>>(),
        vec![64]
    );

    // Remote unclassified model remains detail only
    let remote_unclassified = local_unclassified
        .clone()
        .with_source(AntigravitySnapshotSource::Remote);
    let remote_usage = remote_unclassified.to_usage_snapshot().unwrap();
    assert!(remote_usage.primary.is_none());
    assert!(remote_usage.secondary.is_none());
    assert_eq!(
        remote_usage
            .extra_rate_windows
            .as_ref()
            .unwrap()
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        vec!["MODEL_PLACEHOLDER_NEW"]
    );

    // Fully unused local model remains available as compact fallback
    let unused_local = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Experimental Model",
            "MODEL_PLACEHOLDER_NEW",
            Some(1.0),
            None,
            None,
        )],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Local);
    let unused_usage = unused_local.to_usage_snapshot().unwrap();
    let unused_extras = unused_usage.extra_rate_windows.as_ref().unwrap();
    assert_eq!(
        unused_extras[0].id,
        "antigravity-compact-fallback-MODEL_PLACEHOLDER_NEW"
    );
    assert_eq!(unused_extras[0].window.used_percent, 0.0);
}

// ============================================================================
// 4. Status Probe Tests (AntigravityStatusProbeTests.swift)
// ============================================================================

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityStatusProbeTests.swift:7-293
fn status_probe_command_line_csrf_ports_and_candidate_endpoints() {
    let antigravity = "/applications/antigravity.app/contents/resources/app/extensions/antigravity/bin/language_server_macos_arm --app_data_dir antigravity --csrf_token abc";
    assert!(AntigravityStatusProbe::is_antigravity_language_server_command_line(antigravity));

    let cli_daemon = "/users/test/.antigravity-cli/bin/language_server_macos_arm --server_port 50412 --random_port=false --workspace_id=file_users_test_workspace";
    assert!(AntigravityStatusProbe::is_antigravity_language_server_command_line(cli_daemon));

    let other_app = "/applications/cursor.app/contents/resources/app/extensions/cursor/bin/language_server_macos_arm --csrf_token abc";
    assert!(!AntigravityStatusProbe::is_antigravity_language_server_command_line(other_app));

    assert_eq!(
        AntigravityStatusProbe::resolved_csrf_token(Some("csrf-token"), None, antigravity),
        Some("csrf-token".to_owned())
    );
    assert_eq!(
        AntigravityStatusProbe::resolved_csrf_token(
            None,
            Some("extension-token"),
            "/applications/antigravity.app/bin/language_server_macos_arm --app_data_dir antigravity"
        ),
        Some("extension-token".to_owned())
    );
    assert_eq!(
        AntigravityStatusProbe::resolved_csrf_token(None, None, cli_daemon),
        Some(String::new())
    );
    assert_eq!(
        AntigravityStatusProbe::resolved_csrf_token(
            None,
            None,
            "/applications/antigravity.app/bin/language_server_macos_arm --app_data_dir antigravity"
        ),
        None
    );

    // Scoped process matching
    let ps_output = "\
101 /Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token app-token --extension_server_port 42100
202 /Users/test/.vscode/extensions/antigravity/bin/language_server --app_data_dir antigravity --csrf_token ide-token
303 /Users/test/.antigravity-cli/bin/language_server_macos_arm --server_port 51170 --csrf_token cli-token
";
    let app_only = AntigravityStatusProbe::process_infos_from_process_list_output(
        ps_output,
        ProcessScope::AppOnly,
    )
    .unwrap();
    assert_eq!(
        app_only.iter().map(|p| p.pid).collect::<Vec<_>>(),
        vec![101]
    );

    let ide_only = AntigravityStatusProbe::process_infos_from_process_list_output(
        ps_output,
        ProcessScope::IdeOnly,
    )
    .unwrap();
    assert_eq!(
        ide_only.iter().map(|p| p.pid).collect::<Vec<_>>(),
        vec![202]
    );

    let ide_and_cli = AntigravityStatusProbe::process_infos_from_process_list_output(
        ps_output,
        ProcessScope::IdeAndCli,
    )
    .unwrap();
    assert_eq!(
        ide_and_cli.iter().map(|p| p.pid).collect::<Vec<_>>(),
        vec![101, 202, 303]
    );

    // Returns all matching language servers in process order and reports missing CSRF when none have credentials
    let ps_multi = "\
101 /Applications/Cursor.app/Contents/Resources/app/bin/language_server --csrf_token cursor-token
202 /Applications/Antigravity.app/Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm --app_data_dir antigravity
303 /Applications/Antigravity.app/Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm --app_data_dir antigravity --csrf_token first-token --extension_server_port 42100 --extension_server_csrf_token ext-first
404 /Users/test/.antigravity-cli/bin/language_server_macos_arm --server_port 51170 --csrf_token second-token
";
    let matches = AntigravityStatusProbe::process_infos_from_process_list_output(
        ps_multi,
        ProcessScope::IdeAndCli,
    )
    .unwrap();
    assert_eq!(
        matches.iter().map(|p| p.pid).collect::<Vec<_>>(),
        vec![303, 404]
    );
    assert_eq!(
        matches
            .iter()
            .map(|p| p.csrf_token.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("first-token"), Some("second-token")]
    );
    assert_eq!(
        matches.iter().map(|p| p.extension_port).collect::<Vec<_>>(),
        vec![Some(42100), None]
    );

    assert_eq!(
        AntigravityStatusProbe::process_infos_from_process_list_output(
            "202 /Applications/Antigravity.app/Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm --app_data_dir antigravity",
            ProcessScope::IdeAndCli
        ),
        Err(AntigravityStatusProbeError::MissingCSRFToken)
    );

    // Connection candidates
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[51170, 61775],
        Some("lang-token"),
        Some(61775),
        Some("ext-token"),
    );
    assert_eq!(
        candidates,
        vec![
            AntigravityConnectionEndpoint::new(
                "https",
                51170,
                "lang-token",
                EndpointSource::LanguageServer
            ),
            AntigravityConnectionEndpoint::new(
                "https",
                61775,
                "ext-token",
                EndpointSource::ExtensionServer
            ),
        ]
    );

    let ext_only = AntigravityStatusProbe::connection_candidates(
        &[51170],
        None,
        Some(61775),
        Some("ext-token"),
    );
    assert_eq!(
        ext_only,
        vec![AntigravityConnectionEndpoint::new(
            "https",
            61775,
            "ext-token",
            EndpointSource::ExtensionServer
        )]
    );

    let reused_csrf = AntigravityStatusProbe::connection_candidates(
        &[51170, 61775],
        Some("lang-token"),
        Some(61775),
        None,
    );
    assert_eq!(
        reused_csrf,
        vec![
            AntigravityConnectionEndpoint::new(
                "https",
                51170,
                "lang-token",
                EndpointSource::LanguageServer
            ),
            AntigravityConnectionEndpoint::new(
                "https",
                61775,
                "lang-token",
                EndpointSource::ExtensionServer
            ),
        ]
    );

    let empty_csrf =
        AntigravityStatusProbe::connection_candidates(&[51170], Some(""), Some(61775), None);
    assert_eq!(
        empty_csrf,
        vec![AntigravityConnectionEndpoint::new(
            "https",
            51170,
            "",
            EndpointSource::LanguageServer
        )]
    );

    // request_endpoints deduplicates while keeping primary first
    let primary = AntigravityConnectionEndpoint::new(
        "https",
        51170,
        "lang-token",
        EndpointSource::LanguageServer,
    );
    let req_endpoints = AntigravityStatusProbe::request_endpoints(&primary, &candidates);
    assert_eq!(req_endpoints, candidates);
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityStatusProbeTests.swift:295-315, 1615-1642
fn local_snapshot_selection_and_reachable_probe_errors() {
    let cli_snapshot = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Gemini 3 Pro",
            "gemini-3-pro",
            Some(0.8),
            None,
            None,
        )],
        None,
        None,
    );
    let ide_snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new("Gemini 3 Pro", "gemini-3-pro", Some(0.5), None, None),
            AntigravityModelQuota::new(
                "Claude Sonnet 4.6",
                "claude-sonnet-4-6",
                Some(0.75),
                None,
                None,
            ),
        ],
        Some("user@example.com".to_owned()),
        Some("Pro".to_owned()),
    );

    let preferred =
        AntigravityStatusProbe::preferred_local_snapshot(&[cli_snapshot, ide_snapshot]).unwrap();
    assert_eq!(preferred.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(preferred.account_plan.as_deref(), Some("Pro"));
    assert_eq!(preferred.model_quotas.len(), 2);

    assert!(AntigravityStatusProbe::is_reachable_probe_error(
        &AntigravityStatusProbeError::ApiError("HTTP 403: Forbidden".to_owned())
    ));
    assert!(AntigravityStatusProbe::is_reachable_probe_error(
        &AntigravityStatusProbeError::ApiError("HTTP 404: Not Found".to_owned())
    ));
    assert!(!AntigravityStatusProbe::is_reachable_probe_error(
        &AntigravityStatusProbeError::ApiError("Invalid response".to_owned())
    ));
    assert!(!AntigravityStatusProbe::is_reachable_probe_error(
        &AntigravityStatusProbeError::NotRunning
    ));

    assert_eq!(
        AntigravityStatusProbe::fallback_probe_port(&[51170, 61775], Some(61775)),
        Some(51170)
    );
    assert_eq!(
        AntigravityStatusProbe::fallback_probe_port(&[61775], Some(61775)),
        Some(61775)
    );
    assert_eq!(
        AntigravityStatusProbe::fallback_probe_port(&[51170, 61775], None),
        Some(51170)
    );
}

#[tokio::test]
// CodexBar: Tests/CodexBarTests/AntigravityStatusProbeTests.swift:326-795
async fn probe_endpoint_resolution_process_snapshots_and_user_status_fallback() {
    let candidates = vec![
        AntigravityConnectionEndpoint::new(
            "https",
            51170,
            "lang-token",
            EndpointSource::LanguageServer,
        ),
        AntigravityConnectionEndpoint::new(
            "https",
            61775,
            "ext-token",
            EndpointSource::ExtensionServer,
        ),
    ];

    // Working endpoint prefers reachable HTTP error over unreachable first candidate
    let op = OperationDeadline::new(Duration::from_secs(4));
    let resolved = AntigravityStatusProbe::resolve_working_endpoint(
        &candidates,
        &op,
        |candidate, _| async move {
            if candidate.port == 51170 {
                Err(AntigravityStatusProbeError::ApiError(
                    "Connection refused".to_owned(),
                ))
            } else {
                Err(AntigravityStatusProbeError::ApiError(
                    "HTTP 403: Forbidden".to_owned(),
                ))
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(resolved, candidates[1]);

    // Working endpoint propagates timeout when every candidate times out
    let timed_out =
        AntigravityStatusProbe::resolve_working_endpoint(&candidates, &op, |_, _| async move {
            Err(AntigravityStatusProbeError::TimedOut)
        })
        .await;
    assert_eq!(timed_out, Err(AntigravityStatusProbeError::TimedOut));

    // fetch_process_snapshots continues after per-process timeout when overall deadline remains
    let processes = vec![
        ProcessInfoResult {
            pid: 101,
            csrf_token: Some("first".to_owned()),
            extension_port: None,
            extension_csrf_token: None,
            command: "language_server --csrf_token first".to_owned(),
            identity: None,
        },
        ProcessInfoResult {
            pid: 202,
            csrf_token: Some("second".to_owned()),
            extension_port: None,
            extension_csrf_token: None,
            command: "language_server --csrf_token second".to_owned(),
            identity: None,
        },
    ];
    let snapshots = AntigravityStatusProbe::fetch_process_snapshots(
        &processes,
        &OperationDeadline::new(Duration::from_secs(4)),
        |process| async move {
            if process.pid == 101 {
                Err(AntigravityStatusProbeError::TimedOut)
            } else {
                Ok(AntigravityStatusSnapshot::new(
                    vec![AntigravityModelQuota::new(
                        "Gemini 3 Pro",
                        "gemini-3-pro",
                        Some(0.75),
                        None,
                        None,
                    )],
                    Some("user@example.com".to_owned()),
                    Some("Pro".to_owned()),
                ))
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        snapshots[0].account_email.as_deref(),
        Some("user@example.com")
    );

    // make_parsed_request falls back to next endpoint when first payload fails parsing
    let context = RequestContext {
        endpoints: candidates.clone(),
        timeout: Duration::from_secs(4),
    };
    let send_parse_fallback: EndpointRequestSender = Arc::new(|_payload, endpoint, _timeout| {
        Box::pin(async move {
            if endpoint.port == 51170 {
                Ok(b"{}".to_vec())
            } else {
                Ok(br#"{"userStatus":{"email":"user@example.com","cascadeModelConfigData":{"clientModelConfigs":[]}}}"#.to_vec())
            }
        })
    });
    let parsed_snap = AntigravityStatusProbe::make_parsed_request(
        RequestPayload::new(GET_USER_STATUS_PATH, json!({})),
        &context,
        &OperationDeadline::new(Duration::from_secs(4)),
        send_parse_fallback,
        AntigravityStatusProbe::parse_user_status_response,
    )
    .await
    .unwrap();
    assert_eq!(
        parsed_snap.account_email.as_deref(),
        Some("user@example.com")
    );

    // fetch_snapshot falls back to CommandModelConfigs when GetUserStatus fails
    let seen_paths = Arc::new(Mutex::new(Vec::new()));
    let seen_paths_clone = Arc::clone(&seen_paths);
    let send_cmd_fallback: EndpointRequestSender = Arc::new(move |payload, _endpoint, _timeout| {
        let seen = Arc::clone(&seen_paths_clone);
        Box::pin(async move {
            seen.lock().unwrap().push(payload.path.clone());
            if payload.path == GET_USER_STATUS_PATH {
                Ok(b"{}".to_vec())
            } else {
                Ok(br#"{"clientModelConfigs":[{"label":"Gemini 3 Pro","modelOrAlias":{"model":"gemini-3-pro"},"quotaInfo":{"remainingFraction":0.5}}]}"#.to_vec())
            }
        })
    });
    let single_ctx = RequestContext {
        endpoints: vec![candidates[0].clone()],
        timeout: Duration::from_secs(4),
    };
    let snap = AntigravityStatusProbe::fetch_snapshot(
        &single_ctx,
        &OperationDeadline::new(Duration::from_secs(4)),
        send_cmd_fallback,
    )
    .await
    .unwrap();
    assert_eq!(
        *seen_paths.lock().unwrap(),
        vec![
            GET_USER_STATUS_PATH.to_owned(),
            COMMAND_MODEL_CONFIG_PATH.to_owned()
        ]
    );
    assert_eq!(snap.model_quotas.len(), 1);

    // fetch_snapshot preserves GetUserStatus error when CommandModelConfig is unsupported (HTTP 404)
    let send_404: EndpointRequestSender = Arc::new(|payload, _endpoint, _timeout| {
        Box::pin(async move {
            if payload.path == GET_USER_STATUS_PATH {
                Ok(b"{}".to_vec())
            } else {
                Err(AntigravityStatusProbeError::ApiError(
                    "HTTP 404: Not Found".to_owned(),
                ))
            }
        })
    });
    let err_404 = AntigravityStatusProbe::fetch_snapshot(
        &single_ctx,
        &OperationDeadline::new(Duration::from_secs(4)),
        send_404,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err_404,
        AntigravityStatusProbeError::ParseFailed("Missing userStatus".to_owned())
    );
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityStatusProbeTests.swift:797-1612
fn status_snapshot_to_usage_snapshot_pools_and_source_filtering() {
    // Orders Gemini pool primary, Claude/GPT pool secondary, and omits tertiary
    let snap1 = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro Low",
                "gemini-3-pro-low",
                Some(0.5),
                None,
                None,
            ),
            AntigravityModelQuota::new("Gemini 3 Flash", "gemini-3-flash", Some(0.2), None, None),
            AntigravityModelQuota::new("Claude Sonnet 4", "claude-sonnet-4", Some(0.8), None, None),
        ],
        None,
        None,
    );
    let usage1 = snap1.to_usage_snapshot().unwrap();
    assert_eq!(
        usage1.primary.as_ref().unwrap().remaining_percent().round(),
        20.0
    );
    assert_eq!(
        usage1
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        80.0
    );
    assert!(usage1.tertiary.is_none());

    // Uses Flash when Pro is missing
    let snap2 = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new("Gemini 3 Flash", "gemini-3-flash", Some(0.1), None, None),
            AntigravityModelQuota::new("Claude Sonnet 4", "claude-sonnet-4", Some(0.3), None, None),
        ],
        None,
        None,
    );
    let usage2 = snap2.to_usage_snapshot().unwrap();
    assert_eq!(
        usage2.primary.as_ref().unwrap().remaining_percent().round(),
        10.0
    );
    assert_eq!(
        usage2
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        30.0
    );
    assert!(usage2.tertiary.is_none());

    // Uses Claude when Gemini models are missing
    let snap3 = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Claude Sonnet 4",
            "claude-sonnet-4",
            Some(0.3),
            None,
            None,
        )],
        None,
        None,
    );
    let usage3 = snap3.to_usage_snapshot().unwrap();
    assert!(usage3.primary.is_none());
    assert_eq!(
        usage3
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        30.0
    );

    // Gemini pool unavailable when only excluded Lite variants exist
    let snap4 = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini Pro Lite",
                "gemini-3-pro-lite",
                Some(0.6),
                None,
                None,
            ),
            AntigravityModelQuota::new("Claude Sonnet 4", "claude-sonnet-4", Some(0.3), None, None),
        ],
        None,
        None,
    );
    let usage4 = snap4.to_usage_snapshot().unwrap();
    assert!(usage4.primary.is_none());
    assert_eq!(
        usage4
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        30.0
    );

    // Gemini pool ignores reset-only placeholder when remaining data exists
    let snap5 = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro (Low)",
                "MODEL_PLACEHOLDER_M36",
                None,
                Some(Timestamp::from_second(1_735_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (High)",
                "MODEL_PLACEHOLDER_M37",
                Some(1.0),
                Some(Timestamp::from_second(1_735_100_000).unwrap()),
                None,
            ),
        ],
        None,
        None,
    );
    let usage5 = snap5.to_usage_snapshot().unwrap();
    assert_eq!(
        usage5.primary.as_ref().unwrap().remaining_percent().round(),
        100.0
    );
    assert!(usage5.secondary.is_none());

    // Group without remaining fraction preserves reset metadata as unavailable grouped window
    let reset_ts = Timestamp::from_second(1_735_000_000).unwrap();
    let snap_group_unknown = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Gemini 3.1 Pro (Low)",
            "MODEL_PLACEHOLDER_M36",
            None,
            Some(reset_ts),
            None,
        )],
        None,
        None,
    );
    let usage_group_unknown = snap_group_unknown.to_usage_snapshot().unwrap();
    assert!(usage_group_unknown.primary.is_none());
    let model_win = &usage_group_unknown.extra_rate_windows.as_ref().unwrap()[0];
    assert_eq!(model_win.id, "antigravity-gemini");
    assert_eq!(model_win.title, "Gemini Models");
    assert_eq!(model_win.window.resets_at, Some(reset_ts));
    assert!(!model_win.usage_known);

    // Filtered variants stay out of summary but remain distinct extras
    let snap_filtered = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro Lite",
                "gemini-3-pro-lite",
                Some(0.6),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Flash Lite",
                "gemini-3-flash-lite",
                Some(0.2),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Tab Autocomplete",
                "tab_autocomplete_model",
                Some(0.9),
                None,
                None,
            ),
        ],
        Some("test@example.com".to_owned()),
        Some("Pro".to_owned()),
    )
    .with_source(AntigravitySnapshotSource::Local);
    let usage_filtered = snap_filtered.to_usage_snapshot().unwrap();
    assert!(usage_filtered.primary.is_none());
    assert!(usage_filtered.secondary.is_none());
    assert_eq!(
        usage_filtered
            .extra_rate_windows
            .as_ref()
            .unwrap()
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "antigravity-compact-fallback-gemini-3-pro-lite",
            "antigravity-compact-fallback-gemini-3-flash-lite",
            "antigravity-compact-fallback-tab_autocomplete_model",
        ]
    );

    // Remote source collapses recognized family models and hides unconsumed junk
    let snap_remote_clean = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 2.5 Flash Image",
                "gemini-2-5-flash-image",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Tab Flash Lite Vertex",
                "tab_flash_lite_vertex",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new("Gemini 2.5 Pro", "gemini-2-5-pro", Some(1.0), None, None),
            AntigravityModelQuota::new(
                "Gemini 2.5 Flash Lite",
                "gemini-2-5-flash-lite",
                Some(1.0),
                None,
                None,
            ),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Remote);
    let usage_remote_clean = snap_remote_clean.to_usage_snapshot().unwrap();
    assert_eq!(
        usage_remote_clean
            .primary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        100.0
    );
    assert!(usage_remote_clean.extra_rate_windows.is_none());

    // Remote source shows consumed junk models despite filter
    let snap_remote_consumed = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Tab Flash Lite Vertex",
                "tab_flash_lite_vertex",
                Some(0.4),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro Image",
                "gemini-3-pro-image",
                Some(0.4),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Tab Jump Flash Lite Vertex",
                "tab_jump_flash_lite_vertex",
                Some(0.9995),
                None,
                None,
            ),
            AntigravityModelQuota::new("Gemini 3 Flash", "gemini-3-flash", Some(1.0), None, None),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Remote);
    let usage_remote_consumed = snap_remote_consumed.to_usage_snapshot().unwrap();
    let extra_ids: Vec<&str> = usage_remote_consumed
        .extra_rate_windows
        .as_ref()
        .unwrap()
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    assert!(extra_ids.contains(&"tab_flash_lite_vertex"));
    assert!(extra_ids.contains(&"gemini-3-pro-image"));
    assert!(!extra_ids.contains(&"tab_jump_flash_lite_vertex"));

    // Nil version unknown family models sort deterministically by label
    let snap_unknown_sort = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Zebra Unknown Model",
                "MODEL_PLACEHOLDER_MA",
                Some(0.5),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Alpha Unknown Model",
                "MODEL_PLACEHOLDER_MB",
                Some(0.5),
                None,
                None,
            ),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Local);
    let usage_unknown_sort = snap_unknown_sort.to_usage_snapshot().unwrap();
    let titles: Vec<&str> = usage_unknown_sort
        .extra_rate_windows
        .as_ref()
        .unwrap()
        .iter()
        .map(|w| w.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Alpha Unknown Model", "Zebra Unknown Model"]);
}

// ============================================================================
// 5. Quota Summary, Representative Selection & Source Parity Tests
// ============================================================================

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityQuotaSummaryTests.swift:7-565
fn quota_summary_parsing_and_presentation_selection() {
    let raw = br#"{
        "status": "SUCCESS",
        "command": {
            "name": "usage",
            "data": {
                "groups": [
                    {
                        "name": "Gemini Models",
                        "buckets": [
                            {
                                "id": "gemini-5h",
                                "name": "5-Hour",
                                "window": "5h",
                                "remaining_fraction": 0.65,
                                "reset_time": "2030-01-01T05:00:00Z"
                            },
                            {
                                "id": "gemini-weekly",
                                "name": "Weekly",
                                "window": "weekly",
                                "remaining_fraction": 0.4,
                                "reset_time": "2030-01-07T00:00:00Z"
                            }
                        ]
                    },
                    {
                        "name": "Claude and GPT",
                        "buckets": [
                            {
                                "id": "3p-weekly",
                                "name": "Weekly",
                                "window": "weekly",
                                "remaining_fraction": 0.25,
                                "reset_time": "2030-01-07T00:00:00Z"
                            }
                        ]
                    }
                ]
            }
        }
    }"#;

    let snapshot = AntigravityStatusProbe::parse_cli_usage_report(raw).unwrap();
    let usage = snapshot.to_usage_snapshot().unwrap();

    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        40.0
    );
    assert_eq!(usage.primary.as_ref().unwrap().window_minutes, Some(10_080));
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        25.0
    );
    assert_eq!(
        usage.secondary.as_ref().unwrap().window_minutes,
        Some(10_080)
    );
    assert_eq!(
        usage
            .extra_rate_windows
            .as_ref()
            .unwrap()
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "antigravity-quota-summary-gemini-5h",
            "antigravity-quota-summary-gemini-weekly",
            "antigravity-quota-summary-3p-weekly",
        ]
    );

    let semantic = AntigravityProviderPresentation::semantic_windows(&usage);
    assert_eq!(semantic.session.as_ref().unwrap().window_minutes, Some(300));
    assert_eq!(
        semantic
            .session
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        65.0
    );
    assert_eq!(
        semantic.weekly.as_ref().unwrap().window_minutes,
        Some(10_080)
    );
    assert_eq!(
        semantic
            .weekly
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        25.0
    );

    let icons = AntigravityProviderPresentation::icon_windows(IconWindowsContext {
        snapshot: &usage,
        secondary_override_window_id: None,
        now: usage.updated_at,
    });
    assert_eq!(icons.primary, semantic.session);
    assert_eq!(icons.secondary, semantic.weekly);

    // Exhausted quota prioritization in menuBarWindow
    let exhausted_snapshot = AntigravityStatusSnapshot::new(Vec::new(), None, None)
        .with_quota_summary(Some(AntigravityQuotaSummary::new(vec![
            AntigravityQuotaSummaryGroup::new(
                "Gemini Models",
                vec![AntigravityQuotaSummaryBucket::new(
                    "gemini-5h",
                    "5-Hour",
                    Some("5h".to_owned()),
                    Some(300),
                    Some(0.6),
                    None,
                    false,
                )],
            ),
            AntigravityQuotaSummaryGroup::new(
                "Claude and GPT",
                vec![AntigravityQuotaSummaryBucket::new(
                    "3p-weekly",
                    "Weekly",
                    Some("weekly".to_owned()),
                    Some(10_080),
                    Some(0.0),
                    None,
                    false,
                )],
            ),
        ])));
    let exhausted_usage = exhausted_snapshot.to_usage_snapshot().unwrap();
    let sel = AntigravityProviderPresentation::menu_bar_window(MenuBarWindowContext {
        metric: MenuBarMetric::Primary,
        snapshot: &exhausted_usage,
        supports_average: false,
        prioritizes_exhausted_quotas: true,
        now: exhausted_usage.updated_at,
    });
    assert_eq!(
        sel,
        MenuBarWindowSelection::Resolved(exhausted_usage.secondary.unwrap())
    );
}

#[test]
// CodexBar: TestsLinux/AntigravityQuotaRepresentativeTests.swift:9-133
fn quota_representative_selection_cases() {
    for ordinary_id in ["gemini-weekly", "gemini_weekly", "gemini-weekly limit"] {
        let report = json!({
            "status": "SUCCESS",
            "command": {
                "name": "usage",
                "data": {
                    "groups": [{
                        "name": "Gemini Models",
                        "buckets": [
                            {"id": ordinary_id, "name": "Synthetic quota", "window": "weekly", "disabled": false, "reset_time": "2030-03-17T17:46:40Z", "remaining_fraction": 0.9996},
                            {"id": "gemini-synthetic-limited-weekly", "name": "Synthetic quota", "window": "weekly", "disabled": false, "reset_time": "2030-03-17T17:46:40Z", "remaining_fraction": 0.0},
                            {"id": "gemini-synthetic-special-weekly", "name": "Synthetic quota", "window": "weekly", "disabled": false, "reset_time": "2030-03-17T17:46:40Z", "remaining_fraction": 0.1}
                        ]
                    }]
                }
            }
        });
        let usage =
            AntigravityStatusProbe::parse_cli_usage_report(&serde_json::to_vec(&report).unwrap())
                .unwrap()
                .to_usage_snapshot()
                .unwrap();
        let primary = usage.primary.as_ref().unwrap();
        assert!((primary.used_percent - 0.04).abs() < 0.000001);
        assert_eq!(primary.window_minutes, Some(10_080));
        assert_eq!(
            primary.resets_at,
            Some(Timestamp::from_second(1_900_000_000).unwrap())
        );
        let semantic = AntigravityProviderPresentation::semantic_windows(&usage);
        assert_eq!(semantic.weekly.as_ref().unwrap().used_percent, 100.0);
    }

    // Unknown quota never becomes a measured zero
    let unknown_quota = json!({
        "groups": [{
            "name": "Gemini Models",
            "buckets": [
                {"id": "gemini-weekly", "name": "Synthetic quota", "window": "weekly", "disabled": false, "reset_time": "2030-03-17T17:46:40Z"}
            ]
        }]
    });
    let unknown_usage = AntigravityStatusProbe::parse_quota_summary_response(
        &serde_json::to_vec(&unknown_quota).unwrap(),
    )
    .unwrap()
    .to_usage_snapshot()
    .unwrap();
    assert!(unknown_usage.primary.is_none());
    let semantic = AntigravityProviderPresentation::semantic_windows(&unknown_usage);
    assert!(semantic.weekly.is_none());
    assert!(!unknown_usage.extra_rate_windows.as_ref().unwrap()[0].usage_known);
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityQuotaSourceParityTests.swift:7-155
fn quota_source_parity_and_family_visibility() {
    let reset_time = Timestamp::from_second(1_775_000_000).unwrap();
    let local_snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro (High)",
                "MODEL_PLACEHOLDER_M1",
                Some(0.9),
                Some(reset_time),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (Low)",
                "MODEL_PLACEHOLDER_M2",
                Some(0.4),
                Some(reset_time),
                None,
            ),
            AntigravityModelQuota::new(
                "Claude Sonnet 4.6",
                "MODEL_PLACEHOLDER_M3",
                Some(0.75),
                Some(reset_time),
                None,
            ),
            AntigravityModelQuota::new(
                "GPT-OSS 120B (Medium)",
                "MODEL_PLACEHOLDER_M4",
                Some(0.25),
                Some(reset_time),
                None,
            ),
        ],
        Some("user@example.com".to_owned()),
        Some("Pro".to_owned()),
    )
    .with_source(AntigravitySnapshotSource::Local);

    let remote_snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro (High)",
                "gemini-3-pro-high",
                Some(0.9),
                Some(reset_time),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (Low)",
                "gemini-3-pro-low",
                Some(0.4),
                Some(reset_time),
                None,
            ),
            AntigravityModelQuota::new(
                "Claude Sonnet 4.6",
                "claude-sonnet-4-6",
                Some(0.75),
                Some(reset_time),
                None,
            ),
            AntigravityModelQuota::new(
                "GPT-OSS 120B (Medium)",
                "gpt-oss-120b-medium",
                Some(0.25),
                Some(reset_time),
                None,
            ),
        ],
        Some("user@example.com".to_owned()),
        Some("Pro".to_owned()),
    )
    .with_source(AntigravitySnapshotSource::Remote);

    let local_usage = local_snapshot.to_usage_snapshot().unwrap();
    let remote_usage = remote_snapshot.to_usage_snapshot().unwrap();
    assert_eq!(local_usage.primary, remote_usage.primary);
    assert_eq!(local_usage.secondary, remote_usage.secondary);
    assert_eq!(
        local_usage.extra_rate_windows,
        remote_usage.extra_rate_windows
    );

    // idle_window_ids marks only unconsumed families as idle
    let quotas = vec![
        AntigravityModelQuota::new("Gemini 3 Pro", "gemini-3-pro", Some(1.0), None, None),
        AntigravityModelQuota::new("Claude Sonnet 4", "claude-sonnet-4", Some(0.5), None, None),
    ];
    let idle = idle_window_ids(&quotas);
    let mut expected = BTreeSet::new();
    expected.insert(KnownFamily::Gemini.window_id().to_owned());
    assert_eq!(idle, expected);
    assert_eq!(known_family(&quotas[0]), Some(KnownFamily::Gemini));
    assert_eq!(known_family(&quotas[1]), Some(KnownFamily::ClaudeGpt));
}

// ============================================================================
// 6. OAuth Credentials Store Tests (AntigravityOAuthCredentialsStoreTests.swift)
// ============================================================================

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityOAuthCredentialsStoreTests.swift:6-403
fn oauth_credentials_store_and_client_artifact_extraction() {
    let dir = tempdir().unwrap();

    // 1. Resolves OAuth client from installed app bundle JS
    let app_bundle = dir.path().join("Antigravity.app");
    let config_file =
        app_bundle.join("Contents/Resources/app/extensions/antigravity/dist/extension.js");
    fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    let js = r#"const oauth = { clientId: "123456-installed.apps.googleusercontent.com", clientSecret: "GOCSPX-installed-secret" };"#;
    fs::write(&config_file, js).unwrap();
    let client = AntigravityOAuthConfig::resolve_client(
        &BTreeMap::new(),
        Some(&app_bundle),
        Some(dir.path()),
    )
    .unwrap();
    assert_eq!(
        client,
        AntigravityOAuthClient::new(
            "123456-installed.apps.googleusercontent.com",
            "GOCSPX-installed-secret"
        )
    );

    // 2. Resolves OAuth client from Mach-O string table slice
    let macho_bundle = dir.path().join("MachOBundle.app");
    let binary_url = macho_bundle.join("Contents/MacOS/Antigravity");
    fs::create_dir_all(binary_url.parent().unwrap()).unwrap();
    let macho_slice = make_mach_o_64_slice(&[
        (
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
        ),
        (
            "123456-macho.apps.googleusercontent.com",
            "GOCSPX-macho-secret",
        ),
    ]);
    fs::write(&binary_url, &macho_slice).unwrap();
    let macho_client = AntigravityOAuthConfig::resolve_client(
        &BTreeMap::new(),
        Some(&macho_bundle),
        Some(dir.path()),
    )
    .unwrap();
    assert_eq!(
        macho_client,
        AntigravityOAuthClient::new(
            "123456-macho.apps.googleusercontent.com",
            "GOCSPX-macho-secret"
        )
    );

    // 3. Resolves OAuth client from fat Mach-O slice
    let fat_bundle = dir.path().join("FatBundle.app");
    let fat_binary_url = fat_bundle.join("Contents/MacOS/Gemini");
    fs::create_dir_all(fat_binary_url.parent().unwrap()).unwrap();
    let fat_slice =
        make_mach_o_64_slice(&[("123456-fat.apps.googleusercontent.com", "GOCSPX-fat-secret")]);
    let fat_binary = make_fat_mach_o_binary(&fat_slice);
    fs::write(&fat_binary_url, &fat_binary).unwrap();
    let fat_client = AntigravityOAuthConfig::resolve_client(
        &BTreeMap::new(),
        Some(&fat_bundle),
        Some(dir.path()),
    )
    .unwrap();
    assert_eq!(
        fat_client,
        AntigravityOAuthClient::new("123456-fat.apps.googleusercontent.com", "GOCSPX-fat-secret")
    );

    // 4. Rejects ambiguous Mach-O OAuth client candidates
    let ambig_macho = make_mach_o_64_slice(&[
        (
            "123456-first.apps.googleusercontent.com",
            "GOCSPX-first-secret",
        ),
        (
            "654321-second.apps.googleusercontent.com",
            "GOCSPX-second-secret",
        ),
    ]);
    assert!(
        AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&ambig_macho).is_none()
    );

    // 5. Rejects ambiguous JS OAuth client candidates
    let ambig_js = r#"
        const first = { clientId: "123456-first.apps.googleusercontent.com", clientSecret: "GOCSPX-first-secret" };
        const second = { clientId: "654321-second.apps.googleusercontent.com", clientSecret: "GOCSPX-second-secret" };
    "#;
    assert!(AntigravityOAuthConfig::parse_client_from_installed_artifact_text(ambig_js).is_none());

    // 6. Environment override takes precedence over installed app bundle
    let mut env_override = BTreeMap::new();
    env_override.insert(
        AntigravityOAuthConfig::CLIENT_ID_ENV_KEY.to_owned(),
        "override-client-id".to_owned(),
    );
    env_override.insert(
        AntigravityOAuthConfig::CLIENT_SECRET_ENV_KEY.to_owned(),
        "override-client-secret".to_owned(),
    );
    let overridden =
        AntigravityOAuthConfig::resolve_client(&env_override, Some(&app_bundle), Some(dir.path()))
            .unwrap();
    assert_eq!(
        overridden,
        AntigravityOAuthClient::new("override-client-id", "override-client-secret")
    );

    // 7. Installed app search includes Gemini.app and Linux paths
    let fake_home = PathBuf::from("/tmp/antigravity-home");
    let app_urls = AntigravityOAuthConfig::installed_app_urls(Some(&fake_home));
    assert!(app_urls.starts_with(&[
        PathBuf::from("/Applications/Gemini.app"),
        PathBuf::from("/Applications/Antigravity.app"),
        fake_home.join("Applications/Gemini.app"),
        fake_home.join("Applications/Antigravity.app"),
    ]));
    assert!(app_urls.contains(&fake_home.join(".local/share/antigravity")));
    assert!(app_urls.contains(&PathBuf::from("/opt/Antigravity")));
    assert!(app_urls.contains(&PathBuf::from("/usr/share/antigravity")));

    // 8.Save, load, and delete credentials with 0600 permissions
    let store_url = dir.path().join("oauth_creds.json");
    let creds = AntigravityOAuthCredentials::new(
        "access-token",
        Some("refresh-token".to_owned()),
        Some(Timestamp::from_second(1_900_000_000).unwrap()),
        Some(make_id_token("user@example.com", None)),
        Some("user@example.com".to_owned()),
        Some("project-123".to_owned()),
        Some("123456-installed.apps.googleusercontent.com".to_owned()),
        Some("GOCSPX-installed-secret".to_owned()),
    );
    AntigravityOAuthCredentialsStore::save(&creds, &store_url).unwrap();
    assert!(AntigravityOAuthCredentialsStore::exists(&store_url));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&store_url).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let loaded = AntigravityOAuthCredentialsStore::load(&store_url).unwrap();
    assert_eq!(loaded, creds);
    AntigravityOAuthCredentialsStore::delete(&store_url).unwrap();
    assert!(!AntigravityOAuthCredentialsStore::exists(&store_url));

    // 9. Antigravity 2.19.1 Go instruction record tests (arm64 and x86_64)
    let go_client = AntigravityOAuthClient::new(
        "123456789012-desktop.apps.googleusercontent.com",
        format!("GOCSPX-{}", "a".repeat(28)),
    );
    let go_alt = AntigravityOAuthClient::new(
        "123456789012-alternate.apps.googleusercontent.com",
        format!("GOCSPX-{}", "b".repeat(28)),
    );
    let go_consumer = AntigravityOAuthClient::new(
        AntigravityOAuthConfig::CONSUMER_CLIENT_ID,
        format!("GOCSPX-{}", "c".repeat(28)),
    );
    for intel in [false, true] {
        let bin = make_go_instruction_binary_fixture(&[&go_client, &go_alt], intel, false);
        assert_eq!(
            AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&bin),
            Some(go_client.clone())
        );

        // Prefers CONSUMER_CLIENT_ID regardless of order
        for consumer_first in [false, true] {
            let pair = if consumer_first {
                vec![&go_consumer, &go_alt]
            } else {
                vec![&go_alt, &go_consumer]
            };
            let consumer_bin = make_go_instruction_binary_fixture(&pair, intel, true);
            assert_eq!(
                AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&consumer_bin),
                Some(go_consumer.clone())
            );
        }

        // Rejects malformed Mach-O metadata
        for offset in [20usize, 36, 96, 152] {
            let mut malformed =
                make_go_instruction_binary_fixture(&[&go_client, &go_alt], intel, false);
            malformed[offset..offset + 4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
            assert!(
                AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&malformed)
                    .is_none()
            );
        }

        // Does not cross-pair broken records
        let mut broken = make_go_instruction_binary_fixture(&[&go_client, &go_alt], intel, false);
        broken[if intel { 548 } else { 543 }] = 0;
        broken[if intel { 593 } else { 591 }] = 0;
        assert!(
            AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&broken).is_none()
        );
    }

    // Keeps a lone UTF-8 pair beyond the 4000-byte text window
    let beyond_window = format!(
        "{}{}\0{}",
        " ".repeat(5000),
        go_client.client_id,
        go_client.client_secret
    );
    assert_eq!(
        AntigravityOAuthConfig::parse_client_from_installed_artifact_data(beyond_window.as_bytes()),
        Some(go_client)
    );
}

fn make_go_instruction_binary_fixture(
    clients: &[&AntigravityOAuthClient],
    intel: bool,
    reverse_secrets: bool,
) -> Vec<u8> {
    let mut data = vec![0u8; 16384];
    let word = |buf: &mut [u8], offset: usize, value: u32| {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };
    let bytes = |buf: &mut [u8], offset: usize, values: &[u8]| {
        buf[offset..offset + values.len()].copy_from_slice(values);
    };
    let len_u32 = data.len() as u32;
    word(&mut data, 0, 0xFEED_FACF);
    word(&mut data, 4, if intel { 0x0100_0007 } else { 0x0100_000C });
    word(&mut data, 16, 1);
    word(&mut data, 20, 152);
    word(&mut data, 32, 0x19);
    word(&mut data, 36, 152);
    bytes(&mut data, 40, b"__TEXT");
    word(&mut data, 60, 1); // vmaddr = 0x100000000
    word(&mut data, 64, len_u32);
    word(&mut data, 80, len_u32);
    word(&mut data, 96, 1);
    bytes(&mut data, 104, b"__text");
    word(&mut data, 136, 512);
    word(&mut data, 140, 1);
    word(&mut data, 144, (clients.len() * 64) as u32);
    word(&mut data, 152, 512);

    for (index, client) in clients.iter().enumerate() {
        let instruction = 512 + index * 64;
        let secret_idx = if reverse_secrets {
            clients.len() - index - 1
        } else {
            index
        };
        let secret = 8192 + secret_idx * 128;
        let id = 12288 + index * 128;
        bytes(&mut data, secret, client.client_secret.as_bytes());
        bytes(&mut data, id, client.client_id.as_bytes());
        if intel {
            bytes(&mut data, instruction, &[0x48, 0xC7, 0x40, 0x08]);
            word(&mut data, instruction + 4, client.client_id.len() as u32);
            bytes(&mut data, instruction + 8, &[0x48, 0x8D, 0x0D]);
            word(&mut data, instruction + 11, (id - instruction - 15) as u32);
            bytes(
                &mut data,
                instruction + 15,
                &[0x48, 0x89, 0x08, 0x48, 0xC7, 0x40, 0x18],
            );
            word(&mut data, instruction + 22, 35);
            bytes(&mut data, instruction + 26, &[0x48, 0x8D, 0x0D]);
            word(
                &mut data,
                instruction + 29,
                (secret - instruction - 33) as u32,
            );
            bytes(&mut data, instruction + 33, &[0x48, 0x89, 0x48, 0x10]);
        } else {
            for (field, address, length) in [
                (0usize, id, client.client_id.len()),
                (1usize, secret, 35usize),
            ] {
                let offset = instruction + field * 16;
                let pages = ((address >> 12) - (offset >> 12)) as u32;
                word(
                    &mut data,
                    offset,
                    0x9000_0001 | ((pages & 3) << 29) | ((pages >> 2) << 5),
                );
                word(
                    &mut data,
                    offset + 4,
                    0x9100_0021 | (((address & 4095) as u32) << 10),
                );
                word(&mut data, offset + 8, 0xD280_0002 | ((length as u32) << 5));
                word(
                    &mut data,
                    offset + 12,
                    if field == 0 { 0xA900_0801 } else { 0xA901_0801 },
                );
            }
        }
    }
    data
}

fn make_mach_o_64_slice(pairs: &[(&str, &str)]) -> Vec<u8> {
    let header_size = 32usize;
    let segment_cmd_size = 72 + 80 * 2;
    let text_offset = 256usize;
    let string_vmaddr_base = 0x1000u64;

    let mut string_bytes = Vec::new();
    let mut descriptors = Vec::new();
    for (first, second) in pairs {
        for value in [first, second] {
            let start = string_bytes.len();
            let raw = value.as_bytes();
            string_bytes.extend_from_slice(raw);
            string_bytes.push(0);
            let tagged_ptr = (0xABCDu64 << 48) | (string_vmaddr_base + start as u64);
            descriptors.push((tagged_ptr, raw.len() as u64));
        }
    }

    let const_offset = (text_offset + string_bytes.len() + 7) & !7;
    let mut const_bytes = Vec::new();
    for (ptr, len) in descriptors {
        const_bytes.extend_from_slice(&ptr.to_le_bytes());
        const_bytes.extend_from_slice(&len.to_le_bytes());
    }
    let total_size = const_offset + const_bytes.len();
    let mut data = vec![0u8; total_size];

    let put_u32 = |buf: &mut [u8], off: usize, val: u32| {
        buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
    };
    let put_u64 = |buf: &mut [u8], off: usize, val: u64| {
        buf[off..off + 8].copy_from_slice(&val.to_le_bytes());
    };
    let put_str = |buf: &mut [u8], off: usize, s: &str| {
        let b = s.as_bytes();
        let len = b.len().min(16);
        buf[off..off + len].copy_from_slice(&b[..len]);
    };

    put_u32(&mut data, 0, 0xFEED_FACF);
    put_u32(&mut data, 16, 1);
    put_u32(&mut data, 20, segment_cmd_size as u32);

    let cmd_off = header_size;
    put_u32(&mut data, cmd_off, 0x19);
    put_u32(&mut data, cmd_off + 4, segment_cmd_size as u32);
    put_str(&mut data, cmd_off + 8, "__TEXT");
    put_u64(&mut data, cmd_off + 24, string_vmaddr_base);
    put_u64(&mut data, cmd_off + 32, (total_size - text_offset) as u64);
    put_u64(&mut data, cmd_off + 40, text_offset as u64);
    put_u64(&mut data, cmd_off + 48, (total_size - text_offset) as u64);
    put_u32(&mut data, cmd_off + 64, 2);

    let cstring_sec = cmd_off + 72;
    put_str(&mut data, cstring_sec, "__cstring");
    put_str(&mut data, cstring_sec + 16, "__TEXT");
    put_u64(&mut data, cstring_sec + 32, string_vmaddr_base);
    put_u64(&mut data, cstring_sec + 40, string_bytes.len() as u64);
    put_u32(&mut data, cstring_sec + 48, text_offset as u32);

    let const_sec = cstring_sec + 80;
    put_str(&mut data, const_sec, "__const");
    put_str(&mut data, const_sec + 16, "__DATA_CONST");
    put_u64(
        &mut data,
        const_sec + 32,
        string_vmaddr_base + (const_offset - text_offset) as u64,
    );
    put_u64(&mut data, const_sec + 40, const_bytes.len() as u64);
    put_u32(&mut data, const_sec + 48, const_offset as u32);

    data[text_offset..text_offset + string_bytes.len()].copy_from_slice(&string_bytes);
    data[const_offset..const_offset + const_bytes.len()].copy_from_slice(&const_bytes);
    data
}

fn make_fat_mach_o_binary(slice: &[u8]) -> Vec<u8> {
    let slice_offset = 64usize;
    let mut data = vec![0u8; slice_offset + slice.len()];
    let put_be_u32 = |buf: &mut [u8], off: usize, val: u32| {
        buf[off..off + 4].copy_from_slice(&val.to_be_bytes());
    };
    put_be_u32(&mut data, 0, 0xCAFE_BABE);
    put_be_u32(&mut data, 4, 1);
    put_be_u32(&mut data, 16, slice_offset as u32);
    put_be_u32(&mut data, 20, slice.len() as u32);
    data[slice_offset..slice_offset + slice.len()].copy_from_slice(slice);
    data
}

// ============================================================================
// 7. Remote Usage Fetcher Tests (AntigravityRemoteUsageFetcherTests.swift)
// ============================================================================

fn available_models_response() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "models": {
            "claude-sonnet-4": {
                "displayName": "Claude Sonnet 4",
                "quotaInfo": {
                    "remainingFraction": 0.5,
                    "resetTime": "2025-01-01T00:00:00Z",
                }
            },
            "gemini-3-pro-low": {
                "displayName": "Gemini 3 Pro Low",
                "quotaInfo": {
                    "remainingFraction": 0.8,
                    "resetTime": "2025-01-01T00:00:00Z",
                }
            },
            "gemini-3-flash": {
                "displayName": "Gemini 3 Flash",
                "quotaInfo": {
                    "remainingFraction": 0.2,
                    "resetTime": "2025-01-01T00:00:00Z",
                }
            },
            "gemini-3-flash-lite": {
                "displayName": "Gemini 3 Flash Lite",
                "quotaInfo": {
                    "remainingFraction": 0.7,
                    "resetTime": "2025-01-01T00:00:00Z",
                }
            }
        }
    }))
    .unwrap()
}

#[tokio::test]
// CodexBar: Tests/CodexBarTests/AntigravityRemoteUsageFetcherTests.swift:20-73
async fn remote_plan_respects_paid_metadata_and_measured_five_hour_quotas() {
    for scenario in [
        "paid-tier",
        "paid-tier-id",
        "five-hour",
        "weekly",
        "disabled",
        "unmeasured",
    ] {
        let env = TestEnvironment::new();
        env.write_antigravity_credentials(
            "token",
            None,
            Timestamp::now()
                .saturating_add(jiff::Span::new().seconds(3600))
                .unwrap(),
            None,
            Some("user@example.com"),
            Some("test-project"),
            None,
            None,
        );

        let scenario_owned = scenario.to_owned();
        let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
            .with_data_loader(move |req| {
                let scenario = scenario_owned.clone();
                Box::pin(async move {
                    let body = match req.url.path() {
                        "/v1internal:loadCodeAssist" => {
                            let mut map = serde_json::Map::new();
                            map.insert(
                                "currentTier".to_owned(),
                                json!({"id": "free-tier", "name": "Free"}),
                            );
                            if scenario == "paid-tier" {
                                map.insert(
                                    "paidTier".to_owned(),
                                    json!({"id": "g1-pro-tier", "name": "Google AI Pro"}),
                                );
                                map.insert("planInfo".to_owned(), json!({"planType": "Free"}));
                            } else if scenario == "paid-tier-id" {
                                map.insert("paidTier".to_owned(), json!({"id": "standard-tier"}));
                                map.insert("planInfo".to_owned(), json!({"planType": "Free"}));
                            }
                            Value::Object(map)
                        }
                        "/v1internal:retrieveUserQuotaSummary" => {
                            let mut buckets = vec![json!({
                                "bucketId": "gemini-weekly",
                                "window": "weekly",
                                "remainingFraction": 0.75,
                            })];
                            if scenario != "weekly" {
                                let mut session = serde_json::Map::new();
                                session.insert("bucketId".to_owned(), json!("gemini-5h"));
                                session.insert("window".to_owned(), json!("5h"));
                                if scenario != "unmeasured" {
                                    session.insert("remainingFraction".to_owned(), json!(0.5));
                                }
                                session
                                    .insert("disabled".to_owned(), json!(scenario == "disabled"));
                                buckets.push(Value::Object(session));
                            }
                            json!({
                                "groups": [{
                                    "displayName": "Gemini Models",
                                    "buckets": buckets
                                }]
                            })
                        }
                        other => panic!("unexpected path {other}"),
                    };
                    Ok(RemoteHttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&body).unwrap(),
                    })
                })
            });

        let snapshot = fetcher.fetch().await.unwrap();
        let expected_plan = match scenario {
            "paid-tier" => Some("Google AI Pro"),
            "paid-tier-id" => Some("Paid"),
            "five-hour" => None,
            _ => Some("Free"),
        };
        assert_eq!(snapshot.account_plan.as_deref(), expected_plan);
        let usage = snapshot.to_usage_snapshot().unwrap();
        assert_eq!(
            usage.identity.as_ref().unwrap().login_method.as_deref(),
            expected_plan
        );
        assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
        let weekly_used = usage
            .extra_rate_windows
            .as_ref()
            .unwrap()
            .iter()
            .find(|w| w.window.window_minutes == Some(10_080))
            .unwrap()
            .window
            .used_percent;
        assert_eq!(weekly_used, 25.0);
    }
}

#[test]
// CodexBar: Tests/CodexBarTests/AntigravityRemoteUsageFetcherTests.swift:75-103
fn oauth_credentials_round_trip_through_token_account_value() {
    assert_eq!(
        AntigravityProviderPresentation::TOKEN_ACCOUNT_TITLE,
        "Google accounts"
    );
    const {
        assert!(!AntigravityProviderPresentation::TOKEN_ACCOUNT_REQUIRES_MANUAL_COOKIE_SOURCE);
    }
    assert_eq!(
        AntigravityProviderPresentation::token_account_env_override("serialized-credentials")
            .get(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY)
            .map(String::as_str),
        Some("serialized-credentials")
    );

    let credentials = AntigravityOAuthCredentials::new(
        "access-token",
        Some("refresh-token".to_owned()),
        Some(Timestamp::from_second(1_700_000_000).unwrap()),
        Some(make_id_token("user@example.com", None)),
        Some("user@example.com".to_owned()),
        Some("project-123".to_owned()),
        Some("client-id".to_owned()),
        Some("client-secret".to_owned()),
    );
    let token = AntigravityOAuthCredentialsStore::token_account_value(&credentials).unwrap();
    let decoded =
        AntigravityOAuthCredentialsStore::credentials_from_token_account_value(&token).unwrap();
    assert_eq!(decoded, credentials);
}

#[tokio::test]
// CodexBar: Tests/CodexBarTests/AntigravityRemoteUsageFetcherTests.swift:105-345
async fn remote_fetch_selected_account_precedence_refresh_and_error_handling() {
    for suffix in ["", "+&=%2B /東京"] {
        let env = TestEnvironment::new();
        env.write_antigravity_credentials(
            "shared-token",
            Some("shared-refresh"),
            Timestamp::now()
                .saturating_add(jiff::Span::new().seconds(3600))
                .unwrap(),
            Some(&make_id_token("shared@example.com", None)),
            Some("shared@example.com"),
            None,
            Some("shared-client-id"),
            Some("shared-client-secret"),
        );

        let refresh_token = format!("selected-refresh{suffix}");
        let client_id = format!("selected-client-id{suffix}");
        let client_secret = format!("selected-client-secret{suffix}");
        let selected = AntigravityOAuthCredentials::new(
            "selected-old-token",
            Some(refresh_token.clone()),
            Some(
                Timestamp::now()
                    .saturating_sub(jiff::Span::new().seconds(3600))
                    .unwrap(),
            ),
            Some(make_id_token("selected-old@example.com", None)),
            Some("selected-old@example.com".to_owned()),
            None,
            Some(client_id.clone()),
            Some(client_secret.clone()),
        );
        let token = AntigravityOAuthCredentialsStore::token_account_value(&selected).unwrap();
        let captured_updates = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured_updates);

        let mut env_map = BTreeMap::new();
        env_map.insert(
            AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
            token,
        );

        let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
            .with_environment(env_map)
            .with_credentials_update_handler(move |creds| {
                let captured = Arc::clone(&captured_clone);
                Box::pin(async move {
                    captured.lock().unwrap().push(creds);
                    Ok(())
                })
            })
            .with_data_loader(move |req| {
                let refresh_token = refresh_token.clone();
                let client_id = client_id.clone();
                let client_secret = client_secret.clone();
                Box::pin(async move {
                    match req.url.host_str().unwrap() {
                        "oauth2.googleapis.com" => {
                            let decoded = decode_form_body(req.body.as_ref().unwrap());
                            let mut expected = BTreeMap::new();
                            expected.insert("client_id".to_owned(), client_id);
                            expected.insert("client_secret".to_owned(), client_secret);
                            expected.insert("refresh_token".to_owned(), refresh_token);
                            expected.insert("grant_type".to_owned(), "refresh_token".to_owned());
                            assert_eq!(decoded, expected);
                            Ok(RemoteHttpResponse {
                                status: 200,
                                body: serde_json::to_vec(&json!({
                                    "access_token": "selected-new-token",
                                    "expires_in": 3600,
                                    "id_token": make_id_token("selected-new@example.com", None),
                                }))
                                .unwrap(),
                            })
                        }
                        "cloudcode-pa.googleapis.com" => {
                            assert_eq!(
                                req.header("Authorization"),
                                Some("Bearer selected-new-token")
                            );
                            if req.url.path() == "/v1internal:loadCodeAssist" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: serde_json::to_vec(&json!({
                                        "currentTier": {"id": "free-tier", "name": "free"},
                                        "cloudaicompanionProject": "selected-project-123",
                                    }))
                                    .unwrap(),
                                });
                            }
                            if req.url.path() == "/v1internal:fetchAvailableModels" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: available_models_response(),
                                });
                            }
                            Ok(RemoteHttpResponse {
                                status: 404,
                                body: Vec::new(),
                            })
                        }
                        other => panic!("unexpected host {other}"),
                    }
                })
            });

        let snapshot = fetcher.fetch().await.unwrap();
        let shared = env.read_antigravity_credentials_json();
        let updates = captured_updates.lock().unwrap();

        assert_eq!(
            snapshot.account_email.as_deref(),
            Some("selected-new@example.com")
        );
        assert_eq!(shared.get("access_token").unwrap(), "shared-token");
        assert_eq!(shared.get("email").unwrap(), "shared@example.com");
        assert_eq!(updates.last().unwrap().access_token, "selected-new-token");
        assert_eq!(
            updates.last().unwrap().project_id.as_deref(),
            Some("selected-project-123")
        );
    }

    // Rejects invalid selected token account
    let env = TestEnvironment::new();
    let mut bad_env = BTreeMap::new();
    bad_env.insert(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
        "not-json".to_owned(),
    );
    let bad_fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_environment(bad_env);
    let err = bad_fetcher.fetch().await.unwrap_err();
    assert!(matches!(
        err,
        AntigravityRemoteFetchError::ParseFailed(ref msg) if msg.contains("selected account")
    ));
}

#[tokio::test]
// CodexBar: Tests/CodexBarTests/AntigravityRemoteUsageFetcherTests.swift:348-1388
async fn remote_fetch_models_verification_onboarding_refresh_and_tos_recovery() {
    // 1. Verifies full model quotas with retrieveUserQuota and drops rows absent from partial verification
    let env = TestEnvironment::new();
    env.write_antigravity_credentials(
        "token",
        None,
        Timestamp::now()
            .saturating_add(jiff::Span::new().seconds(3600))
            .unwrap(),
        Some(&make_id_token("user@example.com", None)),
        Some("user@example.com"),
        None,
        None,
        None,
    );
    let quota_calls = Arc::new(AtomicUsize::new(0));
    let quota_calls_clone = Arc::clone(&quota_calls);
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let quota_calls = Arc::clone(&quota_calls_clone);
            Box::pin(async move {
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(RemoteHttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&json!({
                            "currentTier": {"id": "standard-tier", "name": "standard"},
                            "cloudaicompanionProject": "managed-project-123",
                        }))
                        .unwrap(),
                    }),
                    "/v1internal:fetchAvailableModels" => Ok(RemoteHttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&json!({
                            "models": {
                                "claude-sonnet-4": {
                                    "displayName": "Claude Sonnet 4",
                                    "quotaInfo": {"remainingFraction": 1}
                                },
                                "gemini-2.5-pro": {
                                    "displayName": "Gemini 2.5 Pro",
                                    "quotaInfo": {"remainingFraction": 1}
                                }
                            }
                        }))
                        .unwrap(),
                    }),
                    "/v1internal:retrieveUserQuota" => {
                        quota_calls.fetch_add(1, Ordering::SeqCst);
                        Ok(RemoteHttpResponse {
                            status: 200,
                            body: serde_json::to_vec(&json!({
                                "buckets": [
                                    {"modelId": "gemini-2.5-pro", "remainingFraction": 0.5}
                                ]
                            }))
                            .unwrap(),
                        })
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: Vec::new(),
                    }),
                }
            })
        });
    let snapshot = fetcher.fetch().await.unwrap();
    assert_eq!(quota_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        snapshot
            .model_quotas
            .iter()
            .map(|q| q.model_id.as_str())
            .collect::<Vec<_>>(),
        vec!["gemini-2.5-pro"]
    );
    assert_eq!(
        snapshot
            .model_quotas
            .iter()
            .map(|q| q.remaining_fraction)
            .collect::<Vec<_>>(),
        vec![Some(0.5)]
    );

    // 2. Propagates quota verification server errors (HTTP 503)
    let fetcher_503 = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(|req| {
            Box::pin(async move {
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(RemoteHttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&json!({
                            "currentTier": {"id": "standard-tier"},
                            "cloudaicompanionProject": "managed-project-123"
                        }))
                        .unwrap(),
                    }),
                    "/v1internal:fetchAvailableModels" => Ok(RemoteHttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&json!({
                            "models": {
                                "gemini-2.5-pro": {"displayName": "Gemini 2.5 Pro", "quotaInfo": {"remainingFraction": 1}}
                            }
                        }))
                        .unwrap(),
                    }),
                    "/v1internal:retrieveUserQuota" => Ok(RemoteHttpResponse {
                        status: 503,
                        body: b"temporary outage".to_vec(),
                    }),
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: Vec::new(),
                    }),
                }
            })
        });
    let err_503 = fetcher_503.fetch().await.unwrap_err();
    assert!(matches!(
        err_503,
        AntigravityRemoteFetchError::ApiError(ref msg) if msg.contains("HTTP 503")
    ));

    // 3. Refreshes expired shared Google token and persists updated access_token
    let env_refresh = TestEnvironment::new();
    env_refresh.write_antigravity_credentials(
        "old-token",
        Some("refresh-token"),
        Timestamp::now()
            .saturating_sub(jiff::Span::new().seconds(3600))
            .unwrap(),
        Some(&make_id_token("stale@example.com", None)),
        Some("stale@example.com"),
        None,
        Some("test-client-id"),
        Some("test-client-secret"),
    );
    let fetcher_refresh =
        AntigravityRemoteUsageFetcher::new(Duration::from_secs(2), env_refresh.home())
            .with_data_loader(|req| {
                Box::pin(async move {
                    match req.url.host_str().unwrap() {
                        "oauth2.googleapis.com" => Ok(RemoteHttpResponse {
                            status: 200,
                            body: serde_json::to_vec(&json!({
                                "access_token": "new-token",
                                "expires_in": 3600,
                                "id_token": make_id_token("refreshed@example.com", None),
                            }))
                            .unwrap(),
                        }),
                        "cloudcode-pa.googleapis.com" => {
                            assert_eq!(req.header("Authorization"), Some("Bearer new-token"));
                            if req.url.path() == "/v1internal:loadCodeAssist" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: serde_json::to_vec(&json!({
                                        "currentTier": {"id": "standard-tier", "name": "standard"},
                                        "cloudaicompanionProject": "managed-project-123",
                                    }))
                                    .unwrap(),
                                });
                            }
                            if req.url.path() == "/v1internal:fetchAvailableModels" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: available_models_response(),
                                });
                            }
                            Ok(RemoteHttpResponse {
                                status: 404,
                                body: Vec::new(),
                            })
                        }
                        other => panic!("unexpected host {other}"),
                    }
                })
            });
    let refreshed_snap = fetcher_refresh.fetch().await.unwrap();
    let updated_json = env_refresh.read_antigravity_credentials_json();
    assert_eq!(updated_json.get("access_token").unwrap(), "new-token");
    assert_eq!(
        refreshed_snap.account_email.as_deref(),
        Some("refreshed@example.com")
    );

    // 4. Remote refresh requires configured OAuth client
    let env_no_client = TestEnvironment::new();
    env_no_client.write_antigravity_credentials(
        "old-token",
        Some("refresh-token"),
        Timestamp::now()
            .saturating_sub(jiff::Span::new().seconds(3600))
            .unwrap(),
        Some(&make_id_token("user@example.com", None)),
        Some("user@example.com"),
        None,
        None,
        None,
    );
    let err_no_client =
        AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_no_client.home())
            .with_oauth_client_resolver(|| None)
            .fetch()
            .await
            .unwrap_err();
    assert!(matches!(
        err_no_client,
        AntigravityRemoteFetchError::ApiError(ref msg) if msg.contains("ANTIGRAVITY_OAUTH_CLIENT_ID")
    ));

    // 5. Onboards project before fetching models & prefers stored project ID
    let env_onboard = TestEnvironment::new();
    env_onboard.write_antigravity_credentials(
        "token",
        None,
        Timestamp::now()
            .saturating_add(jiff::Span::new().seconds(3600))
            .unwrap(),
        Some(&make_id_token("user@example.com", None)),
        Some("user@example.com"),
        None,
        None,
        None,
    );
    let recorded_projects = Arc::new(Mutex::new(Vec::new()));
    let recorded_clone = Arc::clone(&recorded_projects);
    let fetcher_onboard =
        AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_onboard.home())
            .with_data_loader(move |req| {
                let recorded = Arc::clone(&recorded_clone);
                Box::pin(async move {
                    match req.url.path() {
                        "/v1internal:loadCodeAssist" => Ok(RemoteHttpResponse {
                            status: 200,
                            body: serde_json::to_vec(&json!({
                                "currentTier": {"id": "standard-tier", "name": "standard"},
                                "allowedTiers": [{"id": "standard-tier", "isDefault": true}]
                            }))
                            .unwrap(),
                        }),
                        "/v1internal:onboardUser" => Ok(RemoteHttpResponse {
                            status: 200,
                            body: serde_json::to_vec(&json!({
                                "response": {
                                    "cloudaicompanionProject": {"id": "onboarded-project-456"}
                                }
                            }))
                            .unwrap(),
                        }),
                        "/v1internal:fetchAvailableModels" => {
                            let body: serde_json::Map<String, Value> =
                                serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
                            if let Some(p) = body.get("project").and_then(Value::as_str) {
                                recorded.lock().unwrap().push(p.to_owned());
                            }
                            Ok(RemoteHttpResponse {
                                status: 200,
                                body: available_models_response(),
                            })
                        }
                        _ => Ok(RemoteHttpResponse {
                            status: 404,
                            body: Vec::new(),
                        }),
                    }
                })
            });
    fetcher_onboard.fetch().await.unwrap();
    assert_eq!(
        recorded_projects.lock().unwrap().last().map(String::as_str),
        Some("onboarded-project-456")
    );

    // 6. Returns identity when both remote quota endpoints are forbidden (403)
    let fetcher_both_403 =
        AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_onboard.home())
            .with_data_loader(|req| {
                Box::pin(async move {
                    match req.url.path() {
                        "/v1internal:loadCodeAssist" => Ok(RemoteHttpResponse {
                            status: 200,
                            body: serde_json::to_vec(&json!({
                                "currentTier": {"id": "standard-tier", "name": "standard"},
                                "cloudaicompanionProject": "managed-project-123"
                            }))
                            .unwrap(),
                        }),
                        "/v1internal:fetchAvailableModels" | "/v1internal:retrieveUserQuota" => {
                            Ok(RemoteHttpResponse {
                                status: 403,
                                body: serde_json::to_vec(&json!({
                                    "error": {
                                        "code": 403,
                                        "message": "The caller does not have permission",
                                        "status": "PERMISSION_DENIED"
                                    }
                                }))
                                .unwrap(),
                            })
                        }
                        _ => Ok(RemoteHttpResponse {
                            status: 404,
                            body: Vec::new(),
                        }),
                    }
                })
            });
    let snap_403 = fetcher_both_403.fetch().await.unwrap();
    let usage_403 = AntigravityOAuthFetchStrategy::usage_snapshot_from(&snap_403).unwrap();
    assert!(snap_403.model_quotas.is_empty());
    assert_eq!(snap_403.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(snap_403.account_plan.as_deref(), Some("Paid"));
    assert!(AntigravityOAuthFetchStrategy::rate_limits_unavailable(
        &usage_403
    ));

    // 7. Ignores Gemini credentials when Antigravity auth is missing
    let env_gemini_only = TestEnvironment::new();
    env_gemini_only.write_gemini_credentials(
        "gemini-token",
        Timestamp::now()
            .saturating_add(jiff::Span::new().seconds(3600))
            .unwrap(),
        &make_id_token("gemini@example.com", None),
    );
    let err_missing =
        AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_gemini_only.home())
            .fetch()
            .await
            .unwrap_err();
    assert_eq!(err_missing, AntigravityRemoteFetchError::NotLoggedIn);

    // 8. Asks for sign-in again when OAuth client cannot read consumer quota (GOOGLE_TOS_NOT_SUPPORTED_BY_CLIENT)
    let env_tos = TestEnvironment::new();
    env_tos.write_antigravity_credentials(
        "token",
        None,
        Timestamp::now()
            .saturating_add(jiff::Span::new().seconds(3600))
            .unwrap(),
        Some(&make_id_token("user@example.com", None)),
        Some("user@example.com"),
        None,
        None,
        None,
    );
    let tos_paths = Arc::new(Mutex::new(Vec::new()));
    let tos_paths_clone = Arc::clone(&tos_paths);
    let fetcher_tos = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_tos.home())
        .with_data_loader(move |req| {
            let paths = Arc::clone(&tos_paths_clone);
            Box::pin(async move {
                paths.lock().unwrap().push(req.url.path().to_owned());
                Ok(RemoteHttpResponse {
                    status: 200,
                    body: serde_json::to_vec(&json!({
                        "allowedTiers": [{
                            "id": "standard-tier",
                            "isDefault": true,
                            "userDefinedCloudaicompanionProject": true
                        }],
                        "ineligibleTiers": [{
                            "reasonCode": "GOOGLE_TOS_NOT_SUPPORTED_BY_CLIENT",
                            "reasonMessage": "Client does not support Google TOS.",
                            "tierId": "free-tier"
                        }]
                    }))
                    .unwrap(),
                })
            })
        });
    let err_tos = fetcher_tos.fetch().await.unwrap_err();
    assert_eq!(
        err_tos,
        AntigravityRemoteFetchError::ReauthenticationRequired
    );
    assert_eq!(
        *tos_paths.lock().unwrap(),
        vec!["/v1internal:loadCodeAssist".to_owned()]
    );
    let diag = AntigravityOfflinePlaceholderStrategy::diagnostic_for_prior_failure(
        &AntigravityRemoteFetchError::ReauthenticationRequired,
    );
    assert!(diag.unwrap().contains("Sign in to it again."));

    // 9. Consumer client recovery preserves onboarded and unrelated eligibility responses
    for case_name in [
        "stored-project",
        "returned-project",
        "current-tier",
        "other-reason",
    ] {
        let env_case = TestEnvironment::new();
        env_case.write_antigravity_credentials(
            "token",
            None,
            Timestamp::now()
                .saturating_add(jiff::Span::new().seconds(3600))
                .unwrap(),
            None,
            Some("user@example.com"),
            if case_name == "stored-project" {
                Some("saved-project")
            } else {
                None
            },
            None,
            None,
        );
        let case_owned = case_name.to_owned();
        let snap_case = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_case.home())
            .with_data_loader(move |req| {
                let case_name = case_owned.clone();
                Box::pin(async move {
                    let body = match req.url.path() {
                        "/v1internal:loadCodeAssist" => {
                            let mut map = serde_json::Map::new();
                            let reason = if case_name == "other-reason" {
                                "OTHER_REASON"
                            } else {
                                "GOOGLE_TOS_NOT_SUPPORTED_BY_CLIENT"
                            };
                            map.insert(
                                "ineligibleTiers".to_owned(),
                                json!([{"reasonCode": reason}]),
                            );
                            if case_name == "returned-project" {
                                map.insert(
                                    "cloudaicompanionProject".to_owned(),
                                    json!("returned-project"),
                                );
                            }
                            if case_name == "current-tier" {
                                map.insert(
                                    "currentTier".to_owned(),
                                    json!({"id": "standard-tier"}),
                                );
                            }
                            Value::Object(map)
                        }
                        "/v1internal:onboardUser" => {
                            json!({"response": {"cloudaicompanionProject": "onboarded-project"}})
                        }
                        _ => json!({
                            "groups": [{
                                "displayName": "All Models",
                                "buckets": [{"bucketId": "gemini-3.1-pro-high", "remainingFraction": 0.5}]
                            }]
                        }),
                    };
                    Ok(RemoteHttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&body).unwrap(),
                    })
                })
            })
            .fetch()
            .await
            .unwrap();
        assert!(snap_case.has_known_quota_summary());
        assert_eq!(snap_case.account_email.as_deref(), Some("user@example.com"));
    }
}
