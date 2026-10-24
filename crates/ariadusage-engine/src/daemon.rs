//! Engine daemon runtime, notification boundary, and signal-managed IPC v1 server loop.

use std::collections::HashMap;
use std::future::Future;
#[cfg(unix)]
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use ariadusage_core::config::{Config, CookieSource, ProviderConfig, ProviderEntry};
use ariadusage_core::notifications::NotificationPost;
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::ids::{ActionId, ProviderId, SettingId};
use ariadusage_protocol::ipc::{ErrorCode, IpcError, NoticeLevel, ServerMessage};
use ariadusage_protocol::settings::{
    ChoiceOption, DescriptorKind, NumberConfig, SettingDescriptor, SettingsPage, SettingsScope,
    SettingsSection,
};
use ariadusage_protocol::snapshot::{
    EngineInfo, EngineSnapshot, ProviderPresentation, ProviderSnapshot,
};
use tokio::sync::{Mutex, broadcast};

use crate::config_store::ConfigStore;
use crate::error::StoreError;
use crate::hooks::HookRunner;
#[cfg(unix)]
use crate::ipc_server::start_ipc_server;
use crate::ipc_server::{IpcBackend, IpcServerError};
use crate::paths::{PathError, data_dir};
#[cfg(unix)]
use crate::paths::{config_path, socket_path};
use crate::runtime::EngineRuntime;

/// Errors encountered by the desktop notification boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotificationError {
    #[error("desktop notifications are unsupported in this build")]
    Unsupported,

    #[error("notification delivery failed: {0}")]
    DeliveryFailed(String),
}

/// Desktop notification boundary for quota and status transition alerts.
pub trait Notifier: Send + Sync {
    /// Post a desktop notification with `title` and `body`.
    fn notify(&self, title: &str, body: &str) -> Result<(), NotificationError>;

    /// Post a structured [`NotificationPost`] with identifier and sound metadata.
    fn notify_post(&self, post: &NotificationPost) -> Result<(), NotificationError> {
        self.notify(&post.title, &post.body)
    }
}

/// Default notifier for the engine daemon when invoked outside an IPC-backed runtime.
///
/// Direct `notify` calls return [`NotificationError::Unsupported`] on macOS (`UNUserNotificationCenter`
/// runs in the bundled `ariadusage-macos` app process over IPC `ServerMessage::Notice`).
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultNotifier;

impl Notifier for DefaultNotifier {
    fn notify(&self, _title: &str, _body: &str) -> Result<(), NotificationError> {
        Err(NotificationError::Unsupported)
    }
}

/// Silent no-op [`Notifier`] used by default in test runtimes so unit/integration tests never
/// trigger real OS notifications unless a notifier is explicitly injected.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify(&self, _title: &str, _body: &str) -> Result<(), NotificationError> {
        Ok(())
    }

    fn notify_post(&self, _post: &NotificationPost) -> Result<(), NotificationError> {
        Ok(())
    }
}

/// In-memory [`Notifier`] that records every [`NotificationPost`] for deterministic inspection
/// in tests.
#[derive(Debug, Default)]
pub struct RecordingNotifier {
    posts: std::sync::Mutex<Vec<NotificationPost>>,
}

impl RecordingNotifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn posts(&self) -> Vec<NotificationPost> {
        self.posts.lock().expect("recording notifier lock").clone()
    }
}

impl Notifier for RecordingNotifier {
    fn notify(&self, title: &str, body: &str) -> Result<(), NotificationError> {
        self.notify_post(&NotificationPost::new("custom", title, body))
    }

    fn notify_post(&self, post: &NotificationPost) -> Result<(), NotificationError> {
        self.posts
            .lock()
            .expect("recording notifier lock")
            .push(post.clone());
        Ok(())
    }
}

/// [`Notifier`] backend that broadcasts [`ServerMessage::Notice`] over IPC v1 so connected
/// frontends (such as `ariadusage-macos` via `UNUserNotificationCenter`) can render desktop
/// notifications with bundle identity.
#[derive(Debug, Clone)]
pub struct IpcNoticeNotifier {
    tx: broadcast::Sender<ServerMessage>,
}

impl IpcNoticeNotifier {
    pub const fn new(tx: broadcast::Sender<ServerMessage>) -> Self {
        Self { tx }
    }
}

