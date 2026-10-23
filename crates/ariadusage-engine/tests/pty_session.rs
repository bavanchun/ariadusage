// Ported from CodexBar Tests/CodexBarTests/TTYCommandRunnerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/BoundedChildProcessProofTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/SpawnedProcessGroupTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/process.rs"]
mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::process::pty::fail_next_open_for_test;
use ariadusage_engine::brokers::process::{
    ProcessEnv, ProcessRegistry, PtyCompletionReason, PtyError, PtyScript, PtySession, PtySize,
    SendOnSubstring, SubstringSource,
};
use tempfile::tempdir;

async fn spawn(args: &[&str], env: ProcessEnv) -> PtySession {
    PtySession::spawn(
        support::call(FetchInteraction::UserInitiated),
        support::command(args).env(env),
        PtySize::default(),
        &ProcessRegistry::new(),
        &LaunchGate::default(),
    )
    .await
    .unwrap()
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:178
async fn pty_backfills_injected_home_and_adds_default_terminal_environment() {
    support::require_nextest();
    let temp = tempdir().unwrap();
    let mut command = support::command(&["pty-prompt", "env"]).env(ProcessEnv::from_allowlist([
        ("HOME", "/fakehome/pty"),
        ("PATH", "/usr/bin"),
    ]));
    command.set_cwd(Some(temp.path().to_path_buf()));
    let mut session = PtySession::spawn(
        support::call(FetchInteraction::UserInitiated),
        command,
        PtySize::new(50, 160),
        &ProcessRegistry::new(),
        &LaunchGate::default(),
    )
    .await
    .unwrap();
    let transcript = session
        .run_script(PtyScript::new(Vec::new(), Duration::from_secs(2)))
        .await
        .unwrap();
    assert!(transcript.text.contains("HOME=/fakehome/pty"));
    assert!(transcript.text.contains("TERM=xterm-256color"));
    assert!(transcript.text.contains("COLORTERM=truecolor"));
    assert!(transcript.text.contains("LANG=en_US.UTF-8"));
    assert!(transcript.text.contains("CI=0"));
    assert!(
        transcript
            .text
            .contains(&format!("PWD={}", temp.path().display()))
    );
    assert!(
        transcript
            .text
            .contains(&format!("CWD={}", temp.path().display()))
    );
    session.close().await;
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:198
async fn an_existing_term_and_ci_value_are_preserved() {
    support::require_nextest();
    let env = ProcessEnv::empty()
        .with("TERM", "synthetic-terminal")
        .with("CI", "synthetic-ci");
    let mut session = spawn(&["pty-prompt", "env"], env).await;
    let transcript = session
        .run_script(PtyScript::new(Vec::new(), Duration::from_secs(2)))
        .await
        .unwrap();
    assert!(transcript.text.contains("TERM=synthetic-terminal"));
    assert!(transcript.text.contains("CI=synthetic-ci"));
    session.close().await;
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:208
async fn environment_allowlist_excludes_unlisted_names() {
    support::require_nextest();
    let env = ProcessEnv::from_allowlist([
        ("HOME", "/fakehome/pty"),
        ("UNLISTED_PROCESS_TEST", "invented-value"),
    ]);
    let mut session = spawn(&["pty-prompt", "env"], env).await;
    let transcript = session
        .run_script(PtyScript::new(Vec::new(), Duration::from_secs(2)))
        .await
        .unwrap();
    assert!(transcript.text.contains("UNLISTED_PROCESS_TEST=<missing>"));
    session.close().await;
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:244
async fn working_directory_is_applied_to_the_child_and_pwd() {
    support::require_nextest();
    let temp = tempdir().unwrap();
    let mut command = support::command(&["pty-prompt", "env"]).env(ProcessEnv::empty());
    command.set_cwd(Some(temp.path().to_path_buf()));
    let mut session = PtySession::spawn(
        support::call(FetchInteraction::UserInitiated),
        command,
        PtySize::default(),
        &ProcessRegistry::new(),
        &LaunchGate::default(),
    )
    .await
    .unwrap();
    let transcript = session
        .run_script(PtyScript::new(Vec::new(), Duration::from_secs(2)))
        .await
        .unwrap();
    assert!(
        transcript
            .text
            .contains(&format!("PWD={}", temp.path().display()))
    );
    assert!(
        transcript
            .text
            .contains(&format!("CWD={}", temp.path().display()))
    );
    session.close().await;
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:267
async fn fast_exit_drains_buffered_terminal_output() {
    support::require_nextest();
    let mut session = spawn(&["pty-prompt", "clean-exit"], ProcessEnv::empty()).await;
    let transcript = session
        .run_script(PtyScript::new(Vec::new(), Duration::from_secs(2)))
        .await
        .unwrap();
    assert_eq!(transcript.reason, PtyCompletionReason::ProcessExited);
    assert!(transcript.text.contains("buffered-before-exit"));
    session.close().await;
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:288
async fn transcript_has_an_ansi_stripped_text_view_and_redacted_debug() {
    support::require_nextest();
    let mut session = spawn(&["pty-prompt", "prompt"], ProcessEnv::empty()).await;
    let mut script = PtyScript::new(
        b"synthetic-account-session-value".to_vec(),
        Duration::from_secs(2),
    );
    script.stop_needles.push(b"ack:".to_vec());
    let transcript = session.run_script(script).await.unwrap();
    assert!(transcript.text.contains("ready>"));
    assert!(
        transcript
            .text
            .contains("ack:synthetic-account-session-value")
    );
    assert!(!transcript.text.contains('\x1b'));
    let debug = format!("{transcript:?}");
    assert!(!debug.contains("synthetic-account-session-value"));
    session.close().await;
}

#[tokio::test]
// CodexBar: BoundedChildProcessProofTests.swift:14
async fn pty_output_overflow_closes_the_master_and_reaps_the_child() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let mut session = PtySession::spawn(
        support::call(FetchInteraction::UserInitiated),
        support::command(&["pty-flood"]).env(ProcessEnv::empty()),
        PtySize::default(),
        &registry,
        &LaunchGate::default(),
    )
    .await
    .unwrap();
    let error = session
        .run_script(PtyScript::new(Vec::new(), Duration::from_secs(5)))
        .await
        .unwrap_err();
    assert_eq!(error, PtyError::OutputTooLarge);
    assert_eq!(registry.active_launches(), 0);
    assert!(!format!("{error:?}").contains("xxxxxxxx"));
}

#[tokio::test]
async fn registry_shutdown_reaps_an_idle_pty_child_and_releases_its_permit() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let mut session = PtySession::spawn(
        support::call(FetchInteraction::UserInitiated),
        support::command(&["pty-prompt", "idle"]).env(ProcessEnv::empty()),
        PtySize::default(),
        &registry,
        &LaunchGate::default(),
    )
    .await
    .unwrap();
    assert_eq!(registry.active_launches(), 1);

    tokio::time::timeout(Duration::from_secs(3), registry.shutdown())
        .await
        .expect("registry shutdown should reap the PTY child");
    assert_eq!(registry.active_launches(), 0);
    session.close().await;
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:700
async fn idle_stop_requires_output_and_url_stop_is_detected_across_reads() {
    support::require_nextest();
    let mut idle_session = spawn(&["pty-prompt", "idle"], ProcessEnv::empty()).await;
    let mut idle = PtyScript::new(Vec::new(), Duration::from_secs(2));
    idle.idle_timeout = Some(Duration::from_millis(100));
    let started = Instant::now();
    let transcript = idle_session.run_script(idle).await.unwrap();
    assert_eq!(transcript.reason, PtyCompletionReason::IdleTimeout);
    assert!(started.elapsed() >= Duration::from_millis(80));
    idle_session.close().await;

    let mut url_session = spawn(&["pty-prompt", "split-url"], ProcessEnv::empty()).await;
    let mut url = PtyScript::new(Vec::new(), Duration::from_secs(2));
    url.stop_on_url = true;
    url.settle = Duration::from_millis(20);
    let transcript = url_session.run_script(url).await.unwrap();
    assert_eq!(transcript.reason, PtyCompletionReason::OutputCondition);
    assert!(transcript.text.contains("https://"));
    url_session.close().await;
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:735
async fn recent_text_window_triggers_after_cr_stripping() {
    support::require_nextest();
    let mut session = spawn(&["pty-prompt", "text-window"], ProcessEnv::empty()).await;
    let mut script = PtyScript::new(Vec::new(), Duration::from_secs(2));
    script.send_on_substrings.push(SendOnSubstring::new(
        b"Question?".to_vec(),
        b"answer\r".to_vec(),
        SubstringSource::RecentText,
    ));
    let transcript = session.run_script(script).await.unwrap();
    assert_eq!(transcript.reason, PtyCompletionReason::ProcessExited);
    assert!(transcript.text.contains("accepted"));
    session.close().await;
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:494
async fn raw_substring_reply_fires_once_and_split_stop_needles_are_found() {
    support::require_nextest();
    let mut prompt = spawn(&["pty-prompt", "prompt"], ProcessEnv::empty()).await;
    let mut script = PtyScript::new(Vec::new(), Duration::from_secs(2));
    script.send_on_substrings.push(SendOnSubstring::new(
        b"ready>".to_vec(),
        b"continue\r".to_vec(),
        SubstringSource::RawBytes,
    ));
    let transcript = prompt.run_script(script).await.unwrap();
    assert!(transcript.text.contains("ack:continue"));
    prompt.close().await;

    let mut split = spawn(&["pty-prompt", "split-stop"], ProcessEnv::empty()).await;
    let mut script = PtyScript::new(Vec::new(), Duration::from_secs(2));
    script.stop_needles.push(b"NEEDLE".to_vec());
    script.settle = Duration::from_millis(20);
    let transcript = split.run_script(script).await.unwrap();
    assert_eq!(transcript.reason, PtyCompletionReason::OutputCondition);
    split.close().await;
}

#[tokio::test]
async fn send_enter_repeats_until_a_split_url_is_seen() {
    support::require_nextest();
    let mut session = spawn(&["pty-prompt", "enter-url"], ProcessEnv::empty()).await;
    let mut script = PtyScript::new(Vec::new(), Duration::from_secs(2));
    script.send_enter_every = Some(Duration::from_millis(50));
    script.stop_on_url = true;
    script.settle = Duration::from_millis(20);
    let transcript = session.run_script(script).await.unwrap();
    assert_eq!(transcript.reason, PtyCompletionReason::OutputCondition);
    assert!(transcript.text.contains("http://example.invalid"));
    session.close().await;
}

#[tokio::test]
async fn close_sends_the_exit_command_before_group_teardown() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let mut session = PtySession::spawn(
        support::call(FetchInteraction::UserInitiated),
        support::command(&["pty-prompt", "wait-exit"]).env(ProcessEnv::empty()),
        PtySize::default(),
        &registry,
        &LaunchGate::default(),
    )
    .await
    .unwrap();
    session.close().await;
    assert_eq!(registry.active_launches(), 0);
}

#[tokio::test(flavor = "current_thread")]
// CodexBar: SpawnedProcessGroupTests.swift:214
async fn pty_child_inherits_the_observed_blocked_term_mask() {
    support::require_nextest();
    use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};

    struct RestoreMask(SigSet);
    impl Drop for RestoreMask {
        fn drop(&mut self) {
            let _ = pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&self.0), None);
        }
    }

    let mut blocked = SigSet::empty();
    blocked.add(Signal::SIGTERM);
    let mut original = SigSet::empty();
    pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut original)).unwrap();
    let _restore = RestoreMask(original);
    let mut session = spawn(&["pty-prompt", "signal-mask"], ProcessEnv::empty()).await;
    let transcript = session
        .run_script(PtyScript::new(Vec::new(), Duration::from_secs(2)))
        .await
        .unwrap();
    assert!(transcript.text.contains("true"));
    session.close().await;
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:304
async fn pty_open_failure_includes_the_captured_posix_error() {
    support::require_nextest();
    use nix::errno::Errno;

    for errno in [Errno::EAGAIN, Errno::ENOENT, Errno::EMFILE] {
        let code = errno as i32;
        let description = std::io::Error::from_raw_os_error(code).to_string();
        let description = description
            .strip_suffix(&format!(" (os error {code})"))
            .unwrap_or(&description)
            .to_owned();
        let registry = ProcessRegistry::new();
        fail_next_open_for_test(code);
        let error = PtySession::spawn(
            support::call(FetchInteraction::UserInitiated),
            support::command(&["pty-prompt", "clean-exit"]).env(ProcessEnv::empty()),
            PtySize::default(),
            &registry,
            &LaunchGate::default(),
        )
        .await
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains(&format!("errno {code}")), "{message}");
        assert!(message.contains(&description), "{message}");
        assert!(matches!(error, PtyError::OpenFailed { errno, .. } if errno == code));
        assert_eq!(registry.active_launches(), 0);
    }
}

fn terminal_descriptor_count() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter(|target| target == Path::new("/dev/ptmx") || target.starts_with("/dev/pts/"))
        .count()
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:321
async fn repeated_pty_sessions_release_descriptors_on_exit_failure_cancellation_and_timeout() {
    support::require_nextest();
    let baseline = terminal_descriptor_count();
    let missing_directory = tempdir().unwrap().path().join("missing-pty-directory");

    for _ in 0..10 {
        let mut exited = spawn(&["pty-prompt", "clean-exit"], ProcessEnv::empty()).await;
        let transcript = exited
            .run_script(PtyScript::new(Vec::new(), Duration::from_secs(2)))
            .await
            .unwrap();
        assert_eq!(transcript.reason, PtyCompletionReason::ProcessExited);
        exited.close().await;
        drop(exited);
        assert!(terminal_descriptor_count() <= baseline);

        let mut command = support::command(&["pty-prompt", "clean-exit"]).env(ProcessEnv::empty());
        command.set_cwd(Some(missing_directory.clone()));
        let failed = PtySession::spawn(
            support::call(FetchInteraction::UserInitiated),
            command,
            PtySize::default(),
            &ProcessRegistry::new(),
            &LaunchGate::default(),
        )
        .await;
        assert!(failed.is_err());
        drop(failed);
        assert!(terminal_descriptor_count() <= baseline);

        let call = support::call(FetchInteraction::UserInitiated);
        call.cancel.cancel();
        let mut cancelled = PtySession::spawn(
            call,
            support::command(&["pty-prompt", "idle"]).env(ProcessEnv::empty()),
            PtySize::default(),
            &ProcessRegistry::new(),
            &LaunchGate::default(),
        )
        .await
        .unwrap();
        let error = cancelled
            .run_script(PtyScript::new(Vec::new(), Duration::from_secs(2)))
            .await
            .unwrap_err();
        assert_eq!(error, PtyError::Cancelled);
        drop(cancelled);
        assert!(terminal_descriptor_count() <= baseline);

        let mut timed_out = spawn(&["pty-prompt", "idle"], ProcessEnv::empty()).await;
        let transcript = timed_out
            .run_script(PtyScript::new(Vec::new(), Duration::ZERO))
            .await
            .unwrap();
        assert_eq!(transcript.reason, PtyCompletionReason::DeadlineExceeded);
        timed_out.close().await;
        drop(timed_out);
        let cleanup_deadline = Instant::now() + Duration::from_secs(30);
        while terminal_descriptor_count() > baseline && Instant::now() < cleanup_deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(terminal_descriptor_count() <= baseline);
    }
}

#[tokio::test]
// CodexBar: TTYCommandRunnerTests.swift:267
async fn output_written_before_the_child_exits_survives_an_exit_seen_before_the_first_read() {
    support::require_nextest();
    for _ in 0..10 {
        let mut session = spawn(&["pty-prompt", "clean-exit"], ProcessEnv::empty()).await;
        // Let the child write, exit and be reaped before the session reads anything.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let transcript = session
            .run_script(PtyScript::new(Vec::new(), Duration::from_secs(2)))
            .await
            .unwrap();
        assert_eq!(transcript.reason, PtyCompletionReason::ProcessExited);
        assert_eq!(transcript.text.trim(), "buffered-before-exit");
        session.close().await;
    }
}
