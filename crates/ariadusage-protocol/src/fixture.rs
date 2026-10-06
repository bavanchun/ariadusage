//! In-process Unix domain socket fixture server for IPC testing and UI development.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

use crate::codec::{IpcCodec, IpcCodecError};
use crate::ids::{ActionId, ProviderId, SettingId};
use crate::ipc::{
    ClientMessage, ErrorCode, IpcError, PROTOCOL_V1, ServerMessage, Topic, parse_client_message,
};
use crate::metric::{Confidence, Metric, MetricSource, MetricState, SourceKind};
use crate::settings::{
    ActionConfirmation, ActionItem, ActionStyle, ChoiceOption, DescriptorKind, MultiChoiceEntry,
    NumberConfig, SettingDescriptor, SettingsPage, SettingsScope, SettingsSection, TextConfig,
    TokenAccountRow, TokenAccountsConfig,
};
use crate::snapshot::{EngineInfo, EngineSnapshot, ProviderSnapshot, ProviderWindows};
use crate::usage::{
    DetailRow, DetailSection, ProviderError, ProviderErrorCategory, ProviderErrorKind, RateWindow,
};

/// Misbehavior modes for protocol conformance and stress testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MisbehaveMode {
    /// Send an unterminated 50 MiB stream to test client buffering boundaries.
    OversizeFrame,
}

/// Configuration options for the fixture server.
#[derive(Debug, Clone)]
pub struct FixtureConfig {
    pub socket_path: PathBuf,
    pub step_seconds: u64,
    pub misbehave: Option<MisbehaveMode>,
    pub log_sink: Option<Arc<Mutex<Vec<String>>>>,
}

impl Default for FixtureConfig {
    fn default() -> Self {
        let socket_path = std::env::var("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("ariadusage-dev/engine.sock"))
            .unwrap_or_else(|_| PathBuf::from("/tmp/ariadusage-dev/engine.sock"));
        Self {
            socket_path,
            step_seconds: 5,
            misbehave: None,
            log_sink: None,
        }
    }
}

/// Handle to a running in-process fixture server.
pub struct FixtureServerHandle {
    pub socket_path: PathBuf,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
    pub log_sink: Arc<Mutex<Vec<String>>>,
    pub step_counter: Arc<AtomicUsize>,
}

impl FixtureServerHandle {
    /// Stop the fixture server and remove the socket file.
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

impl Drop for FixtureServerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

struct ServerShared {
    step_counter: Arc<AtomicUsize>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
    settings: Mutex<HashMap<SettingId, serde_json::Value>>,
    secrets: Mutex<HashSet<SettingId>>,
    log_sink: Arc<Mutex<Vec<String>>>,
}

impl ServerShared {
    async fn log(&self, msg: String) {
        self.log_sink.lock().await.push(msg);
    }
}

/// Start an in-process fixture server listening on Unix socket.
pub async fn start_fixture_server(
    config: FixtureConfig,
) -> Result<FixtureServerHandle, std::io::Error> {
    ensure_socket_directory(&config.socket_path)?;

    let listener = UnixListener::bind(&config.socket_path)?;
    // Enforce 0600 permissions on the socket file.
    std::fs::set_permissions(&config.socket_path, std::fs::Permissions::from_mode(0o600))?;

    let log_sink = config
        .log_sink
        .unwrap_or_else(|| Arc::new(Mutex::new(Vec::new())));
    let (broadcast_tx, _) = broadcast::channel(32);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let step_counter = Arc::new(AtomicUsize::new(0));

    let shared = Arc::new(ServerShared {
        step_counter: Arc::clone(&step_counter),
        broadcast_tx: broadcast_tx.clone(),
        settings: Mutex::new(HashMap::new()),
        secrets: Mutex::new(HashSet::new()),
        log_sink: Arc::clone(&log_sink),
    });

    let is_running = Arc::new(AtomicBool::new(true));

    // Background task: ticker advancing scenario step and pushing snapshots
    let ticker_shared = Arc::clone(&shared);
    let ticker_running = Arc::clone(&is_running);
    let step_duration = Duration::from_secs(config.step_seconds.max(1));
    tokio::spawn(async move {
        while ticker_running.load(Ordering::Relaxed) {
            tokio::time::sleep(step_duration).await;
            if !ticker_running.load(Ordering::Relaxed) {
                break;
            }
            let step = ticker_shared.step_counter.fetch_add(1, Ordering::Relaxed) + 1;
            let snapshot = generate_scenario_snapshot(step);
            let _ = ticker_shared.broadcast_tx.send(ServerMessage::Snapshot {
                snapshot: Box::new(snapshot),
            });
        }
    });

    // Accept loop
    let accept_shared = Arc::clone(&shared);
    let misbehave = config.misbehave;
    let accept_running = Arc::clone(&is_running);

    let join_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    accept_running.store(false, Ordering::Relaxed);
                    break;
                }
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, _)) => {
                            let client_shared = Arc::clone(&accept_shared);
                            tokio::spawn(async move {
                                handle_client(stream, client_shared, misbehave).await;
                            });
                        }
                        Err(_) => {
                            if !accept_running.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                    }
                }
            }
        }
    });

    Ok(FixtureServerHandle {
        socket_path: config.socket_path,
        shutdown_tx: Some(shutdown_tx),
        join_handle: Some(join_handle),
        log_sink,
        step_counter,
    })
}

