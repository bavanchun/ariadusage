// Ported from CodexBar Tests/CodexBarTests/AntigravityWarmAgyReuseTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::pipeline::FetchResult;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::providers::antigravity::cli_https::{
    AntigravityCLIHTTPSFetchStrategy, WarmAgyDependencies, WarmAgyProcess, process_matches_binary,
    try_warm_agy_fetch,
};
use ariadusage_engine::providers::antigravity::descriptor::AntigravityStrategyConfig;
use ariadusage_engine::providers::antigravity::status_probe::{
    AntigravityModelQuota, AntigravitySnapshotSource, AntigravityStatusProbe,
    AntigravityStatusProbeError, AntigravityStatusSnapshot, ProcessEntry, ProcessInfoResult,
    ProcessScope,
};
use tokio_util::sync::CancellationToken;

type Error = AntigravityStatusProbeError;

fn snapshot(email: &str) -> AntigravityStatusSnapshot {
    AntigravityStatusSnapshot {
        model_quotas: vec![AntigravityModelQuota::new(
            "Gemini Pro",
            "gemini-pro",
            Some(0.8),
            None,
            None,
        )],
        account_email: Some(email.to_owned()),
        account_plan: Some("Pro".to_owned()),
        source: AntigravitySnapshotSource::Local,
        quota_summary: None,
    }
}

fn process(
    pid: i32,
    command: &str,
    executable_path: Option<&str>,
    owner_uid: u32,
) -> WarmAgyProcess {
    WarmAgyProcess {
        info: ProcessInfoResult {
            pid,
            csrf_token: Some(String::new()),
            extension_port: None,
            extension_csrf_token: None,
            command: command.to_owned(),
            identity: None,
        },
        executable_path: executable_path.map(PathBuf::from),
        owner_uid: Some(owner_uid),
    }
}

fn deps(
    processes: Vec<WarmAgyProcess>,
    listening: impl Fn(i32) -> Vec<u16> + Send + Sync + 'static,
    fetch: impl Fn(Vec<u16>) -> Result<AntigravityStatusSnapshot, Error> + Send + Sync + 'static,
) -> WarmAgyDependencies {
    let listening = Arc::new(listening);
    let fetch = Arc::new(fetch);
    WarmAgyDependencies {
        process_infos: Arc::new(move |_| {
            let processes = processes.clone();
            Box::pin(async move { Ok(processes) })
        }),
        listening_ports: Arc::new(move |pid, _| {
            let listening = Arc::clone(&listening);
            Box::pin(async move { Ok(listening(pid)) })
        }),
        fetch_snapshot: Arc::new(move |ports, _| {
            let fetch = Arc::clone(&fetch);
            Box::pin(async move { fetch(ports) })
        }),
        current_uid: 501,
        owned_pid: Arc::new(|| Box::pin(async { None })),
        now: Arc::new(Instant::now),
    }
}

fn strategy() -> AntigravityCLIHTTPSFetchStrategy {
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    AntigravityCLIHTTPSFetchStrategy::new(AntigravityStrategyConfig::new(
        net,
        "/tmp/ariadusage-warm-test",
        "/tmp/ariadusage-warm-test-home",
    ))
}

fn result(email: &str) -> FetchResult {
    AntigravityCLIHTTPSFetchStrategy::make_result(&snapshot(email)).unwrap()
}