impl Notifier for IpcNoticeNotifier {
    fn notify(&self, title: &str, body: &str) -> Result<(), NotificationError> {
        let _ = self.tx.send(ServerMessage::Notice {
            level: NoticeLevel::Info,
            title: Some(title.to_string()),
            message: body.to_string(),
            notification_id: None,
            sound: Some(true),
        });
        Ok(())
    }

    fn notify_post(&self, post: &NotificationPost) -> Result<(), NotificationError> {
        let notification_id = post
            .identifier
            .clone()
            .or_else(|| Some(post.id_prefix.clone()));
        let _ = self.tx.send(ServerMessage::Notice {
            level: NoticeLevel::Info,
            title: Some(post.title.clone()),
            message: post.body.clone(),
            notification_id,
            sound: Some(post.sound_enabled),
        });
        Ok(())
    }
}

/// Linux D-Bus desktop notification backend backed by `notify-rust` (`zbus` on `tokio`).
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxDbusNotifier;

impl LinuxDbusNotifier {
    pub const fn new() -> Self {
        Self
    }
}

impl Notifier for LinuxDbusNotifier {
    fn notify(&self, title: &str, body: &str) -> Result<(), NotificationError> {
        self.notify_post(&NotificationPost::new("ariadusage-notice", title, body))
    }

    #[cfg(target_os = "linux")]
    fn notify_post(&self, post: &NotificationPost) -> Result<(), NotificationError> {
        let title = post.title.clone();
        let body = post.body.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = notify_rust::Notification::new()
                    .appname("AriadUsage")
                    .summary(&title)
                    .body(&body)
                    .show_async()
                    .await;
            });
            Ok(())
        } else {
            notify_rust::Notification::new()
                .appname("AriadUsage")
                .summary(&title)
                .body(&body)
                .show()
                .map(|_| ())
                .map_err(|err| NotificationError::DeliveryFailed(err.to_string()))
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn notify_post(&self, _post: &NotificationPost) -> Result<(), NotificationError> {
        Err(NotificationError::Unsupported)
    }
}

/// Fans out notifications to multiple [`Notifier`] backends.
#[derive(Default)]
pub struct CompositeNotifier {
    backends: Vec<Arc<dyn Notifier>>,
}

impl std::fmt::Debug for CompositeNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeNotifier")
            .field("backends_len", &self.backends.len())
            .finish()
    }
}

impl CompositeNotifier {
    pub fn new(backends: Vec<Arc<dyn Notifier>>) -> Self {
        Self { backends }
    }
}

impl Notifier for CompositeNotifier {
    fn notify(&self, title: &str, body: &str) -> Result<(), NotificationError> {
        let mut last_err = None;
        let mut any_ok = false;
        for backend in &self.backends {
            match backend.notify(title, body) {
                Ok(()) => any_ok = true,
                Err(err) => last_err = Some(err),
            }
        }
        if any_ok || self.backends.is_empty() {
            Ok(())
        } else {
            Err(last_err.unwrap_or(NotificationError::Unsupported))
        }
    }

    fn notify_post(&self, post: &NotificationPost) -> Result<(), NotificationError> {
        let mut last_err = None;
        let mut any_ok = false;
        for backend in &self.backends {
            match backend.notify_post(post) {
                Ok(()) => any_ok = true,
                Err(err) => last_err = Some(err),
            }
        }
        if any_ok || self.backends.is_empty() {
            Ok(())
        } else {
            Err(last_err.unwrap_or(NotificationError::Unsupported))
        }
    }
}

