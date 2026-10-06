//! Integration test for `ariadusage secret set` with non-interactive stdin pipe.

#![cfg(unix)]

use std::path::PathBuf;

use ariadusage_protocol::fixture::{FixtureConfig, start_fixture_server};
use ariadusage_protocol::ipc::ServerMessage;
use ariadusage_protocol::settings::{DescriptorKind, SettingsPage};
use assert_cmd::Command;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn unique_temp_socket_path(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ariadusage-cli-test-{}", std::process::id()));
    dir.join(format!("{test_name}.sock"))
}

#[tokio::test(flavor = "current_thread")]
async fn test_secret_set_via_stdin_pipe() {
    let socket_path = unique_temp_socket_path("secret_set");
    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        step_seconds: 10,
        misbehave: None,
        log_sink: None,
    })
    .await
    .expect("failed to start fixture server");

    let secret_val = "sk-ant-api03-invented-test-key-554433";
    let setting_id = "providers.claude.apiKey";

    // Run `ariadusage secret set --id <setting-id> --socket <socket-path>` with stdin piped.
    // The secret is never passed on argv (which protects /proc/PID/cmdline against observation).
    // Run in spawn_blocking to keep the current-thread runtime polling the fixture server.
    let socket_path_clone = socket_path.clone();
    let secret_val_owned = secret_val.to_string();
    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::cargo_bin("ariadusage").expect("ariadusage binary found");
        let assert = cmd
            .arg("secret")
            .arg("set")
            .arg("--id")
            .arg(setting_id)
            .arg("--socket")
            .arg(&socket_path_clone)
            .write_stdin(format!("{secret_val_owned}\n"))
            .assert();
        assert.success().get_output().clone()
    })
    .await
    .expect("command execution task");

    // 1. Asserts exit 0 and stdout contains "saved"
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("saved"),
        "stdout must contain 'saved', got: {stdout}"
    );

    // 2. Asserts secret value does not appear in stdout or stderr
    assert!(
        !stdout.contains(secret_val),
        "stdout leaked secret: {stdout}"
    );
    assert!(
        !stderr.contains(secret_val),
        "stderr leaked secret: {stderr}"
    );

    // 3. Connect to fixture server to verify getSettings reflects isSet: true
    let mut stream = UnixStream::connect(&socket_path)
        .await
        .expect("connect to fixture server");
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    // Handshake
    writer
        .write_all(
            b"{\"type\":\"hello\",\"client\":{\"name\":\"test-verify\",\"version\":\"1.0\"},\"protocols\":[\"ariadusage-ipc/1\"],\"id\":\"req-v1\"}\n",
        )
        .await
        .unwrap();
    let _welcome = lines.next_line().await.unwrap().unwrap();

    // Query settings
    writer
        .write_all(b"{\"type\":\"getSettings\",\"scope\":\"app\",\"id\":\"req-check\"}\n")
        .await
        .unwrap();
    let resp_line = lines.next_line().await.unwrap().unwrap();
    let resp: ServerMessage = serde_json::from_str(&resp_line).unwrap();

    match resp {
        ServerMessage::Response {
            id,
            ok,
            payload: Some(payload),
            ..
        } => {
            assert_eq!(id.as_deref(), Some("req-check"));
            assert_eq!(ok, Some(true));
            let page: SettingsPage = serde_json::from_value(payload).expect("valid SettingsPage");
            let mut found_secret = false;
            for section in page.sections {
                for descriptor in section.descriptors {
                    if descriptor.id.as_str() != setting_id {
                        continue;
                    }
                    if let DescriptorKind::Secret { is_set, .. } = descriptor.kind {
                        assert!(is_set, "isSet must be true after setSecret");
                        found_secret = true;
                    }
                }
            }
            assert!(
                found_secret,
                "setting descriptor for {setting_id} must exist"
            );
        }
        other => panic!("expected getSettings response, got {other:?}"),
    }

    server.stop().await;
}
