//! Redaction integration tests verifying that secrets are never leaked in Debug, logs, or error responses.

use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::ipc::ClientMessage;
use ariadusage_protocol::secret::SecretString;

#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use ariadusage_protocol::fixture::{FixtureConfig, start_fixture_server};
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(unix)]
use tokio::sync::Mutex;

#[cfg(unix)]
fn unique_temp_socket_path(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ariadusage-redact-{}", std::process::id()));
    dir.join(format!("{test_name}.sock"))
}

#[test]
fn test_debug_format_does_not_leak_secret() {
    let secret_val = format!("{}-{}-{}-{}", "mock", "planted", "secret", "998877");
    let msg = ClientMessage::SetSecret {
        id: SettingId::new("providers.claude.apiKey").unwrap(),
        value: SecretString::new(&secret_val),
    };
    let debug_str = format!("{msg:?}");
    assert!(
        !debug_str.contains(&secret_val),
        "ClientMessage::SetSecret debug format must not leak raw secret"
    );
    assert!(
        debug_str.contains("[redacted]"),
        "ClientMessage::SetSecret debug format must show [redacted]"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn test_secret_not_in_logs_or_error_responses() {
    let secret_val = format!("{}-{}-{}-{}", "mock", "planted", "secret", "998877");
    let socket_path = unique_temp_socket_path("redaction");
    let log_sink = Arc::new(Mutex::new(Vec::new()));

    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        step_seconds: 10,
        misbehave: None,
        log_sink: Some(Arc::clone(&log_sink)),
    })
    .await
    .unwrap();

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    // 1. Plant secret via setSecret
    let plant_cmd = format!(
        "{{\"type\":\"setSecret\",\"id\":\"providers.claude.apiKey\",\"value\":\"{secret_val}\"}}\n"
    );
    writer.write_all(plant_cmd.as_bytes()).await.unwrap();
    let ok_line = lines.next_line().await.unwrap().unwrap();
    assert!(
        ok_line.contains("\"type\":\"response\"") && ok_line.contains("\"ok\":true"),
        "expected successful response, got: {ok_line}"
    );
    assert!(
        !ok_line.contains(&secret_val),
        "ok response must not echo secret"
    );

    // 2. Check fixture server log sink
    let logs = log_sink.lock().await;
    assert!(!logs.is_empty(), "expected server logs to be recorded");
    for entry in logs.iter() {
        assert!(
            !entry.contains(&secret_val),
            "fixture server log entry leaked secret: {entry}"
        );
    }
    drop(logs);

    // 3. Protocol error response if setSecret is malformed (e.g. invalid json syntax)
    let malformed = format!(
        "{{\"type\":\"setSecret\",\"id\":\"providers.claude.apiKey\",\"value\":\"{secret_val}\",BROKEN\n"
    );
    writer.write_all(malformed.as_bytes()).await.unwrap();
    let err_line = lines.next_line().await.unwrap().unwrap();
    assert!(
        !err_line.contains(&secret_val),
        "malformed JSON error response must not echo secret: {err_line}"
    );

    // 4. Error response if setSetting attempts to write with malformed field / bad payload containing secret
    let malformed_setting = format!(
        "{{\"type\":\"setSetting\",\"id\":\"providers.claude.apiKey\",\"value\":{{\"secret\":\"{secret_val}\"}}\n"
    );
    writer
        .write_all(malformed_setting.as_bytes())
        .await
        .unwrap();
    let err_setting_line = lines.next_line().await.unwrap().unwrap();
    assert!(
        !err_setting_line.contains(&secret_val),
        "setSetting error response must not echo secret: {err_setting_line}"
    );

    server.stop().await;
}
