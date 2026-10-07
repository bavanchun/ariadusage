// Ported from CodexBar Tests/CodexBarTests/SubprocessRunnerTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/SpawnedProcessGroupTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ProcessPipeCaptureLinuxTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ShellCommandSessionLinuxTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ShellCommandOutputLimitTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::process::{
    AbsolutePath, Command, LaunchMode, ProcessEnv, ProcessError, ProcessRegistry, StdinSpec,
    StreamPolicy, run as run_registered,
};
use tokio_util::sync::CancellationToken;

fn call() -> BrokerCall {
    BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "synthetic-process-test".into(),
    }
}

fn command(args: &[&str]) -> Command {
    Command::new(
        AbsolutePath::new(env!("CARGO_BIN_EXE_ariadusage-test-child")).unwrap(),
        args.iter().map(std::ffi::OsString::from).collect(),
    )
    .env(ProcessEnv::empty())
    .timeout(Duration::from_secs(5))
}

async fn run(
    call: BrokerCall,
    command: Command,
) -> Result<ariadusage_engine::brokers::process::Output, ProcessError> {
    run_registered(
        call,
        command,
        &ProcessRegistry::new(),
        &LaunchGate::default(),
    )
    .await
}

fn require_nextest() {
    if std::env::var_os("NEXTEST_RUN_ID").is_none() {
        panic!("destructive process tests require NEXTEST_RUN_ID");
    }
}