async fn handle_client(
    mut stream: UnixStream,
    shared: Arc<ServerShared>,
    misbehave: Option<MisbehaveMode>,
) {
    if misbehave == Some(MisbehaveMode::OversizeFrame) {
        // Test mode: write 50 MiB unterminated line directly to stream
        let chunk = vec![b'A'; 1024 * 1024]; // 1 MiB chunk
        for _ in 0..50 {
            if stream.write_all(&chunk).await.is_err() {
                return;
            }
        }
        let _ = stream.flush().await;
        return;
    }

    let (mut reader, mut writer) = stream.into_split();
    let mut broadcast_rx = shared.broadcast_tx.subscribe();
    let mut subscribed_topics: HashSet<Topic> = HashSet::new();

    let mut read_codec = IpcCodec::new();
    let mut write_codec = IpcCodec::new();
    let mut read_buf = BytesMut::new();
    let mut write_buf = BytesMut::new();

    let mut raw_chunk = [0u8; 8192];

    loop {
        tokio::select! {
            read_res = reader.read(&mut raw_chunk) => {
                match read_res {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        read_buf.extend_from_slice(&raw_chunk[..n]);
                        loop {
                            match read_codec.decode(&mut read_buf) {
                                Ok(Some(line)) => {
                                    let envelope_id = match serde_json::from_str::<crate::ipc::RawEnvelope>(&line) {
                                        Ok(env) => env.id,
                                        Err(_) => None,
                                    };

                                    match parse_client_message(&line) {
                                        Ok(msg) => {
                                            shared.log(format!("[fixture] recv: type={} id={:?}", msg.msg_type(), envelope_id)).await;
                                            match msg {
                                                ClientMessage::Hello { id, .. } => {
                                                    let resp = ServerMessage::Welcome {
                                                        protocol: PROTOCOL_V1.to_string(),
                                                        engine_version: "0.1.0".to_string(),
                                                        capabilities: vec!["snapshot".into(), "settings".into(), "notices".into()],
                                                        id,
                                                    };
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;
                                                }
                                                ClientMessage::Subscribe { topics, id } => {
                                                    for topic in topics {
                                                        subscribed_topics.insert(topic);
                                                    }
                                                    let resp = ServerMessage::ok(id);
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;

                                                    // Immediate snapshot push upon subscribe
                                                    if subscribed_topics.contains(&Topic::Snapshot) {
                                                        let current_step = shared.step_counter.load(Ordering::Relaxed);
                                                        let snapshot = generate_scenario_snapshot(current_step);
                                                        let push = ServerMessage::Snapshot {
                                                            snapshot: Box::new(snapshot),
                                                        };
                                                        send_server_message(&mut writer, &mut write_codec, &mut write_buf, &push, &shared).await;
                                                    }
                                                }
                                                ClientMessage::GetSnapshot { id: _ } => {
                                                    let current_step = shared.step_counter.load(Ordering::Relaxed);
                                                    let snapshot = generate_scenario_snapshot(current_step);
                                                    let push = ServerMessage::Snapshot {
                                                        snapshot: Box::new(snapshot),
                                                    };
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &push, &shared).await;
                                                }
                                                ClientMessage::Refresh { provider: _, id } => {
                                                    let resp = ServerMessage::ok(id);
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;
                                                    let current_step = shared.step_counter.load(Ordering::Relaxed);
                                                    let snapshot = generate_scenario_snapshot(current_step);
                                                    let push = ServerMessage::Snapshot {
                                                        snapshot: Box::new(snapshot),
                                                    };
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &push, &shared).await;
                                                }
                                                ClientMessage::GetSettings { scope, id } => {
                                                    let page = generate_settings_page(&scope, &shared).await;
                                                    let resp = ServerMessage::ok_with_payload(id, serde_json::to_value(page).unwrap());
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;
                                                }
                                                ClientMessage::SetSetting { id, value } => {
                                                    shared.settings.lock().await.insert(id, value);
                                                    let resp = ServerMessage::ok(envelope_id);
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;

                                                    let change_push = ServerMessage::SettingsChanged { scope: SettingsScope::App };
                                                    let _ = shared.broadcast_tx.send(change_push);
                                                }
                                                ClientMessage::RunAction { id: _, confirm: _ } => {
                                                    let resp = ServerMessage::ok(envelope_id);
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;
                                                }
                                                ClientMessage::SetSecret { id, value } => {
                                                    // Store only isSet boolean; immediately drop value
                                                    shared.secrets.lock().await.insert(id);
                                                    drop(value);

                                                    let resp = ServerMessage::ok(envelope_id);
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;

                                                    let change_push = ServerMessage::SettingsChanged { scope: SettingsScope::App };
                                                    let _ = shared.broadcast_tx.send(change_push);
                                                }
                                            }
                                        }
                                        Err((req_id, err)) => {
                                            shared.log(format!("[fixture] recv error: code={:?} id={:?}", err.code, req_id)).await;
                                            let resp = ServerMessage::error(req_id, err);
                                            send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;
                                        }
                                    }
                                }
                                Ok(None) => break, // Need more bytes
                                Err(IpcCodecError::MaxLineLengthExceeded) => {
                                    shared.log("[fixture] recv error: inbound frame exceeded 1 MiB".to_string()).await;
                                    let resp = ServerMessage::error(None, IpcError::new(ErrorCode::PayloadTooLarge));
                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;
                                }
                                Err(_) => return,
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            pushed = broadcast_rx.recv() => {
                if let Ok(msg) = pushed {
                    let should_send = match &msg {
                        ServerMessage::Snapshot { .. } => subscribed_topics.contains(&Topic::Snapshot),
                        ServerMessage::SettingsChanged { .. } => subscribed_topics.contains(&Topic::Settings),
                        ServerMessage::Notice { .. } => subscribed_topics.contains(&Topic::Notices),
                        _ => false,
                    };
                    if should_send {
                        send_server_message(&mut writer, &mut write_codec, &mut write_buf, &msg, &shared).await;
                    }
                }
            }
        }
    }
}

async fn send_server_message(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    codec: &mut IpcCodec,
    buf: &mut BytesMut,
    msg: &ServerMessage,
    shared: &ServerShared,
) {
    let json = match serde_json::to_string(msg) {
        Ok(j) => j,
        Err(_) => return,
    };
    shared
        .log(format!("[fixture] send: type={}", msg.msg_type()))
        .await;
    buf.clear();
    if codec.encode(&json, buf).is_ok() {
        let _ = writer.write_all(buf).await;
        buf.clear();
    }
}

/// Ensure the parent directory exists with 0700 permissions and clean up any stale socket.
pub fn ensure_socket_directory(socket_path: &Path) -> std::io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }
    Ok(())
}

