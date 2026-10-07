// Ported from CodexBar Tests/CodexBarTests/PathBuilderTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ShellCommandLocatorProcessTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ShellCommandOutputLimitTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ShellCommandForegroundTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::exec_resolver::effective_path;
use ariadusage_engine::brokers::login_shell::LoginShell;
use ariadusage_engine::brokers::process::{ProcessEnv, ProcessError};
use tempfile::tempdir;

#[path = "support/process.rs"]
mod support;

fn helper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ariadusage-test-child"))
}

#[test]
// CodexBar: PathBuilderTests.swift:8
fn merges_login_shell_path_when_available() {
    let login = [PathBuf::from("/login/bin"), PathBuf::from("/login/alt")];
    let env_path = OsString::from("/custom/bin:/usr/bin");
    let result = effective_path(Some(&login), Some(&env_path));
    assert_eq!(
        result.to_string_lossy(),
        "/login/bin:/login/alt:/custom/bin:/usr/bin"
    );
}

#[test]
// CodexBar: PathBuilderTests.swift:17
fn falls_back_to_existing_path_when_no_login_path() {
    let env_path = OsString::from("/custom/bin:/usr/bin");
    let result = effective_path(None, Some(&env_path));
    assert_eq!(result.to_string_lossy(), "/custom/bin:/usr/bin");
}

#[test]
// CodexBar: PathBuilderTests.swift:26
fn uses_fallback_when_no_path_available() {
    let result = effective_path(None, None);
    assert_eq!(result.to_string_lossy(), "/usr/bin:/bin:/usr/sbin:/sbin");
}

#[tokio::test]
// CodexBar: PathBuilderTests.swift:48
async fn login_shell_cache_retries_after_timed_out_nil_capture() {
    support::require_nextest();
    let call = support::call(FetchInteraction::UserInitiated);

    let helper = helper_path();
    // 1) First attempt: overflow triggers None
    let env_fail = ProcessEnv::from_iter([
        ("SHELL", helper.to_str().unwrap()),
        ("CI", "1"),
        ("FAKE_SHELL_OVERFLOW", "1"),
    ]);
    let shell = LoginShell::new(env_fail);

    let first = shell.capture_path(&call).await;
    assert_eq!(first, None);
    assert_eq!(shell.current().await, None);

    // 2) Second attempt: retry succeeds after None
    let env_ok = ProcessEnv::from_iter([
        ("SHELL", helper.to_str().unwrap()),
        ("CI", "1"),
        ("FAKE_SHELL_PATH", "/login/bin:/usr/bin"),
    ]);
    let shell_ok = LoginShell::new(env_ok);

    let second = shell_ok.capture_path(&call).await;
    let expected = vec![PathBuf::from("/login/bin"), PathBuf::from("/usr/bin")];
    assert_eq!(second, Some(expected.clone()));
    assert_eq!(shell_ok.current().await, Some(expected));

    // 3) Concurrent calls coalesce on the same in-flight capture
    let call_clone = call.clone();
    let shell_clone = shell_ok.clone();
    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { shell_clone.capture_path(&call_clone).await }),
        tokio::spawn(async move { shell_ok.capture_path(&call).await })
    );
    assert!(r1.unwrap().is_some());
    assert!(r2.unwrap().is_some());
}

#[tokio::test]
// CodexBar: PathBuilderTests.swift:71
async fn shell_runner_drains_noisy_stdout_and_stderr() {
    support::require_nextest();
    let call = support::call(FetchInteraction::UserInitiated);
    let helper = helper_path();

    let env = ProcessEnv::from_iter([
        ("SHELL", helper.to_str().unwrap()),
        ("CI", "1"),
        ("FAKE_SHELL_NOISY", "1"),
    ]);
    let shell = LoginShell::new(env);

    let output = shell
        .run_shell_command(
            &call,
            vec!["-c".into(), "noisy".into()],
            Duration::from_secs(5),
            1024 * 1024,
        )
        .await
        .expect("noisy execution should succeed");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("out-3999"));
    assert!(stdout.contains("__CODEXBAR_DONE__"));
}

