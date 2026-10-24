//! In-process Unix domain socket fixture server for IPC testing and UI development.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinSet;
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
    NumberConfig, SettingCondition, SettingDescriptor, SettingsPage, SettingsScope,
    SettingsSection, TextConfig, TokenAccountRow, TokenAccountsConfig,
};
use crate::snapshot::{
    AccountSnapshot, EngineInfo, EngineSnapshot, ProviderPresentation, ProviderSnapshot,
    ProviderWindows,
};
use crate::usage::{
    BillingDate, Cost, CostDailyEntry, CostHistory, CostModelBreakdown, CostProvenance,
    CostWindowSummary, CreditLimit, Credits, DetailRow, DetailSection, Identity, NamedWindow, Pace,
    PaceStage, PlanUtilizationEntry, PlanUtilizationSeries, ProviderError, ProviderErrorCategory,
    ProviderErrorKind, ProviderStatus, RateWindow, ResetCredits, RowProgress, RowSeverity,
    SessionEquivalentForecast, StatusComponent, StatusIndicator, SubscriptionDates,
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
    pub extra_windows: bool,
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
            extra_windows: false,
        }
    }
}

/// Handle to a running in-process fixture server.
pub struct FixtureServerHandle {
    pub socket_path: PathBuf,
    shutdown_tx: watch::Sender<bool>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
    pub log_sink: Arc<Mutex<Vec<String>>>,
    pub step_counter: Arc<AtomicUsize>,
}

