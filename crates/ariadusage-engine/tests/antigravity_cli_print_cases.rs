// Ported from CodexBar Tests/CodexBarTests/AntigravityCLIUsageReportTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::model::{ProviderIdentitySnapshot, UsageSnapshot};
use ariadusage_core::pipeline::{
    FetchContext, FetchInteraction, FetchPhase, FetchResult, FetchRuntime,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::process::{OutputStream, ProcessError};
use ariadusage_engine::providers::antigravity::cli_https::{
    AgyVersionResolver, CliProcessOutput, CliProcessRequest, CliProcessRunner, PRINT_ARGUMENTS,
    WarmAgyDependencies, WarmAgyProcess, agy_version, child_environment,
    fetch_by_spawning_if_reachable, fetch_with_report_fallback, parse_version, run_print_usage,
};
use ariadusage_engine::providers::antigravity::cli_print_failure::{
    AntigravityCLIPrintFailure, ExitReason,
};
use ariadusage_engine::providers::antigravity::oauth_store::AntigravityOAuthCredentialsStore;
use ariadusage_engine::providers::antigravity::status_probe::{
    AntigravityStatusProbeError, ProcessInfoResult,
};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::metric::{Confidence, SourceKind};
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

// upstream: AntigravityCLIUsageReportTests.swift:19 CLI report parses structured quotas
#[test]
fn cli_report_parses_structured_quotas() {
    let json = br#"{"status":"SUCCESS","command":{"name":"usage","data":{"description":"Models share limits","groups":[{"name":"Gemini Models","buckets":[{"id":"gemini-weekly","name":"Weekly Limit Remaining","window":"weekly","remaining_fraction":0.86,"reset_time":"2026-09-17T18:40:27Z"},{"id":"gemini-5h","name":"Five Hour Limit Remaining","window":"5h","remaining_fraction":0.95,"reset_time":"2026-09-13T03:48:04Z"}]},{"name":"Claude and GPT models","buckets":[{"id":"3p-weekly","name":"Weekly Limit Remaining","window":"weekly","remaining_fraction":0.89,"reset_time":"2026-09-17T02:38:46Z"},{"id":"3p-5h","name":"Five Hour Limit Remaining","window":"5h","remaining_fraction":1.0,"reset_time":"2026-09-13T05:29:19Z"}]}]}}}"#;
    let snapshot = ariadusage_engine::providers::antigravity::parse_cli_usage_report(json).unwrap();
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert!(usage.primary.is_some());
    assert!(usage.secondary.is_some());
    assert_eq!(usage.extra_rate_windows.as_ref().unwrap().len(), 4);
}

// upstream: AntigravityCLIUsageReportTests.swift:97 CLI report requires successful command status
#[test]
fn cli_report_requires_successful_command_status() {
    for status in ["ERROR", "PENDING", "success"] {
        let report = report_json_with(status, "usage", Some(0.6), false);
        assert_eq!(
            ariadusage_engine::providers::antigravity::parse_cli_usage_report(&report).unwrap_err(),
            AntigravityStatusProbeError::ParseFailed("Unsuccessful CLI usage report".into())
        );
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:105 CLI report rejects unrelated commands and unavailable quota
#[test]
fn cli_report_rejects_unrelated_commands_and_unavailable_quota() {
    let report = report_json_with("SUCCESS", "models", Some(0.6), false);
    assert_eq!(
        ariadusage_engine::providers::antigravity::parse_cli_usage_report(&report).unwrap_err(),
        AntigravityStatusProbeError::ParseFailed("Unsuccessful CLI usage report".into())
    );
    for report in [
        report_json_with("SUCCESS", "usage", None, false),
        report_json_with("SUCCESS", "usage", Some(0.6), true),
    ] {
        assert_eq!(
            ariadusage_engine::providers::antigravity::parse_cli_usage_report(&report).unwrap_err(),
            AntigravityStatusProbeError::ParseFailed("CLI usage report has no known quota".into())
        );
    }
    assert!(
        ariadusage_engine::providers::antigravity::parse_cli_usage_report(b"not-json").is_err()
    );
}

// upstream: AntigravityCLIUsageReportTests.swift:275 print failure does not expose stderr
#[test]
fn print_failure_does_not_expose_stderr() {
    let error = AntigravityCLIPrintFailure::non_zero_exit_error(
        7,
        b"synthetic-private-diagnostic https://lh3.googleusercontent.com/a/private",
    );
    assert_eq!(
        error,
        AntigravityStatusProbeError::CliReportFailed(AntigravityCLIPrintFailure::Exited {
            code: 7,
            reason: ExitReason::Unspecified,
        })
    );
    let message = error.to_string();
    assert!(!message.contains("synthetic-private-diagnostic"));
    assert!(!message.contains("googleusercontent"));
}

// upstream: AntigravityCLIUsageReportTests.swift:291 print failure maps agy stderr to a safe diagnostic
#[test]
fn print_failure_maps_agy_stderr_to_a_safe_diagnostic() {
    let cases = [
        (
            &br#"Eligibility check failed: failed to get profile picture: Get "https://lh3.googleusercontent.com/a/private": EOF"#[..],
            AntigravityCLIPrintFailure::Exited {
                code: 1,
                reason: ExitReason::EligibilityNetwork,
            },
        ),
        (
            &b"Eligibility check failed: account does not support Google ToS"[..],
            AntigravityCLIPrintFailure::Exited {
                code: 1,
                reason: ExitReason::Ineligible,
            },
        ),
        (
            &b"You are not logged into Antigravity"[..],
            // Authentication is asserted separately below because it is not a
            // CliReportFailed diagnostic.
            AntigravityCLIPrintFailure::Exited {
                code: 1,
                reason: ExitReason::Unspecified,
            },
        ),
        (
            &br#"Post "https://usage.invalid/v1": dial tcp: no such host"#[..],
            AntigravityCLIPrintFailure::Exited {
                code: 1,
                reason: ExitReason::Network,
            },
        ),
    ];

    for (index, (stderr, expected)) in cases.into_iter().enumerate() {
        let error = AntigravityCLIPrintFailure::non_zero_exit_error(1, stderr);
        if index == 2 {
            assert_eq!(error, AntigravityStatusProbeError::AuthenticationRequired);
        } else {
            assert_eq!(
                error,
                AntigravityStatusProbeError::CliReportFailed(expected)
            );
        }
        let message = error.to_string();
        assert!(!message.contains("googleusercontent"));
        assert!(!message.contains("usage.invalid"));
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:331 print failure classifier keeps timeouts and blank exits safe
#[test]
fn print_failure_classifier_keeps_timeouts_and_blank_exits_safe() {
    assert_eq!(
        AntigravityCLIPrintFailure::process_error(&ProcessError::TimedOut),
        AntigravityStatusProbeError::TimedOut
    );
    assert_eq!(
        AntigravityCLIPrintFailure::non_zero_exit_error(2, b"  "),
        AntigravityStatusProbeError::CliReportFailed(AntigravityCLIPrintFailure::Exited {
            code: 2,
            reason: ExitReason::Unspecified,
        })
    );
    assert_eq!(
        AntigravityCLIPrintFailure::process_error(&ProcessError::LaunchFailed),
        AntigravityStatusProbeError::CliReportFailed(AntigravityCLIPrintFailure::LaunchFailed)
    );
    assert_eq!(
        AntigravityCLIPrintFailure::process_error(&ProcessError::OutputTooLarge {
            stream: OutputStream::Stdout,
            cap: 1_048_576,
        }),
        AntigravityStatusProbeError::ParseFailed("CLI usage report failed".to_owned())
    );
}

// Additional parameter rows for the upstream diagnostic case above.
#[test]
fn print_failure_authentication_markers_take_precedence() {
    for stderr in [
        "not logged in",
        "not signed in",
        "unauthenticated",
        "authentication required",
        "login required",
        "please log in",
        "please sign in",
        "Select login method:",
    ] {
        assert_eq!(
            AntigravityCLIPrintFailure::non_zero_exit_error(1, stderr.as_bytes()),
            AntigravityStatusProbeError::AuthenticationRequired,
            "marker: {stderr}"
        );
    }
}

// Additional network marker rows for the upstream diagnostic case above.
#[test]
fn print_failure_network_markers_use_word_boundaries() {
    for stderr in [
        "EOF",
        "request timed out",
        "timeout",
        "deadline exceeded",
        "i/o timeout",
        "no such host",
        "DNS failure",
        "connection refused",
        "connection reset",
        "network is unreachable",
        "host unreachable",
        "proxy error",
        "certificate verify failed",
        "TLS handshake failed",
        "SSL error",
        "Get \"https://usage.invalid\" failed",
        "Post \"https://usage.invalid\" failed",
    ] {
        assert_eq!(
            AntigravityCLIPrintFailure::non_zero_exit_error(9, stderr.as_bytes()),
            AntigravityStatusProbeError::CliReportFailed(AntigravityCLIPrintFailure::Exited {
                code: 9,
                reason: ExitReason::Network,
            }),
            "marker: {stderr}"
        );
    }
    assert_eq!(
        AntigravityCLIPrintFailure::non_zero_exit_error(9, b"eofish"),
        AntigravityStatusProbeError::CliReportFailed(AntigravityCLIPrintFailure::Exited {
            code: 9,
            reason: ExitReason::Unspecified,
        })
    );
}

#[test]
fn print_failure_messages_are_fixed_and_reviewable() {
    let cases = [
        (
            AntigravityCLIPrintFailure::ExecutableNotFound,
            "agy executable not found",
        ),
        (
            AntigravityCLIPrintFailure::LaunchFailed,
            "agy failed to launch",
        ),
        (
            AntigravityCLIPrintFailure::Exited {
                code: 7,
                reason: ExitReason::Unspecified,
            },
            "agy exited 7",
        ),
        (
            AntigravityCLIPrintFailure::Exited {
                code: 7,
                reason: ExitReason::Network,
            },
            "agy exited 7; a network request failed (check network or proxy settings)",
        ),
        (
            AntigravityCLIPrintFailure::Exited {
                code: 7,
                reason: ExitReason::EligibilityNetwork,
            },
            "agy exited 7; the eligibility check failed on a network request (check network or proxy settings)",
        ),
        (
            AntigravityCLIPrintFailure::Exited {
                code: 7,
                reason: ExitReason::Ineligible,
            },
            "agy exited 7; the account is not eligible for Antigravity",
        ),
    ];
    for (failure, expected) in cases {
        assert_eq!(failure.to_string(), expected);
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:120 verified HTTPS result keeps its identity without running print
#[tokio::test]
async fn verified_https_result_keeps_identity_without_running_print() {
    let expected = fetch_result(Some("owner@example.com"));
    let result = fetch_with_report_fallback(
        &context(SourceMode::Auto, None, BTreeMap::new()),
        || {
            let expected = expected.clone();
            async move { Ok(expected) }
        },
        || async {
            panic!("a successful HTTPS result must not run print fallback");
        },
        None::<fn() -> std::future::Ready<Result<FetchResult, AntigravityStatusProbeError>>>,
    )
    .await
    .unwrap();
    assert_eq!(
        result
            .usage
            .identity
            .as_ref()
            .and_then(|id| id.account_email.as_deref()),
        Some("owner@example.com")
    );
}

// upstream: AntigravityCLIUsageReportTests.swift:134 unselected fetch falls back to identity free print
#[tokio::test]
async fn unselected_fetch_falls_back_to_identity_free_print() {
    for mode in [SourceMode::Auto, SourceMode::Cli] {
        let expected = fetch_result(None);
        let result = fetch_with_report_fallback(
            &context(mode, None, BTreeMap::new()),
            || async {
                Err(AntigravityStatusProbeError::ApiError(
                    "usage endpoint unavailable".into(),
                ))
            },
            || {
                let expected = expected.clone();
                async move { Ok(expected) }
            },
            None::<fn() -> std::future::Ready<Result<FetchResult, AntigravityStatusProbeError>>>,
        )
        .await
        .unwrap();
        assert!(result.usage.identity.is_none());
        assert_eq!(result.source_label, "cli");
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:146 selected and injected auto accounts never use identity free print
#[tokio::test]
async fn selected_and_injected_auto_accounts_never_use_identity_free_print() {
    let key = AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned();
    let contexts = [
        context(SourceMode::Auto, Some("selected-account"), BTreeMap::new()),
        context(
            SourceMode::Auto,
            None,
            BTreeMap::from([(key.clone(), SecretString::new("synthetic-selected-token"))]),
        ),
        context(
            SourceMode::Auto,
            None,
            BTreeMap::from([(key.clone(), SecretString::new("malformed"))]),
        ),
        context(
            SourceMode::Auto,
            None,
            BTreeMap::from([(key, SecretString::new(""))]),
        ),
    ];
    for cx in contexts {
        let result = fetch_with_report_fallback(
            &cx,
            || async { Err(AntigravityStatusProbeError::TimedOut) },
            || async {
                panic!("identity-free print cannot prove the requested OAuth account");
            },
            None::<fn() -> std::future::Ready<Result<FetchResult, AntigravityStatusProbeError>>>,
        )
        .await;
        assert_eq!(result, Err(AntigravityStatusProbeError::TimedOut));
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:168 explicit CLI mode retains its source authority
#[tokio::test]
async fn explicit_cli_mode_retains_source_authority() {
    let cx = context(SourceMode::Cli, Some("selected-account"), BTreeMap::new());
    let expected = fetch_result(None);
    let result = fetch_with_report_fallback(
        &cx,
        || async { Err(AntigravityStatusProbeError::TimedOut) },
        || {
            let expected = expected.clone();
            async move { Ok(expected) }
        },
        None::<fn() -> std::future::Ready<Result<FetchResult, AntigravityStatusProbeError>>>,
    )
    .await
    .unwrap();
    assert!(result.usage.identity.is_none());
}

// upstream: AntigravityCLIUsageReportTests.swift:181 cancelled HTTPS work never starts print fallback
#[tokio::test]
async fn cancelled_https_work_never_starts_print_fallback() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut cx = context(SourceMode::Auto, None, BTreeMap::new());
    cx.cancel = cancel;
    let result = fetch_with_report_fallback(
        &cx,
        || async { Err(AntigravityStatusProbeError::Cancelled) },
        || async {
            panic!("cancellation must stop the provider pipeline");
        },
        None::<fn() -> std::future::Ready<Result<FetchResult, AntigravityStatusProbeError>>>,
    )
    .await;
    assert_eq!(result, Err(AntigravityStatusProbeError::Cancelled));
}

// upstream: AntigravityCLIUsageReportTests.swift:235 print subprocess uses the pinned executable and supported command
#[tokio::test]
async fn print_subprocess_uses_the_pinned_executable_and_supported_command() {
    for version in ["1.1.11", "1.2.2", "2.0.0"] {
        let requests = Arc::new(Mutex::new(Vec::<CliProcessRequest>::new()));
        let seen = Arc::clone(&requests);
        let report = report_json();
        let runner = runner(move |request| {
            let is_version = request.arguments == vec!["--version"];
            seen.lock().unwrap().push(request);
            Ok(CliProcessOutput {
                stdout: if is_version {
                    version.as_bytes().to_vec()
                } else {
                    report.clone()
                },
                stderr: Vec::new(),
                exit_code: 0,
            })
        });
        let directory = tempdir().unwrap();
        let environment = BTreeMap::from([
            ("PATH".into(), "/usr/bin:/bin".into()),
            (
                AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
                "synthetic-app-owned-credential".into(),
            ),
        ]);
        let resolver = AgyVersionResolver::new();
        let result = run_print_usage(
            Path::new("/synthetic/agy"),
            &environment,
            directory.path(),
            Duration::from_secs(10),
            &resolver,
            &runner,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(result.has_known_quota_summary());
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].binary, Path::new("/synthetic/agy"));
        assert_eq!(
            requests[1].arguments,
            PRINT_ARGUMENTS
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect::<Vec<_>>()
        );
        assert!(
            !requests[1]
                .environment
                .contains_key(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY)
        );
        assert_eq!(requests[1].directory.as_deref(), Some(directory.path()));
        assert_eq!(requests[1].max_output_bytes, 1_048_576);
        assert!(
            requests[1].environment["PATH"]
                .starts_with(&format!("{}:", directory.path().display()))
        );
        assert_eq!(
            requests[1].environment["BROWSER"],
            directory.path().join("open").display().to_string()
        );
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:194 print usage cannot launch the system browser
#[tokio::test]
async fn print_usage_cannot_launch_the_system_browser() {
    let requests = Arc::new(Mutex::new(Vec::<CliProcessRequest>::new()));
    let seen = Arc::clone(&requests);
    let runner = runner(move |request| {
        let is_version = request.arguments == vec!["--version"];
        if !is_version {
            let browser = request
                .environment
                .get("BROWSER")
                .cloned()
                .unwrap_or_default();
            assert!(browser.ends_with("/open"));
            assert!(request.environment["PATH"].starts_with(&format!(
                "{}:",
                request.directory.as_deref().unwrap().display()
            )));
        }
        seen.lock().unwrap().push(request);
        Ok(CliProcessOutput {
            stdout: if is_version {
                b"1.2.2".to_vec()
            } else {
                Vec::new()
            },
            stderr: if is_version {
                Vec::new()
            } else {
                b"Authentication required. Please sign in".to_vec()
            },
            exit_code: if is_version { 0 } else { 1 },
        })
    });
    let result = run_print_usage(
        Path::new("/synthetic/agy"),
        &BTreeMap::new(),
        tempdir().unwrap().path(),
        Duration::from_secs(10),
        &AgyVersionResolver::new(),
        &runner,
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(
        result,
        Err(AntigravityStatusProbeError::AuthenticationRequired)
    );
    assert_eq!(requests.lock().unwrap().len(), 2);
}

// upstream: AntigravityCLIUsageReportTests.swift:261 unsupported versions never execute a print prompt
#[tokio::test]
async fn unsupported_versions_never_execute_a_print_prompt() {
    for version in [
        "1.1.10",
        "1.0.99",
        "running",
        "",
        "1.2.2-preview",
        "1.2.2.3",
        "+1.2.2",
    ] {
        let count = Arc::new(Mutex::new(0_usize));
        let seen = Arc::clone(&count);
        let version = version.to_owned();
        let runner = runner(move |request| {
            *seen.lock().unwrap() += 1;
            Ok(CliProcessOutput {
                stdout: if request.arguments == vec!["--version"] {
                    version.as_bytes().to_vec()
                } else {
                    panic!("unsupported version must not run print")
                },
                stderr: Vec::new(),
                exit_code: 0,
            })
        });
        let directory = tempdir().unwrap();
        let result = run_print_usage(
            Path::new("/synthetic/agy"),
            &BTreeMap::new(),
            directory.path(),
            Duration::from_secs(10),
            &AgyVersionResolver::new(),
            &runner,
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(
            result,
            Err(AntigravityStatusProbeError::ParseFailed(
                "CLI usage reports require agy 1.1.11 or later".into()
            ))
        );
        assert_eq!(*count.lock().unwrap(), 1);
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:502 unknown agy version is resolved once and still fails the print guard
#[tokio::test]
async fn unknown_agy_version_is_resolved_once_and_still_fails_the_print_guard() {
    let calls = Arc::new(Mutex::new(0_usize));
    let seen = Arc::clone(&calls);
    let runner = runner(move |request| {
        *seen.lock().unwrap() += 1;
        assert_eq!(request.arguments, vec!["--version"]);
        Ok(CliProcessOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
        })
    });
    let result = run_print_usage(
        Path::new("/synthetic/agy"),
        &BTreeMap::new(),
        tempdir().unwrap().path(),
        Duration::from_secs(10),
        &AgyVersionResolver::new(),
        &runner,
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(
        result,
        Err(AntigravityStatusProbeError::ParseFailed(
            "CLI usage reports require agy 1.1.11 or later".into()
        ))
    );
    assert_eq!(*calls.lock().unwrap(), 1);
}

// upstream: AntigravityCLIUsageReportTests.swift:344 oversized print output is rejected before decoding
#[tokio::test]
async fn oversized_print_output_is_rejected_before_decoding() {
    let runner = runner(|request| {
        if request.arguments == vec!["--version"] {
            return Ok(CliProcessOutput {
                stdout: b"1.2.2".to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
            });
        }
        Err(ProcessError::OutputTooLarge {
            stream: OutputStream::Stdout,
            cap: 1_048_576,
        })
    });
    let result = run_print_usage(
        Path::new("/synthetic/agy"),
        &BTreeMap::new(),
        tempdir().unwrap().path(),
        Duration::from_secs(10),
        &AgyVersionResolver::new(),
        &runner,
        &CancellationToken::new(),
    )
    .await;
    assert!(matches!(
        result,
        Err(AntigravityStatusProbeError::ParseFailed(_))
    ));
}

#[cfg(unix)]
fn owned_child_runner(
    version_known: bool,
    pid_file: &Path,
    registry: Arc<ariadusage_engine::brokers::process::ProcessRegistry>,
) -> CliProcessRunner {
    use ariadusage_core::gates::launch::LaunchGate;
    use ariadusage_engine::brokers::call::BrokerCall;
    let actual = ariadusage_engine::providers::antigravity::cli_https::broker_runner(
        BrokerCall {
            interaction: FetchInteraction::UserInitiated,
            cancel: CancellationToken::new(),
            request_id: "antigravity-print-child".into(),
        },
        registry,
        Arc::new(LaunchGate::default()),
    );
    let pid_file = pid_file.to_path_buf();
    Arc::new(move |mut request, cancel| {
        if version_known && request.arguments == ["--version"] {
            return Box::pin(async {
                Ok(CliProcessOutput {
                    stdout: b"1.2.2".to_vec(),
                    stderr: vec![],
                    exit_code: 0,
                })
            });
        }
        request.binary = env!("CARGO_BIN_EXE_ariadusage-test-child").into();
        request.arguments = vec![
            "ignore-term-ready".into(),
            "10000".into(),
            pid_file.to_string_lossy().into_owned(),
        ];
        actual(request, cancel)
    })
}
#[cfg(unix)]
async fn observed_child_pid(path: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<i32>()
            && pid > 0
            && ariadusage_engine::brokers::process::process_identity(Path::new("/proc"), pid)
                .is_some()
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "child PID was not published");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
#[cfg(unix)]
fn assert_child_reaped(
    path: &Path,
    registry: &ariadusage_engine::brokers::process::ProcessRegistry,
) {
    let pid = std::fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    assert!(
        ariadusage_engine::brokers::process::process_identity(Path::new("/proc"), pid).is_none(),
        "owned print child remains alive"
    );
    assert_eq!(registry.active_launches(), 0);
}

#[cfg(unix)]
// upstream: AntigravityCLIUsageReportTests.swift:354 print timeout terminates its process
#[tokio::test]
async fn print_timeout_terminates_its_process() {
    for version_known in [true, false] {
        let directory = tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let registry = Arc::new(ariadusage_engine::brokers::process::ProcessRegistry::new());
        let runner = owned_child_runner(version_known, &pid_file, registry.clone());
        let result = run_print_usage(
            Path::new("/synthetic/agy"),
            &BTreeMap::new(),
            directory.path(),
            Duration::from_secs(3),
            &AgyVersionResolver::new(),
            &runner,
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(result, Err(AntigravityStatusProbeError::TimedOut));
        assert_child_reaped(&pid_file, &registry);
    }
}

#[cfg(unix)]
// upstream: AntigravityCLIUsageReportTests.swift:367 print cancellation terminates its process
#[tokio::test]
async fn print_cancellation_terminates_its_process() {
    for version_known in [true, false] {
        let directory = tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let registry = Arc::new(ariadusage_engine::brokers::process::ProcessRegistry::new());
        let runner = owned_child_runner(version_known, &pid_file, registry.clone());
        let cancel = CancellationToken::new();
        let task = {
            let root = directory.path().to_path_buf();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                run_print_usage(
                    Path::new("/synthetic/agy"),
                    &BTreeMap::new(),
                    &root,
                    Duration::from_secs(5),
                    &AgyVersionResolver::new(),
                    &runner,
                    &cancel,
                )
                .await
            })
        };
        let _observed_pid = observed_child_pid(&pid_file).await;
        cancel.cancel();
        assert_eq!(
            task.await.unwrap(),
            Err(AntigravityStatusProbeError::Cancelled)
        );
        assert_child_reaped(&pid_file, &registry);
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:397 only CSRF gated agy versions skip the managed spawn
#[tokio::test]
async fn only_csrf_gated_agy_versions_skip_the_managed_spawn() {
    for (version, spawns) in [
        ("1.1.28", true),
        ("1.2.0", true),
        ("1.2.1", true),
        ("1.2.2", false),
        ("1.2.4", false),
        ("1.10.0", false),
        ("2.0.0", false),
        ("1.2.2-preview", true),
        ("", true),
    ] {
        let called = Arc::new(Mutex::new(false));
        let seen = Arc::clone(&called);
        let _ = fetch_by_spawning_if_reachable(parse_version(version), move || {
            *seen.lock().unwrap() = true;
            async { Err::<(), _>(AntigravityStatusProbeError::NotRunning) }
        })
        .await;
        assert_eq!(*called.lock().unwrap(), spawns, "version: {version}");
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:418 CSRF gated agy never starts a managed spawn
#[tokio::test]
async fn csrf_gated_agy_never_starts_a_managed_spawn() {
    let called = Arc::new(Mutex::new(false));
    let seen = Arc::clone(&called);
    let result = fetch_by_spawning_if_reachable(Some((1, 2, 2)), move || {
        *seen.lock().unwrap() = true;
        async { Ok::<_, AntigravityStatusProbeError>(()) }
    })
    .await;
    assert_eq!(
        result,
        Err(AntigravityStatusProbeError::ApiError(
            "agy 1.2.2 or later requires a local CSRF token".into()
        ))
    );
    assert!(!*called.lock().unwrap());
}

// upstream: AntigravityCLIUsageReportTests.swift:442 CSRF gated CLI fetch reaches the print report without spawning
#[tokio::test]
async fn csrf_gated_cli_fetch_reaches_the_print_report_without_spawning() {
    let spawn_called = Arc::new(Mutex::new(false));
    let seen = Arc::clone(&spawn_called);
    let gate = fetch_by_spawning_if_reachable(Some((1, 2, 2)), move || {
        *seen.lock().unwrap() = true;
        async { Ok::<_, AntigravityStatusProbeError>(()) }
    })
    .await;
    assert_eq!(
        gate,
        Err(AntigravityStatusProbeError::ApiError(
            "agy 1.2.2 or later requires a local CSRF token".into()
        ))
    );
    assert!(!*spawn_called.lock().unwrap());

    let result = run_print_usage(
        Path::new("/synthetic/agy"),
        &BTreeMap::new(),
        tempdir().unwrap().path(),
        Duration::from_secs(10),
        &AgyVersionResolver::new(),
        &runner(|request| {
            if request.arguments == vec!["--version"] {
                return Ok(CliProcessOutput {
                    stdout: b"1.2.2".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                });
            }
            Ok(CliProcessOutput {
                stdout: report_json(),
                stderr: Vec::new(),
                exit_code: 0,
            })
        }),
        &CancellationToken::new(),
    )
    .await;
    assert!(result.unwrap().has_known_quota_summary());
}

// upstream: AntigravityCLIUsageReportTests.swift:430 older or unknown agy keeps the managed spawn
#[tokio::test]
async fn older_or_unknown_agy_keeps_the_managed_spawn() {
    for version in ["", "1.2.1"] {
        let result = fetch_by_spawning_if_reachable(parse_version(version), || async {
            Ok::<_, AntigravityStatusProbeError>("spawned")
        })
        .await;
        assert_eq!(result.unwrap(), "spawned");
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:462 full fetch retains managed spawning for older and unknown versions
#[tokio::test]
async fn full_fetch_retains_managed_spawning_for_older_and_unknown_versions() {
    for version in [None, Some((1, 2, 1))] {
        let called = Arc::new(Mutex::new(false));
        let seen = Arc::clone(&called);
        let result = fetch_by_spawning_if_reachable(version, move || {
            *seen.lock().unwrap() = true;
            async { Ok::<_, AntigravityStatusProbeError>("managed-spawn") }
        })
        .await
        .unwrap();
        assert_eq!(result, "managed-spawn");
        assert!(*called.lock().unwrap());
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:481 CSRF gated CLI fetch resolves the agy version once
#[tokio::test]
async fn version_resolver_reuses_one_probe_result() {
    let resolver = AgyVersionResolver::new();
    let calls = Arc::new(Mutex::new(0_usize));
    for _ in 0..2 {
        let seen = Arc::clone(&calls);
        assert_eq!(
            resolver
                .resolve(&CancellationToken::new(), || async move {
                    *seen.lock().unwrap() += 1;
                    Ok(Some((1, 2, 2)))
                })
                .await
                .unwrap(),
            Some((1, 2, 2))
        );
    }
    assert_eq!(*calls.lock().unwrap(), 1);
}

// upstream: AntigravityCLIUsageReportTests.swift:524 shared resolver serves the legacy gate and the print report from one run
#[tokio::test]
async fn shared_resolver_serves_the_version_gate_and_print_report_from_one_run() {
    let calls = Arc::new(Mutex::new(0_usize));
    let seen = Arc::clone(&calls);
    let runner = runner(move |request| {
        if request.arguments == vec!["--version"] {
            *seen.lock().unwrap() += 1;
            return Ok(CliProcessOutput {
                stdout: b"1.2.2".to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
            });
        }
        Ok(CliProcessOutput {
            stdout: report_json(),
            stderr: Vec::new(),
            exit_code: 0,
        })
    });
    let resolver = AgyVersionResolver::new();
    let directory = tempdir().unwrap();
    let gate = agy_version(
        ariadusage_engine::providers::antigravity::version_probe_request(
            Path::new("/synthetic/agy"),
            &BTreeMap::new(),
            Some(directory.path()),
            Duration::from_secs(3),
            4_096,
        ),
        &resolver,
        &runner,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(gate, Some((1, 2, 2)));
    let snapshot = run_print_usage(
        Path::new("/synthetic/agy"),
        &BTreeMap::new(),
        directory.path(),
        Duration::from_secs(10),
        &resolver,
        &runner,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(snapshot.has_known_quota_summary());
    assert_eq!(*calls.lock().unwrap(), 1);
}

// upstream: AntigravityCLIUsageReportTests.swift:549 version resolver replays a probe failure to the print report
#[tokio::test]
async fn version_resolver_replays_a_probe_failure() {
    let resolver = AgyVersionResolver::new();
    let first = resolver
        .resolve(&CancellationToken::new(), || async {
            Err(AntigravityStatusProbeError::ApiError(
                "synthetic failure".into(),
            ))
        })
        .await;
    let second = resolver
        .resolve(&CancellationToken::new(), || async {
            panic!("cached version failure must not probe again");
        })
        .await;
    assert_eq!(first, second);
}

// upstream: AntigravityCLIUsageReportTests.swift:569 version resolver does not cache cancellation
#[tokio::test]
async fn version_resolver_does_not_cache_cancellation() {
    let resolver = AgyVersionResolver::new();
    let first = resolver
        .resolve(&CancellationToken::new(), || async {
            Err(AntigravityStatusProbeError::Cancelled)
        })
        .await;
    assert_eq!(first, Err(AntigravityStatusProbeError::Cancelled));
    let second = resolver
        .resolve(&CancellationToken::new(), || async { Ok(Some((1, 2, 2))) })
        .await;
    assert_eq!(second, Ok(Some((1, 2, 2))));
}

// upstream: AntigravityCLIUsageReportTests.swift:579 CSRF skip cannot enable identity free reports for scoped Auto accounts
#[tokio::test]
async fn csrf_skip_cannot_enable_identity_free_reports_for_scoped_auto_accounts() {
    for selected in [false, true] {
        let mut cx = context(
            SourceMode::Auto,
            selected.then_some("selected-account"),
            BTreeMap::new(),
        );
        if !selected {
            cx.env.insert(
                AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
                SecretString::new("synthetic-account-token"),
            );
        }
        let gate = fetch_by_spawning_if_reachable(Some((1, 2, 2)), || async {
            Ok::<_, AntigravityStatusProbeError>(())
        })
        .await;
        assert!(gate.is_err());
        let result = fetch_with_report_fallback(
            &cx,
            || async {
                Err(AntigravityStatusProbeError::ApiError(
                    "spawn skipped".into(),
                ))
            },
            || async { panic!("identity-free print cannot run for scoped Auto accounts") },
            None::<fn() -> std::future::Ready<Result<FetchResult, AntigravityStatusProbeError>>>,
        )
        .await;
        assert_eq!(
            result,
            Err(AntigravityStatusProbeError::ApiError(
                "spawn skipped".into()
            ))
        );
    }
}

// upstream: AntigravityCLIUsageReportTests.swift:605 warm usage bypasses version probing and print subprocesses
#[tokio::test]
async fn warm_usage_bypasses_version_probing_and_print_subprocesses() {
    let snapshot = ariadusage_engine::providers::antigravity::status_probe::AntigravityStatusSnapshot {
        model_quotas: vec![],
        account_email: None,
        account_plan: None,
        source: ariadusage_engine::providers::antigravity::status_probe::AntigravitySnapshotSource::Cli,
        quota_summary: Some(ariadusage_engine::providers::antigravity::quota_summary::AntigravityQuotaSummary {
            raw_description: None,
            groups: vec![ariadusage_engine::providers::antigravity::quota_summary::AntigravityQuotaSummaryGroup {
                key: "gemini".into(),
                label: "Gemini Models".into(),
                buckets: vec![ariadusage_engine::providers::antigravity::quota_summary::AntigravityQuotaSummaryBucket {
                    bucket_id: "gemini-5h".into(),
                    display_name: Some("Five Hour Limit Remaining".into()),
                    window: Some("5h".into()),
                    disabled: false,
                    remaining_fraction: Some(0.6),
                    reset_time: None,
                    reset_description: None,
                }],
            }],
        }),
    };
    let process = WarmAgyProcess {
        info: ProcessInfoResult {
            pid: 9901,
            extension_port: None,
            extension_csrf_token: None,
            csrf_token: Some(String::new()),
            command: "/synthetic/agy".into(),
            identity: None,
        },
        executable_path: Some("/synthetic/agy".into()),
        owner_uid: Some(1000),
    };
    let process = Arc::new(Mutex::new(Some(process)));
    let deps = WarmAgyDependencies {
        process_infos: Arc::new(move |_| {
            let process = process.lock().unwrap().take().unwrap();
            Box::pin(async move { Ok(vec![process]) })
        }),
        listening_ports: Arc::new(|_, _| Box::pin(async move { Ok(vec![56789]) })),
        fetch_snapshot: Arc::new(move |_, _| {
            let snapshot = snapshot.clone();
            Box::pin(async move { Ok(snapshot) })
        }),
        current_uid: 1000,
        owned_pid: Arc::new(|| Box::pin(async move { None })),
        now: Arc::new(Instant::now),
    };
    let result = ariadusage_engine::providers::antigravity::cli_https::try_warm_agy_fetch(
        Duration::from_secs(2),
        Some(Path::new("/synthetic/agy")),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(result.is_some());
}

#[test]
fn child_environment_removes_app_owned_credentials() {
    let key = AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned();
    let environment = BTreeMap::from([
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        (key.clone(), "synthetic-secret".to_owned()),
    ]);
    let child = child_environment(&environment);
    assert_eq!(child["PATH"], "/usr/bin:/bin");
    assert!(!child.contains_key(&key));
}

fn context(
    source_mode: SourceMode,
    selected_token_account: Option<&str>,
    env: BTreeMap<String, SecretString>,
) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("antigravity").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "synthetic-antigravity-cli".to_owned(),
        source_mode,
        env,
        include_credits: false,
        selected_token_account: selected_token_account.map(str::to_owned),
        cancel: CancellationToken::new(),
    }
}

fn fetch_result(email: Option<&str>) -> FetchResult {
    let identity = email.map(|email| {
        ProviderIdentitySnapshot::new(
            Some(ProviderId::new("antigravity").unwrap()),
            Some(email.to_owned()),
            None,
            None,
            None,
        )
    });
    let usage = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        Timestamp::now(),
        identity,
        Confidence::Exact,
    )
    .unwrap();
    FetchResult::new(usage, "cli", "antigravity.cli-https", SourceKind::Cli)
}

fn runner<F>(handler: F) -> CliProcessRunner
where
    F: Fn(CliProcessRequest) -> Result<CliProcessOutput, ProcessError> + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    Arc::new(move |request, _cancel| {
        let handler = Arc::clone(&handler);
        Box::pin(async move { handler(request) })
    })
}

fn report_json() -> Vec<u8> {
    br#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"name":"Gemini Models","buckets":[{"id":"gemini-5h","name":"Five Hour Limit Remaining","remaining_fraction":0.6}]}]}}}"#.to_vec()
}

fn report_json_with(status: &str, command: &str, fraction: Option<f64>, disabled: bool) -> Vec<u8> {
    let fraction = fraction.map_or(String::new(), |value| {
        format!(",\"remaining_fraction\":{value}")
    });
    format!(
        "{{\"status\":\"{status}\",\"command\":{{\"name\":\"{command}\",\"data\":{{\"groups\":[{{\"name\":\"Gemini Models\",\"buckets\":[{{\"id\":\"gemini-5h\",\"name\":\"Five Hour Limit Remaining\",\"disabled\":{disabled}{fraction}}}]}}]}}}}}}"
    )
    .into_bytes()
}
