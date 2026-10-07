// Ported from CodexBar TestsLinux/AntigravityPortDiscoveryTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::time::Duration;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::local_probe::{ArgvView, LocalProbe, ProbeError, SecretArgv};
use ariadusage_engine::brokers::process::{ProcessIdentity, procscan};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

fn test_broker_call() -> BrokerCall {
    BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "test-local-probe".to_string(),
    }
}

fn write_fake_process(
    proc_root: &Path,
    pid: i32,
    uid: u32,
    start_ticks: u64,
    net_ns: Option<&str>,
    argv: Option<&[&str]>,
) {
    let process = proc_root.join(pid.to_string());
    fs::create_dir_all(&process).unwrap();

    let pid_str = pid.to_string();
    let start_ticks_str = start_ticks.to_string();
    let mut fields = vec!["0"; 50];
    fields[0] = "S";
    fields[1] = "1";
    fields[2] = &pid_str;
    fields[3] = &pid_str;
    fields[19] = &start_ticks_str;

    fs::write(
        process.join("stat"),
        format!("{pid} (fixture) {}\n", fields.join(" ")),
    )
    .unwrap();

    fs::write(
        process.join("status"),
        format!(
            "Name:\tfixture\nState:\tS\nTgid:\t{pid}\nPid:\t{pid}\nPPid:\t1\nTracerPid:\t0\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nGid:\t{uid}\t{uid}\t{uid}\t{uid}\nFDSize:\t1\nGroups:\t{uid}\nThreads:\t1\nSigQ:\t0/0\nSigPnd:\t0000000000000000\nShdPnd:\t0000000000000000\nSigBlk:\t0000000000000000\nSigIgn:\t0000000000000000\nSigCgt:\t0000000000000000\nCapInh:\t0000000000000000\nCapPrm:\t0000000000000000\nCapEff:\t0000000000000000\n"
        ),
    )
    .unwrap();

    if let Some(target_ns) = net_ns {
        let ns_dir = process.join("ns");
        fs::create_dir_all(&ns_dir).unwrap();
        symlink(target_ns, ns_dir.join("net")).unwrap();
    }

    if let Some(args) = argv {
        let mut cmdline_bytes = Vec::new();
        for arg in args {
            cmdline_bytes.extend_from_slice(arg.as_bytes());
            cmdline_bytes.push(0);
        }
        fs::write(process.join("cmdline"), cmdline_bytes).unwrap();
    }
}

#[tokio::test]
// CodexBar: TestsLinux/AntigravityPortDiscoveryTests.swift:17
async fn failed_or_empty_lsof_recovers_only_process_owned_proc_listeners() {
    let proc_root = tempdir().unwrap();
    let process_root = proc_root.path().join("42");
    let fd_dir = process_root.join("fd");
    let net_dir = process_root.join("net");
    fs::create_dir_all(&fd_dir).unwrap();
    fs::create_dir_all(&net_dir).unwrap();

    symlink("socket:[111111]", fd_dir.join("7")).unwrap();

    let table = "\
sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n\
0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 111111\n\
1: 0100007F:C000 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 999999\n";
    fs::write(net_dir.join("tcp"), table).unwrap();

    let call = test_broker_call();
    let probe = LocalProbe::with_hooks(
        proc_root.path().to_path_buf(),
        1000,
        Some("net:[12345]".to_string()),
    );
    let ports = probe
        .listening_tcp_ports(&call, 42, Duration::from_secs(5))
        .await
        .unwrap();

    assert_eq!(ports, vec![8080]);
}