// upstream: AntigravityWarmAgyReuseTests.swift:8 warm agy found reuses ports without spawn
#[tokio::test]
async fn warm_agy_found_reuses_ports_without_spawn() {
    let listening = Arc::new(Mutex::new(Vec::new()));
    let fetched = Arc::new(Mutex::new(0));
    let listening_seen = Arc::clone(&listening);
    let fetched_seen = Arc::clone(&fetched);
    let deps = deps(
        vec![process(
            9901,
            "/usr/local/bin/agy",
            Some("/usr/local/bin/agy"),
            501,
        )],
        move |pid| {
            listening_seen.lock().unwrap().push(pid);
            vec![56789]
        },
        move |ports| {
            fetched_seen.lock().unwrap().clone_from(&1);
            assert_eq!(ports, [56789]);
            Ok(snapshot("warm@example.com"))
        },
    );
    let found = try_warm_agy_fetch(
        Duration::from_secs(2),
        Some(Path::new("/usr/local/bin/agy")),
        None,
        &deps,
        &CancellationToken::new(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(found.account_email.as_deref(), Some("warm@example.com"));
    assert_eq!(found.model_quotas[0].model_id, "gemini-pro");
    assert_eq!(&*listening.lock().unwrap(), &[9901]);
    assert_eq!(*fetched.lock().unwrap(), 1);
}

// upstream: AntigravityWarmAgyReuseTests.swift:34 no warm agy returns nil
#[tokio::test]
async fn no_warm_agy_returns_nil() {
    let deps = deps(
        vec![],
        |_| panic!("listeningPorts must not be called"),
        |_| panic!("fetchSnapshot must not be called"),
    );
    assert!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            None,
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .is_none()
    );
}

// upstream: AntigravityWarmAgyReuseTests.swift:52 process infos throws returns nil
#[tokio::test]
async fn process_infos_throws_returns_nil() {
    let deps = WarmAgyDependencies {
        process_infos: Arc::new(|_| Box::pin(async { Err(Error::MissingCSRFToken) })),
        listening_ports: Arc::new(|_, _| Box::pin(async { panic!("must not listen") })),
        fetch_snapshot: Arc::new(|_, _| Box::pin(async { panic!("must not fetch") })),
        current_uid: 501,
        owned_pid: Arc::new(|| Box::pin(async { None })),
        now: Arc::new(Instant::now),
    };
    assert!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            None,
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .is_none()
    );
}

// upstream: AntigravityWarmAgyReuseTests.swift:72 warm agy fetch fails returns nil
#[tokio::test]
async fn warm_agy_fetch_fails_returns_nil() {
    let calls = Arc::new(Mutex::new(0));
    let seen = Arc::clone(&calls);
    let deps = deps(
        vec![process(
            7701,
            "/usr/local/bin/agy",
            Some("/usr/local/bin/agy"),
            501,
        )],
        |_| vec![55555],
        move |_| {
            *seen.lock().unwrap() += 1;
            Err(Error::PortDetectionFailed("endpoint not ready".into()))
        },
    );
    assert!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            None,
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(*calls.lock().unwrap(), 1);
}

// upstream: AntigravityWarmAgyReuseTests.swift:91 ide process ignored not reuseable as warm CLI
#[tokio::test]
async fn ide_process_is_ignored_as_warm_cli() {
    let fetched = Arc::new(Mutex::new(0));
    let seen = Arc::clone(&fetched);
    let deps = deps(
        vec![process(
            8801,
            "/Applications/Antigravity IDE.app/Contents/Resources/language_server --csrf_token abc123 --app_data_dir antigravity-ide",
            None,
            501,
        )],
        |_| vec![44444],
        move |_| {
            *seen.lock().unwrap() += 1;
            Ok(snapshot("ide@example.com"))
        },
    );
    assert!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            None,
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(*fetched.lock().unwrap(), 0);
}

// upstream: AntigravityWarmAgyReuseTests.swift:119 owned agy excluded falls back to spawn path
#[tokio::test]
async fn owned_agy_is_excluded() {
    let deps = deps(
        vec![process(
            4242,
            "/usr/local/bin/agy",
            Some("/usr/local/bin/agy"),
            501,
        )],
        |_| panic!("owned agy must not be probed"),
        |_| panic!("owned agy must not be fetched"),
    );
    let deps = WarmAgyDependencies {
        owned_pid: Arc::new(|| Box::pin(async { Some(4242) })),
        ..deps
    };
    assert!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            None,
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .is_none()
    );
}

// upstream: AntigravityWarmAgyReuseTests.swift:144 external agy reused when owned also present
#[tokio::test]
async fn external_agy_is_reused_when_owned_is_present() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&seen);
    let deps = deps(
        vec![
            process(4242, "/usr/local/bin/agy", Some("/usr/local/bin/agy"), 501),
            process(7000, "/usr/local/bin/agy", Some("/usr/local/bin/agy"), 501),
        ],
        move |pid| {
            observed.lock().unwrap().push(pid);
            vec![50050]
        },
        |_| Ok(snapshot("external@example.com")),
    );
    let deps = WarmAgyDependencies {
        owned_pid: Arc::new(|| Box::pin(async { Some(4242) })),
        ..deps
    };
    assert_eq!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            None,
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .unwrap()
        .account_email
        .as_deref(),
        Some("external@example.com")
    );
    assert_eq!(&*seen.lock().unwrap(), &[7000]);
}

