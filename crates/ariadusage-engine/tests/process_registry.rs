// Ported from CodexBar Tests/CodexBarTests/TTYCommandRunnerTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexCLILaunchGateTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/process.rs"]
mod support;

use std::time::{Duration, Instant};

use ariadusage_core::gates::launch::{LaunchFailureKind, LaunchGate};
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::process::{
    AbsolutePath, Command, ProcessError, ProcessIdentity, ProcessRegistry, process_descendants,
};
use tempfile::tempdir;

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:74, :89
async fn shutdown_fence_rejects_new_launches() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    registry.fence();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("should-not-launch");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let result = support::run_with(
        support::call(FetchInteraction::UserInitiated),
        support::command(&["ready-wait", &pid_path_arg]),
        &registry,
        &LaunchGate::default(),
    )
    .await;

    assert_eq!(result.unwrap_err(), ProcessError::ShuttingDown);
    assert!(!pid_path.exists());
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:32, :48, :63, :114
async fn shutdown_terms_then_kills_registered_processes_and_drains() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let gate = LaunchGate::default();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("registered-child");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let second_pid_path = temp.path().join("second-registered-child");
    let second_pid_path_arg = second_pid_path.to_string_lossy().to_string();
    let request = support::call(FetchInteraction::UserInitiated);
    let command = support::command(&["ignore-term-ready", "50000", &pid_path_arg]);
    let task_registry = registry.clone();
    let task_gate = gate.clone();
    let task = tokio::spawn(async move {
        support::run_with(request, command, &task_registry, &task_gate).await
    });
    let second_request = support::call(FetchInteraction::UserInitiated);
    let second_command = support::command(&["ignore-term-ready", "50000", &second_pid_path_arg]);
    let second_registry = registry.clone();
    let second_gate = gate.clone();
    let second_task = tokio::spawn(async move {
        support::run_with(
            second_request,
            second_command,
            &second_registry,
            &second_gate,
        )
        .await
    });
    let pid = support::wait_for_pid(&pid_path).await;
    let second_pid = support::wait_for_pid(&second_pid_path).await;
    assert_eq!(registry.active_launches(), 2);
    let started = Instant::now();

    registry.shutdown().await;
    assert!(registry.is_fenced());
    assert_eq!(registry.active_launches(), 0);
    assert!(started.elapsed() >= Duration::from_millis(350));
    support::wait_until_stopped(pid).await;
    support::wait_until_stopped(second_pid).await;
    let _ = task.await.unwrap();
    let _ = second_task.await.unwrap();
}

#[tokio::test]
// CodexBar: CodexCLILaunchGateTests.swift:8
async fn engine_suppresses_only_background_launches_after_a_failure() {
    support::require_nextest();
    let binary = env!("CARGO_BIN_EXE_ariadusage-test-child");
    let gate = LaunchGate::default();
    gate.record_failure(binary, LaunchFailureKind::Process);
    let registry = ProcessRegistry::new();
    let mut blocked = support::command(&["print", "stdout", "8"]);
    blocked.set_timeout(Duration::from_secs(2));
    assert!(matches!(
        support::run_with(
            support::call(FetchInteraction::Background),
            blocked,
            &registry,
            &gate,
        )
        .await,
        Err(ProcessError::LaunchSuppressed { .. })
    ));

    let output = support::run_with(
        support::call(FetchInteraction::UserInitiated),
        support::command(&["print", "stdout", "8"]),
        &registry,
        &gate,
    )
    .await
    .unwrap();
    assert_eq!(output.stdout.as_slice(), b"xxxxxxxx");
}