#[tokio::test]
// CodexBar: TestsLinux/AntigravityPortDiscoveryTests.swift:24
async fn missing_lsof_executable_recovers_through_proc() {
    let proc_root = tempdir().unwrap();
    let process_root = proc_root.path().join("42");
    let fd_dir = process_root.join("fd");
    let net_dir = process_root.join("net");
    fs::create_dir_all(&fd_dir).unwrap();
    fs::create_dir_all(&net_dir).unwrap();

    symlink("socket:[111111]", fd_dir.join("7")).unwrap();

    let table = "\
sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n\
0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 111111\n";
    fs::write(net_dir.join("tcp"), table).unwrap();

    let call = test_broker_call();
    let probe = LocalProbe::with_hooks(
        proc_root.path().to_path_buf(),
        1000,
        Some("net:[12345]".to_string()),
    );
    let ports = probe
        .listening_tcp_ports(&call, 42, Duration::from_secs(5))
        .await
        .unwrap();

    assert_eq!(ports, vec![8080]);
}

#[tokio::test]
// CodexBar: TestsLinux/AntigravityPortDiscoveryTests.swift:91
async fn absent_process_retains_no_ports_error() {
    let proc_root = tempdir().unwrap();
    let call = test_broker_call();
    let probe = LocalProbe::with_hooks(
        proc_root.path().to_path_buf(),
        1000,
        Some("net:[12345]".to_string()),
    );
    let result = probe
        .listening_tcp_ports(&call, 999, Duration::from_secs(5))
        .await;

    assert_eq!(result, Err(ProbeError::NoListeningPorts));
}

#[tokio::test]
// CodexBar: TestsLinux/AntigravityPortDiscoveryTests.swift:104
async fn ordinary_empty_discovery_retains_no_ports_classification() {
    let proc_root = tempdir().unwrap();
    let process_root = proc_root.path().join("42");
    let fd_dir = process_root.join("fd");
    fs::create_dir_all(&fd_dir).unwrap();

    let call = test_broker_call();
    let probe = LocalProbe::with_hooks(
        proc_root.path().to_path_buf(),
        1000,
        Some("net:[12345]".to_string()),
    );
    let result = probe
        .listening_tcp_ports(&call, 42, Duration::from_secs(5))
        .await;

    assert_eq!(result, Err(ProbeError::NoListeningPorts));
}

#[tokio::test]
// CodexBar: TestsLinux/AntigravityPortDiscoveryTests.swift:114
async fn cancelled_discovery_returns_cancelled() {
    let proc_root = tempdir().unwrap();
    let process_root = proc_root.path().join("42");
    let fd_dir = process_root.join("fd");
    let net_dir = process_root.join("net");
    fs::create_dir_all(&fd_dir).unwrap();
    fs::create_dir_all(&net_dir).unwrap();
    symlink("socket:[111111]", fd_dir.join("7")).unwrap();

    let call = test_broker_call();
    call.cancel.cancel();

    let probe = LocalProbe::with_hooks(
        proc_root.path().to_path_buf(),
        1000,
        Some("net:[12345]".to_string()),
    );
    let result = probe
        .listening_tcp_ports(&call, 42, Duration::from_secs(5))
        .await;

    assert_eq!(result, Err(ProbeError::Cancelled));
}

#[tokio::test]
// CodexBar: TestsLinux/AntigravityPortDiscoveryTests.swift:142
async fn linux_proc_recovers_a_real_listener_on_own_pid() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let my_pid = std::process::id() as i32;

    let call = test_broker_call();
    let probe = LocalProbe::new();
    let ports = probe
        .listening_tcp_ports(&call, my_pid, Duration::from_secs(5))
        .await
        .unwrap();

    assert!(ports.contains(&port));

    let identity = procscan::process_identity(Path::new("/proc"), my_pid).unwrap();
    assert!(
        probe
            .listener_still_owned(&call, identity, port)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn foreign_uid_and_foreign_namespace_processes_are_excluded() {
    let proc_root = tempdir().unwrap();
    let uid = 1000;
    let own_ns = "net:[1001]";

    // Process 101: matching uid and namespace, matching argv
    write_fake_process(
        proc_root.path(),
        101,
        uid,
        10000,
        Some(own_ns),
        Some(&["agy", "serve", "--port", "4242"]),
    );
    // Process 102: foreign uid
    write_fake_process(
        proc_root.path(),
        102,
        2000,
        10000,
        Some(own_ns),
        Some(&["agy", "serve", "--port", "4242"]),
    );
    // Process 103: foreign namespace
    write_fake_process(
        proc_root.path(),
        103,
        uid,
        10000,
        Some("net:[9999]"),
        Some(&["agy", "serve", "--port", "4242"]),
    );
    // Process 104: non-matching argv
    write_fake_process(
        proc_root.path(),
        104,
        uid,
        10000,
        Some(own_ns),
        Some(&["other", "tool"]),
    );

    let call = test_broker_call();
    let probe = LocalProbe::with_hooks(
        proc_root.path().to_path_buf(),
        uid,
        Some(own_ns.to_string()),
    );

    let found = probe
        .same_user_processes(&call, |view| view.command_line().contains("agy"))
        .await
        .unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].identity.pid, 101);
}