// upstream: AntigravityWarmAgyReuseTests.swift:166 other user agy is ignored
#[tokio::test]
async fn other_user_agy_is_ignored() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&seen);
    let deps = deps(
        vec![
            process(6001, "/usr/local/bin/agy", Some("/usr/local/bin/agy"), 502),
            process(6002, "/usr/local/bin/agy", Some("/usr/local/bin/agy"), 501),
        ],
        move |pid| {
            observed.lock().unwrap().push(pid);
            vec![pid as u16]
        },
        |_| Ok(snapshot("same-user@example.com")),
    );
    assert_eq!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            None,
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .unwrap()
        .account_email
        .as_deref(),
        Some("same-user@example.com")
    );
    assert_eq!(&*seen.lock().unwrap(), &[6002]);
}

// upstream: AntigravityWarmAgyReuseTests.swift:186 account mismatch tries next warm agy
#[tokio::test]
async fn account_mismatch_tries_next_warm_agy() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&seen);
    let deps = deps(
        vec![
            process(6101, "/usr/local/bin/agy", Some("/usr/local/bin/agy"), 501),
            process(6102, "/usr/local/bin/agy", Some("/usr/local/bin/agy"), 501),
        ],
        move |pid| {
            observed.lock().unwrap().push(pid);
            vec![pid as u16]
        },
        |ports| {
            Ok(snapshot(if ports == [6101] {
                "other@example.com"
            } else {
                "SELECTED@example.com"
            }))
        },
    );
    assert_eq!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            None,
            Some("selected@example.com"),
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .unwrap()
        .account_email
        .as_deref(),
        Some("SELECTED@example.com")
    );
    assert_eq!(&*seen.lock().unwrap(), &[6101, 6102]);
}

// upstream: AntigravityWarmAgyReuseTests.swift:208 binary mismatch tries next warm agy
#[tokio::test]
async fn binary_mismatch_tries_next_warm_agy() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&seen);
    let deps = deps(
        vec![
            process(6151, "/other/agy", Some("/other/agy"), 501),
            process(6152, "/selected/agy", Some("/selected/agy"), 501),
        ],
        move |pid| {
            observed.lock().unwrap().push(pid);
            vec![pid as u16]
        },
        |_| Ok(snapshot("selected@example.com")),
    );
    assert_eq!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            Some(Path::new("/selected/agy")),
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .unwrap()
        .account_email
        .as_deref(),
        Some("selected@example.com")
    );
    assert_eq!(&*seen.lock().unwrap(), &[6152]);
}

// upstream: AntigravityWarmAgyReuseTests.swift:232 warm reuse matches verified executable identity rather than argv spelling
#[tokio::test]
async fn warm_reuse_matches_verified_executable_identity_parameter_rows() {
    let cases = [
        ("agy", Some("/selected/agy"), true),
        ("agy --debug", Some("/selected/agy"), true),
        ("/selected/agy", Some("/other/agy"), false),
        ("agy", None, false),
        ("/selected/agy", None, true),
        ("/selected/agy-helper", None, false),
        ("/selected/agy", Some("agy"), false),
    ];
    for (command, executable, expected) in cases {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&seen);
        let deps = deps(
            vec![process(6150, command, executable, 501)],
            move |pid| {
                observed.lock().unwrap().push(pid);
                vec![56789]
            },
            |_| Ok(snapshot("selected@example.com")),
        );
        let actual = try_warm_agy_fetch(
            Duration::from_secs(2),
            Some(Path::new("/selected/agy")),
            None,
            &deps,
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .is_some();
        assert_eq!(
            actual, expected,
            "command={command:?}, executable={executable:?}"
        );
        assert_eq!(
            &*seen.lock().unwrap(),
            if expected { &[6150][..] } else { &[][..] }
        );
    }
}