#[tokio::test]
async fn process_launch_failure_sets_the_binary_cooldown() {
    support::require_nextest();
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let missing_program = temp.path().join("synthetic-program");
    symlink(
        env!("CARGO_BIN_EXE_ariadusage-test-child"),
        &missing_program,
    )
    .unwrap();
    let program = AbsolutePath::new(&missing_program).unwrap();
    std::fs::remove_file(&missing_program).unwrap();

    let registry = ProcessRegistry::new();
    let gate = LaunchGate::default();
    let first = Command::new(program.clone(), Vec::new());
    assert!(matches!(
        support::run_with(
            support::call(FetchInteraction::Background),
            first,
            &registry,
            &gate,
        )
        .await,
        Err(ProcessError::LaunchFailed)
    ));

    let second = Command::new(program, Vec::new());
    assert!(matches!(
        support::run_with(
            support::call(FetchInteraction::Background),
            second,
            &registry,
            &gate,
        )
        .await,
        Err(ProcessError::LaunchSuppressed { .. })
    ));
}

#[tokio::test]
async fn drain_waits_for_registered_launches_to_finish() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let gate = LaunchGate::default();
    let temp = tempdir().unwrap();
    let pid_path = temp.path().join("drain-child");
    let pid_path_arg = pid_path.to_string_lossy().to_string();
    let request = support::call(FetchInteraction::UserInitiated);
    let cancel = request.cancel.clone();
    let task_registry = registry.clone();
    let task_gate = gate.clone();
    let command = support::command(&["ready-wait", &pid_path_arg]);
    let task = tokio::spawn(async move {
        support::run_with(request, command, &task_registry, &task_gate).await
    });
    let pid = support::wait_for_pid(&pid_path).await;
    registry.fence();
    let drain = tokio::spawn({
        let registry = registry.clone();
        async move { registry.drain().await }
    });
    tokio::task::yield_now().await;
    assert!(!drain.is_finished());

    cancel.cancel();
    drain.await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(ProcessError::Cancelled)));
    support::wait_until_stopped(pid).await;
}

#[test]
// CodexBar: TTYCommandRunnerTests.swift:136, :151
fn descendant_resolver_walks_thread_children_and_returns_unique_identities() {
    use std::path::Path;

    let proc_root = tempdir().unwrap();
    write_fake_process(proc_root.path(), 500, 5_000, &[501, 1500]);
    write_fake_task(proc_root.path(), 500, 500, &[501, 501]);
    write_fake_task(proc_root.path(), 500, 1500, &[502]);
    write_fake_process(proc_root.path(), 501, 5_001, &[503]);
    write_fake_process(proc_root.path(), 502, 5_002, &[]);
    write_fake_process(proc_root.path(), 503, 5_003, &[]);

    let descendants = process_descendants(
        Path::new(proc_root.path()),
        ProcessIdentity {
            pid: 500,
            start_ticks: 5_000,
        },
        1000,
    );
    assert_eq!(
        descendants,
        [
            ProcessIdentity {
                pid: 501,
                start_ticks: 5_001,
            },
            ProcessIdentity {
                pid: 502,
                start_ticks: 5_002,
            },
            ProcessIdentity {
                pid: 503,
                start_ticks: 5_003,
            },
        ]
    );
}

fn write_fake_process(root: &std::path::Path, pid: i32, start_ticks: u64, children: &[i32]) {
    write_fake_task(root, pid, pid, children);
    let mut fields = vec!["0".to_owned(); 50];
    fields[0] = "S".to_owned();
    fields[1] = "1".to_owned();
    fields[2] = pid.to_string();
    fields[19] = start_ticks.to_string();
    let process = root.join(pid.to_string());
    std::fs::write(
        process.join("stat"),
        format!("{pid} (synthetic process) {}\n", fields.join(" ")),
    )
    .unwrap();
    std::fs::write(
        process.join("status"),
        "Name:\tsynthetic\nUid:\t1000\t1000\t1000\t1000\n",
    )
    .unwrap();
}

fn write_fake_task(root: &std::path::Path, pid: i32, task: i32, children: &[i32]) {
    let task_dir = root
        .join(pid.to_string())
        .join("task")
        .join(task.to_string());
    std::fs::create_dir_all(&task_dir).unwrap();
    let children = children
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::write(task_dir.join("children"), children).unwrap();
}