impl FixtureServerHandle {
    /// Stop the fixture server, close all active client sessions, and remove the socket file.
    pub async fn stop(&mut self) {
        let _ = self.shutdown_tx.send(true);
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
        let _ = self.shutdown_tx.send(true);
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
    extra_windows: bool,
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
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let step_counter = Arc::new(AtomicUsize::new(0));

    let shared = Arc::new(ServerShared {
        step_counter: Arc::clone(&step_counter),
        broadcast_tx: broadcast_tx.clone(),
        settings: Mutex::new(HashMap::new()),
        secrets: Mutex::new(HashSet::new()),
        log_sink: Arc::clone(&log_sink),
        extra_windows: config.extra_windows,
    });

    let is_running = Arc::new(AtomicBool::new(true));

    // Background task: ticker advancing scenario step and pushing snapshots
    let ticker_shared = Arc::clone(&shared);
    let ticker_running = Arc::clone(&is_running);
    let mut ticker_shutdown = shutdown_rx.clone();
    let step_duration = Duration::from_secs(config.step_seconds.max(1));
    tokio::spawn(async move {
        while ticker_running.load(Ordering::Relaxed) {
            tokio::select! {
                _ = ticker_shutdown.changed() => break,
                () = tokio::time::sleep(step_duration) => {}
            }
            if !ticker_running.load(Ordering::Relaxed) || *ticker_shutdown.borrow() {
                break;
            }
            let step = ticker_shared.step_counter.fetch_add(1, Ordering::Relaxed) + 1;
            let snapshot = generate_scenario_snapshot_with_extra(step, ticker_shared.extra_windows);
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
        let mut clients = JoinSet::new();
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    accept_running.store(false, Ordering::Relaxed);
                    break;
                }
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, _)) => {
                            let client_shared = Arc::clone(&accept_shared);
                            let client_shutdown = shutdown_rx.clone();
                            clients.spawn(async move {
                                handle_client(stream, client_shared, misbehave, client_shutdown).await;
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
        while clients.join_next().await.is_some() {}
    });

    Ok(FixtureServerHandle {
        socket_path: config.socket_path,
        shutdown_tx,
        join_handle: Some(join_handle),
        log_sink,
        step_counter,
    })
}

async fn handle_client(
    mut stream: UnixStream,
    shared: Arc<ServerShared>,
    misbehave: Option<MisbehaveMode>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    if *shutdown_rx.borrow_and_update() {
        return;
    }
    if misbehave == Some(MisbehaveMode::OversizeFrame) {
        // Test mode: write 50 MiB unterminated line directly to stream
        let chunk = vec![b'A'; 1024 * 1024]; // 1 MiB chunk
        for _ in 0..50 {
            tokio::select! {
                _ = shutdown_rx.changed() => return,
                res = stream.write_all(&chunk) => {
                    if res.is_err() {
                        return;
                    }
                }
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
            _ = shutdown_rx.changed() => break,
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
                                                        let snapshot = generate_scenario_snapshot_with_extra(current_step, shared.extra_windows);
                                                        let push = ServerMessage::Snapshot {
                                                            snapshot: Box::new(snapshot),
                                                        };
                                                        send_server_message(&mut writer, &mut write_codec, &mut write_buf, &push, &shared).await;
                                                    }
                                                }
                                                ClientMessage::GetSnapshot { id: _ } => {
                                                    let current_step = shared.step_counter.load(Ordering::Relaxed);
                                                    let snapshot = generate_scenario_snapshot_with_extra(current_step, shared.extra_windows);
                                                    let push = ServerMessage::Snapshot {
                                                        snapshot: Box::new(snapshot),
                                                    };
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &push, &shared).await;
                                                }
                                                ClientMessage::Refresh { provider: _, id } => {
                                                    let resp = ServerMessage::ok(id);
                                                    send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp, &shared).await;
                                                    let current_step = shared.step_counter.load(Ordering::Relaxed);
                                                    let snapshot = generate_scenario_snapshot_with_extra(current_step, shared.extra_windows);
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
    generate_scenario_snapshot_with_extra(step, false)
}

/// Generate a scripted snapshot for the 5-state scenario with optional extra rate windows.
pub fn generate_scenario_snapshot_with_extra(step: usize, extra_windows: bool) -> EngineSnapshot {
    let raw_now = jiff::Timestamp::now();
    let now = jiff::Timestamp::from_second(raw_now.as_second()).unwrap_or(raw_now);
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
        make_provider_snapshot("claude", "Claude", "cli", claude_state, extra_windows),
        make_provider_snapshot("codex", "Codex", "web", codex_state, extra_windows),
        make_provider_snapshot(
            "antigravity",
            "Antigravity",
            "api",
            agy_state,
            extra_windows,
        ),
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
    extra_windows: bool,
) -> ProviderSnapshot {
    let pid = ProviderId::new(id_str).unwrap();
    let mut snap = ProviderSnapshot::new(pid.clone(), name, true, mode);
    snap.presentation = fixture_presentation(id_str);

    let as_of = "2026-10-06T12:48:00Z".parse::<jiff::Timestamp>().ok();
    snap.updated_at = as_of;
    let has_data = matches!(state, MetricState::Value | MetricState::Stale);
    let source = MetricSource::new(SourceKind::Cli, format!("{id_str}-strategy"));

    let primary_pace = Pace {
        stage: PaceStage::SlightlyBehind,
        delta_percent: -6.0,
        expected_used_percent: 48.0,
        will_last_to_reset: true,
        eta_seconds: None,
        speed_multiplier_to_reset: Some(1.2),
        run_out_probability: None,
        summary: "6% in reserve | Expected 48% used | Lasts until reset".to_string(),
    };

    let rate_value = has_data.then(|| RateWindow {
        used_percent: 42.0,
        window_minutes: Some(300),
        resets_at: "2026-10-06T18:00:00Z".parse::<jiff::Timestamp>().ok(),
        reset_description: Some("Resets in 5 hours".to_string()),
        next_regen_percent: None,
        pace: Some(primary_pace),
        session_equivalent: None,
    });

    let primary = Metric::new(
        state,
        rate_value,
        as_of,
        Some(source.clone()),
        Confidence::Exact,
        None,
    )
    .unwrap();

    let (secondary_used, secondary_pace, secondary_session_equiv) = match id_str {
        "claude" => (
            64.0,
            Pace {
                stage: PaceStage::Ahead,
                delta_percent: 9.0,
                expected_used_percent: 55.0,
                will_last_to_reset: false,
                eta_seconds: Some(172_800),
                speed_multiplier_to_reset: Some(0.82),
                run_out_probability: Some(0.74),
                summary: "9% in deficit | Expected 55% used | Runs out in 2d".to_string(),
            },
            Some(SessionEquivalentForecast::new(2.4, 16, Some(12))),
        ),
        "codex" => (
            51.0,
            Pace {
                stage: PaceStage::Behind,
                delta_percent: -18.0,
                expected_used_percent: 69.0,
                will_last_to_reset: true,
                eta_seconds: None,
                speed_multiplier_to_reset: Some(1.65),
                run_out_probability: Some(0.05),
                summary: "18% in reserve | Expected 69% used | Lasts until reset | 1.5× headroom"
                    .to_string(),
            },
            Some(SessionEquivalentForecast::new(3.25, 20, Some(18))),
        ),
        _ => (
            28.0,
            Pace {
                stage: PaceStage::OnTrack,
                delta_percent: -1.0,
                expected_used_percent: 29.0,
                will_last_to_reset: true,
                eta_seconds: None,
                speed_multiplier_to_reset: Some(1.4),
                run_out_probability: None,
                summary: "On pace | Expected 29% used | Lasts until reset".to_string(),
            },
            None,
        ),
    };

    let secondary = has_data.then(|| {
        Metric::new(
            state,
            Some(RateWindow {
                used_percent: secondary_used,
                window_minutes: Some(10080),
                resets_at: "2026-10-10T09:00:00Z".parse::<jiff::Timestamp>().ok(),
                reset_description: Some("Resets Sat at 9:00 AM".to_string()),
                next_regen_percent: None,
                pace: Some(secondary_pace),
                session_equivalent: secondary_session_equiv,
            }),
            as_of,
            Some(source.clone()),
            Confidence::Exact,
            None,
        )
        .unwrap()
    });

    let mut extra = Vec::new();
    if extra_windows {
        let extra_rate = has_data.then(|| RateWindow {
            used_percent: 15.0,
            window_minutes: Some(60),
            resets_at: "2026-10-06T14:00:00Z".parse::<jiff::Timestamp>().ok(),
            reset_description: Some("Resets in 1 hour".to_string()),
            next_regen_percent: None,
            pace: None,
            session_equivalent: None,
        });
        let extra_metric = Metric::new(
            state,
            extra_rate,
            as_of,
            Some(MetricSource::new(
                SourceKind::Cli,
                format!("{id_str}-extra"),
            )),
            Confidence::Exact,
            None,
        )
        .unwrap();
        extra.push(NamedWindow {
            id: "burst".to_string(),
            title: "1-Hour Burst".to_string(),
            window: extra_metric,
        });
    }
    if id_str == "antigravity" && has_data {
        let flash_metric = Metric::new(
            state,
            Some(RateWindow {
                used_percent: 19.0,
                window_minutes: Some(300),
                resets_at: "2026-10-06T18:00:00Z".parse::<jiff::Timestamp>().ok(),
                reset_description: Some("Resets in 5 hours".to_string()),
                next_regen_percent: None,
                pace: None,
                session_equivalent: None,
            }),
            as_of,
            Some(source.clone()),
            Confidence::Exact,
            None,
        )
        .unwrap();
        extra.push(NamedWindow {
            id: "gemini-flash".to_string(),
            title: "Gemini 2.5 Flash".to_string(),
            window: flash_metric,
        });
    }

    // Claude carries a tertiary window whenever it has data, so frontends can
    // render the row titled by `presentation.tertiaryLabel`.
    let tertiary = match state {
        MetricState::Value | MetricState::Stale if id_str == "claude" => Some(
            Metric::new(
                state,
                Some(RateWindow {
                    used_percent: 18.0,
                    window_minutes: Some(10080),
                    resets_at: "2026-10-10T09:00:00Z".parse::<jiff::Timestamp>().ok(),
                    reset_description: Some("Resets in 4 days".to_string()),
                    next_regen_percent: None,
                    pace: None,
                    session_equivalent: None,
                }),
                as_of,
                Some(source.clone()),
                Confidence::Exact,
                None,
            )
            .unwrap(),
        ),
        _ => None,
    };

    snap.windows = ProviderWindows {
        primary: Some(primary),
        secondary,
        tertiary,
        extra,
    };

    let plan_name = match id_str {
        "claude" => "Max",
        "codex" => "Pro",
        _ => "Pro",
    };
    snap.identity = Some(
        Metric::new(
            state,
            has_data.then(|| Identity {
                provider_id: pid.clone(),
                account_email: Some("alex@example.invalid".to_string()),
                account_organization: (id_str != "antigravity").then(|| "Acme Labs".to_string()),
                login_method: Some(plan_name.to_string()),
                account_id: Some(format!("acc-{id_str}-1")),
                plan: Some(plan_name.to_string()),
            }),
            as_of,
            Some(source.clone()),
            Confidence::Exact,
            None,
        )
        .unwrap(),
    );

    let subscription_dates = match id_str {
        "claude" => SubscriptionDates {
            renews: Some(BillingDate {
                at: "2026-11-01T00:00:00Z".parse().unwrap(),
                date_only: true,
            }),
            expires: None,
        },
        "codex" => SubscriptionDates {
            renews: Some(BillingDate {
                at: "2026-11-15T00:00:00Z".parse().unwrap(),
                date_only: true,
            }),
            expires: None,
        },
        _ => SubscriptionDates {
            renews: None,
            expires: Some(BillingDate {
                at: "2026-12-01T00:00:00Z".parse().unwrap(),
                date_only: true,
            }),
        },
    };
    snap.subscription = Some(
        Metric::new(
            state,
            has_data.then_some(subscription_dates),
            as_of,
            Some(source.clone()),
            Confidence::Exact,
            None,
        )
        .unwrap(),
    );

    if matches!(id_str, "claude" | "codex") {
        let credits_val = has_data.then(|| {
            if id_str == "codex" {
                Credits {
                    remaining: 184.5,
                    unit: Some("credits".to_string()),
                    balance_is_workspace: Some(false),
                    updated_at: as_of,
                    balance_read_succeeded: Some(true),
                    credits_available: Some(true),
                    credit_limit: Some(CreditLimit {
                        title: Some("Monthly credit allowance".to_string()),
                        used: 65.5,
                        total: 250.0,
                        remaining: Some(184.5),
                        unit: Some("credits".to_string()),
                        resets_at: "2026-11-01T00:00:00Z".parse::<jiff::Timestamp>().ok(),
                        reset_description: Some("Resets Nov 1".to_string()),
                    }),
                    reset_credits: Some(ResetCredits {
                        available_count: 2,
                        next_expires_at: "2026-10-20T00:00:00Z".parse::<jiff::Timestamp>().ok(),
                    }),
                }
            } else {
                Credits {
                    remaining: 38.20,
                    unit: Some("USD".to_string()),
                    balance_is_workspace: Some(true),
                    updated_at: as_of,
                    balance_read_succeeded: Some(true),
                    credits_available: Some(true),
                    credit_limit: None,
                    reset_credits: None,
                }
            }
        });
        snap.credits = Some(
            Metric::new(
                state,
                credits_val,
                as_of,
                Some(source.clone()),
                Confidence::Exact,
                None,
            )
            .unwrap(),
        );

        let cost_val = has_data.then(|| {
            if id_str == "claude" {
                Cost {
                    used: 61.80,
                    limit: Some(100.0),
                    currency_code: "USD".to_string(),
                    period: Some("Monthly".to_string()),
                    resets_at: "2026-11-01T00:00:00Z".parse::<jiff::Timestamp>().ok(),
                    balance: Some(38.20),
                    next_regen_amount: Some(25.0),
                    personal_used: Some(19.40),
                    balance_updated_at: as_of,
                    balance_is_workspace: Some(true),
                    balance_is_unavailable: Some(false),
                }
            } else {
                Cost {
                    used: 42.50,
                    limit: Some(120.0),
                    currency_code: "USD".to_string(),
                    period: Some("Monthly".to_string()),
                    resets_at: "2026-11-01T00:00:00Z".parse::<jiff::Timestamp>().ok(),
                    balance: Some(77.50),
                    next_regen_amount: None,
                    personal_used: Some(42.50),
                    balance_updated_at: as_of,
                    balance_is_workspace: Some(false),
                    balance_is_unavailable: Some(false),
                }
            }
        });
        snap.cost = Some(
            Metric::new(
                state,
                cost_val,
                as_of,
                Some(source.clone()),
                Confidence::Exact,
                None,
            )
            .unwrap(),
        );

        snap.cost_history = Some(
            Metric::new(
                state,
                has_data.then(|| fixture_cost_history(id_str)),
                as_of,
                Some(source.clone()),
                Confidence::Estimated,
                None,
            )
            .unwrap(),
        );

        snap.utilization = Some(
            Metric::new(
                state,
                has_data.then(|| fixture_utilization(id_str)),
                as_of,
                Some(source.clone()),
                Confidence::Exact,
                None,
            )
            .unwrap(),
        );
    }

    let status_val = fixture_status(id_str, as_of);
    snap.status = Some(
        Metric::new(
            state,
            has_data.then_some(status_val),
            as_of,
            Some(source.clone()),
            Confidence::Exact,
            None,
        )
        .unwrap(),
    );

    if state == MetricState::Error {
        snap.last_error = Some(ProviderError {
            kind: ProviderErrorKind::RateLimited,
            category: ProviderErrorCategory::Api,
            message: "Rate limit encountered on provider".to_string(),
            retry_after_seconds: Some(60),
        });
    }

    let mut sections = vec![
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
    if id_str == "claude" && has_data {
        sections.push(
            DetailSection::new(
                Some("Cloud credits"),
                vec![
                    DetailRow::new(
                        Some("cloud-credits-available"),
                        "Available",
                        "$38.20",
                        Some("Workspace balance · $61.80 / $100.00 used"),
                        Some(RowProgress::new(61.8, 100.0).unwrap()),
                        Some(38.20),
                    )
                    .unwrap()
                    .with_severity(Some(RowSeverity::Normal)),
                ],
                None,
            )
            .unwrap(),
        );
    }
    snap.details = sections;

    if id_str == "codex" && has_data {
        snap.accounts = vec![
            AccountSnapshot {
                id: "acc-1".to_string(),
                label: "Work Account".to_string(),
                active: true,
                windows: snap.windows.clone(),
                credits: snap.credits.clone(),
                cost: snap.cost.clone(),
                cost_history: snap.cost_history.clone(),
                identity: snap.identity.clone(),
                status: snap.status.clone(),
                utilization: snap.utilization.clone(),
                subscription: snap.subscription.clone(),
                last_error: None,
                updated_at: as_of,
            },
            AccountSnapshot {
                id: "acc-2".to_string(),
                label: "Personal Account".to_string(),
                active: false,
                windows: ProviderWindows {
                    primary: Some(
                        Metric::new(
                            MetricState::Value,
                            Some(RateWindow::new(12.0)),
                            as_of,
                            Some(source),
                            Confidence::Exact,
                            None,
                        )
                        .unwrap(),
                    ),
                    secondary: None,
                    tertiary: None,
                    extra: Vec::new(),
                },
                credits: None,
                cost: None,
                cost_history: None,
                identity: None,
                status: None,
                utilization: None,
                subscription: None,
                last_error: None,
                updated_at: as_of,
            },
        ];
    }

    snap
}

fn fixture_cost_history(id_str: &str) -> CostHistory {
    let is_claude = id_str == "claude";
    let primary_model = if is_claude {
        "claude-sonnet-4-5"
    } else {
        "gpt-5-codex"
    };
    let secondary_model = if is_claude { "claude-opus-4-1" } else { "o3" };

    let mut daily = Vec::with_capacity(30);
    let mut total_tokens_30d = 0_u64;
    let mut total_cost_30d = 0.0_f64;
    let mut total_requests_30d = 0_u64;

    for idx in 0..30_u32 {
        let (month, day) = if idx < 24 {
            (9, 7 + idx)
        } else {
            (10, idx - 23)
        };
        let date = format!("2026-{month:02}-{day:02}");
        let primary_tokens = 120_000_u64 + u64::from((idx * 17_500) % 95_000);
        let secondary_tokens = 25_000_u64 + u64::from((idx * 7_300) % 35_000);
        let day_tokens = primary_tokens + secondary_tokens;
        let primary_cost = (primary_tokens as f64 / 1_000_000.0) * 9.0;
        let secondary_cost = (secondary_tokens as f64 / 1_000_000.0) * 24.0;
        let day_cost = ((primary_cost + secondary_cost) * 100.0).round() / 100.0;
        let day_requests = 42_u64 + u64::from((idx * 11) % 38);

        total_tokens_30d += day_tokens;
        total_cost_30d += day_cost;
        total_requests_30d += day_requests;

        let primary_breakdown = CostModelBreakdown {
            model_name: primary_model.to_string(),
            cost_usd: Some((primary_cost * 100.0).round() / 100.0),
            total_tokens: Some(primary_tokens),
            standard_cost_usd: (!is_claude).then(|| ((primary_cost * 0.7) * 100.0).round() / 100.0),
            priority_cost_usd: (!is_claude).then(|| ((primary_cost * 0.3) * 100.0).round() / 100.0),
            standard_tokens: (!is_claude).then_some((primary_tokens * 7) / 10),
            priority_tokens: (!is_claude).then_some(primary_tokens - (primary_tokens * 7) / 10),
        };
        let secondary_breakdown = CostModelBreakdown {
            model_name: secondary_model.to_string(),
            cost_usd: Some((secondary_cost * 100.0).round() / 100.0),
            total_tokens: Some(secondary_tokens),
            standard_cost_usd: None,
            priority_cost_usd: None,
            standard_tokens: None,
            priority_tokens: None,
        };

        daily.push(CostDailyEntry {
            date,
            total_tokens: Some(day_tokens),
            cost_usd: Some(day_cost),
            total_requests: Some(day_requests),
            models_used: vec![primary_model.to_string(), secondary_model.to_string()],
            model_breakdowns: vec![primary_breakdown, secondary_breakdown],
        });
    }

    let rounded_30d_cost = (total_cost_30d * 100.0).round() / 100.0;
    let last_entry = daily.last().unwrap();
    let provenance = if is_claude {
        CostProvenance::ListPriceEstimate
    } else {
        CostProvenance::Mixed
    };

    CostHistory {
        session_tokens: last_entry.total_tokens,
        session_cost_usd: last_entry.cost_usd,
        session_requests: last_entry.total_requests,
        last_30_days_tokens: Some(total_tokens_30d),
        last_30_days_cost_usd: Some(rounded_30d_cost),
        last_30_days_requests: Some(total_requests_30d),
        history_days: Some(30),
        history_label: Some("Last 30 days".to_string()),
        history_coverage_is_established: true,
        history_scan_is_partial: false,
        metered_cost_usd: (!is_claude).then_some(42.50),
        currency_code: Some("USD".to_string()),
        cost_provenance: Some(provenance),
        daily,
        comparison_windows: vec![
            CostWindowSummary {
                days: 7,
                cost_usd: Some(((rounded_30d_cost * 0.24) * 100.0).round() / 100.0),
                metered_cost_usd: (!is_claude).then_some(10.20),
                total_tokens: Some(total_tokens_30d / 4),
                total_requests: Some(total_requests_30d / 4),
                incomplete_request_count: None,
                entry_count: Some(7),
                provenance: Some(provenance),
            },
            CostWindowSummary {
                days: 14,
                cost_usd: Some(((rounded_30d_cost * 0.48) * 100.0).round() / 100.0),
                metered_cost_usd: (!is_claude).then_some(20.40),
                total_tokens: Some(total_tokens_30d / 2),
                total_requests: Some(total_requests_30d / 2),
                incomplete_request_count: None,
                entry_count: Some(14),
                provenance: Some(provenance),
            },
            CostWindowSummary {
                days: 30,
                cost_usd: Some(rounded_30d_cost),
                metered_cost_usd: (!is_claude).then_some(42.50),
                total_tokens: Some(total_tokens_30d),
                total_requests: Some(total_requests_30d),
                incomplete_request_count: None,
                entry_count: Some(30),
                provenance: Some(provenance),
            },
        ],
        top_model: Some(CostModelBreakdown {
            model_name: primary_model.to_string(),
            cost_usd: Some(((rounded_30d_cost * 0.68) * 100.0).round() / 100.0),
            total_tokens: Some((total_tokens_30d * 4) / 5),
            standard_cost_usd: None,
            priority_cost_usd: None,
            standard_tokens: None,
            priority_tokens: None,
        }),
        hint_lines: vec![if is_claude {
            "Estimated from local session logs using standard token rates.".to_string()
        } else {
            "Combines provider metered spend with local token log estimates.".to_string()
        }],
    }
}

fn fixture_utilization(id_str: &str) -> Vec<PlanUtilizationSeries> {
    let session_reset = "2026-10-06T18:00:00Z".parse::<jiff::Timestamp>().ok();
    let weekly_reset = "2026-10-10T09:00:00Z".parse::<jiff::Timestamp>().ok();

    let session_entries: Vec<PlanUtilizationEntry> = [
        ("2026-10-06T08:00:00Z", 5.0),
        ("2026-10-06T09:00:00Z", 14.0),
        ("2026-10-06T10:00:00Z", 23.0),
        ("2026-10-06T11:00:00Z", 31.0),
        ("2026-10-06T12:00:00Z", 38.0),
        ("2026-10-06T12:48:00Z", 42.0),
    ]
    .into_iter()
    .map(|(ts, used)| PlanUtilizationEntry {
        captured_at: ts.parse().unwrap(),
        used_percent: used,
        resets_at: session_reset,
    })
    .collect();

    let weekly_peak = if id_str == "claude" { 64.0 } else { 51.0 };
    let weekly_entries: Vec<PlanUtilizationEntry> = [
        ("2026-10-03T12:00:00Z", 10.0),
        ("2026-10-04T12:00:00Z", 24.0),
        ("2026-10-05T12:00:00Z", 44.0),
        ("2026-10-06T12:48:00Z", weekly_peak),
    ]
    .into_iter()
    .map(|(ts, used)| PlanUtilizationEntry {
        captured_at: ts.parse().unwrap(),
        used_percent: used,
        resets_at: weekly_reset,
    })
    .collect();

    let mut series = vec![
        PlanUtilizationSeries {
            name: "session".to_string(),
            window_minutes: 300,
            title: Some("Session".to_string()),
            entries: session_entries,
        },
        PlanUtilizationSeries {
            name: "weekly".to_string(),
            window_minutes: 10080,
            title: Some("Weekly".to_string()),
            entries: weekly_entries,
        },
    ];

    if id_str == "claude" {
        series.push(PlanUtilizationSeries {
            name: "opus".to_string(),
            window_minutes: 10080,
            title: Some("Sonnet".to_string()),
            entries: vec![
                PlanUtilizationEntry {
                    captured_at: "2026-10-05T12:00:00Z".parse().unwrap(),
                    used_percent: 11.0,
                    resets_at: weekly_reset,
                },
                PlanUtilizationEntry {
                    captured_at: "2026-10-06T12:48:00Z".parse().unwrap(),
                    used_percent: 18.0,
                    resets_at: weekly_reset,
                },
            ],
        });
    }

    series
}

fn fixture_status(id_str: &str, as_of: Option<jiff::Timestamp>) -> ProviderStatus {
    match id_str {
        "claude" => ProviderStatus {
            indicator: StatusIndicator::Minor,
            description: Some("Elevated latency on Claude API".to_string()),
            updated_at: as_of,
            page_url: Some("https://status.anthropic.com".to_string()),
            components: vec![
                StatusComponent {
                    id: "claude-api".to_string(),
                    name: "Claude API".to_string(),
                    status: "Degraded Performance".to_string(),
                    indicator: StatusIndicator::Minor,
                    updated_at: as_of,
                    children: vec![
                        StatusComponent {
                            id: "messages-api".to_string(),
                            name: "Messages API".to_string(),
                            status: "Degraded Performance".to_string(),
                            indicator: StatusIndicator::Minor,
                            updated_at: as_of,
                            children: Vec::new(),
                        },
                        StatusComponent {
                            id: "batches-api".to_string(),
                            name: "Message Batches".to_string(),
                            status: "Operational".to_string(),
                            indicator: StatusIndicator::None,
                            updated_at: as_of,
                            children: Vec::new(),
                        },
                    ],
                },
                StatusComponent {
                    id: "claude-web".to_string(),
                    name: "claude.ai".to_string(),
                    status: "Operational".to_string(),
                    indicator: StatusIndicator::None,
                    updated_at: as_of,
                    children: Vec::new(),
                },
                StatusComponent {
                    id: "claude-console".to_string(),
                    name: "Console".to_string(),
                    status: "Operational".to_string(),
                    indicator: StatusIndicator::None,
                    updated_at: as_of,
                    children: Vec::new(),
                },
            ],
        },
        "codex" => ProviderStatus {
            indicator: StatusIndicator::None,
            description: Some("All Systems Operational".to_string()),
            updated_at: as_of,
            page_url: Some("https://status.openai.com".to_string()),
            components: vec![
                StatusComponent {
                    id: "codex-agent".to_string(),
                    name: "Codex Cloud Agent".to_string(),
                    status: "Operational".to_string(),
                    indicator: StatusIndicator::None,
                    updated_at: as_of,
                    children: vec![StatusComponent {
                        id: "sandbox-runtime".to_string(),
                        name: "Container Sandbox".to_string(),
                        status: "Degraded Performance".to_string(),
                        indicator: StatusIndicator::Minor,
                        updated_at: as_of,
                        children: Vec::new(),
                    }],
                },
                StatusComponent {
                    id: "responses-api".to_string(),
                    name: "Responses API".to_string(),
                    status: "Operational".to_string(),
                    indicator: StatusIndicator::None,
                    updated_at: as_of,
                    children: Vec::new(),
                },
            ],
        },
        _ => ProviderStatus {
            indicator: StatusIndicator::None,
            description: Some("All Systems Operational".to_string()),
            updated_at: as_of,
            page_url: Some("https://status.cloud.google.com".to_string()),
            components: vec![StatusComponent {
                id: "gemini-models".to_string(),
                name: "Gemini Code Assist".to_string(),
                status: "Operational".to_string(),
                indicator: StatusIndicator::None,
                updated_at: as_of,
                children: Vec::new(),
            }],
        },
    }
}

/// Presentation blocks for the fixture providers. The protocol crate cannot
/// depend on the engine's provider descriptors, so these literals duplicate
/// them; a core test keeps the two in step.
fn fixture_presentation(id_str: &str) -> Option<ProviderPresentation> {
    let (short_name, session, weekly, tertiary, accent) = match id_str {
        "claude" => ("Claude", "Session", "Weekly", Some("Sonnet"), "#CC7C5E"),
        "codex" => ("Codex", "Session", "Weekly", None, "#49A3B0"),
        "antigravity" => ("Anti", "Gemini Models", "Claude and GPT", None, "#60BA7E"),
        _ => return None,
    };
    Some(
        ProviderPresentation::new(
            short_name,
            session,
            weekly,
            tertiary.map(String::from),
            Some(accent.to_string()),
        )
        .unwrap(),
    )
}

async fn generate_settings_page(scope: &SettingsScope, shared: &ServerShared) -> SettingsPage {
    if let SettingsScope::Provider(provider_id) = scope {
        return generate_provider_settings_page(provider_id, shared).await;
    }

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

    let icon_style = settings
        .get(&SettingId::new("menuBar.iconStyle").unwrap())
        .and_then(|v| v.as_str())
        .unwrap_or("bars")
        .to_string();

    let show_used = settings
        .get(&SettingId::new("menuBar.showUsed").unwrap())
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let merge_icons = settings
        .get(&SettingId::new("menuBar.mergeIcons").unwrap())
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let show_optional_credits = settings
        .get(&SettingId::new("menu.showOptionalCreditsAndExtraUsage").unwrap())
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

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
        )
        .with_help("Default data collection strategy when a provider is set to Auto."),
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
        )
        .with_help("Background polling cadence in seconds."),
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
    ];

