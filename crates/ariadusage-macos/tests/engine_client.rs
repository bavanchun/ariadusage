#![cfg(unix)]

use std::io::Cursor;
use std::sync::mpsc;
use std::time::Duration;

use ariadusage_macos::engine_client::{EngineClient, EngineEvent, read_frame};
use ariadusage_protocol::fixture::{FixtureConfig, start_fixture_server};

#[test]
fn read_frame_splits_lines_and_strips_carriage_returns() {
    let mut input = Cursor::new(b"one\r\ntwo\npartial".to_vec());
    assert_eq!(read_frame(&mut input, 64).unwrap(), Some(b"one".to_vec()));
    assert_eq!(read_frame(&mut input, 64).unwrap(), Some(b"two".to_vec()));
    // A trailing line without a newline is not a complete frame.
    assert_eq!(read_frame(&mut input, 64).unwrap(), None);
}

#[test]
fn read_frame_skips_oversized_frames_and_keeps_reading() {
    let mut data = vec![b'x'; 100];
    data.extend_from_slice(b"\nok\n");
    let mut input = std::io::BufReader::with_capacity(8, Cursor::new(data));
    assert_eq!(read_frame(&mut input, 16).unwrap(), Some(b"ok".to_vec()));
}

#[test]
fn read_frame_caps_content_excluding_line_terminator() {
    let exact_lf = b"0123456789abcdef\n";
    let exact_crlf = b"0123456789abcdef\r\n";
    let one_over = b"0123456789abcdefg\n";
    let mut data = Vec::new();
    data.extend_from_slice(exact_lf);
    data.extend_from_slice(exact_crlf);
    data.extend_from_slice(one_over);
    data.extend_from_slice(b"after\n");
    let mut input = std::io::BufReader::with_capacity(5, Cursor::new(data));
    assert_eq!(
        read_frame(&mut input, 16).unwrap(),
        Some(b"0123456789abcdef".to_vec())
    );
    assert_eq!(
        read_frame(&mut input, 16).unwrap(),
        Some(b"0123456789abcdef".to_vec())
    );
    assert_eq!(read_frame(&mut input, 16).unwrap(), Some(b"after".to_vec()));
}

#[tokio::test(flavor = "current_thread")]
async fn client_handshakes_and_receives_snapshots_from_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("engine.sock");
    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        ..FixtureConfig::default()
    })
    .await
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let _client = EngineClient::spawn(socket_path, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();

    let events = tokio::task::spawn_blocking(move || {
        let mut connected = false;
        let mut snapshot = None;
        while snapshot.is_none() {
            match rx
                .recv_timeout(Duration::from_secs(10))
                .expect("engine event")
            {
                EngineEvent::Connected { .. } => connected = true,
                EngineEvent::Snapshot(s) => snapshot = Some(s),
                EngineEvent::Disconnected => {}
            }
        }
        (connected, snapshot.unwrap())
    })
    .await
    .unwrap();

    assert!(events.0, "the welcome arrives before the first snapshot");
    assert_eq!(events.1.providers.len(), 3);
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn client_reconnects_and_resets_backoff_after_the_engine_restarts() {
    // Second step is 60 s so a reconnect that failed to reset `attempt` back
    // to 0 after receiving a snapshot would time out the 3 s receive window.
    static BACKOFF: [Duration; 2] = [Duration::from_millis(500), Duration::from_secs(60)];

    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("engine.sock");

    let (tx, rx) = mpsc::channel();
    let client = EngineClient::spawn_with_backoff(socket_path.clone(), &BACKOFF, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();

    // Let the first connect attempt fail before the server binds, advancing
    // `attempt` to 1.
    let rx = tokio::task::spawn_blocking(move || {
        match rx
            .recv_timeout(Duration::from_secs(3))
            .expect("initial disconnect")
        {
            EngineEvent::Disconnected => rx,
            other => panic!("expected Disconnected before server start, got {other:?}"),
        }
    })
    .await
    .unwrap();

    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        ..FixtureConfig::default()
    })
    .await
    .unwrap();

    let rx = tokio::task::spawn_blocking(move || {
        loop {
            if let EngineEvent::Snapshot(_) = rx
                .recv_timeout(Duration::from_secs(3))
                .expect("first snapshot")
            {
                break;
            }
        }
        rx
    })
    .await
    .unwrap();

    client.refresh(None);
    server.stop().await;

    let rx = tokio::task::spawn_blocking(move || {
        loop {
            if let EngineEvent::Disconnected = rx
                .recv_timeout(Duration::from_secs(3))
                .expect("disconnect event")
            {
                break;
            }
        }
        rx
    })
    .await
    .unwrap();

    let mut server2 = start_fixture_server(FixtureConfig {
        socket_path,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();

    let reconnected = tokio::task::spawn_blocking(move || {
        let mut connected = false;
        let mut snapshot = None;
        while snapshot.is_none() {
            match rx
                .recv_timeout(Duration::from_secs(3))
                .expect("reconnect event within reset backoff")
            {
                EngineEvent::Connected { .. } => connected = true,
                EngineEvent::Snapshot(s) => snapshot = Some(s),
                EngineEvent::Disconnected => {}
            }
        }
        (connected, snapshot.unwrap())
    })
    .await
    .unwrap();

    assert!(reconnected.0);
    assert_eq!(reconnected.1.providers.len(), 3);
    server2.stop().await;
}
