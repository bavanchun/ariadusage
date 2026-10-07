// Ported from CodexBar Tests/CodexBarTests/CodexCLILaunchGateTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::gates::launch::{LaunchFailureKind, LaunchGate};
use ariadusage_core::pipeline::FetchInteraction;

#[test]
// CodexBar: CodexCLILaunchGateTests.swift:8
fn background_launch_failures_suppress_until_the_cooldown_but_allow_user_launches() {
    let now = Arc::new(Mutex::new(Instant::now()));
    let clock = Arc::clone(&now);
    let gate = LaunchGate::new(move || *clock.lock().unwrap());

    let until = gate
        .record_failure("/synthetic/bin/codex", LaunchFailureKind::Process)
        .expect("process launch failures should be throttled");
    assert_eq!(
        until.duration_since(*now.lock().unwrap()),
        Duration::from_secs(30 * 60)
    );
    assert_eq!(
        gate.check("/synthetic/bin/codex", FetchInteraction::Background),
        Err(until)
    );
    assert_eq!(
        gate.check("/synthetic/bin/codex", FetchInteraction::UserInitiated),
        Ok(())
    );
    assert_eq!(
        gate.check("/synthetic/bin/codex", FetchInteraction::Background),
        Err(until)
    );

    *now.lock().unwrap() += Duration::from_secs(30 * 60 - 1);
    assert_eq!(
        gate.check("/synthetic/bin/codex", FetchInteraction::Background),
        Err(until)
    );
    *now.lock().unwrap() += Duration::from_secs(1);
    assert_eq!(
        gate.check("/synthetic/bin/codex", FetchInteraction::Background),
        Ok(())
    );
}

#[test]
// CodexBar: CodexCLILaunchGateTests.swift:35
fn pty_infrastructure_failures_do_not_suppress_background_launches() {
    let gate = LaunchGate::new(Instant::now);
    assert_eq!(
        gate.record_failure("/synthetic/bin/codex", LaunchFailureKind::PtyInfrastructure),
        None
    );
    assert_eq!(
        gate.check("/synthetic/bin/codex", FetchInteraction::Background),
        Ok(())
    );
}