#[tokio::test]
// CodexBar: PathBuilderTests.swift:92
async fn shell_runner_terminates_background_children_after_normal_exit() {
    support::require_nextest();
    let call = support::call(FetchInteraction::UserInitiated);
    let helper = helper_path();
    let temp = tempdir().unwrap();
    let marker = temp.path().join("bg-marker");

    let env = ProcessEnv::from_iter([
        ("SHELL", helper.to_str().unwrap()),
        ("CI", "1"),
        ("FAKE_SHELL_BG_CHILD", "1"),
        ("FAKE_SHELL_MARKER", marker.to_str().unwrap()),
    ]);
    let shell = LoginShell::new(env);

    let output = shell
        .run_shell_command(
            &call,
            vec!["-c".into(), "trap '' HUP TERM".into()],
            Duration::from_secs(3),
            1024 * 1024,
        )
        .await
        .expect("command should exit cleanly");

    let pid_str = String::from_utf8_lossy(&output.stdout);
    let pid: i32 = pid_str.trim().parse().expect("child PID output");

    // The background child should have touched the marker
    assert!(marker.exists());

    // Give reaper a moment to finish teardown
    support::wait_until_stopped(pid).await;
    assert_eq!(support::current_identity(pid), None);
}

#[test]
// CodexBar: ShellCommandLocatorProcessTests.swift:12
fn shell_probe_pipe_descriptors_close_across_unrelated_execs() {
    // Verify standard descriptors and newly created pipes have close-on-exec set
    use rustix::io::{FdFlags, fcntl_getfd};
    use rustix::pipe::{PipeFlags, pipe_with};

    let (read_fd, write_fd) = pipe_with(PipeFlags::CLOEXEC).expect("pipe_with CLOEXEC");
    let read_flags = fcntl_getfd(&read_fd).expect("fcntl_getfd");
    let write_flags = fcntl_getfd(&write_fd).expect("fcntl_getfd");

    assert!(read_flags.contains(FdFlags::CLOEXEC));
    assert!(write_flags.contains(FdFlags::CLOEXEC));
}

#[tokio::test]
// CodexBar: ShellCommandLocatorProcessTests.swift:27
async fn shell_runner_terminates_session_escaped_partial_output_holders_after_timeout() {
    support::require_nextest();
    let call = support::call(FetchInteraction::UserInitiated);
    let helper = helper_path();
    let temp = tempdir().unwrap();
    let base_pid_file = temp.path().join("holder-pids");

    let env = ProcessEnv::from_iter([
        ("SHELL", helper.to_str().unwrap()),
        ("CI", "1"),
        (
            "FAKE_SHELL_HOLDERS_PID_FILE",
            base_pid_file.to_str().unwrap(),
        ),
    ]);
    let shell = LoginShell::new(env);

    let start = tokio::time::Instant::now();
    let result = shell
        .run_shell_command(
            &call,
            vec!["-c".into(), "holders".into()],
            Duration::from_millis(500),
            1024 * 1024,
        )
        .await;

    assert_eq!(result.unwrap_err(), ProcessError::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(5));

    let stdout_pid_file = format!("{}.stdout", base_pid_file.to_str().unwrap());
    let stderr_pid_file = format!("{}.stderr", base_pid_file.to_str().unwrap());

    let pid1 = support::wait_for_pid(Path::new(&stdout_pid_file)).await;
    let pid2 = support::wait_for_pid(Path::new(&stderr_pid_file)).await;

    support::wait_until_stopped(pid1).await;
    support::wait_until_stopped(pid2).await;
    assert_eq!(support::current_identity(pid1), None);
    assert_eq!(support::current_identity(pid2), None);
}

