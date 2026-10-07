// Ported from CodexBar Tests/CodexBarTests/SubprocessRunnerTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/SpawnedProcessGroupTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/process.rs"]
mod support;

use ariadusage_core::pipeline::FetchInteraction;
use tempfile::tempdir;

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:133, SpawnedProcessGroupTests.swift:561
async fn session_escaped_output_holders_are_reaped_after_parent_exit() {
    support::require_nextest();
    let command = support::command(&["hold-detached-pipe", "50000"]);
    let output = support::run_fresh(call(), command).await.unwrap();
    let pid = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    support::wait_until_stopped(pid).await;
}

#[tokio::test]
// CodexBar: SpawnedProcessGroupTests.swift:615
async fn holder_scan_rechecks_for_helpers_spawned_during_term() {
    support::require_nextest();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("holder-child-pid");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let command = support::command(&["hold-detached-term-spawn", &pid_path_arg]);
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        support::run_fresh(call(), command),
    )
    .await
    .expect("holder cleanup should be bounded")
    .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("READY"));
    let pid = support::wait_for_pid(&pid_path).await;
    support::wait_until_stopped(pid).await;
}

#[tokio::test]
// CodexBar: SpawnedProcessGroupTests.swift:696
async fn same_group_helper_holding_output_pipes_is_cleaned_up() {
    support::require_nextest();
    let command = support::command(&["hold-pipe", "50000"]);
    let output = support::run_fresh(call(), command).await.unwrap();
    let pid = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<i32>()
        .unwrap();
    support::wait_until_stopped(pid).await;
}

#[tokio::test]
async fn engine_holding_pipe_read_ends_is_never_selected_as_a_holder() {
    support::require_nextest();
    let output = support::run_fresh(call(), support::command(&["print", "stdout", "32"]))
        .await
        .unwrap();
    assert_eq!(
        output.stdout.as_slice(),
        b"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
    );
}

fn call() -> ariadusage_engine::brokers::call::BrokerCall {
    support::call(FetchInteraction::UserInitiated)
}
