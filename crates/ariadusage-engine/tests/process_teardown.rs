// Ported from CodexBar TestsLinux/ProcessOwnershipReaperTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/SubprocessRunnerTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/SpawnedProcessGroupTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/process.rs"]
mod support;

use std::time::{Duration, Instant};

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::process::{ProcessError, ProcessSignal, signal};
use tempfile::tempdir;

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:214, ProcessOwnershipReaperTests.swift:14
async fn timeout_reaps_a_detached_descendant_from_the_launch_tree() {
    support::require_nextest();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("detached-pid");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let mut command = support::command(&["spawn-detached-wait", "50000", &pid_path_arg]);
    command.set_timeout(Duration::from_millis(150));
    let task = tokio::spawn(support::run_fresh(
        support::call(FetchInteraction::UserInitiated),
        command,
    ));

    let pid = support::wait_for_pid(&pid_path).await;
    assert!(matches!(task.await.unwrap(), Err(ProcessError::TimedOut)));
    support::wait_until_stopped(pid).await;
}

#[tokio::test]
// CodexBar: ProcessOwnershipReaperTests.swift:110
async fn timeout_kills_a_same_group_child_that_cleared_its_environment() {
    support::require_nextest();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("env-cleared-pid");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let mut command = support::command(&["spawn-clear-env-wait", &pid_path_arg]);
    command.set_timeout(Duration::from_millis(150));
    let task = tokio::spawn(support::run_fresh(
        support::call(FetchInteraction::UserInitiated),
        command,
    ));

    let pid = support::wait_for_pid(&pid_path).await;
    assert!(matches!(task.await.unwrap(), Err(ProcessError::TimedOut)));
    support::wait_until_stopped(pid).await;
}

#[tokio::test]
// CodexBar: SpawnedProcessGroupTests.swift:285, :396
async fn teardown_allows_the_term_grace_before_kill_escalation() {
    support::require_nextest();
    let mut command = support::command(&["ignore-term", "50000"]);
    command.set_timeout(Duration::from_millis(100));
    let started = Instant::now();
    assert!(matches!(
        support::run_fresh(support::call(FetchInteraction::UserInitiated), command).await,
        Err(ProcessError::TimedOut)
    ));
    assert!(started.elapsed() >= Duration::from_millis(350));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
// CodexBar: SpawnedProcessGroupTests.swift:474
async fn reaper_catches_a_session_escaped_helper_spawned_during_term() {
    support::require_nextest();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("term-spawned-pid");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let mut command = support::command(&["spawn-detached-on-term", &pid_path_arg]);
    command.set_reap_marker(true);
    command.set_timeout(Duration::from_millis(150));

    assert!(matches!(
        support::run_fresh(support::call(FetchInteraction::UserInitiated), command).await,
        Err(ProcessError::TimedOut)
    ));
    let pid = support::wait_for_pid(&pid_path).await;
    support::wait_until_stopped(pid).await;
}

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:268, :324, :382
async fn concurrent_hangs_terminate_without_starving_other_launches() {
    support::require_nextest();
    let started = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let mut command = support::command(&["ignore-term", "50000"]);
        command.set_timeout(Duration::from_millis(100));
        tasks.spawn(support::run_fresh(
            support::call(FetchInteraction::UserInitiated),
            command,
        ));
    }
    let mut timed_out = 0;
    while let Some(result) = tokio::time::timeout(Duration::from_secs(5), tasks.join_next())
        .await
        .unwrap()
    {
        if matches!(result.unwrap(), Err(ProcessError::TimedOut)) {
            timed_out += 1;
        }
    }
    assert_eq!(timed_out, 4);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:324, :382
async fn repeated_cancel_timeout_races_leave_no_running_helper() {
    support::require_nextest();
    for _ in 0..5 {
        let request = support::call(FetchInteraction::UserInitiated);
        let cancel = request.cancel.clone();
        let mut command = support::command(&["ignore-term", "50000"]);
        command.set_timeout(Duration::from_millis(100));
        let task = tokio::spawn(support::run_fresh(request, command));
        tokio::time::sleep(Duration::from_millis(90)).await;
        cancel.cancel();
        assert!(matches!(
            task.await.unwrap(),
            Err(ProcessError::Cancelled | ProcessError::TimedOut)
        ));
    }
}

#[tokio::test]
// CodexBar: SpawnedProcessGroupTests.swift:341
async fn normal_root_exit_reaps_reparented_same_group_members() {
    support::require_nextest();
    let output = support::run_fresh(
        support::call(FetchInteraction::UserInitiated),
        support::command(&["spawn-group-no-pipe", "50000"]),
    )
    .await
    .unwrap();
    let pid = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    support::wait_until_stopped(pid).await;
}

#[test]
fn pid_identity_mismatch_is_rejected_before_escalation() {
    support::require_nextest();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ariadusage-test-child"))
        .env_clear()
        .args(["sleep", "50000"])
        .spawn()
        .unwrap();
    let current = support::current_identity(child.id() as i32).unwrap();
    let stale = ariadusage_engine::brokers::process::ProcessIdentity {
        pid: current.pid,
        start_ticks: current.start_ticks.wrapping_add(1),
    };

    assert!(!signal(stale, ProcessSignal::Kill).unwrap());
    assert!(support::current_identity(current.pid).is_some());
    child.kill().unwrap();
    let _ = child.wait();
}
