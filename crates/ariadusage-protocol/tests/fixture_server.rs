//! Integration tests for the in-process Unix domain socket fixture server.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use ariadusage_protocol::fixture::{FixtureConfig, start_fixture_server};
use ariadusage_protocol::ipc::{ErrorCode, ServerMessage};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn unique_temp_socket_path(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ariadusage-test-{}", std::process::id()));
    dir.join(format!("{test_name}.sock"))
}

#[tokio::test(flavor = "current_thread")]
async fn test_socket_permissions_and_handshake() {
    let socket_path = unique_temp_socket_path("handshake");
    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        step_seconds: 10,
        misbehave: None,
        log_sink: None,
    })
    .await
    .expect("failed to start fixture server");

    // Verify directory permissions are 0700
    let parent = socket_path.parent().expect("parent dir");
    let parent_mode = std::fs::metadata(parent).unwrap().permissions().mode() & 0o777;
    assert_eq!(parent_mode, 0o700, "directory permissions must be 0700");

    // Verify socket file permissions are 0600
    let socket_mode = std::fs::metadata(&socket_path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(socket_mode, 0o600, "socket permissions must be 0600");

    // Connect and perform handshake
    let mut stream = UnixStream::connect(&socket_path)
        .await
        .expect("failed to connect");
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    writer
        .write_all(
            b"{\"type\":\"hello\",\"client\":{\"name\":\"test\",\"version\":\"1.0\"},\"protocols\":[\"ariadusage-ipc/1\"],\"id\":\"req-hello\"}\n",
        )
        .await
        .unwrap();

    let resp_line = lines.next_line().await.unwrap().expect("expected response");
    let resp: ServerMessage = serde_json::from_str(&resp_line).expect("valid server message");

    match resp {
        ServerMessage::Welcome {
            protocol,
            engine_version,
            capabilities,
            id,
        } => {
            assert_eq!(protocol, "ariadusage-ipc/1");
            assert_eq!(engine_version, "0.1.0");
            assert!(capabilities.contains(&"snapshot".into()));
            assert_eq!(id.as_deref(), Some("req-hello"));
        }
        other => panic!("expected welcome message, got {other:?}"),
    }

    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_subscribe_immediate_push() {
    let socket_path = unique_temp_socket_path("subscribe");
    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        step_seconds: 10,
        misbehave: None,
        log_sink: None,
    })
    .await
    .unwrap();

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    // Send subscribe
    writer
        .write_all(b"{\"type\":\"subscribe\",\"topics\":[\"snapshot\"],\"id\":\"req-sub\"}\n")
        .await
        .unwrap();

    // First response is ok
    let ok_line = lines.next_line().await.unwrap().expect("ok response");
    let ok_msg: ServerMessage = serde_json::from_str(&ok_line).unwrap();
    match ok_msg {
        ServerMessage::Response { id, ok, .. } => {
            assert_eq!(id.as_deref(), Some("req-sub"));
            assert_eq!(ok, Some(true));
        }
        other => panic!("expected ok response, got {other:?}"),
    }

    // Followed immediately by snapshot push (within 2 seconds)
    let push_future = lines.next_line();
    let push_line = tokio::time::timeout(Duration::from_secs(2), push_future)
        .await
        .expect("timeout waiting for snapshot push")
        .unwrap()
        .expect("snapshot push line");

    let push_msg: ServerMessage = serde_json::from_str(&push_line).unwrap();
    match push_msg {
        ServerMessage::Snapshot { snapshot } => {
            assert_eq!(snapshot.providers.len(), 3);
            assert_eq!(snapshot.engine.version, "0.1.0");
        }
        other => panic!("expected snapshot push, got {other:?}"),
    }

    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_set_setting_triggers_settings_changed() {
    let socket_path = unique_temp_socket_path("settings");
    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        step_seconds: 10,
        misbehave: None,
        log_sink: None,
    })
    .await
    .unwrap();

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    // Subscribe to settings
    writer
        .write_all(b"{\"type\":\"subscribe\",\"topics\":[\"settings\"],\"id\":\"sub-set\"}\n")
        .await
        .unwrap();
    let _ = lines.next_line().await.unwrap().unwrap();

    // Send setSetting
    writer
        .write_all(b"{\"type\":\"setSetting\",\"id\":\"sourceMode\",\"value\":\"web\"}\n")
        .await
        .unwrap();

    let ok_line = lines.next_line().await.unwrap().unwrap();
    let ok_msg: ServerMessage = serde_json::from_str(&ok_line).unwrap();
    assert!(matches!(
        ok_msg,
        ServerMessage::Response { ok: Some(true), .. }
    ));

    // Read settingsChanged push
    let change_line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .expect("timeout on settingsChanged")
        .unwrap()
        .unwrap();
    let change_msg: ServerMessage = serde_json::from_str(&change_line).unwrap();
    assert!(matches!(change_msg, ServerMessage::SettingsChanged { .. }));

    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_protocol_rejection_unsupported_message() {
    let socket_path = unique_temp_socket_path("unsupported");
    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        step_seconds: 10,
        misbehave: None,
        log_sink: None,
    })
    .await
    .unwrap();

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    writer
        .write_all(b"{\"type\":\"hackThePlanet\",\"id\":\"req-hack\"}\n")
        .await
        .unwrap();

    let err_line = lines.next_line().await.unwrap().unwrap();
    let err_msg: ServerMessage = serde_json::from_str(&err_line).unwrap();

    match err_msg {
        ServerMessage::Response {
            id,
            error: Some(error),
            ok,
            ..
        } => {
            assert_eq!(id.as_deref(), Some("req-hack"));
            assert_eq!(ok, Some(false));
            assert_eq!(error.code, ErrorCode::UnsupportedMessage);
            assert_eq!(error.message, "unsupported message type");
        }
        other => panic!("expected error response, got {other:?}"),
    }

    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_framing_oversize_payload_too_large_and_recovery() {
    let socket_path = unique_temp_socket_path("oversize");
    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        step_seconds: 10,
        misbehave: None,
        log_sink: None,
    })
    .await
    .unwrap();

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    // Send an inbound frame exceeding 1 MiB (1024 * 1024 + 10 bytes)
    let oversize_padding = "x".repeat(1024 * 1024 + 10);
    let oversize_line = format!("{{\"type\":\"{}\"}}\n", oversize_padding);
    writer.write_all(oversize_line.as_bytes()).await.unwrap();

    let err_line = lines.next_line().await.unwrap().unwrap();
    let err_msg: ServerMessage = serde_json::from_str(&err_line).unwrap();

    match err_msg {
        ServerMessage::Response {
            error: Some(error),
            ok,
            ..
        } => {
            assert_eq!(ok, Some(false));
            assert_eq!(error.code, ErrorCode::PayloadTooLarge);
            assert_eq!(error.message, ErrorCode::PayloadTooLarge.static_message());
        }
        other => panic!("expected payloadTooLarge error response, got {other:?}"),
    }

    // Assert connection remains usable for subsequent valid message
    writer
        .write_all(
            b"{\"type\":\"hello\",\"client\":{\"name\":\"reconnect-test\",\"version\":\"1.0\"},\"protocols\":[\"ariadusage-ipc/1\"],\"id\":\"req-recov\"}\n",
        )
        .await
        .unwrap();

    let ok_line = lines.next_line().await.unwrap().unwrap();
    let ok_msg: ServerMessage = serde_json::from_str(&ok_line).unwrap();
    match ok_msg {
        ServerMessage::Welcome { id, .. } => {
            assert_eq!(id.as_deref(), Some("req-recov"));
        }
        other => panic!("expected welcome response after recovery, got {other:?}"),
    }

    server.stop().await;
}