/// Errors encountered while starting or running the engine daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)]
    Path(#[from] PathError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error(transparent)]
    Ipc(#[from] IpcServerError),

    #[error("signal handler registration failed: {0}")]
    Signal(#[from] std::io::Error),
}

/// Default [`IpcBackend`] backed by [`ConfigStore`].
///
/// Produces an honest [`EngineSnapshot`] for the three v1 providers (`claude`, `codex`,
/// `antigravity`) reflecting their configured enabled/source state and presentation metadata
/// without any invented usage figures.
pub struct ConfigIpcBackend {
    config_store: ConfigStore,
    runtime_settings: Mutex<HashMap<SettingId, serde_json::Value>>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
}

impl ConfigIpcBackend {
    /// Creates a new [`ConfigIpcBackend`] backed by `config_store`.
    pub fn new(config_store: ConfigStore) -> Self {
        let (broadcast_tx, _) = broadcast::channel(32);
        Self {
            config_store,
            runtime_settings: Mutex::new(HashMap::new()),
            broadcast_tx,
        }
    }

    fn build_snapshot(&self) -> EngineSnapshot {
        let cfg = self
            .config_store
            .load_effective()
            .unwrap_or_else(|_| Config::new(Config::CURRENT_VERSION, Vec::new()));

        let raw_now = jiff::Timestamp::now();
        let now = jiff::Timestamp::from_second(raw_now.as_second()).unwrap_or(raw_now);

        let providers = [
            (
                "claude",
                "Claude",
                "cli",
                ProviderPresentation {
                    short_display_name: "Claude".to_string(),
                    session_label: "Session".to_string(),
                    weekly_label: "Weekly".to_string(),
                    tertiary_label: Some("Sonnet".to_string()),
                    accent_color: Some("#CC7C5E".to_string()),
                },
            ),
            (
                "codex",
                "Codex",
                "cli",
                ProviderPresentation {
                    short_display_name: "Codex".to_string(),
                    session_label: "Session".to_string(),
                    weekly_label: "Weekly".to_string(),
                    tertiary_label: None,
                    accent_color: Some("#49A3B0".to_string()),
                },
            ),
            (
                "antigravity",
                "Antigravity",
                "auto",
                ProviderPresentation {
                    short_display_name: "Anti".to_string(),
                    session_label: "Claude".to_string(),
                    weekly_label: "Gemini Pro".to_string(),
                    tertiary_label: Some("Gemini Flash".to_string()),
                    accent_color: Some("#60BA7E".to_string()),
                },
            ),
        ]
        .into_iter()
        .filter_map(|(id_str, display_name, default_source, mut presentation)| {
            let pid = ProviderId::new(id_str).ok()?;
            let entry = cfg.providers.iter().find_map(|e| match e {
                ProviderEntry::Typed(p) if p.id == pid => Some(p),
                _ => None,
            });
            let enabled = entry.and_then(|p| p.enabled).unwrap_or(true);
            let source = entry
                .and_then(|p| p.source)
                .map(source_mode_str)
                .unwrap_or(default_source);
            if let Some(custom_color) = entry.and_then(|p| p.accent_color.clone()) {
                presentation.accent_color = Some(custom_color);
            }
            let mut snap = ProviderSnapshot::new(pid, display_name, enabled, source);
            snap.presentation = Some(presentation);
            Some(snap)
        })
        .collect();

        EngineSnapshot::new(
            now,
            300,
            EngineInfo {
                version: env!("CARGO_PKG_VERSION").to_string(),
                refreshing: false,
            },
            providers,
        )
    }

    async fn build_settings_page(&self, scope: SettingsScope) -> Result<SettingsPage, IpcError> {
        let cfg = self
            .config_store
            .load_effective()
            .map_err(|_| IpcError::new(ErrorCode::InternalError))?;
        let overrides = self.runtime_settings.lock().await;

        let provider_entry = |id_str: &str| -> Option<&ProviderConfig> {
            cfg.providers.iter().find_map(|e| match e {
                ProviderEntry::Typed(p) if p.id.as_str() == id_str => Some(p),
                _ => None,
            })
        };

        let get_bool = |id: &str, default: bool| -> bool {
            SettingId::new(id)
                .ok()
                .and_then(|sid| overrides.get(&sid))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(default)
        };
        let get_str = |id: &str, default: &str| -> String {
            SettingId::new(id)
                .ok()
                .and_then(|sid| overrides.get(&sid))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(default)
                .to_string()
        };
        let get_f64 = |id: &str, default: f64| -> f64 {
            SettingId::new(id)
                .ok()
                .and_then(|sid| overrides.get(&sid))
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(default)
        };

        let mut sections = Vec::new();
        for (pid, label, default_source) in [
            ("claude", "Claude", "cli"),
            ("codex", "Codex", "cli"),
            ("antigravity", "Antigravity", "auto"),
        ] {
            let p_cfg = provider_entry(pid);
            let enabled_id = format!("providers.{pid}.enabled");
            let source_id = format!("providers.{pid}.sourceMode");
            let cookie_id = format!("providers.{pid}.cookieSource");

            let enabled_default = p_cfg.and_then(|p| p.enabled).unwrap_or(true);
            let source_default = p_cfg
                .and_then(|p| p.source)
                .map(source_mode_str)
                .unwrap_or(default_source);
            let cookie_default = p_cfg
                .and_then(|p| p.cookie_source)
                .map(cookie_source_str)
                .unwrap_or("off");

            let mut sec = SettingsSection::new(
                format!("provider-{pid}"),
                label,
                vec![
                    SettingDescriptor::new(
                        SettingId::new(&enabled_id).unwrap(),
                        format!("Enable {label}"),
                        DescriptorKind::Toggle {
                            value: get_bool(&enabled_id, enabled_default),
                        },
                    ),
                    SettingDescriptor::new(
                        SettingId::new(&source_id).unwrap(),
                        "Source Mode",
                        DescriptorKind::Choice {
                            selected: get_str(&source_id, source_default),
                            options: vec![
                                ChoiceOption::new("auto", "Auto"),
                                ChoiceOption::new("web", "Web"),
                                ChoiceOption::new("cli", "CLI"),
                                ChoiceOption::new("oauth", "OAuth"),
                                ChoiceOption::new("api", "API"),
                            ],
                        },
                    ),
                    SettingDescriptor::new(
                        SettingId::new(&cookie_id).unwrap(),
                        "Cookie Source",
                        DescriptorKind::Choice {
                            selected: get_str(&cookie_id, cookie_default),
                            options: vec![
                                ChoiceOption::new("auto", "Automatic (Browser)"),
                                ChoiceOption::new("manual", "Manual Header"),
                                ChoiceOption::new("off", "Off"),
                            ],
                        },
                    ),
                ],
            );
            sec.pane = Some("providers".to_string());
            sections.push(sec);
        }

        let mut general_sec = SettingsSection::new(
            "general",
            "General",
            vec![SettingDescriptor::new(
                SettingId::new("refreshIntervalSeconds").unwrap(),
                "Refresh Cadence",
                DescriptorKind::Number {
                    value: get_f64("refreshIntervalSeconds", 300.0),
                    config: NumberConfig {
                        min: Some(0.0),
                        max: Some(900.0),
                        step: Some(60.0),
                        unit: Some("s".to_string()),
                    },
                },
            )],
        );
        general_sec.pane = Some("general".to_string());
        sections.push(general_sec);

        Ok(SettingsPage::new(scope, sections))
    }
}

const fn source_mode_str(mode: SourceMode) -> &'static str {
    match mode {
        SourceMode::Auto => "auto",
        SourceMode::Web => "web",
        SourceMode::Cli => "cli",
        SourceMode::Oauth => "oauth",
        SourceMode::Api => "api",
    }
}

fn parse_source_mode(raw: &str) -> Option<SourceMode> {
    match raw {
        "auto" => Some(SourceMode::Auto),
        "web" => Some(SourceMode::Web),
        "cli" => Some(SourceMode::Cli),
        "oauth" => Some(SourceMode::Oauth),
        "api" => Some(SourceMode::Api),
        _ => None,
    }
}

const fn cookie_source_str(source: CookieSource) -> &'static str {
    match source {
        CookieSource::Auto => "auto",
        CookieSource::Manual => "manual",
        CookieSource::Off => "off",
    }
}