#[tokio::test]
async fn argv_of_non_matching_process_is_not_retained_and_debug_is_redacted() {
    let proc_root = tempdir().unwrap();
    let uid = 1000;
    let own_ns = "net:[1001]";

    write_fake_process(
        proc_root.path(),
        101,
        uid,
        10000,
        Some(own_ns),
        Some(&["agy", "serve", "--csrf_token=invented-secret"]),
    );
    write_fake_process(
        proc_root.path(),
        102,
        uid,
        10000,
        Some(own_ns),
        Some(&["other", "process", "--sensitive=123"]),
    );

    let call = test_broker_call();
    let probe = LocalProbe::with_hooks(
        proc_root.path().to_path_buf(),
        uid,
        Some(own_ns.to_string()),
    );

    let found = probe
        .same_user_processes(&call, |view| view.command_line().contains("agy"))
        .await
        .unwrap();

    assert_eq!(found.len(), 1);
    let proc = &found[0];
    assert_eq!(proc.identity.pid, 101);

    // ArgvView debug redaction check
    let args = vec!["secret1".to_string(), "secret2".to_string()];
    let view = ArgvView::new(&args);
    let view_debug = format!("{view:?}");
    assert!(view_debug.contains("argc: 2"));
    assert!(!view_debug.contains("secret"));

    // SecretArgv debug redaction check
    let secret_argv = SecretArgv::new(vec!["secret1".to_string(), "secret2".to_string()]);
    let secret_debug = format!("{secret_argv:?}");
    assert!(secret_debug.contains("argc: 2"));
    assert!(!secret_debug.contains("secret"));

    // ProbeProcess debug redaction check
    let proc_debug = format!("{proc:?}");
    assert!(proc_debug.contains("argc: 3"));
    assert!(!proc_debug.contains("invented-secret"));
    assert!(!proc_debug.contains("sensitive"));
}

#[tokio::test]
async fn listener_ownership_recheck_detects_closed_listener_and_changed_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let my_pid = std::process::id() as i32;

    let call = test_broker_call();
    let probe = LocalProbe::new();
    let identity = procscan::process_identity(Path::new("/proc"), my_pid).unwrap();

    // 1. While socket is open: owned
    assert!(
        probe
            .listener_still_owned(&call, identity, port)
            .await
            .unwrap()
    );

    // 2. Changed identity (start_ticks differ): false
    let fake_identity = ProcessIdentity {
        pid: my_pid,
        start_ticks: identity.start_ticks.wrapping_add(9999),
    };
    assert!(
        !probe
            .listener_still_owned(&call, fake_identity, port)
            .await
            .unwrap()
    );

    // 3. Close the listener: false
    drop(listener);
    assert!(
        !probe
            .listener_still_owned(&call, identity, port)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn scan_deadline_returns_timed_out() {
    let proc_root = tempdir().unwrap();
    let call = test_broker_call();
    let probe = LocalProbe::with_hooks(
        proc_root.path().to_path_buf(),
        1000,
        Some("net:[12345]".to_string()),
    );

    let result = probe.listening_tcp_ports(&call, 42, Duration::ZERO).await;

    assert_eq!(result, Err(ProbeError::TimedOut));
}
