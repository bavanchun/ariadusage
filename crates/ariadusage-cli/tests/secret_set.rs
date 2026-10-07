#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;

use assert_cmd::Command;
use tempfile::TempDir;

fn isolated_command(home: &TempDir) -> Command {
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
fn non_tty_fails_closed_without_file_fallback_consent() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut command = isolated_command(&home);
    let config_home = home.path().join("config");
    let data_home = home.path().join("data");
    let output = command
        .args(["secret", "set", "--id", "providers.claude.apiKey"])
        .write_stdin("invented-value\n")
        .assert()
        .code(15)
        .get_output()
        .clone();

    assert_eq!(output.stdout, b"");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("invented-value"));
    assert!(!config_home.join("ariadusage/config.json").exists());
    assert!(!data_home.join("ariadusage/secrets.json").exists());
}

#[test]
fn explicit_fallback_consent_writes_private_file_and_records_config() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut command = isolated_command(&home);
    let config_home = home.path().join("config");
    let data_home = home.path().join("data");
    let output = command
        .args([
            "secret",
            "set",
            "--id",
            "providers.claude.apiKey",
            "--allow-file-fallback",
        ])
        .write_stdin("invented-value\n")
        .assert()
        .success()
        .get_output()
        .clone();

    assert_eq!(output.stdout, b"saved\n");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("invented-value"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("invented-value"));

    let config = fs::read_to_string(config_home.join("ariadusage/config.json"))
        .expect("consent config written");
    assert!(config.contains("\"secretFileFallback\": true"));

    let secret_path = data_home.join("ariadusage/secrets.json");
    let secret_file = fs::read_to_string(&secret_path).expect("secret file written");
    assert!(
        secret_file.contains("invented-value"),
        "secret was not persisted"
    );
    let mode = fs::metadata(&secret_path)
        .expect("secret file metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let parent_mode = fs::metadata(data_home.join("ariadusage"))
        .expect("secret directory metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(parent_mode, 0o700);

    let mut command = isolated_command(&home);
    let output = command
        .args(["secret", "set", "--id", "providers.claude.apiKey"])
        .write_stdin("second-invented-value\n")
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(output.stdout, b"saved\n");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("second-invented-value"));
    assert!(
        fs::read_to_string(secret_path)
            .expect("secret file remains readable")
            .contains("second-invented-value")
    );
}

#[test]
fn stdin_over_64_kib_is_rejected_before_writing() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut command = isolated_command(&home);
    let config_home = home.path().join("config");
    let data_home = home.path().join("data");
    let input = vec![b'x'; 64 * 1024 + 1];
    let output = command
        .args(["secret", "set", "--id", "providers.claude.apiKey"])
        .write_stdin(input)
        .assert()
        .code(4)
        .get_output()
        .clone();

    assert_eq!(output.stdout, b"");
    assert!(!config_home.join("ariadusage/config.json").exists());
    assert!(!data_home.join("ariadusage/secrets.json").exists());
}

#[test]
fn invalid_setting_id_is_rejected_without_printing_it() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut command = isolated_command(&home);
    let config_home = home.path().join("config");
    let data_home = home.path().join("data");
    let output = command
        .args(["secret", "set", "--id", "providers.unknown.apiKey"])
        .write_stdin("invented-value\n")
        .assert()
        .code(2)
        .get_output()
        .clone();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid setting id"));
    assert!(!stderr.contains("providers.unknown.apiKey"));
    assert!(!stderr.contains("invented-value"));
    assert!(!config_home.join("ariadusage/config.json").exists());
    assert!(!data_home.join("ariadusage/secrets.json").exists());
}

#[test]
fn socket_option_is_rejected_and_engine_socket_is_never_contacted() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut command = isolated_command(&home);
    let socket_dir = home.path().join("runtime/ariadusage");
    fs::create_dir_all(&socket_dir).expect("socket directory");
    let socket_path = socket_dir.join("engine.sock");
    let listener = UnixListener::bind(socket_path).expect("test listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");

    command
        .args([
            "secret",
            "set",
            "--id",
            "providers.claude.apiKey",
            "--socket",
            "/tmp/unused.sock",
        ])
        .write_stdin("invented-value\n")
        .assert()
        .code(2);

    let mut command = isolated_command(&home);
    command
        .args(["secret", "set", "--id", "providers.claude.apiKey"])
        .write_stdin("invented-value\n")
        .assert()
        .code(15);
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}