fn parse_cookie_source(raw: &str) -> Option<CookieSource> {
    match raw {
        "auto" => Some(CookieSource::Auto),
        "manual" => Some(CookieSource::Manual),
        "off" => Some(CookieSource::Off),
        _ => None,
    }
}

impl IpcBackend for ConfigIpcBackend {
    fn subscribe_pushes(&self) -> broadcast::Receiver<ServerMessage> {
        self.broadcast_tx.subscribe()
    }

    fn current_snapshot(&self) -> Pin<Box<dyn Future<Output = EngineSnapshot> + Send + '_>> {
        Box::pin(async move { self.build_snapshot() })
    }

    fn refresh(
        &self,
        _provider: Option<ProviderId>,
    ) -> Pin<Box<dyn Future<Output = Result<EngineSnapshot, IpcError>> + Send + '_>> {
        Box::pin(async move {
            let snapshot = self.build_snapshot();
            let _ = self.broadcast_tx.send(ServerMessage::Snapshot {
                snapshot: Box::new(snapshot.clone()),
            });
            Ok(snapshot)
        })
    }

    fn get_settings(
        &self,
        scope: SettingsScope,
    ) -> Pin<Box<dyn Future<Output = Result<SettingsPage, IpcError>> + Send + '_>> {
        Box::pin(async move { self.build_settings_page(scope).await })
    }

    fn set_setting(
        &self,
        id: SettingId,
        value: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<SettingsScope, IpcError>> + Send + '_>> {
        Box::pin(async move {
            let id_str = id.as_str().to_string();
            if let Some(rest) = id_str.strip_prefix("providers.")
                && let Some((pid_str, field)) = rest.split_once('.')
                && let Ok(pid) = ProviderId::new(pid_str)
            {
                let _ = self.config_store.update(|cfg| {
                    let entry = cfg.providers.iter_mut().find_map(|e| match e {
                        ProviderEntry::Typed(p) if p.id == pid => Some(p),
                        _ => None,
                    });
                    let provider_cfg = match entry {
                        Some(p) => p,
                        None => {
                            cfg.providers
                                .push(ProviderEntry::Typed(ProviderConfig::new(pid.clone())));
                            match cfg.providers.last_mut() {
                                Some(ProviderEntry::Typed(p)) => p,
                                _ => unreachable!(),
                            }
                        }
                    };
                    match field {
                        "enabled" => {
                            if let Some(b) = value.as_bool() {
                                provider_cfg.enabled = Some(b);
                            }
                        }
                        "sourceMode" => {
                            if let Some(s) = value.as_str().and_then(parse_source_mode) {
                                provider_cfg.source = Some(s);
                            }
                        }
                        "cookieSource" => {
                            if let Some(c) = value.as_str().and_then(parse_cookie_source) {
                                provider_cfg.cookie_source = Some(c);
                            }
                        }
                        _ => {}
                    }
                });
            }

            self.runtime_settings.lock().await.insert(id, value);
            let _ = self.broadcast_tx.send(ServerMessage::SettingsChanged {
                scope: SettingsScope::App,
            });
            let snapshot = self.build_snapshot();
            let _ = self.broadcast_tx.send(ServerMessage::Snapshot {
                snapshot: Box::new(snapshot),
            });
            Ok(SettingsScope::App)
        })
    }

    fn run_action(
        &self,
        _id: ActionId,
        _confirm: Option<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<serde_json::Value>, IpcError>> + Send + '_>>
    {
        Box::pin(async move { Ok(None) })
    }
}

