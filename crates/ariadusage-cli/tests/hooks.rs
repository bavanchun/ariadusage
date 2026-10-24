use std::fs;

use assert_cmd::Command;
use tempfile::TempDir;

fn isolated_hooks_command(home: &TempDir) -> Command {
    let config_home = home.path().join("config");
    let data_home = home.path().join("data");
    let state_home = home.path().join("state");
    let runtime_home = home.path().join("runtime");

    let mut command = Command::cargo_bin("ariadusage").expect("ariadusage binary found");
    command
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_STATE_HOME", &state_home)
        .env("XDG_RUNTIME_DIR", &runtime_home)
        .env("ARIADUSAGE_DISABLE_KEYRING", "1")
        .env_remove("DBUS_SESSION_BUS_ADDRESS");
    command
}

#[test]
fn hooks_default_list_enable_and_disable_cli_round_trip() {
    let home = tempfile::tempdir().expect("tempdir");

    // `ariadusage hooks` (default subcommand = list)
    let out = isolated_hooks_command(&home)
        .args(["hooks"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8(out.stdout).expect("utf8"),
        "Hooks: disabled\nNo rules configured.\n"
    );

    // `ariadusage hooks enable`
    let out = isolated_hooks_command(&home)
        .args(["hooks", "enable"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8(out.stdout).expect("utf8"),
        "Hooks: enabled\n"
    );

    // `ariadusage hooks list --json`
    let out = isolated_hooks_command(&home)
        .args(["hooks", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(json["enabled"], serde_json::Value::Bool(true));
    assert_eq!(json["events"], serde_json::json!([]));

    // `ariadusage hooks disable --json`
    let out = isolated_hooks_command(&home)
        .args(["hooks", "disable", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(json["enabled"], serde_json::Value::Bool(false));
}

#[test]
fn hooks_test_validates_event_provider_and_disabled_state() {
    let home = tempfile::tempdir().expect("tempdir");

    // Missing event
    let out = isolated_hooks_command(&home)
        .args(["hooks", "test"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("Unknown or missing event. Use one of: quota_low, quota_reached")
    );

    // Missing provider
    let out = isolated_hooks_command(&home)
        .args(["hooks", "test", "quota_reached"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "Unknown or missing provider. Use --provider <name>.\n"
    );

    // Disabled hooks in --json mode
    let out = isolated_hooks_command(&home)
        .args([
            "hooks",
            "test",
            "quota_reached",
            "--provider",
            "codex",
            "--json",
        ])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON error");
    assert_eq!(payload[0]["provider"], "cli");
    assert_eq!(payload[0]["error"]["code"], 1);
    assert_eq!(payload[0]["error"]["kind"], "config");
    assert_eq!(payload[0]["error"]["message"], "Hooks are disabled.");
}

#[test]
fn hooks_watch_validates_arguments_before_reading_broken_config() {
    let home = tempfile::tempdir().expect("tempdir");
    let config_dir = home.path().join("config").join("ariadusage");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(config_dir.join("config.json"), b"{broken json").expect("write broken config");

    // `--once` rejected before reading config.json
    let out = isolated_hooks_command(&home)
        .args(["hooks", "watch", "--once"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "Unknown flag --once: hooks watch requires a continuous loop so transition state survives between polls.\n"
    );

    // `--interval 30` rejected before reading config.json
    let out = isolated_hooks_command(&home)
        .args(["hooks", "watch", "--interval", "30"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "--interval must be at least 60 seconds.\n"
    );

    // `--provider not-a-provider` rejected before reading config.json
    let out = isolated_hooks_command(&home)
        .args(["hooks", "watch", "--provider", "not-a-provider"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "Unknown provider: not-a-provider\n"
    );
}