    let menu_bar_descriptors = vec![
        SettingDescriptor::new(
            SettingId::new("menuBar.iconStyle").unwrap(),
            "Status Icon Style",
            DescriptorKind::Choice {
                selected: icon_style,
                options: vec![
                    ChoiceOption::new("bars", "Bars"),
                    ChoiceOption::new("critters", "Critters"),
                ],
            },
        ),
        SettingDescriptor::new(
            SettingId::new("menuBar.showUsed").unwrap(),
            "Show Used Quota Instead of Remaining",
            DescriptorKind::Toggle { value: show_used },
        ),
        SettingDescriptor::new(
            SettingId::new("menuBar.mergeIcons").unwrap(),
            "Merge Provider Status Items",
            DescriptorKind::Toggle { value: merge_icons },
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

    let menu_content_descriptors = vec![
        SettingDescriptor::new(
            SettingId::new("menu.showOptionalCreditsAndExtraUsage").unwrap(),
            "Show Credits and Extra Usage",
            DescriptorKind::Toggle {
                value: show_optional_credits,
            },
        ),
        SettingDescriptor::new(
            SettingId::new("notifications.enabled").unwrap(),
            "Desktop Notifications",
            DescriptorKind::Toggle {
                value: notif_enabled,
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
            SettingsSection::new("general", "General Settings", general_descriptors)
                .with_pane("general")
                .with_subtitle("Polling cadence and device synchronization"),
            SettingsSection::new("menuBar", "Menu Bar", menu_bar_descriptors)
                .with_pane("menuBar")
                .with_subtitle("Status item meters and icon appearance"),
            SettingsSection::new("menu", "Menu Content", menu_content_descriptors)
                .with_pane("menu")
                .with_subtitle("Card sections and quota alerts"),
            SettingsSection::new("providers", "Provider Configurations", provider_descriptors)
                .with_pane("providers"),
            SettingsSection::new("actions", "System Actions", actions_descriptors)
                .with_pane("advanced"),
        ],
    )
}

async fn generate_provider_settings_page(
    provider_id: &ProviderId,
    shared: &ServerShared,
) -> SettingsPage {
    let pid = provider_id.as_str();
    let display_name = match pid {
        "claude" => "Claude",
        "codex" => "Codex",
        "antigravity" => "Antigravity",
        other => other,
    };

    let secret_id_str = format!("providers.{pid}.apiKey");
    let secret_id = SettingId::new(&secret_id_str)
        .unwrap_or_else(|_| SettingId::new("providers.claude.apiKey").unwrap());
    let is_secret_set = shared.secrets.lock().await.contains(&secret_id);

    let settings = shared.settings.lock().await;
    let enabled_id = SettingId::new(format!("providers.{pid}.enabled"))
        .unwrap_or_else(|_| SettingId::new("providers.claude.enabled").unwrap());
    let enabled = settings
        .get(&enabled_id)
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let mode_id = SettingId::new(format!("providers.{pid}.sourceMode"))
        .unwrap_or_else(|_| SettingId::new("providers.claude.sourceMode").unwrap());
    let default_mode = match pid {
        "claude" => "cli",
        "codex" => "web",
        _ => "api",
    };
    let selected_mode = settings
        .get(&mode_id)
        .and_then(|v| v.as_str())
        .unwrap_or(default_mode)
        .to_string();

    let overview_section = SettingsSection::new(
        "overview",
        format!("{display_name} Overview"),
        vec![
            SettingDescriptor::new(
                enabled_id.clone(),
                format!("Enable {display_name}"),
                DescriptorKind::Toggle { value: enabled },
            )
            .with_help(format!(
                "Show {display_name} in the menu bar and refresh cycle."
            )),
            SettingDescriptor::new(
                mode_id.clone(),
                "Usage Source",
                DescriptorKind::Choice {
                    selected: selected_mode,
                    options: vec![
                        ChoiceOption::new("auto", "Auto"),
                        ChoiceOption::new("cli", "CLI"),
                        ChoiceOption::new("web", "Web Cookie"),
                        ChoiceOption::new("api", "API Token"),
                    ],
                },
            )
            .with_enabled_when(SettingCondition::equals(enabled_id, true)),
        ],
    )
    .with_pane("providers")
    .with_subtitle(format!(
        "Connection and credential settings for {display_name}"
    ));

    let credentials_section = SettingsSection::new(
        "credentials",
        "Credentials & Accounts",
        vec![
            SettingDescriptor::new(
                secret_id,
                format!("{display_name} Secret Token"),
                DescriptorKind::Secret {
                    is_set: is_secret_set,
                    source: Some("Keychain".to_string()),
                },
            )
            .with_help("Configured via `ariadusage secret set`; secret values never cross IPC."),
            SettingDescriptor::new(
                SettingId::new(format!("accounts.{pid}"))
                    .unwrap_or_else(|_| SettingId::new("accounts.codex").unwrap()),
                format!("{display_name} Accounts"),
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
        ],
    )
    .with_pane("providers");

    let actions_section = SettingsSection::new(
        "providerActions",
        "Diagnostics",
        vec![
            SettingDescriptor::new(
                SettingId::new(format!("providers.{pid}.cliVersion"))
                    .unwrap_or_else(|_| SettingId::new("providers.claude.cliVersion").unwrap()),
                "Detected CLI Version",
                DescriptorKind::Text {
                    value: "1.0.24 (/usr/local/bin)".to_string(),
                    config: TextConfig::default(),
                },
            )
            .with_read_only(true),
            SettingDescriptor::new(
                SettingId::new(format!("providers.{pid}.actions"))
                    .unwrap_or_else(|_| SettingId::new("diagnostics").unwrap()),
                "Provider Actions",
                DescriptorKind::Actions {
                    actions: vec![
                        ActionItem {
                            id: ActionId::new("refreshProvider").unwrap(),
                            label: format!("Refresh {display_name} Now"),
                            style: ActionStyle::Button,
                            confirmation: None,
                        },
                        ActionItem {
                            id: ActionId::new("recheckAccount").unwrap(),
                            label: "Re-authenticate Session".to_string(),
                            style: ActionStyle::Link,
                            confirmation: None,
                        },
                    ],
                },
            ),
        ],
    )
    .with_pane("providers");

    SettingsPage::new(
        SettingsScope::Provider(provider_id.clone()),
        vec![overview_section, credentials_section, actions_section],
    )
}
