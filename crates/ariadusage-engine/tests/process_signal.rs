// Ported from CodexBar Tests/CodexBarTests/TTYCommandRunnerTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]
use ariadusage_engine::brokers::process::{ProcessIdentity, ProcessSignal, signal, signal_group};

#[test]
// CodexBar: TTYCommandRunnerTests.swift:89
fn signaling_refuses_pid_one_engine_pid_and_protected_groups() {
    assert!(
        !signal(
            ProcessIdentity {
                pid: 1,
                start_ticks: 0
            },
            ProcessSignal::Kill
        )
        .unwrap()
    );
    assert!(
        !signal(
            ProcessIdentity {
                pid: std::process::id() as i32,
                start_ticks: 0
            },
            ProcessSignal::Kill
        )
        .unwrap()
    );
    assert!(
        !signal_group(
            ProcessIdentity {
                pid: 1,
                start_ticks: 0
            },
            1,
            ProcessSignal::Kill
        )
        .unwrap()
    );
    assert!(
        !signal_group(
            ProcessIdentity {
                pid: std::process::id() as i32,
                start_ticks: 0
            },
            rustix::process::getpgrp().as_raw_pid(),
            ProcessSignal::Kill
        )
        .unwrap()
    );
}

#[test]
fn pid_reuse_identity_mismatch_is_not_signaled() {
    if std::env::var_os("NEXTEST_RUN_ID").is_none() {
        panic!("destructive process tests require NEXTEST_RUN_ID");
    }

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ariadusage-test-child"))
        .env_clear()
        .args(["sleep", "50000"])
        .spawn()
        .unwrap();
    let identity = ProcessIdentity {
        pid: child.id() as i32,
        start_ticks: u64::MAX,
    };

    assert!(!signal(identity, ProcessSignal::Kill).unwrap());
    assert!(std::fs::metadata(format!("/proc/{}/stat", child.id())).is_ok());
    child.kill().unwrap();
    let _ = child.wait();
}