// upstream: AntigravityWarmAgyReuseTests.swift:264 process scan keeps kernel identity separate from argv and preserves csrf checks
#[test]
fn process_scan_keeps_kernel_identity_separate_from_argv_and_preserves_csrf_checks() {
    let entries = [
        ProcessEntry::new(200, "agy --debug"),
        ProcessEntry::new(
            201,
            "/Applications/Antigravity.app/language_server --app_data_dir antigravity",
        ),
    ];
    let results =
        AntigravityStatusProbe::process_infos_from_entries(&entries, ProcessScope::IdeAndCli)
            .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].command, "agy --debug");
    assert_eq!(results[0].csrf_token.as_deref(), Some(""));
    let kernel_identity = process(200, "agy --debug", Some("/selected/agy"), 501);
    assert!(process_matches_binary(
        &kernel_identity,
        Path::new("/selected/agy")
    ));
    assert_eq!(kernel_identity.info.command, "agy --debug");
    assert!(matches!(
        AntigravityStatusProbe::process_infos_from_entries(&entries, ProcessScope::AppOnly),
        Err(Error::MissingCSRFToken)
    ));
}

// upstream: AntigravityWarmAgyReuseTests.swift:283 warm reuse matches a selected symlink to the kernel executable path
#[cfg(unix)]
#[tokio::test]
async fn warm_reuse_matches_selected_symlink_to_kernel_executable_path() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("actual agy");
    let selected = root.path().join("agy");
    std::fs::write(&executable, b"synthetic executable identity").unwrap();
    std::os::unix::fs::symlink(&executable, &selected).unwrap();
    let canonical = std::fs::canonicalize(&executable).unwrap();
    let deps = deps(
        vec![process(6150, "agy", canonical.to_str(), 501)],
        |_| vec![56789],
        |_| Ok(snapshot("selected@example.com")),
    );
    assert_eq!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            Some(&selected),
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .unwrap()
        .account_email
        .as_deref(),
        Some("selected@example.com")
    );
}