async fn wait_for_child_pid(marker: &Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = fs::read_to_string(marker)
                .and_then(|value| value.parse::<u32>().map_err(std::io::Error::other))
            {
                break pid;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("helper child should publish a PID")
}

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:13
async fn large_finite_timeout_retains_successful_output() {
    require_nextest();
    let mut cmd = command(&["print", "stdout", "8"]);
    cmd.set_timeout(Duration::MAX);
    let output = run(call(), cmd).await.unwrap();
    assert_eq!(output.stdout.as_slice(), b"xxxxxxxx");
}

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:24, :37, :50, :89, SpawnedProcessGroupTests.swift:148
async fn concurrent_streams_are_drained_and_output_over_a_cap_fails_closed() {
    require_nextest();
    let mut cmd = command(&["print", "both", "131072"]);
    cmd.set_stdout(StreamPolicy::Capture { cap: 262_144 });
    cmd.set_stderr(StreamPolicy::Capture { cap: 262_144 });
    let output = run(call(), cmd).await.unwrap();
    assert_eq!(output.stdout.len(), 131_072);
    assert_eq!(output.stderr.len(), 131_072);

    let mut capped = command(&["print", "stdout", "1025"]);
    capped.set_stdout(StreamPolicy::Capture { cap: 1024 });
    assert!(matches!(
        run(call(), capped).await,
        Err(ProcessError::OutputTooLarge { cap: 1024, .. })
    ));
}

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:111
async fn stderr_is_captured_on_nonzero_exit_and_debug_never_discloses_payloads() {
    require_nextest();
    let mut cmd = command(&["stderr-exit", "synthetic-error-value"]);
    cmd.set_stdin(StdinSpec::Secret(zeroize::Zeroizing::new(
        b"synthetic-stdin-secret".to_vec(),
    )));
    let output = run(call(), cmd).await.unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("synthetic-error-value"));
    let rendered = format!("{output:?}");
    assert!(!rendered.contains("synthetic-error-value"));
    assert!(!rendered.contains("synthetic-stdin-secret"));
}

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:186
async fn timeout_kills_the_process_and_drop_guard_kills_an_abandoned_run() {
    require_nextest();
    let mut cmd = command(&["sleep", "50000"]);
    cmd.set_timeout(Duration::from_millis(50));
    assert!(matches!(
        run(call(), cmd).await,
        Err(ProcessError::TimedOut)
    ));

    let marker = std::env::temp_dir().join(format!("ariadusage-ready-{}", std::process::id()));
    let _ = fs::remove_file(&marker);
    let mut abandoned = command(&["ready-wait", marker.to_str().unwrap()]);
    abandoned.set_timeout(Duration::from_secs(30));
    let task = tokio::spawn(run(call(), abandoned));
    let child_pid = wait_for_child_pid(&marker).await;
    task.abort();
    tokio::time::timeout(Duration::from_secs(5), async {
        while PathBuf::from(format!("/proc/{child_pid}")).exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fs::remove_file(marker).unwrap();
}

#[tokio::test]
// CodexBar: SubprocessRunnerTests.swift:346, ShellCommandSessionLinuxTests.swift:9
async fn cancellation_is_prompt_and_session_launch_creates_a_session_leader() {
    require_nextest();
    let marker = std::env::temp_dir().join(format!("ariadusage-cancel-{}", std::process::id()));
    let _ = fs::remove_file(&marker);
    let mut cmd = command(&["ready-wait", marker.to_str().unwrap()]);
    cmd.set_timeout(Duration::from_secs(30));
    cmd.set_launch_mode(LaunchMode::Session);
    let request = call();
    let cancel = request.cancel.clone();
    let task = tokio::spawn(run(request, cmd));
    let child_pid = wait_for_child_pid(&marker).await as i32;
    assert_eq!(
        rustix::process::getsid(rustix::process::Pid::from_raw(child_pid))
            .unwrap()
            .as_raw_pid(),
        child_pid
    );
    cancel.cancel();
    assert!(matches!(task.await.unwrap(), Err(ProcessError::Cancelled)));
    let _ = fs::remove_file(marker);
}

fn inherited_non_cloexec_descriptors() -> std::collections::BTreeMap<i32, String> {
    let mut baseline = std::collections::BTreeMap::new();
    let entries = match fs::read_dir("/proc/self/fd") {
        Ok(read_dir) => read_dir.filter_map(Result::ok).collect::<Vec<_>>(),
        Err(_) => return baseline,
    };
    for entry in entries {
        let Ok(fd) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if fd <= 2 {
            continue;
        }
        let Ok(fdinfo) = fs::read_to_string(format!("/proc/self/fdinfo/{fd}")) else {
            continue;
        };
        let Some(flags_line) = fdinfo.lines().find(|line| line.starts_with("flags:")) else {
            continue;
        };
        let Some(octal_str) = flags_line.strip_prefix("flags:").map(str::trim) else {
            continue;
        };
        let Ok(flags) = u32::from_str_radix(octal_str, 8) else {
            continue;
        };
        const O_CLOEXEC: u32 = 0o02000000;
        if (flags & O_CLOEXEC) == 0
            && let Ok(target) = fs::read_link(format!("/proc/self/fd/{fd}"))
        {
            baseline.insert(fd, target.to_string_lossy().to_string());
        }
    }
    baseline
}

#[tokio::test]
async fn helper_descriptor_listing_does_not_include_an_engine_file() {
    require_nextest();
    let baseline = inherited_non_cloexec_descriptors();
    let file = tempfile::NamedTempFile::new().unwrap();
    let path = file.path().to_string_lossy().to_string();
    let output = run(call(), command(&["list-fds", &path])).await.unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let pid = lines
        .next()
        .and_then(|line| line.strip_prefix("PID "))
        .and_then(|pid| pid.parse::<u32>().ok())
        .expect("helper reports its PID before listing descriptors");
    let descriptors = lines
        .map(|line| {
            let (fd, target) = line.split_once(' ').expect("descriptor and target");
            (fd.parse::<i32>().expect("numeric descriptor"), target)
        })
        .collect::<Vec<_>>();
    for standard_fd in 0..=2 {
        assert!(
            descriptors.iter().any(|(fd, _)| *fd == standard_fd),
            "standard descriptor {standard_fd} should be open"
        );
    }
    let proc_fd_target = format!("/proc/{pid}/fd");
    let mut helper_listing_fd_count = 0;
    for (fd, target) in descriptors.iter().filter(|(fd, _)| *fd > 2) {
        if *target == proc_fd_target {
            helper_listing_fd_count += 1;
        } else {
            assert_eq!(
                baseline.get(fd).map(String::as_str),
                Some(*target),
                "unexpected descriptor {fd}: {target} not in baseline {baseline:?}"
            );
        }
    }
    assert_eq!(
        helper_listing_fd_count, 1,
        "helper should have exactly one descriptor open to its own /proc/<pid>/fd listing"
    );
    assert!(!text.contains(&path));
}

#[tokio::test]
// CodexBar: ProcessPipeCaptureLinuxTests.swift:120, ShellCommandOutputLimitTests.swift:23
async fn pipe_tail_is_drained_and_discarded_stderr_does_not_hit_stdout_cap() {
    require_nextest();
    let output = run(call(), command(&["print", "stdout", "262144"]))
        .await
        .unwrap();
    assert_eq!(output.stdout.len(), 262_144);

    let mut discarded = command(&["stderr-large", "8388608"]);
    discarded.set_stdout(StreamPolicy::Capture { cap: 32 });
    discarded.set_stderr(StreamPolicy::Discard);
    let output = run(call(), discarded).await.unwrap();
    assert_eq!(output.stdout.as_slice(), b"x");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
// CodexBar: ProcessPipeCaptureLinuxTests.swift:135
async fn silent_child_times_out_without_hanging_a_pipe_reader() {
    require_nextest();
    let mut cmd = command(&["sleep", "50000"]);
    cmd.set_timeout(Duration::from_millis(50));
    assert!(matches!(
        run(call(), cmd).await,
        Err(ProcessError::TimedOut)
    ));
}

#[tokio::test]
// CodexBar: ProcessPipeCaptureLinuxTests.swift:67
async fn continuous_output_does_not_defeat_the_capture_timeout() {
    require_nextest();
    let mut cmd = command(&["stream"]);
    cmd.set_stdout(StreamPolicy::Discard);
    cmd.set_timeout(Duration::from_millis(50));
    assert!(matches!(
        run(call(), cmd).await,
        Err(ProcessError::TimedOut)
    ));
}

#[tokio::test]
// CodexBar: ProcessPipeCaptureLinuxTests.swift:211
async fn child_pipe_readers_are_released_after_completed_runs() {
    require_nextest();
    let _ = run(call(), command(&["print", "stdout", "8"]))
        .await
        .unwrap();
    let initial = fs::read_dir("/proc/self/fd").unwrap().count();

    for _ in 0..20 {
        let output = run(call(), command(&["print", "stdout", "8"]))
            .await
            .unwrap();
        assert_eq!(output.stdout.as_slice(), b"xxxxxxxx");
        assert_eq!(fs::read_dir("/proc/self/fd").unwrap().count(), initial);
    }
}

#[tokio::test]
// CodexBar: SpawnedProcessGroupTests.swift:178
async fn launch_resets_the_parent_signal_mask() {
    use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};

    require_nextest();
    let mut blocked = SigSet::empty();
    blocked.add(Signal::SIGTERM);
    let mut previous = SigSet::empty();
    pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut previous)).unwrap();
    let result = run(call(), command(&["signal-mask"])).await;
    pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous), None).unwrap();
    let output = result.unwrap();
    assert_eq!(output.stdout.as_slice(), b"false\n");
}