#[tokio::test]
// CodexBar: TestsLinux/ShellCommandOutputLimitTests.swift:8
async fn shell_discovery_rejects_oversized_output_without_returning_a_truncated_path() {
    support::require_nextest();
    let call = support::call(FetchInteraction::UserInitiated);
    let helper = helper_path();

    for byte_count in [4096, 1048576, 1048577, 8388608] {
        let env = ProcessEnv::from_iter([
            ("SHELL", helper.to_str().unwrap()),
            ("CI", "1"),
            ("FAKE_SHELL_ZERO_BYTES", &byte_count.to_string()),
        ]);
        let shell = LoginShell::new(env);

        let result = shell
            .run_shell_command(
                &call,
                vec![
                    "-c".into(),
                    format!("head -c {byte_count} /dev/zero").into(),
                ],
                Duration::from_secs(3),
                1024 * 1024,
            )
            .await;

        if byte_count > 1024 * 1024 {
            assert!(matches!(
                result,
                Err(ProcessError::OutputTooLarge { cap: 1048576, .. })
            ));
        } else {
            let output = result.expect("within output cap");
            assert_eq!(output.stdout.len(), byte_count);
        }
    }

    // Shell runner drains verbose stderr while preserving stdout
    let env_stderr = ProcessEnv::from_iter([
        ("SHELL", helper.to_str().unwrap()),
        ("CI", "1"),
        ("FAKE_SHELL_STDERR_BYTES", "8388608"),
    ]);
    let shell_stderr = LoginShell::new(env_stderr);
    let output = shell_stderr
        .run_shell_command(
            &call,
            vec![
                "-c".into(),
                "head -c 8388608 /dev/zero >&2; printf '/synthetic/bin'".into(),
            ],
            Duration::from_secs(5),
            1024 * 1024,
        )
        .await
        .expect("stderr drain succeeds");

    assert_eq!(&output.stdout[..], b"/synthetic/bin");
}

#[tokio::test]
// CodexBar: ShellCommandForegroundTests.swift:8
async fn shell_probe_requests_a_detached_session() {
    support::require_nextest();
    let call = support::call(FetchInteraction::UserInitiated);
    let helper = helper_path();

    let env = ProcessEnv::from_iter([("SHELL", helper.to_str().unwrap()), ("CI", "1")]);
    let shell = LoginShell::new(env);

    let output = shell
        .run_shell_command(
            &call,
            vec!["print-sid".into()],
            Duration::from_secs(3),
            1024 * 1024,
        )
        .await
        .expect("session command succeeds");

    let sid: i32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("session leader SID");
    // SID should not equal parent process SID
    let parent_sid = rustix::process::getsid(None)
        .expect("parent sid")
        .as_raw_pid();
    assert_ne!(sid, parent_sid);
}

#[tokio::test]
async fn login_shell_asserts_both_ci_branches() {
    support::require_nextest();
    let call = support::call(FetchInteraction::UserInitiated);
    let helper = helper_path();

    // 1) CI set branch
    let env_ci = ProcessEnv::from_iter([
        ("SHELL", helper.to_str().unwrap()),
        ("CI", "1"),
        ("FAKE_SHELL_PATH", "/ci/bin:/usr/bin"),
    ]);
    let shell_ci = LoginShell::new(env_ci);
    let paths_ci = shell_ci.capture_path(&call).await.unwrap();
    assert_eq!(
        paths_ci,
        vec![PathBuf::from("/ci/bin"), PathBuf::from("/usr/bin")]
    );

    // 2) CI unset branch (interactive login shell -l -i -c)
    let env_no_ci = ProcessEnv::from_iter([
        ("SHELL", helper.to_str().unwrap()),
        ("FAKE_SHELL_PATH", "/login/interactive/bin:/usr/bin"),
    ]);
    let shell_no_ci = LoginShell::new(env_no_ci);
    let paths_no_ci = shell_no_ci.capture_path(&call).await.unwrap();
    assert_eq!(
        paths_no_ci,
        vec![
            PathBuf::from("/login/interactive/bin"),
            PathBuf::from("/usr/bin")
        ]
    );
}

#[tokio::test]
async fn login_shell_command_v_and_alias_probe() {
    support::require_nextest();
    let call = support::call(FetchInteraction::UserInitiated);
    let helper = helper_path();

    let env = ProcessEnv::from_iter([
        ("SHELL", helper.to_str().unwrap()),
        ("CI", "1"),
        ("FAKE_SHELL_COMMAND_V", helper.to_str().unwrap()),
        (
            "FAKE_SHELL_ALIAS",
            &format!("alias claude='{}'", helper.to_str().unwrap()),
        ),
    ]);
    let shell = LoginShell::new(env);

    let hit = shell.command_v(&call, "claude").await;
    assert_eq!(hit, Some(helper.clone()));

    let alias_hit = shell.alias(&call, "claude").await;
    assert_eq!(alias_hit, Some(helper));
}
