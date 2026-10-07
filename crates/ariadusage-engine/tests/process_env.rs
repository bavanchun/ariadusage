// Ported from CodexBar Tests/CodexBarTests/ProcessEnvironmentTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
use std::ffi::OsString;

#[cfg(target_os = "linux")]
use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_engine::brokers::process::ProcessEnv;

const SENTINEL: &str = "synthetic-environment-secret-value";
const SENTINEL_KEY: &str = "ARIADUSAGE_TEST_SECRET";

#[test]
// CodexBar: ProcessEnvironmentTests.swift:16
fn allowlist_keeps_only_exact_names_and_lc_xdg_families() {
    let unlisted_name = format!("UNLISTED_{}", "PROCESS_TEST");
    let unlisted_value = ["invented", "value"].join("-");
    let env = ProcessEnv::from_allowlist([
        ("HOME", "/synthetic/home"),
        ("PATH", "/synthetic/bin"),
        ("LC_ALL", "C.UTF-8"),
        ("XDG_DATA_HOME", "/synthetic/data"),
        ("SSH_AUTH_SOCK", "/synthetic/ssh.sock"),
        ("NODE_OPTIONS", "--require=untrusted.js"),
        ("LD_PRELOAD", "untrusted.so"),
        (unlisted_name.as_str(), unlisted_value.as_str()),
    ]);

    assert_eq!(env.get("HOME"), Some(&OsString::from("/synthetic/home")));
    assert_eq!(env.get("LC_ALL"), Some(&OsString::from("C.UTF-8")));
    assert_eq!(
        env.get("XDG_DATA_HOME"),
        Some(&OsString::from("/synthetic/data"))
    );
    for denied in ["SSH_AUTH_SOCK", "NODE_OPTIONS", "LD_PRELOAD"] {
        assert!(env.get(denied).is_none(), "unexpected key {denied}");
    }
    assert!(env.get(&unlisted_name).is_none());
}

#[test]
// CodexBar: ProcessEnvironmentTests.swift:29
fn explicit_environment_contains_only_the_requested_names() {
    let env = ProcessEnv::from_explicit(
        ["PATH", "HOME", "SSH_AUTH_SOCK"],
        [
            ("PATH", "/synthetic/bin"),
            ("HOME", "/synthetic/home"),
            ("SSH_AUTH_SOCK", "/synthetic/ssh.sock"),
            ("LANG", "C"),
        ],
    );

    assert_eq!(env.len(), 3);
    assert!(env.get("PATH").is_some());
    assert!(env.get("HOME").is_some());
    assert!(env.get("SSH_AUTH_SOCK").is_some());
    assert!(env.get("LANG").is_none());
}

#[test]
// CodexBar: ProcessEnvironmentTests.swift:45
fn provider_values_override_allowlisted_values_and_denylist_applies_last() {
    let env = ProcessEnv::from_allowlist([
        ("HOME", "/ambient/home"),
        ("CLAUDE_CONFIG_DIR", "/ambient/claude"),
    ])
    .with("HOME", "/synthetic/staging")
    .with("CLAUDE_CONFIG_DIR", "/synthetic/claude")
    .without(["CLAUDE_CONFIG_DIR"]);

    assert_eq!(env.get("HOME"), Some(&OsString::from("/synthetic/staging")));
    assert!(env.get("CLAUDE_CONFIG_DIR").is_none());
}

#[test]
// CodexBar: ProcessEnvironmentTests.swift:107
fn environment_debug_reports_only_the_entry_count() {
    let env = ProcessEnv::from_allowlist([("HOME", "/synthetic/home")])
        .with(SENTINEL_KEY, SENTINEL)
        .with("CUSTOM_VALUE", SENTINEL);
    let rendered = format!("{env:?}");

    assert!(rendered.contains("3"));
    assert!(!rendered.contains(SENTINEL));
    assert!(!rendered.contains(SENTINEL_KEY));
    assert!(!rendered.contains("/synthetic/home"));
}