#[test]
fn absolute_program_rejects_relative_and_non_executable_paths() {
    assert!(AbsolutePath::new("relative/program").is_err());
    let file = tempfile::NamedTempFile::new().unwrap();
    assert!(AbsolutePath::new(file.path()).is_err());
}

#[test]
fn command_debug_hides_arguments_environment_and_stdin() {
    let mut cmd = command(&["argument-secret-value"]);
    cmd.set_env(ProcessEnv::empty().with("PRIVATE_ENV", "environment-secret-value"));
    cmd.set_stdin(StdinSpec::Secret(zeroize::Zeroizing::new(
        b"stdin-secret-value".to_vec(),
    )));
    let rendered = format!("{cmd:?}");
    for value in [
        "argument-secret-value",
        "environment-secret-value",
        "stdin-secret-value",
    ] {
        assert!(!rendered.contains(value));
    }
}

#[tokio::test]
async fn output_discards_large_stderr_while_capturing_stdout() {
    require_nextest();
    let mut cmd = command(&["print", "both", "8388608"]);
    cmd.set_stdout(StreamPolicy::Capture { cap: 32 });
    cmd.set_stderr(StreamPolicy::Discard);
    assert!(matches!(
        run(call(), cmd).await,
        Err(ProcessError::OutputTooLarge { stream: _, cap: 32 })
    ));
}

#[tokio::test]
async fn text_busy_retries_three_times_only_when_enabled() {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let result = ariadusage_engine::brokers::process::retry_spawn_for_test(true, move || {
        let attempt = observed.fetch_add(1, Ordering::SeqCst);
        if attempt < 2 {
            Err(io::Error::from_raw_os_error(26))
        } else {
            Ok("spawned")
        }
    })
    .await
    .unwrap();
    assert_eq!(result, "spawned");
    assert_eq!(attempts.load(Ordering::SeqCst), 3);

    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    assert!(
        ariadusage_engine::brokers::process::retry_spawn_for_test(false, move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(io::Error::from_raw_os_error(26))
        })
        .await
        .is_err()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}
