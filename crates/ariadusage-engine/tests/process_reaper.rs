// Ported from CodexBar Tests/CodexBarTests/SubprocessRunnerTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ProcessOwnershipReaperTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/process.rs"]
mod support;

use std::process::Stdio;
use std::time::Duration;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::process::{ProcessEnv, ProcessError, StreamPolicy};
use tempfile::tempdir;

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:404
async fn marker_reaper_kills_a_session_escaped_child_after_parent_success() {
    support::require_nextest();
    let mut command = support::command(&["spawn-detached", "50000"]);
    command.set_reap_marker(true);
    let output = support::run_fresh(call(), command).await.unwrap();
    let pid = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<i32>()
        .unwrap();
    support::wait_until_stopped(pid).await;
}

#[tokio::test]
async fn reaper_marker_overrides_a_caller_denylist_entry() {
    support::require_nextest();
    let mut command = support::command(&["marker-present"]);
    command.set_reap_marker(true);
    command.set_env(ProcessEnv::empty().without(["ARIADUSAGE_PROCESS_MARKER"]));

    let output = support::run_fresh(call(), command).await.unwrap();

    assert_eq!(output.stdout.as_slice(), b"true\n");
}

#[tokio::test]
// CodexBar: ProcessOwnershipReaperTests.swift:14
async fn marked_child_is_reaped_while_an_unmarked_same_uid_twin_survives() {
    support::require_nextest();
    let twin = std::process::Command::new(env!("CARGO_BIN_EXE_ariadusage-test-child"))
        .env_clear()
        .args(["grandchild", "50000"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let twin_pid = twin.id() as i32;
    let mut twin_guard = TwinGuard(twin);
    tokio::time::timeout(Duration::from_secs(2), async {
        while rustix::process::getsid(rustix::process::Pid::from_raw(twin_pid))
            .is_ok_and(|session| session.as_raw_pid() != twin_pid)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("unmarked twin should detach from the test process group");

    let mut command = support::command(&["spawn-detached", "50000"]);
    command.set_reap_marker(true);
    let output = support::run_fresh(call(), command).await.unwrap();
    let marked_pid = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<i32>()
        .unwrap();
    support::wait_until_stopped(marked_pid).await;
    assert!(
        support::current_identity(twin_pid).is_some_and(|identity| {
            ariadusage_engine::brokers::process::process_state(
                std::path::Path::new("/proc"),
                identity.pid,
            )
            .is_some_and(|state| state != 'Z')
        }),
        "an unmarked same-user process must survive the marker reaper"
    );

    twin_guard.kill();
    support::wait_until_stopped(twin_pid).await;
}

#[tokio::test]
// CodexBar: ProcessOwnershipReaperTests.swift:14
async fn marker_reaper_runs_after_cancellation() {
    support::require_nextest();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("cancelled-grandchild-pid");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let mut command = support::command(&["spawn-detached-wait", "50000", &pid_path_arg]);
    command.set_reap_marker(true);
    command.set_timeout(Duration::from_secs(30));
    let request = support::call(FetchInteraction::UserInitiated);
    let cancel = request.cancel.clone();
    let task = tokio::spawn(support::run_fresh(request, command));

    let pid = support::wait_for_pid(&pid_path).await;
    cancel.cancel();
    assert!(matches!(task.await.unwrap(), Err(ProcessError::Cancelled)));
    support::wait_until_stopped(pid).await;
}

#[tokio::test]
// CodexBar: ProcessOwnershipReaperTests.swift:14
async fn marker_reaper_runs_after_output_overflow() {
    support::require_nextest();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("overflow-grandchild-pid");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let mut command = support::command(&["spawn-detached-wait", "50000", &pid_path_arg]);
    command.set_reap_marker(true);
    command.set_stdout(StreamPolicy::Capture { cap: 1 });
    command.set_timeout(Duration::from_secs(10));
    let result = support::run_fresh(call(), command).await;

    assert!(matches!(result, Err(ProcessError::OutputTooLarge { .. })));
    let pid = support::wait_for_pid(&pid_path).await;
    support::wait_until_stopped(pid).await;
}

#[tokio::test]
// CodexBar: ProcessOwnershipReaperTests.swift:14
async fn marker_reaper_runs_after_a_child_exits_with_failure() {
    support::require_nextest();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("failed-grandchild-pid");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let mut command = support::command(&["spawn-detached-failure", "50000", &pid_path_arg]);
    command.set_reap_marker(true);
    let output = support::run_fresh(call(), command).await.unwrap();
    assert!(!output.status.success());
    let pid = support::wait_for_pid(&pid_path).await;
    support::wait_until_stopped(pid).await;
}

fn call() -> ariadusage_engine::brokers::call::BrokerCall {
    support::call(FetchInteraction::UserInitiated)
}

struct TwinGuard(std::process::Child);

impl TwinGuard {
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for TwinGuard {
    fn drop(&mut self) {
        self.kill();
    }
}