#[test]
// CodexBar: ProcessEnvironmentTests.swift:129
fn environment_debug_stays_redacted_after_provider_overrides() {
    let env = ProcessEnv::from_allowlist([("HOME", "/ambient/home")])
        .with("HOME", SENTINEL)
        .with(SENTINEL_KEY, SENTINEL);
    let rendered = format!("{env:?}");

    assert!(!rendered.contains(SENTINEL));
    assert!(!rendered.contains(SENTINEL_KEY));
    assert!(!rendered.contains("/ambient/home"));
}

#[cfg(target_os = "linux")]
fn require_nextest() {
    if std::env::var_os("NEXTEST_RUN_ID").is_none() {
        panic!("destructive process tests require NEXTEST_RUN_ID");
    }
}

#[cfg(target_os = "linux")]
fn child_call() -> ariadusage_engine::brokers::call::BrokerCall {
    use ariadusage_core::pipeline::FetchInteraction;
    use tokio_util::sync::CancellationToken;

    ariadusage_engine::brokers::call::BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "synthetic-process-env-test".into(),
    }
}

#[cfg(target_os = "linux")]
fn child_command(args: &[&str], env: ProcessEnv) -> ariadusage_engine::brokers::process::Command {
    use std::time::Duration;

    use ariadusage_engine::brokers::process::{AbsolutePath, Command};

    Command::new(
        AbsolutePath::new(env!("CARGO_BIN_EXE_ariadusage-test-child")).unwrap(),
        args.iter().map(OsString::from).collect(),
    )
    .env(env)
    .timeout(Duration::from_secs(5))
}

#[cfg(target_os = "linux")]
async fn run_process(
    call: ariadusage_engine::brokers::call::BrokerCall,
    command: ariadusage_engine::brokers::process::Command,
) -> Result<
    ariadusage_engine::brokers::process::Output,
    ariadusage_engine::brokers::process::ProcessError,
> {
    ariadusage_engine::brokers::process::run(
        call,
        command,
        &ariadusage_engine::brokers::process::ProcessRegistry::new(),
        &LaunchGate::default(),
    )
    .await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn allowlist_denylist_and_home_override_are_enforced_at_spawn() {
    require_nextest();
    let unlisted_name = format!("ARIADUSAGE_{}", ["RUNTIME", "ASSEMBLED"].join("_"));
    let env = ProcessEnv::from_allowlist([
        ("HOME", "/ambient/home"),
        ("PATH", "/synthetic/bin"),
        ("NODE_OPTIONS", "synthetic-node-options"),
        ("LD_PRELOAD", "synthetic-library"),
        ("CLAUDE_CONFIG_DIR", "/source/claude-config"),
        (unlisted_name.as_str(), "synthetic-unlisted-value"),
    ])
    .with("HOME", "/synthetic/staging-home")
    .with("CLAUDE_CONFIG_DIR", "/synthetic/claude-config")
    .without(["CLAUDE_CONFIG_DIR"]);

    let names = run_process(
        child_call(),
        child_command(&["list-env-names"], env.clone()),
    )
    .await
    .unwrap();
    let names = String::from_utf8(names.stdout.to_vec()).unwrap();
    assert_eq!(names.lines().collect::<Vec<_>>(), ["HOME", "PATH"]);

    let home = run_process(child_call(), child_command(&["env-value", "HOME"], env))
        .await
        .unwrap();
    assert_eq!(home.stdout.as_slice(), b"/synthetic/staging-home");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn explicit_environment_passes_only_its_requested_names_at_spawn() {
    require_nextest();
    let env = ProcessEnv::from_explicit(
        ["PATH", "HOME", "SSH_AUTH_SOCK"],
        [
            ("PATH", "/synthetic/bin"),
            ("HOME", "/synthetic/home"),
            ("SSH_AUTH_SOCK", "/synthetic/ssh.sock"),
            ("LANG", "C"),
            ("XDG_DATA_HOME", "/synthetic/data"),
        ],
    );
    let output = run_process(child_call(), child_command(&["list-env-names"], env))
        .await
        .unwrap();
    let names = String::from_utf8(output.stdout.to_vec()).unwrap();
    assert_eq!(
        names.lines().collect::<Vec<_>>(),
        ["HOME", "PATH", "SSH_AUTH_SOCK"]
    );
}
