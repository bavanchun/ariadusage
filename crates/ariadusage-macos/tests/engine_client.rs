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