/// Generate a scripted snapshot for the 5-state scenario across 3 providers.
pub fn generate_scenario_snapshot(step: usize) -> EngineSnapshot {
    let now = jiff::Timestamp::now();
    let state_idx = step % 5;

    // 5 states: value, stale, loading, error, unknown
    let (claude_state, codex_state, agy_state) = match state_idx {
        0 => (MetricState::Value, MetricState::Value, MetricState::Value),
        1 => (MetricState::Stale, MetricState::Value, MetricState::Loading),
        2 => (MetricState::Loading, MetricState::Error, MetricState::Value),
        3 => (MetricState::Error, MetricState::Unknown, MetricState::Stale),
        _ => (MetricState::Unknown, MetricState::Stale, MetricState::Error),
    };

    let providers = vec![
        make_provider_snapshot("claude", "Claude", "cli", claude_state),
        make_provider_snapshot("codex", "Codex", "web", codex_state),
        make_provider_snapshot("antigravity", "Antigravity", "api", agy_state),
    ];

    EngineSnapshot::new(
        now,
        15,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        providers,
    )
}

fn make_provider_snapshot(
    id_str: &str,
    name: &str,
    mode: &str,
    state: MetricState,
) -> ProviderSnapshot {
    let pid = ProviderId::new(id_str).unwrap();
    let mut snap = ProviderSnapshot::new(pid, name, true, mode);

    let rate_value = match state {
        MetricState::Value | MetricState::Stale => Some(RateWindow {
            used_percent: 42.0,
            window_minutes: Some(300),
            resets_at: "2026-10-06T18:00:00Z".parse::<jiff::Timestamp>().ok(),
            reset_description: Some("Resets in 5 hours".to_string()),
            next_regen_percent: None,
        }),
        _ => None,
    };

    let metric = Metric::new(
        state,
        rate_value,
        None,
        Some(MetricSource::new(
            SourceKind::Cli,
            format!("{id_str}-strategy"),
        )),
        Confidence::Exact,
        None,
    )
    .unwrap();

    snap.windows = ProviderWindows {
        primary: Some(metric),
        secondary: None,
        tertiary: None,
        extra: Vec::new(),
    };

    if state == MetricState::Error {
        snap.last_error = Some(ProviderError {
            kind: ProviderErrorKind::RateLimited,
            category: ProviderErrorCategory::Api,
            message: "Rate limit encountered on provider".to_string(),
            retry_after_seconds: Some(60),
        });
    }

    snap.details = vec![
        DetailSection::new(
            Some("Usage Details"),
            vec![
                DetailRow::new(
                    Some("row-1"),
                    "Requests Today",
                    "1,240",
                    None::<&str>,
                    None,
                    Some(1240.0),
                )
                .unwrap(),
            ],
            None,
        )
        .unwrap(),
    ];

    snap
}

