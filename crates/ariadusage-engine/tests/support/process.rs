#![cfg(target_os = "linux")]
#![allow(dead_code)] // Shared process-test helpers are used by selected phase test binaries.

use std::path::Path;
use std::time::Duration;

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::process::{
    AbsolutePath, Command, ProcessError, ProcessIdentity, ProcessRegistry, ProcessSignal,
    process_identity, process_state, run, signal,
};
use tokio_util::sync::CancellationToken;

pub fn call(interaction: FetchInteraction) -> BrokerCall {
    BrokerCall {
        interaction,
        cancel: CancellationToken::new(),
        request_id: "synthetic-process-test".to_owned(),
    }
}

pub fn command(args: &[&str]) -> Command {
    Command::new(
        AbsolutePath::new(env!("CARGO_BIN_EXE_ariadusage-test-child")).unwrap(),
        args.iter().map(std::ffi::OsString::from).collect(),
    )
    .timeout(Duration::from_secs(5))
}

pub async fn run_fresh(
    call: BrokerCall,
    command: Command,
) -> Result<ariadusage_engine::brokers::process::Output, ProcessError> {
    run(
        call,
        command,
        &ProcessRegistry::new(),
        &LaunchGate::default(),
    )
    .await
}

pub async fn run_with(
    call: BrokerCall,
    command: Command,
    registry: &ProcessRegistry,
    gate: &LaunchGate,
) -> Result<ariadusage_engine::brokers::process::Output, ProcessError> {
    run(call, command, registry, gate).await
}

pub fn require_nextest() {
    assert!(
        std::env::var_os("NEXTEST_RUN_ID").is_some(),
        "destructive process tests require NEXTEST_RUN_ID"
    );
}

pub async fn wait_for_pid(path: &Path) -> i32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(path)
                .and_then(|value| value.trim().parse::<i32>().map_err(std::io::Error::other))
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("helper child should publish its PID")
}

pub async fn wait_until_stopped(pid: i32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_state(Path::new("/proc"), pid).is_some_and(|state| state != 'Z') {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("process should stop before the cleanup deadline");
}

pub fn kill_exact(pid: i32) {
    if let Some(identity) = process_identity(Path::new("/proc"), pid) {
        let _ = signal(identity, ProcessSignal::Kill);
    }
}

pub fn current_identity(pid: i32) -> Option<ProcessIdentity> {
    process_identity(Path::new("/proc"), pid)
}
