#![cfg(target_os = "linux")]

use std::process::Command;

use ariadusage_engine::hardening::harden_process;

const CHILD_ENV: &str = "ARIADUSAGE_HARDENING_CHILD";

#[test]
fn hardening_is_applied_in_a_child_process() {
    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("hardening_child_checks_dumpability_and_core_limit")
        .arg("--nocapture")
        .env(CHILD_ENV, "1")
        .output()
        .expect("spawn hardening child");
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("hardening verified"));
}

#[test]
fn hardening_child_checks_dumpability_and_core_limit() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }

    harden_process().expect("harden child process");
    let status = std::fs::read_to_string("/proc/self/status").expect("read proc status");
    if let Some(dumpable) = status
        .lines()
        .find_map(|line| line.strip_prefix("Dumpable:").map(str::trim))
    {
        assert_eq!(dumpable, "0");
    }
    assert_eq!(
        rustix::process::dumpable_behavior().expect("read dumpability"),
        rustix::process::DumpableBehavior::NotDumpable
    );
    let limits = rustix::process::getrlimit(rustix::process::Resource::Core);
    assert_eq!(limits.current, Some(0));
    assert_eq!(limits.maximum, Some(0));
    println!("hardening verified");
}