async fn generate_settings_page(scope: &SettingsScope, shared: &ServerShared) -> SettingsPage {
    let is_key_set = shared
        .secrets
        .lock()
        .await
        .contains(&SettingId::new("providers.claude.apiKey").unwrap());

    let settings = shared.settings.lock().await;

    let source_mode = settings
        .get(&SettingId::new("sourceMode").unwrap())
        .and_then(|v| v.as_str())
        .unwrap_or("cli")
        .to_string();

    let notif_enabled = settings
        .get(&SettingId::new("notifications.enabled").unwrap())
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let refresh_interval = settings
        .get(&SettingId::new("refreshInterval").unwrap())
        .and_then(|v| v.as_f64())
        .unwrap_or(900.0);

    let device_id = settings
        .get(&SettingId::new("syncDeviceId").unwrap())
        .and_then(|v| v.as_str())
        .unwrap_or("primary-workstation")
        .to_string();

    let sync_paths = settings
        .get(&SettingId::new("syncDirs").unwrap())
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| vec!["/var/log/ariadusage".to_string()]);

    let accent_color = settings
        .get(&SettingId::new("accentColor").unwrap())
        .and_then(|v| v.as_str())
        .unwrap_or("#cacccc")
        .to_string();

    let general_descriptors = vec![
        SettingDescriptor::new(
            SettingId::new("sourceMode").unwrap(),
            "Source Mode",
            DescriptorKind::Choice {
                selected: source_mode,
                options: vec![
                    ChoiceOption::new("cli", "CLI Tool"),
                    ChoiceOption::new("web", "Web Session"),
                ],
            },
        ),
        SettingDescriptor::new(
            SettingId::new("notifications.enabled").unwrap(),
            "Desktop Notifications",
            DescriptorKind::Toggle {
                value: notif_enabled,
            },
        ),
        SettingDescriptor::new(
            SettingId::new("refreshInterval").unwrap(),
            "Refresh Interval",
            DescriptorKind::Number {
                value: refresh_interval,
                config: NumberConfig {
                    min: Some(30.0),
                    max: Some(3600.0),
                    step: Some(30.0),
                    unit: Some("s".to_string()),
                },
            },
        ),
        SettingDescriptor::new(
            SettingId::new("syncDeviceId").unwrap(),
            "Device Identifier",
            DescriptorKind::Text {
                value: device_id,
                config: TextConfig {
                    placeholder: Some("device-name".to_string()),
                    max_length: Some(64),
                },
            },
        ),
        SettingDescriptor::new(
            SettingId::new("syncDirs").unwrap(),
            "Sync Directories",
            DescriptorKind::PathList { paths: sync_paths },
        ),
        SettingDescriptor::new(
            SettingId::new("accentColor").unwrap(),
            "Custom Accent Color",
            DescriptorKind::Color {
                value: accent_color,
                default: "#cacccc".to_string(),
            },
        ),
    ];

    let provider_descriptors = vec![
        SettingDescriptor::new(
            SettingId::new("providers.active").unwrap(),
            "Active Providers",
            DescriptorKind::MultiChoice {
                entries: vec![
                    MultiChoiceEntry::new("claude", "Claude", false, true),
                    MultiChoiceEntry::new("codex", "Codex", false, true),
                    MultiChoiceEntry::new("antigravity", "Antigravity", false, true),
                ],
            },
        ),
        SettingDescriptor::new(
            SettingId::new("providers.claude.apiKey").unwrap(),
            "Anthropic API Key",
            DescriptorKind::Secret {
                is_set: is_key_set,
                source: Some("Keychain".to_string()),
            },
        ),
        SettingDescriptor::new(
            SettingId::new("accounts.codex").unwrap(),
            "Codex Accounts",
            DescriptorKind::TokenAccounts {
                config: TokenAccountsConfig {
                    accounts: vec![
                        TokenAccountRow {
                            id: "acc-1".to_string(),
                            label: "Work Account".to_string(),
                            active: true,
                            token_is_set: true,
                        },
                        TokenAccountRow {
                            id: "acc-2".to_string(),
                            label: "Personal Account".to_string(),
                            active: false,
                            token_is_set: false,
                        },
                    ],
                    supports_add: true,
                    supports_remove: true,
                    supports_activate: true,
                },
            },
        ),
    ];

    let actions_descriptors = vec![
        SettingDescriptor::new(
            SettingId::new("diagnostics").unwrap(),
            "Maintenance Actions",
            DescriptorKind::Actions {
                actions: vec![
                    ActionItem {
                        id: ActionId::new("refreshAll").unwrap(),
                        label: "Refresh All Providers".to_string(),
                        style: ActionStyle::Button,
                        confirmation: None,
                    },
                    ActionItem {
                        id: ActionId::new("resetUsage").unwrap(),
                        label: "Reset Local Cache".to_string(),
                        style: ActionStyle::Button,
                        confirmation: Some(ActionConfirmation {
                            title: "Confirm Cache Reset".to_string(),
                            message: "Are you sure you want to clear the local usage cache?"
                                .to_string(),
                            confirm_label: "Reset".to_string(),
                        }),
                    },
                ],
            },
        ),
        SettingDescriptor::new(
            SettingId::new("experimental.customWidget").unwrap(),
            "Experimental Widget",
            DescriptorKind::Unknown {
                kind: "customWidget".to_string(),
            },
        ),
    ];

    SettingsPage::new(
        scope.clone(),
        vec![
            SettingsSection::new("general", "General Settings", general_descriptors),
            SettingsSection::new("providers", "Provider Configurations", provider_descriptors),
            SettingsSection::new("actions", "System Actions", actions_descriptors),
        ],
    )
}