/// Builds the default [`SecretStore`](crate::brokers::secret_store::SecretStore) backed by
/// [`ServiceBackend`](crate::brokers::secret_store::service::ServiceBackend) and
/// [`FileBackend`](crate::brokers::secret_store::file::FileBackend) at
/// `<data_dir>/ariadusage/secrets.json`, honoring `config.secret_file_fallback == Some(true)`
/// and `disable_keyring`.
pub fn build_default_secret_store(
    config: &Config,
    disable_keyring: bool,
) -> Result<Arc<crate::brokers::secret_store::SecretStore>, PathError> {
    let consented = config.secret_file_fallback == Some(true);
    let data = data_dir()?;
    let file_backend = crate::brokers::secret_store::file::FileBackend::new(
        data.join("ariadusage").join("secrets.json"),
        consented,
    );
    Ok(Arc::new(
        crate::brokers::secret_store::SecretStore::new(
            crate::brokers::secret_store::service::ServiceBackend::new(disable_keyring),
        )
        .with_file_fallback(file_backend, consented),
    ))
}

/// Builds the default [`EngineRuntime`] wired to `config_store`, [`SecretStore`], [`NetBroker`],
/// and [`ProviderRegistry`].
pub fn build_default_runtime(config_store: ConfigStore) -> Result<Arc<EngineRuntime>, DaemonError> {
    build_default_runtime_with_options(config_store, false)
}

