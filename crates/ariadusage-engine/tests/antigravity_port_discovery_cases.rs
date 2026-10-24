// Ported from CodexBar TestsLinux/AntigravityPortDiscoveryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/AntigravityProcessLauncherLinuxTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::local_probe::{ListenerCommandError, LocalProbe, ProbeError};
use ariadusage_engine::brokers::process::{ProcessEnv, ProcessError, ProcessRegistry};
use ariadusage_engine::providers::antigravity::cli_https::{
    SnapshotWaitDependencies, wait_for_snapshot,
};
use ariadusage_engine::providers::antigravity::cli_session::{
    BrokerProcessLauncher, ProcessLaunchRequest, ProcessLauncher,
};
use ariadusage_engine::providers::antigravity::status_probe::{
    AntigravityModelQuota, AntigravitySnapshotSource, AntigravityStatusSnapshot,
};
use tempfile::{TempDir, tempdir};
use tokio_util::sync::CancellationToken;

fn call() -> BrokerCall {
    BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "port-discovery".into(),
    }
}

struct Fixture {
    root: TempDir,
    lsof: PathBuf,
}
impl Fixture {
    fn new(script: &str) -> Self {
        let root = tempdir().unwrap();
        let lsof = root.path().join("lsof");
        fs::create_dir_all(root.path().join("42/fd")).unwrap();
        fs::create_dir_all(root.path().join("42/net")).unwrap();
        symlink("socket:[111111]", root.path().join("42/fd/7")).unwrap();
        fs::write(root.path().join("42/net/tcp"), concat!(
            "sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n",
            "0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 111111\n",
            "1: 0100007F:C000 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 999999"
        )).unwrap();
        install(&lsof, &format!("#!/bin/sh\n{script}\n"));
        // Synthetic identity metadata is required by the broker security boundary.
        let mut fields = vec!["0"; 50];
        fields[0] = "S";
        fields[19] = "1";
        fs::write(
            root.path().join("42/stat"),
            format!("42 (fixture) {}\n", fields.join(" ")),
        )
        .unwrap();
        fs::write(
            root.path().join("42/status"),
            "Uid:\t1000\t1000\t1000\t1000\n",
        )
        .unwrap();
        fs::create_dir_all(root.path().join("42/ns")).unwrap();
        symlink("net:[12345]", root.path().join("42/ns/net")).unwrap();
        Self { root, lsof }
    }
    fn probe(&self) -> LocalProbe {
        LocalProbe::with_hooks(self.root.path().into(), 1000, Some("net:[12345]".into()))
            .with_listener_executable(Some(self.lsof.clone()))
    }
    async fn ports(&self, timeout: Duration) -> Result<Vec<u16>, ProbeError> {
        self.probe().listening_tcp_ports(&call(), 42, timeout).await
    }
    fn remove_socket(&self) {
        fs::remove_file(self.root.path().join("42/fd/7")).unwrap();
    }
}
fn install(path: &std::path::Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

// upstream: AntigravityPortDiscoveryTests.swift:11 failed or empty lsof recovers only process owned proc listeners
#[tokio::test]
async fn failed_or_empty_lsof_recovers_only_process_owned_proc_listeners() {
    for script in [
        "printf 'lsof: WARNING: cannot stat namespace filesystem\\n' >&2; exit 1",
        "exit 1",
        "exit 0",
        "printf 'lsof: failed\\n' >&2; exit 2",
    ] {
        let fixture = Fixture::new(script);
        // upstream-assert: AntigravityPortDiscoveryTests.swift:20 #expect
        assert_eq!(fixture.ports(Duration::from_secs(5)).await.unwrap(), [8080]);
    }
}

// upstream: AntigravityPortDiscoveryTests.swift:23 missing lsof executable recovers through proc
#[tokio::test]
async fn missing_lsof_executable_recovers_through_proc() {
    let fixture = Fixture::new("exit 0");
    fs::remove_file(&fixture.lsof).unwrap();
    // upstream-assert: AntigravityPortDiscoveryTests.swift:28 #expect
    assert_eq!(fixture.ports(Duration::from_secs(5)).await.unwrap(), [8080]);
}

// upstream: AntigravityPortDiscoveryTests.swift:31 CLI readiness retries namespace warnings until the proc listener appears
#[tokio::test]
async fn cli_readiness_retries_namespace_warnings_until_the_proc_listener_appears() {
    let fixture = Arc::new(Fixture::new(
        "printf 'lsof: WARNING: namespace unavailable\\n' >&2; exit 1",
    ));
    fixture.remove_socket();
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen_attempts = attempts.clone();
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(move |_, timeout| {
            let fixture = fixture.clone();
            let attempt = seen_attempts.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                if attempt == 2 {
                    symlink("socket:[111111]", fixture.root.path().join("42/fd/7")).unwrap();
                }
                fixture.ports(timeout).await.map_err(Into::into)
            })
        }),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(|ports, _| {
            Box::pin(async move {
                // upstream-assert: AntigravityPortDiscoveryTests.swift:52 #expect
                assert_eq!(ports, [8080]);
                Ok(AntigravityStatusSnapshot {
                    model_quotas: vec![AntigravityModelQuota::new(
                        "Gemini Pro",
                        "gemini-pro",
                        Some(0.5),
                        None,
                        None,
                    )],
                    account_email: Some("fixture@example.com".into()),
                    account_plan: Some("Pro".into()),
                    source: AntigravitySnapshotSource::Local,
                    quota_summary: None,
                })
            })
        }),
        now: Arc::new(Instant::now),
    };
    let snapshot = wait_for_snapshot(
        42,
        Instant::now() + Duration::from_secs(15),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    // upstream-assert: AntigravityPortDiscoveryTests.swift:60 #expect
    assert_eq!(
        snapshot.account_email.as_deref(),
        Some("fixture@example.com")
    );
    // upstream-assert: AntigravityPortDiscoveryTests.swift:61 #expect
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

// upstream: AntigravityPortDiscoveryTests.swift:64 successful lsof retains precedence over proc
#[tokio::test]
async fn successful_lsof_retains_precedence_over_proc() {
    let fixture = Fixture::new("printf 'agy 42 user TCP 127.0.0.1:4242 (LISTEN)\\n'");
    // upstream-assert: AntigravityPortDiscoveryTests.swift:68 #expect
    assert_eq!(fixture.ports(Duration::from_secs(5)).await.unwrap(), [4242]);
}

// upstream: AntigravityPortDiscoveryTests.swift:71 failed proc fallback preserves lsof diagnostic
#[tokio::test]
async fn failed_proc_fallback_preserves_lsof_diagnostic() {
    let fixture = Fixture::new("printf 'namespace warning\\n' >&2; exit 1");
    fixture.remove_socket();
    let error = fixture.ports(Duration::from_secs(5)).await.unwrap_err();
    let ProbeError::ListenerDiscoveryPending(ref underlying) = error else {
        panic!("expected pending lsof failure")
    };
    let ListenerCommandError::NonZeroExit { code, stderr } = underlying else {
        panic!("expected original nonzero exit")
    };
    // upstream-assert: AntigravityPortDiscoveryTests.swift:84 #expect
    assert_eq!(*code, 1);
    // upstream-assert: AntigravityPortDiscoveryTests.swift:85 #expect
    assert_eq!(stderr.expose_secret(), "namespace warning\n");
    // upstream-assert: AntigravityPortDiscoveryTests.swift:86 #expect
    assert_eq!(error.to_string(), underlying.to_string());
    assert!(!format!("{error:?} {error}").contains("namespace warning"));
}

// upstream: AntigravityPortDiscoveryTests.swift:90 absent lsof and empty proc retains no ports error
#[tokio::test]
async fn absent_lsof_and_empty_proc_retains_no_ports_error() {
    let fixture = Fixture::new("exit 0");
    let result = fixture
        .probe()
        .with_listener_executable(None)
        .listening_tcp_ports(&call(), 999, Duration::from_secs(5))
        .await;
    // upstream-assert: AntigravityPortDiscoveryTests.swift:94 #expect
    assert_eq!(result, Err(ProbeError::NoListeningPorts));
    assert_eq!(result.unwrap_err().to_string(), "no listening ports found");
}

// upstream: AntigravityPortDiscoveryTests.swift:103 ordinary empty discovery retains the existing no ports classification
#[tokio::test]
async fn ordinary_empty_discovery_retains_the_existing_no_ports_classification() {
    for script in ["exit 0", "exit 1"] {
        let fixture = Fixture::new(script);
        fixture.remove_socket();
        let result = fixture.ports(Duration::from_secs(5)).await;
        // upstream-assert: AntigravityPortDiscoveryTests.swift:108 #expect
        assert_eq!(result, Err(ProbeError::NoListeningPorts));
        assert_eq!(result.unwrap_err().to_string(), "no listening ports found");
    }
}

// upstream: AntigravityPortDiscoveryTests.swift:113 cancelled discovery cannot recover through proc
#[tokio::test]
async fn cancelled_discovery_cannot_recover_through_proc() {
    let fixture = Fixture::new("exit 1");
    let call = call();
    call.cancel.cancel();
    let result = fixture
        .probe()
        .with_listener_executable(None)
        .listening_tcp_ports(&call, 42, Duration::from_secs(5))
        .await;
    // upstream-assert: AntigravityPortDiscoveryTests.swift:125 #expect
    assert_eq!(result, Err(ProbeError::Cancelled));
}

// upstream: AntigravityPortDiscoveryTests.swift:128 lsof timeout cannot recover through proc
#[tokio::test]
async fn lsof_timeout_cannot_recover_through_proc() {
    let fixture = Fixture::new("exec sleep 10");
    let error = fixture
        .ports(Duration::from_secs_f64(0.1))
        .await
        .unwrap_err();
    let ProbeError::ListenerCommand(ListenerCommandError::Process {
        label,
        source: ProcessError::TimedOut,
    }) = error
    else {
        panic!("expected authoritative lsof timeout")
    };
    // upstream-assert: AntigravityPortDiscoveryTests.swift:136 #expect
    assert_eq!(label, "antigravity-lsof");
}

// upstream: AntigravityPortDiscoveryTests.swift:141 Linux proc recovers a real listener after lsof namespace warnings
#[tokio::test]
async fn linux_proc_recovers_a_real_listener_after_lsof_namespace_warnings() {
    let fixture = Fixture::new("printf 'lsof: WARNING: namespace unavailable\\n' >&2; exit 1");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let probe = LocalProbe::new().with_listener_executable(Some(fixture.lsof.clone()));
    let ports = probe
        .listening_tcp_ports(&call(), std::process::id() as i32, Duration::from_secs(5))
        .await
        .unwrap();
    // upstream-assert: AntigravityPortDiscoveryTests.swift:173 #expect
    assert!(ports.contains(&port));
}

// partial-upstream: AntigravityProcessLauncherLinuxTests.swift:12 pty launcher uses home and closes unrelated descriptors
#[tokio::test]
async fn pty_launcher_uses_home() {
    let temp_directory = tempdir().unwrap();
    let home = temp_directory.path().join("home");
    fs::create_dir(&home).unwrap();
    let home = home.canonicalize().unwrap();
    let output = temp_directory.path().join("result.txt");
    let script = temp_directory.path().join("probe.sh");
    install(
        &script,
        &format!("#!/bin/sh\npwd > '{}'\nexec sleep 10\n", output.display()),
    );
    let launcher = BrokerProcessLauncher::new(
        Arc::new(ProcessRegistry::new()),
        Arc::new(LaunchGate::default()),
    );
    let handle = launcher
        .launch_with(ProcessLaunchRequest {
            binary: script,
            call: call(),
            environment: ProcessEnv::empty().with("HOME", home.as_os_str()),
            timeout: Duration::from_secs(5),
        })
        .await
        .unwrap();
    let mut lines = Vec::new();
    for _ in 0..200 {
        if let Ok(text) = fs::read_to_string(&output) {
            lines = text.lines().map(str::to_owned).collect();
            if !lines.is_empty() && text.ends_with('\n') {
                break;
            }
        }
        tokio::time::sleep(Duration::from_secs_f64(0.01)).await;
    }
    handle.terminate().await;
    // Only HOME is ported; the second upstream element "closed" remains MISSING.
    // partial-upstream-assert: AntigravityProcessLauncherLinuxTests.swift:66 #expect
    assert_eq!(lines, [home.to_string_lossy()]);
}

#[tokio::test]
async fn foreign_real_or_effective_uid_and_namespace_never_launch_lsof() {
    for (status, namespace) in [
        ("Uid:\t2000\t1000\t1000\t1000\n", "net:[12345]"),
        ("Uid:\t1000\t2000\t1000\t1000\n", "net:[12345]"),
        ("Uid:\t1000\t1000\t1000\t1000\n", "net:[99999]"),
    ] {
        let fixture = Fixture::new("printf 'agy 42 user TCP 127.0.0.1:4242 (LISTEN)\\n'");
        let launched = fixture.root.path().join("launched");
        install(
            &fixture.lsof,
            &format!("#!/bin/sh\ntouch '{}'\n", launched.display()),
        );
        fs::write(fixture.root.path().join("42/status"), status).unwrap();
        fs::remove_file(fixture.root.path().join("42/ns/net")).unwrap();
        symlink(namespace, fixture.root.path().join("42/ns/net")).unwrap();
        assert_eq!(
            fixture.ports(Duration::from_secs(5)).await,
            Err(ProbeError::NoListeningPorts)
        );
        assert!(!launched.exists());
    }
}

#[tokio::test]
async fn listener_command_uses_exact_argv_and_parses_sorted_unique_ports() {
    let fixture = Fixture::new(concat!(
        "[ \"$#\" -eq 6 ] && [ \"$1\" = '-nP' ] && [ \"$2\" = '-iTCP' ] && ",
        "[ \"$3\" = '-sTCP:LISTEN' ] && [ \"$4\" = '-a' ] && [ \"$5\" = '-p' ] && [ \"$6\" = '42' ] || exit 2\n",
        "printf 'agy 42 user TCP 127.0.0.1:4242 (LISTEN)\\nagy 42 user TCP [::1]:8080 (LISTEN)\\nagy 42 user TCP *:4242 (LISTEN)\\n'"
    ));
    assert_eq!(
        fixture.ports(Duration::from_secs(5)).await.unwrap(),
        [4242, 8080]
    );
}

#[tokio::test]
async fn output_cap_is_terminal_even_when_proc_has_a_listener() {
    let fixture = Fixture::new("exec /usr/bin/head -c 1048577 /dev/zero");
    assert!(matches!(
        fixture.ports(Duration::from_secs(5)).await,
        Err(ProbeError::ListenerCommand(ListenerCommandError::Process {
            source: ProcessError::OutputTooLarge { .. },
            ..
        }))
    ));
}

#[tokio::test]
async fn reused_pid_after_lsof_cannot_return_a_stale_port() {
    let fixture = Fixture::new("exit 0");
    let stat = fixture.root.path().join("42/stat");
    // Change start ticks, not the PID, while lsof is running.
    let mut fields = vec!["0"; 50];
    fields[0] = "S";
    fields[19] = "2";
    fs::write(
        fixture.root.path().join("next-stat"),
        format!("42 (fixture) {}\n", fields.join(" ")),
    )
    .unwrap();
    install(
        &fixture.lsof,
        &format!(
            "#!/bin/sh\n/bin/cp '{}' '{}'\nprintf 'agy 42 user TCP 127.0.0.1:4242 (LISTEN)\\n'\n",
            fixture.root.path().join("next-stat").display(),
            stat.display()
        ),
    );
    assert_eq!(
        fixture.ports(Duration::from_secs(5)).await,
        Err(ProbeError::NoListeningPorts)
    );
}

#[tokio::test]
async fn disabled_lsof_and_valid_process_without_sockets_returns_no_ports() {
    let fixture = Fixture::new("exit 0");
    fixture.remove_socket();
    let result = fixture
        .probe()
        .with_listener_executable(None)
        .listening_tcp_ports(&call(), 42, Duration::from_secs(5))
        .await;
    assert_eq!(result, Err(ProbeError::NoListeningPorts));
}

#[tokio::test]
async fn readiness_deadline_preserves_missing_executable_diagnostic() {
    use ariadusage_engine::providers::antigravity::AntigravityStatusProbeError as Error;
    let fixture = Arc::new(Fixture::new("exit 0"));
    fixture.remove_socket();
    fs::remove_file(&fixture.lsof).unwrap();
    let started = Instant::now();
    let clock = Arc::new(AtomicUsize::new(0));
    let deps = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(move |_, timeout| {
            let fixture = fixture.clone();
            Box::pin(async move { fixture.ports(timeout).await.map_err(Into::into) })
        }),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(|_, _| {
            Box::pin(async { panic!("no listener can produce a snapshot") })
        }),
        now: Arc::new(move || {
            if clock.fetch_add(1, Ordering::SeqCst) < 2 {
                started
            } else {
                started + Duration::from_secs(15)
            }
        }),
    };
    let error = wait_for_snapshot(
        42,
        started + Duration::from_secs(15),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        Error::PortDiscoveryCommandFailed(ListenerCommandError::ExecutableUnavailable)
    ));
}