// upstream: AntigravityWarmAgyReuseTests.swift:305 warm probe deadline is shared across discovery and candidates
#[tokio::test]
async fn warm_probe_deadline_is_shared_across_discovery_and_candidates() {
    let now = Arc::new(Mutex::new(Instant::now()));
    let process_now = Arc::clone(&now);
    let listen_now = Arc::clone(&now);
    let listened = Arc::new(Mutex::new(0));
    let fetched = Arc::new(Mutex::new(0));
    let listened_seen = Arc::clone(&listened);
    let fetched_seen = Arc::clone(&fetched);
    let deps = WarmAgyDependencies {
        process_infos: Arc::new(move |timeout| {
            assert_eq!(timeout, Duration::from_secs(2));
            *process_now.lock().unwrap() += Duration::from_millis(1500);
            Box::pin(async {
                Ok(vec![
                    process(6201, "/usr/local/bin/agy", Some("/usr/local/bin/agy"), 501),
                    process(6202, "/usr/local/bin/agy", Some("/usr/local/bin/agy"), 501),
                ])
            })
        }),
        listening_ports: Arc::new(move |_, timeout| {
            *listened_seen.lock().unwrap() += 1;
            assert!(timeout <= Duration::from_millis(501));
            *listen_now.lock().unwrap() += Duration::from_millis(600);
            Box::pin(async { Ok(vec![62010]) })
        }),
        fetch_snapshot: Arc::new(move |_, _| {
            *fetched_seen.lock().unwrap() += 1;
            Box::pin(async { Ok(snapshot("late@example.com")) })
        }),
        current_uid: 501,
        owned_pid: Arc::new(|| Box::pin(async { None })),
        now: Arc::new(move || *now.lock().unwrap()),
    };
    assert!(
        try_warm_agy_fetch(
            Duration::from_secs(2),
            None,
            None,
            &deps,
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(*listened.lock().unwrap(), 1);
    assert_eq!(*fetched.lock().unwrap(), 0);
}

// upstream: AntigravityWarmAgyReuseTests.swift:338 warm reuse skips spawn path
#[tokio::test]
async fn warm_reuse_skips_spawn_path() {
    let spawn = Arc::new(Mutex::new(0));
    let called = Arc::clone(&spawn);
    let deps = deps(
        vec![process(
            1234,
            "/usr/local/bin/agy",
            Some("/usr/local/bin/agy"),
            501,
        )],
        |_| vec![40000],
        |_| Ok(snapshot("warm@example.com")),
    );
    let result = strategy()
        .fetch_using_warm_session(
            Path::new("/usr/local/bin/agy"),
            None,
            &deps,
            &CancellationToken::new(),
            move || {
                *called.lock().unwrap() += 1;
                Box::pin(async { Ok(result("spawned@example.com")) })
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result.usage.identity.unwrap().account_email.as_deref(),
        Some("warm@example.com")
    );
    assert_eq!(result.source_label, "cli");
    assert_eq!(*spawn.lock().unwrap(), 0);
}

// upstream: AntigravityWarmAgyReuseTests.swift:364 no warm agy falls back to spawn path
#[tokio::test]
async fn no_warm_agy_falls_back_to_spawn_path() {
    let spawn = Arc::new(Mutex::new(0));
    let called = Arc::clone(&spawn);
    let deps = deps(vec![], |_| vec![], |_| Err(Error::NotRunning));
    let result = strategy()
        .fetch_using_warm_session(
            Path::new("/usr/local/bin/agy"),
            None,
            &deps,
            &CancellationToken::new(),
            move || {
                *called.lock().unwrap() += 1;
                Box::pin(async { Ok(result("spawned@example.com")) })
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result.usage.identity.unwrap().account_email.as_deref(),
        Some("spawned@example.com")
    );
    assert_eq!(*spawn.lock().unwrap(), 1);
}

// upstream: AntigravityWarmAgyReuseTests.swift:390 warm probe cancellation does not fall back to spawn
#[tokio::test]
async fn warm_probe_cancellation_does_not_fall_back_to_spawn() {
    let spawn = Arc::new(Mutex::new(0));
    let called = Arc::clone(&spawn);
    let deps = WarmAgyDependencies {
        process_infos: Arc::new(|_| Box::pin(async { Err(Error::Cancelled) })),
        listening_ports: Arc::new(|_, _| Box::pin(async { Ok(vec![]) })),
        fetch_snapshot: Arc::new(|_, _| Box::pin(async { Err(Error::NotRunning) })),
        current_uid: 501,
        owned_pid: Arc::new(|| Box::pin(async { None })),
        now: Arc::new(Instant::now),
    };
    assert!(matches!(
        strategy()
            .fetch_using_warm_session(
                Path::new("/usr/local/bin/agy"),
                None,
                &deps,
                &CancellationToken::new(),
                move || {
                    *called.lock().unwrap() += 1;
                    Box::pin(async { Ok(result("spawned@example.com")) })
                }
            )
            .await,
        Err(Error::Cancelled)
    ));
    assert_eq!(*spawn.lock().unwrap(), 0);
}

// upstream: AntigravityWarmAgyReuseTests.swift:420 long lived session reuses signed in external agy without spawning
#[tokio::test]
async fn long_lived_session_reuses_signed_in_external_agy_without_spawning() {
    let spawn = Arc::new(Mutex::new(0));
    let called = Arc::clone(&spawn);
    let deps = deps(
        vec![process(
            6301,
            "/usr/local/bin/agy",
            Some("/usr/local/bin/agy"),
            501,
        )],
        |_| vec![50080],
        |_| Ok(snapshot("terminal@example.com")),
    );
    let result = strategy()
        .fetch_using_warm_session(
            Path::new("/usr/local/bin/agy"),
            None,
            &deps,
            &CancellationToken::new(),
            move || {
                *called.lock().unwrap() += 1;
                Box::pin(async { Ok(result("spawned@example.com")) })
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result.usage.identity.unwrap().account_email.as_deref(),
        Some("terminal@example.com")
    );
    assert_eq!(*spawn.lock().unwrap(), 0);
}

// upstream: AntigravityWarmAgyReuseTests.swift:443 long lived session falls back to managed agy when external account mismatches
#[tokio::test]
async fn long_lived_session_falls_back_to_managed_agy_when_external_account_mismatches() {
    let spawn = Arc::new(Mutex::new(0));
    let called = Arc::clone(&spawn);
    let deps = deps(
        vec![process(
            6301,
            "/usr/local/bin/agy",
            Some("/usr/local/bin/agy"),
            501,
        )],
        |_| vec![50080],
        |_| Ok(snapshot("other@example.com")),
    );
    let result = strategy()
        .fetch_using_warm_session(
            Path::new("/usr/local/bin/agy"),
            Some("selected@example.com"),
            &deps,
            &CancellationToken::new(),
            move || {
                *called.lock().unwrap() += 1;
                Box::pin(async { Ok(result("selected@example.com")) })
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result.usage.identity.unwrap().account_email.as_deref(),
        Some("selected@example.com")
    );
    assert_eq!(*spawn.lock().unwrap(), 1);
}