/// Builds the default [`EngineRuntime`] with an explicit `disable_keyring` toggle injected by the
/// caller (for example from `ARIADUSAGE_DISABLE_KEYRING` in the CLI binary).
pub fn build_default_runtime_with_options(
    config_store: ConfigStore,
    disable_keyring: bool,
) -> Result<Arc<EngineRuntime>, DaemonError> {
    let config = config_store.load_effective().unwrap_or_else(|_| {
        ariadusage_core::config::normalize(Config::new(Config::CURRENT_VERSION, Vec::new()))
    });
    let secret_store = build_default_secret_store(&config, disable_keyring)?;
    let net = Arc::new(
        crate::brokers::net::NetBroker::new(crate::brokers::net::NetConfig::default())
            .map_err(|err| DaemonError::Signal(std::io::Error::other(err.to_string())))?,
    );
    let cookie_cache = Arc::new(crate::brokers::cookie_cache::CookieCache::new());
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    let home_dir = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| working_dir.clone());
    let claude_config = crate::providers::claude::ClaudeOAuthStrategyConfig::new(
        Arc::clone(&net),
        working_dir.clone(),
        home_dir.clone(),
    )
    .with_cookie_cache(Arc::clone(&cookie_cache));
    let codex_config = crate::providers::codex::CodexStrategyConfig::new(
        Arc::clone(&net),
        std::collections::BTreeMap::new(),
        home_dir.clone(),
        working_dir.clone(),
    );
    let antigravity_config = crate::providers::antigravity::AntigravityStrategyConfig::new(
        Arc::clone(&net),
        working_dir,
        home_dir,
    );
    let registry = crate::registry::ProviderRegistry::with_all_first_party_configs(
        claude_config,
        codex_config,
        antigravity_config,
    );
    let base_env: std::collections::BTreeMap<String, ariadusage_protocol::secret::SecretString> =
        std::env::vars()
            .map(|(k, v)| (k, ariadusage_protocol::secret::SecretString::new(v)))
            .collect();
    let runtime = EngineRuntime::new(config_store, secret_store, net, registry)
        .with_cookie_cache(cookie_cache)
        .with_base_environment(base_env);
    let ipc_notifier: Arc<dyn Notifier> = Arc::new(IpcNoticeNotifier::new(runtime.pushes_sender()));
    let notifier: Arc<dyn Notifier> = {
        #[cfg(target_os = "linux")]
        {
            Arc::new(CompositeNotifier::new(vec![
                ipc_notifier,
                Arc::new(LinuxDbusNotifier::new()),
            ]))
        }
        #[cfg(not(target_os = "linux"))]
        {
            ipc_notifier
        }
    };
    let mut runtime = runtime
        .with_notifier(notifier)
        .with_hook_runner(Arc::new(HookRunner::new_process_broker()));
    if let Ok(state_dir) = crate::paths::state_dir() {
        runtime = runtime.with_notification_state_path(state_dir.join("notifications-state.json"));
    }
    Ok(Arc::new(runtime))
}

/// Runs the engine daemon with a custom shutdown future and optional socket path override.
#[cfg(unix)]
pub async fn run_daemon_with_shutdown(
    socket_override: Option<PathBuf>,
    backend: Arc<dyn IpcBackend>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), DaemonError> {
    crate::private_tempdir::sweep_stale();

    let sock = match socket_override {
        Some(path) => path,
        None => socket_path()?,
    };

    let mut server = start_ipc_server(sock, backend).await?;
    shutdown.await;
    server.stop().await;
    Ok(())
}

/// Runs the foreground engine daemon until `SIGINT` or `SIGTERM` is received.
#[cfg(unix)]
pub async fn run_daemon(socket_override: Option<PathBuf>) -> Result<(), DaemonError> {
    run_daemon_with_options(socket_override, false).await
}

/// Runs the foreground engine daemon with an explicit `disable_keyring` toggle until `SIGINT` or
/// `SIGTERM` is received, starting the background refresh timer while the daemon is active.
#[cfg(unix)]
pub async fn run_daemon_with_options(
    socket_override: Option<PathBuf>,
    disable_keyring: bool,
) -> Result<(), DaemonError> {
    let cfg_path = config_path()?;
    let runtime = build_default_runtime_with_options(ConfigStore::new(cfg_path), disable_keyring)?;
    runtime.start_timer();
    let backend: Arc<dyn IpcBackend> = runtime.clone();

    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    let res = run_daemon_with_shutdown(socket_override, backend, async move {
        tokio::select! {
            _ = sigint.recv() => {}
            _ = sigterm.recv() => {}
        }
    })
    .await;

    runtime.stop_timer();
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_notifier_returns_unsupported() {
        let notifier = DefaultNotifier;
        assert_eq!(
            notifier.notify("Quota warning", "Claude session reached 80%"),
            Err(NotificationError::Unsupported)
        );
    }
}
